use axum::{
    extract::{Path, Query},
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{error::Result, services::auth::User, AppState};

pub fn router() -> Router {
    Router::new()
        // Space-level language settings
        .route(
            "/spaces/:slug/languages",
            get(list_space_languages).post(add_space_language),
        )
        .route(
            "/spaces/:slug/languages/:code",
            delete(remove_space_language),
        )
        // Document language versions
        .route("/documents/:space/:doc/languages", get(list_doc_languages))
        .route(
            "/documents/:space/:doc/languages/:code",
            get(get_doc_language).put(update_doc_language),
        )
        .route(
            "/documents/:space/:doc/languages/:code/translate",
            post(request_translation),
        )
}

#[derive(Deserialize)]
struct AddLanguageRequest {
    language_code: String,
    is_default: Option<bool>,
}

#[derive(Deserialize)]
struct UpdateDocLanguageRequest {
    status: Option<String>,
    content: Option<String>,
}

const DEFAULT_LANGUAGES: &[(&str, &str)] = &[
    ("zh-CN", "简体中文"),
    ("en-US", "English"),
    ("ja-JP", "日本語"),
    ("ko-KR", "한국어"),
    ("fr-FR", "Français"),
    ("de-DE", "Deutsch"),
    ("es-ES", "Español"),
    ("pt-BR", "Português"),
];

fn lang_name(code: &str) -> &'static str {
    DEFAULT_LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
        .unwrap_or("Unknown")
}

async fn list_space_languages(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(slug): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let space = app_state
        .space_service
        .get_space_by_slug(&slug, Some(&user))
        .await?;

    let mut result = db
        .query("SELECT * FROM space_language WHERE space_id = $sid ORDER BY is_default DESC, language_code ASC")
        .bind(("sid", &space.id))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result.take(0).unwrap_or_default();

    // If no languages configured yet, return zh-CN as default
    if items.is_empty() {
        return Ok(Json(json!({
            "success": true,
            "data": [{ "language_code": "zh-CN", "language_name": "简体中文", "is_default": true, "enabled": true }]
        })));
    }

    Ok(Json(json!({ "success": true, "data": items })))
}

async fn add_space_language(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(slug): Path<String>,
    user: User,
    Json(req): Json<AddLanguageRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let space = app_state
        .space_service
        .get_space_by_slug(&slug, Some(&user))
        .await?;
    let now = chrono::Utc::now().to_rfc3339();
    let lang_name_str = lang_name(&req.language_code);

    let mut result = db
        .query(
            "CREATE space_language SET
                space_id = $sid,
                language_code = $code,
                language_name = $name,
                is_default = $is_default,
                enabled = true,
                created_at = $now",
        )
        .bind(("sid", &space.id))
        .bind(("code", &req.language_code))
        .bind(("name", lang_name_str))
        .bind(("is_default", req.is_default.unwrap_or(false)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result.take(0).unwrap_or_default();

    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}

async fn remove_space_language(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((slug, code)): Path<(String, String)>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let space = app_state
        .space_service
        .get_space_by_slug(&slug, Some(&user))
        .await?;
    db.query("DELETE space_language WHERE space_id = $sid AND language_code = $code")
        .bind(("sid", &space.id))
        .bind(("code", &code))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true })))
}

async fn list_doc_languages(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug)): Path<(String, String)>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query(
            "SELECT * FROM doc_language WHERE space_slug = $space AND doc_slug = $doc ORDER BY is_default DESC, language_code ASC"
        )
        .bind(("space", &space_slug))
        .bind(("doc", &doc_slug))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result.take(0).unwrap_or_default();

    Ok(Json(json!({ "success": true, "data": items })))
}

async fn get_doc_language(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug, code)): Path<(String, String, String)>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM doc_language WHERE space_slug = $space AND doc_slug = $doc AND language_code = $code LIMIT 1")
        .bind(("space", &space_slug))
        .bind(("doc", &doc_slug))
        .bind(("code", &code))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result.take(0).unwrap_or_default();
    match items.into_iter().next() {
        Some(v) => Ok(Json(json!({ "success": true, "data": v }))),
        None => Err(crate::error::ApiError::NotFound(
            "Language version not found".into(),
        )),
    }
}

async fn update_doc_language(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug, code)): Path<(String, String, String)>,
    user: User,
    Json(req): Json<UpdateDocLanguageRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();

    // Upsert
    let exists_result = db
        .query("SELECT id FROM doc_language WHERE space_slug = $space AND doc_slug = $doc AND language_code = $code LIMIT 1")
        .bind(("space", &space_slug))
        .bind(("doc", &doc_slug))
        .bind(("code", &code))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()));

    let exists: Vec<Value> = exists_result?.take(0).unwrap_or_default();

    if exists.is_empty() {
        let mut result = db
            .query(
                "CREATE doc_language SET
                    space_slug = $space, doc_slug = $doc, language_code = $code,
                    language_name = $name, status = $status, content = $content,
                    translated_by = $uid, is_default = false,
                    created_at = $now, updated_at = $now",
            )
            .bind(("space", &space_slug))
            .bind(("doc", &doc_slug))
            .bind(("code", &code))
            .bind(("name", lang_name(&code)))
            .bind(("status", req.status.as_deref().unwrap_or("draft")))
            .bind(("content", req.content.as_deref().unwrap_or("")))
            .bind(("uid", &user.id))
            .bind(("now", &now))
            .await
            .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
        let items: Vec<Value> = result.take(0).unwrap_or_default();
        return Ok(Json(
            json!({ "success": true, "data": items.into_iter().next() }),
        ));
    }

    let mut result = db
        .query(
            "UPDATE doc_language SET status = $status, content = $content, translated_by = $uid, updated_at = $now
             WHERE space_slug = $space AND doc_slug = $doc AND language_code = $code"
        )
        .bind(("status", req.status.as_deref().unwrap_or("draft")))
        .bind(("content", req.content.as_deref().unwrap_or("")))
        .bind(("uid", &user.id))
        .bind(("now", &now))
        .bind(("space", &space_slug))
        .bind(("doc", &doc_slug))
        .bind(("code", &code))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result.take(0).unwrap_or_default();
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}

async fn request_translation(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, doc_slug, code)): Path<(String, String, String)>,
    user: User,
    _body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    // Create an AI task for translation
    let mut result = db
        .query(
            "CREATE ai_task SET
                task_type = 'translate',
                document_id = $doc,
                document_title = $doc,
                space_id = $space,
                model = 'claude-3.5',
                target_language = $code,
                status = 'pending',
                progress = 0,
                created_by = $uid,
                is_deleted = false,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("doc", &doc_slug))
        .bind(("space", &space_slug))
        .bind(("code", &code))
        .bind(("uid", &user.id))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result.take(0).unwrap_or_default();
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next(), "message": "Translation task created" }),
    ))
}
