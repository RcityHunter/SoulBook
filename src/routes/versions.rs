use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    error::ApiError,
    models::version::{CreateVersionRequest, DocumentVersion},
    services::database::record_id_to_string,
    services::{
        auth::User,
        versions::{VersionComparison, VersionHistorySummary, VersionService},
    },
};

#[derive(Deserialize)]
pub struct VersionQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub author_id: Option<String>,
}

#[derive(Serialize)]
pub struct VersionListResponse {
    pub versions: Vec<DocumentVersion>,
    pub total_count: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

#[derive(Deserialize)]
pub struct RestoreVersionRequest {
    pub summary: Option<String>,
}

#[derive(Deserialize)]
pub struct CompareVersionsQuery {
    pub from_version: String,
    pub to_version: String,
}

pub async fn get_document_versions(
    Path(document_id): Path<String>,
    Query(query): Query<VersionQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<VersionListResponse>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(20);

    let versions = if let Some(author_id) = query.author_id {
        version_service
            .get_versions_by_author(&document_id, &author_id)
            .await?
    } else {
        version_service
            .get_document_versions(&document_id, page, per_page)
            .await?
    };

    // 获取总数（简化实现）
    let total_count = versions.len() as i64;
    let total_pages = (total_count + per_page - 1) / per_page;

    Ok(Json(VersionListResponse {
        versions,
        total_count,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn create_document_version(
    Path(document_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
    Json(request): Json<CreateVersionRequest>,
) -> Result<Json<DocumentVersion>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.update", Some(&document_id))
        .await?;

    let version = version_service
        .create_version(&document_id, &user.id, request)
        .await?;

    Ok(Json(version))
}

pub async fn get_version(
    Path((document_id, version_id)): Path<(String, String)>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<DocumentVersion>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    let version = version_service.get_version(&version_id).await?;

    // 验证版本属于指定文档
    if record_id_to_string(&version.document_id) != format!("document:{}", document_id) {
        return Err(ApiError::NotFound("Version not found".to_string()));
    }

    Ok(Json(version))
}

pub async fn get_current_version(
    Path(document_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Option<DocumentVersion>>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    let current_version = version_service.get_current_version(&document_id).await?;

    Ok(Json(current_version))
}

pub async fn restore_version(
    Path((document_id, version_id)): Path<(String, String)>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
    Json(_request): Json<RestoreVersionRequest>,
) -> Result<Json<DocumentVersion>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.update", Some(&document_id))
        .await?;

    let restored_version = version_service
        .restore_version(&document_id, &version_id, &user.id)
        .await?;

    Ok(Json(restored_version))
}

pub async fn compare_versions(
    Path(document_id): Path<String>,
    Query(query): Query<CompareVersionsQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<VersionComparison>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    let comparison = version_service
        .compare_versions(&query.from_version, &query.to_version)
        .await?;

    Ok(Json(comparison))
}

pub async fn get_version_history_summary(
    Path(document_id): Path<String>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<VersionHistorySummary>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    let summary = version_service
        .get_version_history_summary(&document_id)
        .await?;

    Ok(Json(summary))
}

pub async fn delete_version(
    Path((document_id, version_id)): Path<(String, String)>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<StatusCode, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    // 只有文档管理员或版本作者可以删除版本
    auth_service
        .check_permission(&user.id, "docs.admin", Some(&document_id))
        .await?;

    version_service.delete_version(&version_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_version_diff(
    Path((document_id, version_id)): Path<(String, String)>,
    Query(query): Query<CompareVersionsQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<VersionComparison>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    // 比较指定版本与前一个版本
    let comparison = version_service
        .compare_versions(&version_id, &query.to_version)
        .await?;

    Ok(Json(comparison))
}

pub async fn get_versions_by_date_range(
    Path(document_id): Path<String>,
    Query(query): Query<DateRangeQuery>,
    Extension(app_state): Extension<Arc<crate::AppState>>,
    user: User,
) -> Result<Json<Vec<DocumentVersion>>, ApiError> {
    let version_service = &app_state.version_service;
    let auth_service = &app_state.auth_service;
    auth_service
        .check_permission(&user.id, "docs.read", Some(&document_id))
        .await?;

    // 简化实现，实际应该根据日期范围过滤
    let versions = version_service
        .get_document_versions(&document_id, 1, 100)
        .await?;

    Ok(Json(versions))
}

#[derive(Deserialize)]
pub struct DateRangeQuery {
    pub from_date: Option<String>,
    pub to_date: Option<String>,
}

pub fn router() -> Router {
    Router::new()
        .route(
            "/:document_id/versions",
            get(get_document_versions).post(create_document_version),
        )
        .route("/:document_id/versions/current", get(get_current_version))
        .route(
            "/:document_id/versions/summary",
            get(get_version_history_summary),
        )
        .route("/:document_id/versions/compare", get(compare_versions))
        .route(
            "/:document_id/versions/date-range",
            get(get_versions_by_date_range),
        )
        .route(
            "/:document_id/versions/:version_id",
            get(get_version).delete(delete_version),
        )
        .route(
            "/:document_id/versions/:version_id/restore",
            post(restore_version),
        )
        .route(
            "/:document_id/versions/:version_id/diff",
            get(get_version_diff),
        )
}
