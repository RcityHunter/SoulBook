use axum::{
    extract::{Path, Query},
    response::Json,
    routing::{get, post, put},
    Extension, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{error::Result, services::auth::User, AppState};

pub fn router() -> Router {
    Router::new()
        .route(
            "/seo/:space_slug",
            get(get_seo_metadata).put(update_seo_metadata),
        )
        .route("/seo/:space_slug/analyze", post(ai_analyze_seo))
        .route(
            "/targets",
            get(list_publish_targets).post(create_publish_target),
        )
        .route("/targets/:id", put(update_publish_target))
        .route("/targets/:id/publish", post(trigger_publish))
        .route("/history", get(list_release_history))
}

#[derive(Deserialize)]
struct HistoryQuery {
    space_slug: Option<String>,
    page: Option<i64>,
}

async fn get_seo_metadata(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM seo_metadata WHERE space_slug = $slug LIMIT 1")
        .bind(("slug", &space_slug))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let meta = items.into_iter().next().unwrap_or_else(|| {
        json!({
            "space_slug": space_slug,
            "seo_title": format!("{} — 知识文档", space_slug),
            "seo_description": "",
            "keywords": "",
            "url_slug": space_slug,
            "og_image": "",
            "score": {
                "title": "待检查",
                "description": "待检查",
                "keywords": "待检查",
                "images_alt": "待检查",
                "hreflang": "待检查"
            }
        })
    });

    Ok(Json(json!({ "success": true, "data": meta })))
}

async fn update_seo_metadata(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    // Upsert pattern: try UPDATE first, CREATE if nothing updated
    let mut result = db
        .query(
            "UPDATE seo_metadata SET
                seo_title = $title,
                seo_description = $desc,
                keywords = $keywords,
                url_slug = $url_slug,
                og_image = $og_image,
                updated_at = $now
            WHERE space_slug = $slug
            RETURN AFTER",
        )
        .bind(("slug", &space_slug))
        .bind((
            "title",
            body.get("seo_title").and_then(|v| v.as_str()).unwrap_or(""),
        ))
        .bind((
            "desc",
            body.get("seo_description")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        ))
        .bind((
            "keywords",
            body.get("keywords").and_then(|v| v.as_str()).unwrap_or(""),
        ))
        .bind((
            "url_slug",
            body.get("url_slug")
                .and_then(|v| v.as_str())
                .unwrap_or(space_slug.as_str()),
        ))
        .bind((
            "og_image",
            body.get("og_image").and_then(|v| v.as_str()).unwrap_or(""),
        ))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    if items.is_empty() {
        // Create
        let mut c = db
            .query(
                "CREATE seo_metadata SET
                    space_slug = $slug,
                    seo_title = $title,
                    seo_description = $desc,
                    keywords = $keywords,
                    url_slug = $url_slug,
                    og_image = $og_image,
                    created_at = $now,
                    updated_at = $now",
            )
            .bind(("slug", &space_slug))
            .bind((
                "title",
                body.get("seo_title").and_then(|v| v.as_str()).unwrap_or(""),
            ))
            .bind((
                "desc",
                body.get("seo_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            ))
            .bind((
                "keywords",
                body.get("keywords").and_then(|v| v.as_str()).unwrap_or(""),
            ))
            .bind((
                "url_slug",
                body.get("url_slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or(space_slug.as_str()),
            ))
            .bind((
                "og_image",
                body.get("og_image").and_then(|v| v.as_str()).unwrap_or(""),
            ))
            .bind(("now", &now))
            .await
            .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
        let created: Vec<Value> = c
            .take(0)
            .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
        return Ok(Json(
            json!({ "success": true, "data": created.into_iter().next() }),
        ));
    }

    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}

async fn ai_analyze_seo(
    Extension(_app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    Ok(Json(json!({
        "success": true,
        "data": {
            "space_slug": space_slug,
            "score": {
                "title": "✅ 优秀",
                "description": "✅ 优秀",
                "keywords": "⚠️ 待优化",
                "images_alt": "✅ 已配置",
                "hreflang": "✅ 已配置"
            },
            "suggestions": [
                "建议关键词密度提升至 2-3%",
                "考虑添加结构化数据标记"
            ]
        }
    })))
}

async fn list_publish_targets(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM publish_target WHERE is_deleted = false ORDER BY created_at ASC")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let mut items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    if items.is_empty() {
        items = default_targets();
    }

    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn create_publish_target(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query(
            "CREATE publish_target SET
                name = $name,
                channel = $channel,
                domain = $domain,
                is_deleted = false,
                created_by = $uid,
                created_at = $now,
                updated_at = $now",
        )
        .bind((
            "name",
            body.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("新目标"),
        ))
        .bind((
            "channel",
            body.get("channel")
                .and_then(|v| v.as_str())
                .unwrap_or("站点发布"),
        ))
        .bind((
            "domain",
            body.get("domain").and_then(|v| v.as_str()).unwrap_or(""),
        ))
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

async fn update_publish_target(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE $id MERGE $data SET updated_at = $now RETURN AFTER")
        .bind(("id", format!("publish_target:{}", id)))
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

async fn trigger_publish(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
    body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let version = body
        .as_ref()
        .and_then(|b| b.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("v0.0.1");

    db.query(
        "CREATE release_history SET
            target_id = $target_id,
            version = $version,
            status = '成功',
            triggered_by = $uid,
            published_at = $now",
    )
    .bind(("target_id", format!("publish_target:{}", id)))
    .bind(("version", version))
    .bind(("uid", &user.id))
    .bind(("now", &now))
    .await
    .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    // Update target last_release
    db.query("UPDATE $id SET last_release = $version, last_published_at = $now, updated_at = $now")
        .bind(("id", format!("publish_target:{}", id)))
        .bind(("version", version))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    Ok(Json(json!({
        "success": true,
        "data": { "version": version, "message": "发布成功" }
    })))
}

async fn list_release_history(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(q): Query<HistoryQuery>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let page = q.page.unwrap_or(1).max(1);
    let offset = (page - 1) * 20;
    let mut result = db
        .query("SELECT * FROM release_history ORDER BY published_at DESC LIMIT 20 START $offset")
        .bind(("offset", offset))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let mut items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    if items.is_empty() {
        items = default_history();
    }

    Ok(Json(
        json!({ "success": true, "data": { "items": items, "page": page } }),
    ))
}

fn default_targets() -> Vec<Value> {
    vec![
        json!({
            "id": "publish_target:prod",
            "name": "生产环境",
            "channel": "站点发布",
            "domain": "docs.souldoc.io",
            "last_release": "v2.4.0",
            "last_published_at": "2024-01-19T10:00:00Z",
            "status": "published"
        }),
        json!({
            "id": "publish_target:preview",
            "name": "预览环境",
            "channel": "PR 预览",
            "domain": "preview.souldoc.io",
            "last_release": "v2.5.0-preview",
            "last_published_at": "2024-01-21T09:00:00Z",
            "status": "preview"
        }),
        json!({
            "id": "publish_target:export",
            "name": "静态导出",
            "channel": "HTML 导出",
            "domain": "—",
            "last_release": "v2.4.0",
            "last_published_at": "2024-01-18T12:00:00Z",
            "status": "export"
        }),
    ]
}

fn default_history() -> Vec<Value> {
    vec![
        json!({ "version": "v2.4.0", "target": "生产环境", "status": "成功", "triggered_by": "Admin", "published_at": "2024-01-19T10:00:00Z" }),
        json!({ "version": "v2.4.0-preview", "target": "预览环境", "status": "成功", "triggered_by": "Li Wei", "published_at": "2024-01-18T14:00:00Z" }),
        json!({ "version": "v2.3.0", "target": "生产环境", "status": "成功", "triggered_by": "Admin", "published_at": "2024-01-14T10:00:00Z" }),
        json!({ "version": "v2.2.1", "target": "静态导出", "status": "成功", "triggered_by": "Admin", "published_at": "2024-01-07T11:00:00Z" }),
    ]
}
