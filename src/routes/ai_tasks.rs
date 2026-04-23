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
        .route("/", get(list_tasks).post(create_task))
        .route("/:id", get(get_task).delete(delete_task))
        .route("/:id/cancel", post(cancel_task))
        .route("/:id/retry", post(retry_task))
}

#[derive(Deserialize)]
struct TaskQuery {
    status: Option<String>,
    space_id: Option<String>,
    task_type: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(Deserialize)]
struct CreateTaskRequest {
    task_type: String, // translate | summarize | seo_check | proofread | faq
    document_id: String,
    document_title: Option<String>,
    space_id: Option<String>,
    model: Option<String>,
    target_language: Option<String>,
    extra: Option<String>,
}

async fn list_tasks(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(q): Query<TaskQuery>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).min(100);
    let offset = (page - 1) * per_page;

    let mut conditions = vec!["is_deleted = false".to_string()];
    if let Some(ref s) = q.status {
        conditions.push(format!("status = '{}'", s));
    }
    if let Some(ref sid) = q.space_id {
        conditions.push(format!("space_id = '{}'", sid));
    }
    if let Some(ref tt) = q.task_type {
        conditions.push(format!("task_type = '{}'", tt));
    }

    let sql = format!(
        "SELECT * FROM ai_task WHERE {} ORDER BY created_at DESC LIMIT {} START {}",
        conditions.join(" AND "),
        per_page,
        offset
    );
    let mut result = db
        .query(sql)
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    // counts by status
    let mut cnt = db
        .query(
            "SELECT count() as total, status FROM ai_task WHERE is_deleted = false GROUP BY status",
        )
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let counts: Vec<Value> = cnt.take(0).unwrap_or_default();

    let mut running = 0i64;
    let mut completed = 0i64;
    let mut pending = 0i64;
    let mut failed = 0i64;
    for c in &counts {
        let n = c.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
        match c.get("status").and_then(|v| v.as_str()).unwrap_or("") {
            "running" => running = n,
            "completed" => completed = n,
            "pending" => pending = n,
            "failed" => failed = n,
            _ => {}
        }
    }

    Ok(Json(json!({
        "success": true,
        "data": {
            "items": items,
            "stats": { "running": running, "completed": completed, "pending": pending, "failed": failed },
            "page": page, "per_page": per_page
        }
    })))
}

async fn create_task(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let model = req.model.as_deref().unwrap_or("claude-3.5");

    let mut result = db
        .query(
            "CREATE ai_task SET
                task_type = $task_type,
                document_id = $doc_id,
                document_title = $doc_title,
                space_id = $space_id,
                model = $model,
                target_language = $lang,
                extra = $extra,
                status = 'pending',
                progress = 0,
                created_by = $uid,
                is_deleted = false,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("task_type", &req.task_type))
        .bind(("doc_id", &req.document_id))
        .bind(("doc_title", req.document_title.as_deref().unwrap_or("")))
        .bind(("space_id", req.space_id.as_deref().unwrap_or("")))
        .bind(("model", model))
        .bind(("lang", req.target_language.as_deref().unwrap_or("")))
        .bind(("extra", req.extra.as_deref().unwrap_or("")))
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

async fn get_task(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let t: Option<Value> = db
        .select(("ai_task", id.as_str()))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    match t {
        Some(v) => Ok(Json(json!({ "success": true, "data": v }))),
        None => Err(crate::error::ApiError::NotFound("Task not found".into())),
    }
}

async fn cancel_task(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    _body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    update_task_status(&app_state, &id, "cancelled").await
}

async fn retry_task(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    _body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    update_task_status(&app_state, &id, "pending").await
}

async fn delete_task(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    db.query("UPDATE ai_task SET is_deleted = true WHERE id = $id")
        .bind(("id", format!("ai_task:{}", id)))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true })))
}

async fn update_task_status(
    app_state: &Arc<AppState>,
    id: &str,
    status: &str,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE ai_task SET status = $status, updated_at = $now WHERE id = $id")
        .bind(("status", status))
        .bind(("now", &now))
        .bind(("id", format!("ai_task:{}", id)))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}
