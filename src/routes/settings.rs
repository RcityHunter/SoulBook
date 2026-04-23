use axum::{
    response::Json,
    routing::{get, put},
    Extension, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{error::Result, services::auth::User, AppState};

pub fn router() -> Router {
    Router::new()
        .route("/", get(get_settings))
        .route("/general", put(update_general))
        .route("/ai", put(update_ai))
        .route("/notifications", put(update_notifications))
        .route("/security", put(update_security))
        .route("/appearance", put(update_appearance))
}

async fn get_settings(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM settings WHERE id = settings:global LIMIT 1")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let settings = items.into_iter().next().unwrap_or_else(|| {
        json!({
            "general": {
                "platform_name": "SoulDoc",
                "default_language": "zh-CN",
                "timezone": "Asia/Shanghai",
                "default_visibility": "private"
            },
            "ai": {
                "default_model": "claude-3-5-sonnet",
                "anthropic_api_key": "",
                "concurrent_tasks": 4,
                "auto_summary": true,
                "ai_translation": true,
                "seo_auto_check": false
            },
            "notifications": {
                "email": true,
                "browser": true,
                "ai_tasks": false
            },
            "security": {
                "two_factor": false,
                "sso_enabled": true,
                "session_timeout": 480
            },
            "appearance": {
                "dark_mode": false,
                "primary_color": "#4f46e5"
            }
        })
    });

    Ok(Json(json!({ "success": true, "data": settings })))
}

async fn update_general(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    merge_settings(&app_state, "general", body).await
}

async fn update_ai(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    merge_settings(&app_state, "ai", body).await
}

async fn update_notifications(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    merge_settings(&app_state, "notifications", body).await
}

async fn update_security(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    merge_settings(&app_state, "security", body).await
}

async fn update_appearance(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    merge_settings(&app_state, "appearance", body).await
}

async fn merge_settings(
    app_state: &Arc<AppState>,
    section: &str,
    data: Value,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let sql = format!(
        "UPDATE settings:global SET {} = $data, updated_at = $now RETURN AFTER",
        section
    );
    let mut result = db
        .query(sql)
        .bind(("data", &data))
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
