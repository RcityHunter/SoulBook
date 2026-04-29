use axum::{
    extract::Query,
    response::Json,
    routing::{get, post},
    Extension, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{
    error::ApiError,
    models::search::{SearchRequest, SearchResponse},
    services::auth::User,
};

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub space_id: Option<String>,
    pub tags: Option<String>, // 逗号分隔的标签
    pub author_id: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub sort: Option<String>,
}

#[derive(Deserialize)]
pub struct SuggestQuery {
    pub q: String,
    pub limit: Option<i64>,
}

pub async fn search_documents(
    Query(query): Query<SearchQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user_id, "docs.read", None)
        .await?;

    // 解析标签
    let tags = query
        .tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    // 解析排序方式
    let sort_by = match query.sort.as_deref() {
        Some("created_at") => Some(crate::models::search::SearchSortBy::CreatedAt),
        Some("updated_at") => Some(crate::models::search::SearchSortBy::UpdatedAt),
        Some("title") => Some(crate::models::search::SearchSortBy::Title),
        _ => Some(crate::models::search::SearchSortBy::Relevance),
    };

    let search_request = SearchRequest {
        query: query.q,
        space_id: query.space_id,
        tags,
        author_id: query.author_id,
        page: query.page,
        per_page: query.per_page,
        sort_by,
    };

    let response = search_service.search(&user_id, search_request).await?;

    Ok(Json(search_response_payload(response)))
}

pub async fn search_suggestions(
    Query(query): Query<SuggestQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user_id, "docs.read", None)
        .await?;

    let limit = query.limit.unwrap_or(10);
    let suggestions = search_service
        .suggest_search_terms(&user_id, &query.q, limit)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": suggestions,
        "query": query.q,
    })))
}

pub async fn reindex_documents(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    // 只有文档管理员可以重建索引
    auth_service
        .check_permission(&user_id, "docs.admin", None)
        .await?;

    let indexed_count = search_service.bulk_reindex().await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "indexed_count": indexed_count,
        },
        "message": "Documents reindexed successfully",
    })))
}

pub async fn search_within_space(
    axum::extract::Path(space_id): axum::extract::Path<String>,
    Query(mut query): Query<SearchQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    // 检查空间访问权限
    auth_service
        .check_permission(&user_id, "docs.read", Some(&space_id))
        .await?;

    // 强制设置空间ID
    query.space_id = Some(space_id);

    let tags = query
        .tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    let sort_by = match query.sort.as_deref() {
        Some("created_at") => Some(crate::models::search::SearchSortBy::CreatedAt),
        Some("updated_at") => Some(crate::models::search::SearchSortBy::UpdatedAt),
        Some("title") => Some(crate::models::search::SearchSortBy::Title),
        _ => Some(crate::models::search::SearchSortBy::Relevance),
    };

    let search_request = SearchRequest {
        query: query.q,
        space_id: query.space_id,
        tags,
        author_id: query.author_id,
        page: query.page,
        per_page: query.per_page,
        sort_by,
    };

    let response = search_service.search(&user_id, search_request).await?;

    Ok(Json(search_response_payload(response)))
}

pub async fn search_by_tags(
    Query(query): Query<SearchQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user_id, "docs.read", None)
        .await?;

    // 确保有标签查询
    if query.tags.is_none() || query.tags.as_ref().unwrap().is_empty() {
        return Err(ApiError::BadRequest(
            "Tags parameter is required".to_string(),
        ));
    }

    let tags = query
        .tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    let search_request = SearchRequest {
        query: query.q,
        space_id: query.space_id,
        tags,
        author_id: query.author_id,
        page: query.page,
        per_page: query.per_page,
        sort_by: Some(crate::models::search::SearchSortBy::Relevance),
    };

    let response = search_service.search(&user_id, search_request).await?;

    Ok(Json(search_response_payload(response)))
}

fn search_response_payload(response: SearchResponse) -> Value {
    let results: Vec<Value> = response
        .results
        .into_iter()
        .map(|result| {
            json!({
                "id": result.document_id,
                "document_id": result.document_id,
                "space_id": result.space_id,
                "title": result.title,
                "snippet": result.excerpt,
                "excerpt": result.excerpt,
                "tags": result.tags,
                "author_id": result.author_id,
                "last_updated": result.last_updated,
                "score": result.score,
                "highlights": result.highlights,
                "result_type": "document",
            })
        })
        .collect();

    json!({
        "success": true,
        "data": {
            "results": results,
            "total": response.total_count,
            "total_count": response.total_count,
            "page": response.page,
            "per_page": response.per_page,
            "total_pages": response.total_pages,
            "query": response.query,
            "took": response.took,
        }
    })
}

pub fn router() -> Router {
    Router::new()
        .route("/", get(search_documents))
        .route("/suggest", get(search_suggestions))
        .route("/reindex", post(reindex_documents))
        .route("/spaces/:space_id", get(search_within_space))
        .route("/tags", get(search_by_tags))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::search::{SearchHighlight, SearchResult};
    use surrealdb::types::Datetime;

    #[test]
    fn search_payload_is_wrapped_for_frontend_contract() {
        let response = SearchResponse {
            results: vec![SearchResult {
                document_id: "document:test".to_string(),
                space_id: "space:docs".to_string(),
                title: "Test document".to_string(),
                excerpt: "Search snippet".to_string(),
                tags: vec!["tag".to_string()],
                author_id: "user:1".to_string(),
                last_updated: Datetime::default(),
                score: 0.8,
                highlights: vec![SearchHighlight {
                    field: "title".to_string(),
                    text: "Test".to_string(),
                    start: 0,
                    end: 4,
                }],
            }],
            total_count: 1,
            page: 1,
            per_page: 20,
            total_pages: 1,
            query: "test".to_string(),
            took: 3,
        };

        let payload = search_response_payload(response);
        assert_eq!(payload["success"], true);
        assert_eq!(payload["data"]["total"], 1);
        assert_eq!(payload["data"]["results"][0]["id"], "document:test");
        assert_eq!(payload["data"]["results"][0]["snippet"], "Search snippet");
    }
}
