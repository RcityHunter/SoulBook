use axum::{
    extract::{Query, State},
    response::Json,
    routing::{get, post},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
pub struct SuggestResponse {
    pub suggestions: Vec<String>,
    pub query: String,
}

#[derive(Serialize)]
pub struct ReindexResponse {
    pub message: String,
    pub indexed_count: i64,
}

pub async fn search_documents(
    Query(query): Query<SearchQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<SearchResponse>, ApiError> {
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    // 检查基本搜索权限
    auth_service
        .check_permission(&user.id, "docs.read", None)
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

    let response = search_service.search(&user.id, search_request).await?;

    Ok(Json(response))
}

pub async fn search_suggestions(
    Query(query): Query<SuggestQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<SuggestResponse>, ApiError> {
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", None)
        .await?;

    let limit = query.limit.unwrap_or(10);
    let suggestions = search_service
        .suggest_search_terms(&user.id, &query.q, limit)
        .await?;

    Ok(Json(SuggestResponse {
        suggestions,
        query: query.q,
    }))
}

pub async fn reindex_documents(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<ReindexResponse>, ApiError> {
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    // 只有文档管理员可以重建索引
    auth_service
        .check_permission(&user.id, "docs.admin", None)
        .await?;

    let indexed_count = search_service.bulk_reindex().await?;

    Ok(Json(ReindexResponse {
        message: "Documents reindexed successfully".to_string(),
        indexed_count,
    }))
}

pub async fn search_within_space(
    axum::extract::Path(space_id): axum::extract::Path<String>,
    Query(mut query): Query<SearchQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<SearchResponse>, ApiError> {
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    // 检查空间访问权限
    auth_service
        .check_permission(&user.id, "docs.read", Some(&space_id))
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

    let response = search_service.search(&user.id, search_request).await?;

    Ok(Json(response))
}

pub async fn search_by_tags(
    Query(query): Query<SearchQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<SearchResponse>, ApiError> {
    let search_service = &app_state.search_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", None)
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

    let response = search_service.search(&user.id, search_request).await?;

    Ok(Json(response))
}

pub fn router() -> Router {
    Router::new()
        .route("/", get(search_documents))
        .route("/suggest", get(search_suggestions))
        .route("/reindex", post(reindex_documents))
        .route("/spaces/:space_id", get(search_within_space))
        .route("/tags", get(search_by_tags))
}
