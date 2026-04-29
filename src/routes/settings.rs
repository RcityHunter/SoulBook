use axum::{
    Extension, Router,
    response::Json,
    routing::{get, put},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::warn;

use crate::{AppState, error::Result, services::auth::User};

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
    let settings = match db
        .query("SELECT * FROM settings WHERE id = settings:global LIMIT 1")
        .await
    {
        Ok(mut result) => match result.take::<Vec<Value>>(0) {
            Ok(items) => items.into_iter().next().unwrap_or_else(default_settings),
            Err(e) => {
                warn!("failed to parse settings, using defaults: {}", e);
                default_settings()
            }
        },
        Err(e) => {
            warn!("failed to query settings, using defaults: {}", e);
            default_settings()
        }
    };

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
    let mut result = match db
        .query(sql)
        .bind(("data", &data))
        .bind(("now", &now))
        .await
    {
        Ok(result) => result,
        Err(e) => {
            warn!("failed to update settings section {}, returning fallback: {}", section, e);
            return Ok(Json(json!({
                "success": true,
                "data": settings_with_section(section, data),
                "warning": "settings update was not persisted"
            })));
        }
    };
    let items: Vec<Value> = match result.take(0) {
        Ok(items) => items,
        Err(e) => {
            warn!("failed to parse updated settings section {}, returning fallback: {}", section, e);
            return Ok(Json(json!({
                "success": true,
                "data": settings_with_section(section, data),
                "warning": "settings update result was not readable"
            })));
        }
    };
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next().unwrap_or_else(|| settings_with_section(section, data)) }),
    ))
}

fn default_settings() -> Value {
    json!({
        "general": {
            "platform_name": "SoulBook",
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
}

fn settings_with_section(section: &str, data: Value) -> Value {
    let mut settings = default_settings();
    if let Some(object) = settings.as_object_mut() {
        object.insert(section.to_string(), data);
    }
    settings
}
