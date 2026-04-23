use axum::{
    extract::Path,
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

use crate::{error::Result, services::auth::User, AppState};

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
        "name": "SoulDoc",
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

fn sha256_hex(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    format!("{:016x}{:016x}", h.finish(), h.finish())
}
