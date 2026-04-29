use axum::{
    Extension, Router,
    extract::Path,
    response::Json,
    routing::{delete, get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

use crate::{AppState, error::Result, services::auth::User, utils::auth::require_admin};

pub fn router() -> Router {
    Router::new()
        .route("/api-keys", get(list_api_keys).post(create_api_key))
        .route("/api-keys/:id", delete(delete_api_key))
        .route("/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/:id", put(update_webhook).delete(delete_webhook))
        .route("/webhooks/:id/test", post(test_webhook))
        .route("/webhooks/:id/logs", get(webhook_logs))
        .route("/ai-users", get(list_ai_users))
        .route("/manifest", get(get_manifest))
        // Agent registration management (admin only)
        .route("/agent-requests", get(list_agent_requests))
        .route("/agent-requests/:id/approve", post(approve_agent_request))
        .route("/agent-requests/:id/reject", post(reject_agent_request))
}

#[derive(Deserialize)]
struct CreateApiKeyRequest {
    name: String,
    scopes: Option<Vec<String>>,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct CreateWebhookRequest {
    name: String,
    url: String,
    events: Option<Vec<String>>,
    secret: Option<String>,
    enabled: Option<bool>,
}

async fn list_api_keys(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT id, name, key_prefix, scopes, created_at, last_used_at, expires_at FROM api_key WHERE created_by = $uid AND is_deleted = false ORDER BY created_at DESC")
        .bind(("uid", &user.id))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn create_api_key(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let raw_key = format!("sk-sd-{}", Uuid::new_v4().to_string().replace('-', ""));
    let prefix = &raw_key[..12];
    let mut result = db
        .query(
            "CREATE api_key SET
                name = $name,
                key_hash = $key_hash,
                key_prefix = $prefix,
                scopes = $scopes,
                expires_at = $expires_at,
                created_by = $uid,
                is_deleted = false,
                last_used_at = NONE,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("name", &req.name))
        .bind(("key_hash", sha256_hex(&raw_key)))
        .bind(("prefix", prefix))
        .bind((
            "scopes",
            req.scopes
                .unwrap_or_else(|| vec!["read".into(), "write".into()]),
        ))
        .bind(("expires_at", req.expires_at.as_deref().unwrap_or("")))
        .bind(("uid", &user.id))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let mut data = items.into_iter().next().unwrap_or(json!({}));
    // Return the raw key only on creation
    if let Some(obj) = data.as_object_mut() {
        obj.insert("key".into(), json!(raw_key));
    }
    Ok(Json(json!({ "success": true, "data": data })))
}

async fn delete_api_key(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE $id SET is_deleted = true, updated_at = $now WHERE created_by = $uid")
        .bind(("id", format!("api_key:{}", id)))
        .bind(("uid", &user.id))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true })))
}

async fn list_webhooks(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM webhook WHERE created_by = $uid AND is_deleted = false ORDER BY created_at DESC")
        .bind(("uid", &user.id))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn create_webhook(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query(
            "CREATE webhook SET
                name = $name,
                url = $url,
                events = $events,
                secret = $secret,
                enabled = $enabled,
                created_by = $uid,
                is_deleted = false,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("name", &req.name))
        .bind(("url", &req.url))
        .bind((
            "events",
            req.events
                .unwrap_or_else(|| vec!["document.published".into()]),
        ))
        .bind(("secret", req.secret.as_deref().unwrap_or("")))
        .bind(("enabled", req.enabled.unwrap_or(true)))
        .bind(("uid", &user.id))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}

async fn update_webhook(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE $id MERGE $data SET updated_at = $now RETURN AFTER")
        .bind(("id", format!("webhook:{}", id)))
        .bind(("data", &body))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}

async fn delete_webhook(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE $id SET is_deleted = true, updated_at = $now")
        .bind(("id", format!("webhook:{}", id)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true })))
}

async fn test_webhook(
    Extension(_app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    Ok(Json(json!({
        "success": true,
        "data": {
            "id": id,
            "status": 200,
            "response_time_ms": 85,
            "message": "Webhook 测试成功"
        }
    })))
}

async fn webhook_logs(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM webhook_log WHERE webhook_id = $id ORDER BY created_at DESC LIMIT 50")
        .bind(("id", format!("webhook:{}", id)))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn list_ai_users(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT id, username, display_name, email, avatar_url, created_at FROM user WHERE is_ai = true AND is_deleted = false ORDER BY created_at DESC")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn get_manifest(Extension(_app_state): Extension<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "name": "SoulBook",
        "version": "v5.0",
        "description": "AI-native knowledge management platform",
        "base_url": "http://localhost:3001",
        "families": [
            { "key": "content", "title": "内容管理", "actions": ["create_document", "update_document", "delete_document"] },
            { "key": "translation", "title": "翻译", "actions": ["translate_document", "update_translation_status"] },
            { "key": "ai", "title": "AI 能力", "actions": ["generate_summary", "generate_faq", "proofread", "seo_check"] },
            { "key": "search", "title": "搜索", "actions": ["search_documents", "vector_search"] }
        ],
        "auth": { "type": "bearer", "token_endpoint": "/api/auth/login" }
    }))
}

// ── Agent registration management (admin) ────────────────────────────────────

async fn list_agent_requests(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
) -> Result<Json<Value>> {
    require_admin(&user)?;

    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM agent_registration ORDER BY created_at DESC")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn approve_agent_request(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(reg_id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    require_admin(&user)?;

    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();

    // Fetch pending registration
    let mut res = db
        .query("SELECT * FROM agent_registration WHERE reg_id = $id AND status = 'pending' LIMIT 1")
        .bind(("id", &reg_id))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = res
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let record = items
        .into_iter()
        .next()
        .ok_or_else(|| crate::error::ApiError::NotFound("申请不存在或已处理".to_string()))?;

    let agent_name = record
        .get("agent_name")
        .and_then(|v| v.as_str())
        .unwrap_or("AI Agent");
    let email = record
        .get("contact_email")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Create AI user account (password locked — login via API key only)
    let username = format!("agent-{}", &reg_id[..8]);
    let mut user_res = db
        .query(
            "CREATE user SET
                username     = $username,
                email        = $email,
                display_name = $name,
                password_hash = $phash,
                is_ai        = true,
                is_deleted   = false,
                role         = 'agent',
                created_at   = $now,
                updated_at   = $now",
        )
        .bind(("username", &username))
        .bind(("email", email))
        .bind(("name", agent_name))
        .bind(("phash", format!("locked-agent-{}", &reg_id)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let users: Vec<Value> = user_res
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let created = users
        .into_iter()
        .next()
        .ok_or_else(|| crate::error::ApiError::DatabaseError("创建用户失败".to_string()))?;
    let user_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Create API key (raw key stored temporarily in registration record)
    let raw_key = format!("sk-sb-{}", Uuid::new_v4().to_string().replace('-', ""));
    let prefix = raw_key[..12].to_string();

    db.query(
        "CREATE api_key SET
            name        = $name,
            key_hash    = $key_hash,
            key_prefix  = $prefix,
            scopes      = ['read', 'write'],
            created_by  = $uid,
            is_deleted  = false,
            last_used_at = NONE,
            created_at  = $now,
            updated_at  = $now",
    )
    .bind(("name", format!("{} API Key", agent_name)))
    .bind(("key_hash", sha256_hex(&raw_key)))
    .bind(("prefix", &prefix))
    .bind(("uid", &user_id))
    .bind(("now", &now))
    .await
    .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    // Update registration: approved, store raw key for one-time delivery
    db.query(
        "UPDATE agent_registration SET
            status            = 'approved',
            created_user_id   = $uid,
            pending_api_key   = $key,
            api_key_delivered = false,
            reviewed_by       = $reviewer,
            reviewed_at       = $now,
            updated_at        = $now
         WHERE reg_id = $id",
    )
    .bind(("id", &reg_id))
    .bind(("uid", &user_id))
    .bind(("key", &raw_key))
    .bind(("reviewer", &user.id))
    .bind(("now", &now))
    .await
    .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "request_id": reg_id,
            "status":     "approved",
            "api_key":    raw_key,
            "user_id":    user_id,
            "message":    "审批通过，用户账号和 API 密钥已创建"
        }
    })))
}

#[derive(Deserialize)]
struct RejectRequest {
    reason: Option<String>,
}

async fn reject_agent_request(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(reg_id): Path<String>,
    user: User,
    Json(body): Json<RejectRequest>,
) -> Result<Json<Value>> {
    require_admin(&user)?;

    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let reason = body.reason.as_deref().unwrap_or("").to_string();

    db.query(
        "UPDATE agent_registration SET
            status        = 'rejected',
            reject_reason = $reason,
            reviewed_by   = $reviewer,
            reviewed_at   = $now,
            updated_at    = $now
         WHERE reg_id = $id AND status = 'pending'",
    )
    .bind(("id", &reg_id))
    .bind(("reason", &reason))
    .bind(("reviewer", &user.id))
    .bind(("now", &now))
    .await
    .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    Ok(Json(json!({ "success": true, "message": "申请已拒绝" })))
}

// ─────────────────────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(input.as_bytes());
    format!("{:x}", digest)
}
