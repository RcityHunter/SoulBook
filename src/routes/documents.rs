use crate::models::document::{CreateDocumentRequest, DocumentQuery, UpdateDocumentRequest};
use crate::services::auth::{OptionalUser, User};
use crate::{
    error::{AppError, Result},
    AppState,
};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

fn normalize_user_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let no_prefix = trimmed
        .strip_prefix("user:")
        .or_else(|| trimmed.strip_prefix("users:"))
        .unwrap_or(trimmed)
        .trim();
    no_prefix
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string()
}

fn is_space_owner(space_owner_id: &str, user_id: &str) -> bool {
    normalize_user_id(space_owner_id) == normalize_user_id(user_id)
}

pub fn router() -> Router {
    Router::new()
        .route("/:space_slug", get(list_documents).post(create_document))
        .route("/:space_slug/tree", get(get_document_tree))
        .route("/:space_slug/batch-delete", post(batch_delete_documents))
        .route("/:space_slug/batch-publish", post(batch_publish_documents))
        .route("/:space_slug/import", post(import_document))
        .route("/create/tree", get(handle_legacy_create_tree)) // Legacy frontend support
        .route(
            "/:space_slug/:doc_slug",
            get(get_document)
                .put(update_document)
                .delete(delete_document),
        )
        .route("/:space_slug/:doc_slug/move", post(move_document_handler))
        .route(
            "/:space_slug/:doc_slug/duplicate",
            post(duplicate_document_handler),
        )
        .route("/:space_slug/:doc_slug/export", get(export_document))
        .route(
            "/:space_slug/:doc_slug/children",
            get(get_document_children),
        )
        .route(
            "/:space_slug/:doc_slug/breadcrumbs",
            get(get_document_breadcrumbs),
        )
        .route(
            "/id/:doc_id",
            get(get_document_by_id)
                .put(update_document_by_id)
                .delete(delete_document_by_id),
        )
        .route("/id/:doc_id/children", get(get_document_children_by_id))
        .route(
            "/id/:doc_id/breadcrumbs",
            get(get_document_breadcrumbs_by_id),
        )
}

/// 获取文档列表
/// GET /api/docs/:space_slug
async fn list_documents(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    Query(query): Query<DocumentQuery>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    // 获取空间信息进行权限检查
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, user.as_ref())
        .await?;

    // 检查读取权限（包括成员权限）
    if let Some(user) = &user {
        if !is_space_owner(&space.owner_id, &user.id) {
            if !app_state
                .space_member_service
                .can_access_space(&space.id, Some(&user.id))
                .await?
            {
                return Err(AppError::Authorization(
                    "Access denied to this space".to_string(),
                ));
            }
            if !app_state
                .space_member_service
                .check_permission(&space.id, &user.id, "docs.read")
                .await?
            {
                return Err(AppError::Authorization(
                    "Permission denied: docs.read required".to_string(),
                ));
            }
        }
    } else {
        // 未登录用户只能访问公开空间
        if !space.is_public {
            return Err(AppError::Authorization(
                "Access denied to private space".to_string(),
            ));
        }
    }

    let result = app_state
        .document_service
        .list_documents(&space.id, query, user.as_ref())
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": result,
        "message": "Documents retrieved successfully"
    })))
}

/// 创建新文档
/// POST /api/docs/:space_slug
async fn create_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    user: User,
    Json(request): Json<CreateDocumentRequest>,
) -> Result<Json<Value>> {
    // 根据slug获取space
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    // 检查空间访问和文档写入权限
    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .can_access_space(&space.id, Some(&user.id))
            .await?
        {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.write")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.write required".to_string(),
            ));
        }
    }

    let result = app_state
        .document_service
        .create_document(&space.id, &user.id, request)
        .await?;

    info!(
        "User {} created document: {} in space: {}",
        user.id, result.slug, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": result,
        "message": "Document created successfully"
    })))
}

/// 获取文档详情
/// GET /api/docs/:space_slug/:doc_slug
async fn get_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let space_service = &app_state.space_service;
    let document_service = &app_state.document_service;
    let auth_service = &app_state.auth_service;

    // 根据slug获取space
    let space = space_service
        .get_space_by_slug(&space_slug, user.as_ref())
        .await?;

    // 检查读取权限
    if let Some(user) = &user {
        auth_service
            .check_permission(&user.id, "docs.read", Some(&space.id))
            .await?;
    }

    // 根据slug获取document
    let document = document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": document,
        "message": "Document retrieved successfully"
    })))
}

/// 更新文档
/// PUT /api/docs/:space_slug/:doc_slug
async fn update_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    user: User,
    Json(request): Json<UpdateDocumentRequest>,
) -> Result<Json<Value>> {
    let space_service = &app_state.space_service;
    let document_service = &app_state.document_service;
    let auth_service = &app_state.auth_service;

    // 根据slug获取space
    let space = space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    // 检查写入权限
    auth_service
        .check_permission(&user.id, "docs.write", Some(&space.id))
        .await?;

    // 根据slug获取document
    let document = document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    // 更新文档
    let document_id = document
        .id
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Document ID is missing")))?;
    let updated_document = document_service
        .update_document(document_id, &user.id, request)
        .await?;

    info!(
        "User {} updated document: {} in space: {}",
        user.id, doc_slug, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": updated_document,
        "message": "Document updated successfully"
    })))
}

/// 删除文档
/// DELETE /api/docs/:space_slug/:doc_slug
async fn delete_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    user: User,
) -> Result<Json<Value>> {
    let space_service = &app_state.space_service;
    let document_service = &app_state.document_service;
    let auth_service = &app_state.auth_service;

    // 根据slug获取space
    let space = space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    // 检查删除权限
    auth_service
        .check_permission(&user.id, "docs.delete", Some(&space.id))
        .await?;

    // 根据slug获取document
    let document = document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    // 删除文档
    let document_id = document
        .id
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Document ID is missing")))?;
    document_service
        .delete_document(document_id, &user.id)
        .await?;

    info!(
        "User {} deleted document: {} in space: {}",
        user.id, doc_slug, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": null,
        "message": "Document deleted successfully"
    })))
}

/// 获取文档树结构
/// GET /api/docs/:space_slug/tree
async fn get_document_tree(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let space_service = &app_state.space_service;
    let document_service = &app_state.document_service;

    // 根据slug获取space
    let space = space_service
        .get_space_by_slug(&space_slug, user.as_ref())
        .await?;

    // 检查读取权限（包括成员权限）
    if let Some(user) = &user {
        if !is_space_owner(&space.owner_id, &user.id) {
            // 首先检查基础权限
            if !app_state
                .space_member_service
                .can_access_space(&space.id, Some(&user.id))
                .await?
            {
                return Err(
                    AppError::Authorization("Access denied to this space".to_string()).into(),
                );
            }
            // 然后检查具体的docs.read权限
            if !app_state
                .space_member_service
                .check_permission(&space.id, &user.id, "docs.read")
                .await?
            {
                return Err(AppError::Authorization(
                    "Permission denied: docs.read required".to_string(),
                )
                .into());
            }
        }
    } else {
        // 未登录用户只能访问公开空间
        if !space.is_public {
            return Err(
                AppError::Authorization("Access denied to private space".to_string()).into(),
            );
        }
    }

    // 获取文档树结构，传递空间ID
    let tree = document_service.get_document_tree(&space.id).await?;

    Ok(Json(json!({
        "success": true,
        "data": tree,
        "message": "Document tree retrieved successfully"
    })))
}

/// 获取文档子级
/// GET /api/docs/:space_slug/:doc_slug/children
async fn get_document_children(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let space_service = &app_state.space_service;
    let document_service = &app_state.document_service;
    let auth_service = &app_state.auth_service;

    // 根据slug获取space
    let space = space_service
        .get_space_by_slug(&space_slug, user.as_ref())
        .await?;

    // 检查读取权限
    if let Some(user) = &user {
        auth_service
            .check_permission(&user.id, "docs.read", Some(&space.id))
            .await?;
    }

    // 根据slug获取document
    let document = document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    // 获取文档子级
    let document_id = document
        .id
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Document ID is missing")))?;
    let children = document_service.get_document_children(document_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": children,
        "message": "Document children retrieved successfully"
    })))
}

/// 获取文档面包屑导航
/// GET /api/docs/:space_slug/:doc_slug/breadcrumbs
async fn get_document_breadcrumbs(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let space_service = &app_state.space_service;
    let document_service = &app_state.document_service;
    let auth_service = &app_state.auth_service;

    // 根据slug获取space
    let space = space_service
        .get_space_by_slug(&space_slug, user.as_ref())
        .await?;

    // 检查读取权限
    if let Some(user) = &user {
        auth_service
            .check_permission(&user.id, "docs.read", Some(&space.id))
            .await?;
    }

    // 根据slug获取document
    let document = document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    // 获取文档面包屑
    let document_id = document
        .id
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Document ID is missing")))?;
    let breadcrumbs = document_service
        .get_document_breadcrumbs(document_id)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": breadcrumbs,
        "message": "Document breadcrumbs retrieved successfully"
    })))
}

/// Legacy handler for frontend calls to /create/tree
/// This is a temporary compatibility route
async fn handle_legacy_create_tree(
    Extension(_app_state): Extension<Arc<AppState>>,
    OptionalUser(_user): OptionalUser,
) -> Result<Json<Value>> {
    Err(AppError::BadRequest(
        "Invalid endpoint. Please use '/api/docs/documents/{space_slug}/tree' instead.".to_string(),
    ))
}

/// 根据ID获取文档详情
/// GET /api/docs/documents/id/:doc_id
async fn get_document_by_id(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(doc_id): Path<String>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let document_service = &app_state.document_service;

    // 根据ID获取document
    let document = document_service.get_document_by_id(&doc_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": document,
        "message": "Document retrieved successfully"
    })))
}

/// 根据ID更新文档
/// PUT /api/docs/documents/id/:doc_id
async fn update_document_by_id(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(doc_id): Path<String>,
    user: User,
    Json(request): Json<UpdateDocumentRequest>,
) -> Result<Json<Value>> {
    let document_service = &app_state.document_service;

    // 根据ID获取document
    let document = document_service.get_document_by_id(&doc_id).await?;

    // 从document.space_id中提取空间ID
    let space_id = if document.space_id.starts_with("space:") {
        document.space_id.strip_prefix("space:").unwrap()
    } else {
        &document.space_id
    };

    // 获取文档所属的空间信息进行权限检查
    let space = app_state
        .space_service
        .get_space_by_id(space_id, Some(&user))
        .await?;

    // 检查写入权限
    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .can_access_space(&space.id, Some(&user.id))
            .await?
        {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.write")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.write required".to_string(),
            ));
        }
    }

    // 更新文档
    let updated_document = document_service
        .update_document(&doc_id, &user.id, request)
        .await?;

    info!("User {} updated document: {} by ID", user.id, doc_id);

    Ok(Json(json!({
        "success": true,
        "data": updated_document,
        "message": "Document updated successfully"
    })))
}

/// 根据ID删除文档
/// DELETE /api/docs/documents/id/:doc_id
async fn delete_document_by_id(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(doc_id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    let document_service = &app_state.document_service;

    // 根据ID获取document
    let document = document_service.get_document_by_id(&doc_id).await?;

    // 从document.space_id中提取空间ID
    let space_id = if document.space_id.starts_with("space:") {
        document.space_id.strip_prefix("space:").unwrap()
    } else {
        &document.space_id
    };

    // 获取文档所属的空间信息进行权限检查
    let space = app_state
        .space_service
        .get_space_by_id(space_id, Some(&user))
        .await?;

    // 检查删除权限
    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .can_access_space(&space.id, Some(&user.id))
            .await?
        {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.delete")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.delete required".to_string(),
            ));
        }
    }

    // 删除文档
    document_service.delete_document(&doc_id, &user.id).await?;

    info!("User {} deleted document: {} by ID", user.id, doc_id);

    Ok(Json(json!({
        "success": true,
        "data": null,
        "message": "Document deleted successfully"
    })))
}

/// 根据ID获取文档子级
/// GET /api/docs/documents/id/:doc_id/children
async fn get_document_children_by_id(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(doc_id): Path<String>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let document_service = &app_state.document_service;

    // 获取文档子级
    let children = document_service
        .get_document_children_by_id(&doc_id)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": children,
        "message": "Document children retrieved successfully"
    })))
}

/// 根据ID获取文档面包屑导航
/// GET /api/docs/documents/id/:doc_id/breadcrumbs
async fn get_document_breadcrumbs_by_id(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(doc_id): Path<String>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let document_service = &app_state.document_service;

    // 获取文档面包屑
    let breadcrumbs = document_service
        .get_document_breadcrumbs_by_id(&doc_id)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": breadcrumbs,
        "message": "Document breadcrumbs retrieved successfully"
    })))
}

// ─── batch-delete ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BatchDeleteRequest {
    document_ids: Vec<String>,
}

async fn batch_delete_documents(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    user: User,
    Json(request): Json<BatchDeleteRequest>,
) -> Result<Json<Value>> {
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .can_access_space(&space.id, Some(&user.id))
            .await?
        {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.delete")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.delete required".to_string(),
            ));
        }
    }

    let mut deleted_count = 0usize;
    let mut failed_ids: Vec<String> = Vec::new();

    for doc_id in &request.document_ids {
        match app_state
            .document_service
            .delete_document(doc_id, &user.id)
            .await
        {
            Ok(_) => deleted_count += 1,
            Err(_) => failed_ids.push(doc_id.clone()),
        }
    }

    info!(
        "User {} batch-deleted {} documents in space {}",
        user.id, deleted_count, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": { "deleted_count": deleted_count, "failed_ids": failed_ids },
        "message": format!("Deleted {} documents", deleted_count)
    })))
}

// ─── batch-publish ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BatchPublishRequest {
    document_ids: Vec<String>,
    is_public: bool,
}

async fn batch_publish_documents(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    user: User,
    Json(request): Json<BatchPublishRequest>,
) -> Result<Json<Value>> {
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .can_access_space(&space.id, Some(&user.id))
            .await?
        {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.write")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.write required".to_string(),
            ));
        }
    }

    let mut updated_count = 0usize;
    let mut failed_ids: Vec<String> = Vec::new();

    for doc_id in &request.document_ids {
        let patch = UpdateDocumentRequest {
            title: None,
            content: None,
            excerpt: None,
            is_public: Some(request.is_public),
            status: None,
            parent_id: None,
            order_index: None,
            metadata: None,
        };
        match app_state
            .document_service
            .update_document(doc_id, &user.id, patch)
            .await
        {
            Ok(_) => updated_count += 1,
            Err(_) => failed_ids.push(doc_id.clone()),
        }
    }

    Ok(Json(json!({
        "success": true,
        "data": { "updated_count": updated_count, "failed_ids": failed_ids },
        "message": format!("Updated {} documents", updated_count)
    })))
}

// ─── import ──────────────────────────────────────────────────────────────────

async fn import_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    user: User,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .can_access_space(&space.id, Some(&user.id))
            .await?
        {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.write")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.write required".to_string(),
            ));
        }
    }

    let mut title = String::new();
    let mut content = String::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let filename = field
                    .file_name()
                    .unwrap_or("import.md")
                    .trim_end_matches(".md")
                    .to_string();
                if title.is_empty() {
                    title = filename;
                }
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                content = String::from_utf8(bytes.to_vec())
                    .map_err(|_| AppError::BadRequest("File must be valid UTF-8".to_string()))?;
            }
            "title" => {
                title = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
            }
            _ => {}
        }
    }

    if content.is_empty() {
        return Err(AppError::BadRequest("No file content provided".to_string()));
    }

    let title = if title.is_empty() {
        "Imported Document".to_string()
    } else {
        title
    };
    let slug = slug::slugify(&title);

    let doc_request = CreateDocumentRequest {
        title,
        slug,
        content: Some(content),
        excerpt: None,
        is_public: Some(false),
        status: None,
        parent_id: None,
        order_index: None,
        metadata: None,
    };

    let document = app_state
        .document_service
        .create_document(&space.id, &user.id, doc_request)
        .await?;

    info!(
        "User {} imported document '{}' into space {}",
        user.id, document.slug, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": document,
        "message": "Document imported successfully"
    })))
}

// ─── move ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MoveDocumentRequest {
    new_parent_id: Option<String>,
    new_order_index: Option<i32>,
}

async fn move_document_handler(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    user: User,
    Json(request): Json<MoveDocumentRequest>,
) -> Result<Json<Value>> {
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.write")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.write required".to_string(),
            ));
        }
    }

    let document = app_state
        .document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    let doc_id = document
        .id
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Document ID missing")))?;

    let updated = app_state
        .document_service
        .move_document(
            doc_id,
            request.new_parent_id,
            request.new_order_index,
            &user.id,
        )
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": updated,
        "message": "Document moved successfully"
    })))
}

// ─── duplicate ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DuplicateDocumentRequest {
    new_title: Option<String>,
    new_slug: Option<String>,
}

async fn duplicate_document_handler(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    user: User,
    Json(request): Json<DuplicateDocumentRequest>,
) -> Result<Json<Value>> {
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    if !is_space_owner(&space.owner_id, &user.id) {
        if !app_state
            .space_member_service
            .check_permission(&space.id, &user.id, "docs.write")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: docs.write required".to_string(),
            ));
        }
    }

    let document = app_state
        .document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    let doc_id = document
        .id
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Document ID missing")))?;

    let duplicated = app_state
        .document_service
        .duplicate_document(doc_id, request.new_title, request.new_slug, &user.id)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": duplicated,
        "message": "Document duplicated successfully"
    })))
}

// ─── export ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

async fn export_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    Query(params): Query<ExportQuery>,
    OptionalUser(user): OptionalUser,
) -> Result<Json<Value>> {
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, user.as_ref())
        .await?;

    if !space.is_public {
        match &user {
            None => {
                return Err(AppError::Authorization(
                    "Authentication required".to_string(),
                ))
            }
            Some(u) => {
                if !is_space_owner(&space.owner_id, &u.id)
                    && !app_state
                        .space_member_service
                        .can_access_space(&space.id, Some(&u.id))
                        .await?
                {
                    return Err(AppError::Authorization(
                        "Access denied to this space".to_string(),
                    ));
                }
            }
        }
    }

    let document = app_state
        .document_service
        .get_document_by_slug(&space.id, &doc_slug)
        .await?;

    let format = params.format.as_deref().unwrap_or("markdown");

    let (content, extension) = match format {
        "html" => {
            use pulldown_cmark::{html, Options, Parser};
            let parser = Parser::new_ext(&document.content, Options::all());
            let mut html_output = String::new();
            html::push_html(&mut html_output, parser);
            (html_output, "html")
        }
        "markdown" | "md" => (document.content.clone(), "md"),
        other => {
            return Err(AppError::BadRequest(format!(
                "Unsupported export format: {}. Use 'markdown' or 'html'.",
                other
            )))
        }
    };

    Ok(Json(json!({
        "success": true,
        "data": {
            "content": content,
            "format": format,
            "filename": format!("{}.{}", document.slug, extension),
            "title": document.title
        },
        "message": "Document exported successfully"
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::document::CreateDocumentRequest;
    use validator::Validate;

    #[tokio::test]
    async fn test_create_document_validation() {
        let request = CreateDocumentRequest {
            title: "".to_string(), // 无效：空标题
            slug: "test-doc".to_string(),
            content: None,
            excerpt: None,
            is_public: None,
            status: None,
            parent_id: None,
            order_index: None,
            metadata: None,
        };

        assert!(request.validate().is_err());
    }

    #[tokio::test]
    async fn test_document_slug_validation() {
        let valid_request = CreateDocumentRequest {
            title: "Test Document".to_string(),
            slug: "test-document".to_string(),
            content: Some("# Test Content".to_string()),
            excerpt: None,
            is_public: Some(true),
            status: None,
            parent_id: None,
            order_index: Some(1),
            metadata: None,
        };

        assert!(valid_request.validate().is_ok());

        let invalid_request = CreateDocumentRequest {
            title: "Test Document".to_string(),
            slug: "Test Document".to_string(), // 无效：包含空格和大写
            content: None,
            excerpt: None,
            is_public: None,
            status: None,
            parent_id: None,
            order_index: None,
            metadata: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_title_length_validation() {
        let long_title = "x".repeat(201); // 超过200字符限制

        let request = CreateDocumentRequest {
            title: long_title,
            slug: "test-doc".to_string(),
            content: None,
            excerpt: None,
            is_public: None,
            status: None,
            parent_id: None,
            order_index: None,
            metadata: None,
        };

        assert!(request.validate().is_err());
    }
}
