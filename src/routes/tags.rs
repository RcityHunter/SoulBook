use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    error::ApiError,
    models::tag::{CreateTagRequest, DocumentTag, Tag, TagDocumentRequest, UpdateTagRequest},
    services::database::record_id_to_string,
    services::{
        auth::User,
        tags::{TagService, TagStatistics},
    },
};

#[derive(Deserialize)]
pub struct TagQuery {
    pub space_id: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub search: Option<String>,
}

#[derive(Deserialize)]
pub struct PopularTagsQuery {
    pub space_id: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize)]
pub struct TagListResponse {
    pub tags: Vec<Tag>,
    pub total_count: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

#[derive(Serialize)]
pub struct DocumentTagsResponse {
    pub document_id: String,
    pub tags: Vec<Tag>,
}

#[derive(Serialize)]
pub struct TagDocumentsResponse {
    pub tag_id: String,
    pub document_ids: Vec<String>,
    pub total_count: i64,
    pub page: i64,
    pub per_page: i64,
}

pub async fn get_tags(
    Query(query): Query<TagQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查读取权限
    if let Some(space_id) = &query.space_id {
        auth_service
            .check_permission(user_id, "docs.read", Some(space_id))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.read", None)
            .await?;
    }

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(20);

    let tags = if let Some(search_query) = &query.search {
        tag_service
            .search_tags(query.space_id.as_deref(), search_query, per_page)
            .await?
    } else {
        tag_service
            .get_tags_by_space(query.space_id.as_deref(), page, per_page)
            .await?
    };

    // 简化实现，实际应该查询真实总数
    let total_count = tags.len() as i64;
    let total_pages = (total_count + per_page - 1) / per_page;

    Ok(Json(serde_json::json!({
        "success": true,
        "data": {
            "tags": tags,
            "total_count": total_count,
            "page": page,
            "per_page": per_page,
            "total_pages": total_pages
        }
    })))
}

pub async fn create_tag(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
    Json(request): Json<CreateTagRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查创建权限
    if let Some(space_id) = &request.space_id {
        auth_service
            .check_permission(user_id, "docs.tag.create", Some(space_id))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.admin", None)
            .await?;
    }

    let tag = tag_service.create_tag(user_id, request).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "data": tag
    })))
}

pub async fn get_tag(
    Path(tag_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    let tag = tag_service.get_tag(&tag_id).await?;

    // 检查读取权限
    if let Some(space_id) = &tag.space_id {
        auth_service
            .check_permission(user_id, "docs.read", Some(&record_id_to_string(space_id)))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.read", None)
            .await?;
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "data": tag
    })))
}

pub async fn update_tag(
    Path(tag_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
    Json(request): Json<UpdateTagRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    let tag = tag_service.get_tag(&tag_id).await?;

    // 检查更新权限
    if let Some(space_id) = &tag.space_id {
        auth_service
            .check_permission(
                user_id,
                "docs.tag.update",
                Some(&record_id_to_string(space_id)),
            )
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.admin", None)
            .await?;
    }

    let updated_tag = tag_service.update_tag(&tag_id, user_id, request).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "data": updated_tag
    })))
}

pub async fn delete_tag(
    Path(tag_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<StatusCode, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    let tag = tag_service.get_tag(&tag_id).await?;

    // 检查删除权限
    if let Some(space_id) = &tag.space_id {
        auth_service
            .check_permission(
                user_id,
                "docs.tag.delete",
                Some(&record_id_to_string(space_id)),
            )
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.admin", None)
            .await?;
    }

    tag_service.delete_tag(&tag_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_popular_tags(
    Query(query): Query<PopularTagsQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查读取权限
    if let Some(space_id) = &query.space_id {
        auth_service
            .check_permission(user_id, "docs.read", Some(space_id))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.read", None)
            .await?;
    }

    let limit = query.limit.unwrap_or(10);
    let tags = tag_service
        .get_popular_tags(query.space_id.as_deref(), limit)
        .await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "data": tags
    })))
}

pub async fn tag_document(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
    Json(request): Json<TagDocumentRequest>,
) -> Result<Json<Vec<DocumentTag>>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查文档标签权限
    auth_service
        .check_permission(user_id, "docs.tag.manage", Some(&request.document_id))
        .await?;

    let document_tags = tag_service.tag_document(user_id, request).await?;
    Ok(Json(document_tags))
}

pub async fn untag_document(
    Path((document_id, tag_id)): Path<(String, String)>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<StatusCode, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查文档标签权限
    auth_service
        .check_permission(user_id, "docs.tag.manage", Some(&document_id))
        .await?;

    tag_service.untag_document(&document_id, &tag_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_document_tags(
    Path(document_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<DocumentTagsResponse>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查文档读取权限
    auth_service
        .check_permission(user_id, "docs.read", Some(&document_id))
        .await?;

    let tags = tag_service.get_document_tags(&document_id).await?;

    Ok(Json(DocumentTagsResponse { document_id, tags }))
}

pub async fn get_documents_by_tag(
    Path(tag_id): Path<String>,
    Query(query): Query<TagQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<TagDocumentsResponse>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    let tag = tag_service.get_tag(&tag_id).await?;

    // 检查标签读取权限
    if let Some(space_id) = &tag.space_id {
        auth_service
            .check_permission(user_id, "docs.read", Some(&record_id_to_string(space_id)))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.read", None)
            .await?;
    }

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(20);

    let document_ids = tag_service
        .get_documents_by_tag(&tag_id, page, per_page)
        .await?;

    // 简化实现，实际应该查询真实总数
    let total_count = document_ids.len() as i64;

    Ok(Json(TagDocumentsResponse {
        tag_id,
        document_ids,
        total_count,
        page,
        per_page,
    }))
}

pub async fn get_tag_statistics(
    Query(query): Query<TagQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<TagStatistics>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查统计查看权限
    if let Some(space_id) = &query.space_id {
        auth_service
            .check_permission(user_id, "docs.read", Some(space_id))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.read", None)
            .await?;
    }

    let statistics = tag_service
        .get_tag_statistics(query.space_id.as_deref())
        .await?;

    Ok(Json(statistics))
}

pub async fn suggest_tags(
    Query(query): Query<TagQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Vec<Tag>>, ApiError> {
    let user_id = &user.id;
    let tag_service = &app_state.tag_service;
    let auth_service = &app_state.auth_service;
    // 检查读取权限
    if let Some(space_id) = &query.space_id {
        auth_service
            .check_permission(user_id, "docs.read", Some(space_id))
            .await?;
    } else {
        auth_service
            .check_permission(user_id, "docs.read", None)
            .await?;
    }

    let search_query = query.search.unwrap_or_default();
    let limit = query.per_page.unwrap_or(10);

    let tags = tag_service
        .search_tags(query.space_id.as_deref(), &search_query, limit)
        .await?;

    Ok(Json(tags))
}

pub fn router() -> Router {
    Router::new()
        .route("/", get(get_tags).post(create_tag))
        .route("/popular", get(get_popular_tags))
        .route("/suggest", get(suggest_tags))
        .route("/statistics", get(get_tag_statistics))
        .route("/:tag_id", get(get_tag).put(update_tag).delete(delete_tag))
        .route("/:tag_id/documents", get(get_documents_by_tag))
        .route("/documents/tag", post(tag_document))
        .route("/documents/:document_id", get(get_document_tags))
        .route(
            "/documents/:document_id/tags/:tag_id",
            delete(untag_document),
        )
}
