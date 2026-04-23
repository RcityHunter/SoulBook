use crate::{
    error::{AppError, Result},
    models::publication::*,
    services::auth::User,
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

/// 发布相关的路由
pub fn router() -> Router {
    Router::new()
        // 管理端点（需要认证）
        .route("/spaces/:space_id/publish", post(publish_space))
        .route("/spaces/:space_id/publications", get(list_publications))
        .route(
            "/publications/:publication_id",
            put(update_publication).delete(delete_publication),
        )
        .route("/publications/:publication_id/republish", post(republish))
        .route("/publications/:publication_id/unpublish", post(unpublish))
        // 预览端点（需要认证）
        .route(
            "/publications/:publication_id",
            get(get_publication_preview),
        )
        .route(
            "/publications/:publication_id/tree",
            get(get_publication_tree_preview),
        )
        .route(
            "/publications/:publication_id/docs/:doc_slug",
            get(get_publication_document_preview),
        )
        // 公开访问端点（无需认证）
        .route("/p/:slug", get(get_publication))
        .route("/p/:slug/tree", get(get_publication_tree))
        .route("/p/:slug/docs/:doc_slug", get(get_publication_document))
}

/// 发布空间
/// POST /api/docs/publications/spaces/:space_id/publish
async fn publish_space(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_id): Path<String>,
    user: User,
    Json(request): Json<CreatePublicationRequest>,
) -> Result<Json<Value>> {
    // 处理space_id格式 - 移除"space:"前缀如果存在
    let clean_space_id = space_id.strip_prefix("space:").unwrap_or(&space_id);

    // 检查用户是否有权限发布此空间
    let space = app_state
        .space_service
        .get_space_by_id(clean_space_id, Some(&user))
        .await?;

    // 检查空间访问权限
    // 空间访问权限已通过 get_space_by_id/get_space_by_slug 验证

    // 检查发布权限（需要admin或owner角色）
    // 由于owner自动拥有所有权限，这里只需要检查一个高级权限
    if !app_state
        .space_member_service
        .check_permission(&space.id, &user.id, "spaces.manage")
        .await?
    {
        return Err(AppError::Authorization(
            "Only space owners and admins can publish".to_string(),
        ));
    }

    // 创建发布
    let result = app_state
        .publication_service
        .create_publication(&space.id, &user.id, request)
        .await?;

    info!(
        "User {} published space {} as {}",
        user.id, space_id, result.slug
    );

    Ok(Json(json!({
        "success": true,
        "data": result,
        "message": "Space published successfully"
    })))
}

/// 获取空间的发布列表
/// GET /api/docs/publications/spaces/:space_id/publications
async fn list_publications(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_id): Path<String>,
    Query(params): Query<ListPublicationsQuery>,
    user: User,
) -> Result<Json<Value>> {
    info!("list_publications called with space_id: {}", space_id);

    // 处理space_id格式 - 移除"space:"前缀如果存在
    let clean_space_id = space_id.strip_prefix("space:").unwrap_or(&space_id);
    info!("clean_space_id: {}", clean_space_id);

    // 检查用户是否有权限查看此空间
    let space = app_state
        .space_service
        .get_space_by_id(clean_space_id, Some(&user))
        .await?;

    // 空间访问权限已通过 get_space_by_id/get_space_by_slug 验证

    let include_inactive = params.include_inactive.unwrap_or(false);
    let result = app_state
        .publication_service
        .list_publications(&space.id, include_inactive)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": result,
        "message": "Publications retrieved successfully"
    })))
}

/// 更新发布
/// PUT /api/docs/publications/publications/:publication_id
async fn update_publication(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(publication_id): Path<String>,
    user: User,
    Json(request): Json<UpdatePublicationRequest>,
) -> Result<Json<Value>> {
    // 获取发布信息以检查权限
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限
    if !app_state
        .space_member_service
        .check_permission(&publication.space_id, &user.id, "spaces.manage")
        .await?
    {
        return Err(AppError::Authorization(
            "Only space owners and admins can update publications".to_string(),
        ));
    }

    let result = app_state
        .publication_service
        .update_publication(&publication_id, &user.id, request)
        .await?;

    info!("User {} updated publication {}", user.id, publication_id);

    Ok(Json(json!({
        "success": true,
        "data": result,
        "message": "Publication updated successfully"
    })))
}

/// 重新发布（更新内容）
/// POST /api/docs/publications/publications/:publication_id/republish
async fn republish(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(publication_id): Path<String>,
    user: User,
    Json(request): Json<RepublishRequest>,
) -> Result<Json<Value>> {
    // 获取发布信息以检查权限
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限
    if !app_state
        .space_member_service
        .check_permission(&publication.space_id, &user.id, "spaces.manage")
        .await?
    {
        return Err(AppError::Authorization(
            "Only space owners and admins can republish".to_string(),
        ));
    }

    let result = app_state
        .publication_service
        .republish(&publication_id, &user.id, request.change_summary)
        .await?;

    info!(
        "User {} republished {} (v{})",
        user.id, result.slug, result.version
    );

    Ok(Json(json!({
        "success": true,
        "data": result,
        "message": "Content republished successfully"
    })))
}

/// 取消发布
/// POST /api/docs/publications/publications/:publication_id/unpublish
async fn unpublish(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(publication_id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    // 获取发布信息以检查权限
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限
    if !app_state
        .space_member_service
        .check_permission(&publication.space_id, &user.id, "spaces.manage")
        .await?
    {
        return Err(AppError::Authorization(
            "Only space owners and admins can unpublish".to_string(),
        ));
    }

    app_state
        .publication_service
        .unpublish(&publication_id)
        .await?;

    info!(
        "User {} unpublished publication {}",
        user.id, publication_id
    );

    Ok(Json(json!({
        "success": true,
        "data": null,
        "message": "Publication unpublished successfully"
    })))
}

/// 删除发布
/// DELETE /api/docs/publications/publications/:publication_id
async fn delete_publication(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(publication_id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    // 获取发布信息以检查权限
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限（只有owner可以删除）
    // 先检查是否有管理权限
    if !app_state
        .space_member_service
        .check_permission(&publication.space_id, &user.id, "spaces.manage")
        .await?
    {
        return Err(AppError::Authorization(
            "Only space owners can delete publications".to_string(),
        ));
    }

    app_state
        .publication_service
        .delete_publication(&publication_id)
        .await?;

    info!("User {} deleted publication {}", user.id, publication_id);

    Ok(Json(json!({
        "success": true,
        "data": null,
        "message": "Publication deleted successfully"
    })))
}

// ===== 预览端点（需要认证） =====

/// 获取发布详情（预览模式）
/// GET /api/docs/publications/publications/:publication_id
async fn get_publication_preview(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(publication_id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    // 获取发布信息
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限
    if !app_state
        .space_member_service
        .can_access_space(&publication.space_id, Some(&user.id))
        .await?
    {
        return Err(AppError::Authorization(
            "Access denied to this publication".to_string(),
        ));
    }

    // 构建发布信息（包含所有字段，用于预览）
    let preview_info = json!({
        "id": publication.id,
        "slug": publication.slug,
        "title": publication.title,
        "description": publication.description,
        "cover_image": publication.cover_image,
        "theme": publication.theme,
        "version": publication.version,
        "published_at": publication.published_at,
        "updated_at": publication.updated_at,
        "enable_search": publication.enable_search,
        "enable_comments": publication.enable_comments,
        "custom_css": publication.custom_css,
        "custom_js": publication.custom_js,
        "seo_title": publication.seo_title,
        "seo_description": publication.seo_description,
        "seo_keywords": publication.seo_keywords,
    });

    Ok(Json(json!({
        "success": true,
        "data": preview_info,
        "message": "Publication preview retrieved successfully"
    })))
}

/// 获取发布的文档树（预览模式）
/// GET /api/docs/publications/publications/:publication_id/tree
async fn get_publication_tree_preview(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(publication_id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    // 获取发布信息以检查权限
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限
    if !app_state
        .space_member_service
        .can_access_space(&publication.space_id, Some(&user.id))
        .await?
    {
        return Err(AppError::Authorization(
            "Access denied to this publication".to_string(),
        ));
    }

    let tree = app_state
        .publication_service
        .get_publication_tree(&publication_id)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": tree,
        "message": "Document tree retrieved successfully"
    })))
}

/// 获取发布的文档内容（预览模式）
/// GET /api/docs/publications/publications/:publication_id/docs/:doc_slug
async fn get_publication_document_preview(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((publication_id, doc_slug)): Path<(String, String)>,
    user: User,
) -> Result<Json<Value>> {
    // 获取发布信息以检查权限
    let publication = app_state
        .publication_service
        .get_publication_by_id(&publication_id)
        .await?;

    // 检查用户权限
    if !app_state
        .space_member_service
        .can_access_space(&publication.space_id, Some(&user.id))
        .await?
    {
        return Err(AppError::Authorization(
            "Access denied to this publication".to_string(),
        ));
    }

    let document = app_state
        .publication_service
        .get_publication_document(&publication_id, &doc_slug)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": document,
        "message": "Document retrieved successfully"
    })))
}

// ===== 公开访问端点 =====

/// 获取发布详情（公开访问）
/// GET /api/docs/publications/p/:slug
async fn get_publication(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<Value>> {
    let publication = app_state
        .publication_service
        .get_publication_by_slug(&slug)
        .await?;

    // 构建公开的发布信息
    let public_info = json!({
        "slug": publication.slug,
        "title": publication.title,
        "description": publication.description,
        "cover_image": publication.cover_image,
        "theme": publication.theme,
        "version": publication.version,
        "published_at": publication.published_at,
        "updated_at": publication.updated_at,
        "enable_search": publication.enable_search,
        "enable_comments": publication.enable_comments,
        "custom_css": publication.custom_css,
        "custom_js": publication.custom_js,
        "seo_title": publication.seo_title,
        "seo_description": publication.seo_description,
        "seo_keywords": publication.seo_keywords,
    });

    Ok(Json(json!({
        "success": true,
        "data": public_info,
        "message": "Publication retrieved successfully"
    })))
}

/// 获取发布的文档树（公开访问）
/// GET /api/docs/publications/p/:slug/tree
async fn get_publication_tree(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<Value>> {
    // 先获取发布信息
    let publication = app_state
        .publication_service
        .get_publication_by_slug(&slug)
        .await?;

    if let Some(pub_id) = &publication.id {
        let tree = app_state
            .publication_service
            .get_publication_tree(pub_id)
            .await?;

        Ok(Json(json!({
            "success": true,
            "data": tree,
            "message": "Document tree retrieved successfully"
        })))
    } else {
        Err(AppError::Internal(anyhow::anyhow!(
            "Publication ID is missing"
        )))
    }
}

/// 获取发布的文档内容（公开访问）
/// GET /api/docs/publications/p/:slug/docs/:doc_slug
async fn get_publication_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((slug, doc_slug)): Path<(String, String)>,
) -> Result<Json<Value>> {
    // 先获取发布信息
    let publication = app_state
        .publication_service
        .get_publication_by_slug(&slug)
        .await?;

    if let Some(pub_id) = &publication.id {
        let document = app_state
            .publication_service
            .get_publication_document(pub_id, &doc_slug)
            .await?;

        // 记录访问统计
        if let Some(doc_id) = &document.id {
            let _ = app_state
                .publication_service
                .track_document_view(pub_id, doc_id)
                .await;
        }

        Ok(Json(json!({
            "success": true,
            "data": document,
            "message": "Document retrieved successfully"
        })))
    } else {
        Err(AppError::Internal(anyhow::anyhow!(
            "Publication ID is missing"
        )))
    }
}

// ===== 请求结构体 =====

#[derive(Debug, Deserialize)]
struct ListPublicationsQuery {
    include_inactive: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RepublishRequest {
    change_summary: Option<String>,
}
