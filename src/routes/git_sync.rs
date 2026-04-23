use axum::{
    extract::Path,
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{error::Result, services::auth::User, AppState};

pub fn router() -> Router {
    Router::new()
        .route("/repositories", get(list_repos).post(create_repo))
        .route(
            "/repositories/:id",
            get(get_repo).put(update_repo).delete(delete_repo),
        )
        .route("/repositories/:id/sync", post(trigger_sync))
        .route("/repositories/:id/logs", get(sync_logs))
}

#[derive(Deserialize)]
struct CreateRepoRequest {
    space_slug: String,
    github_url: String,
    branch: Option<String>,
    direction: Option<String>,
    auto_sync: Option<bool>,
    path_prefix: Option<String>,
}

async fn list_repos(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM git_repo WHERE is_deleted = false ORDER BY created_at DESC")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn create_repo(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateRepoRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query(
            "CREATE git_repo SET
                space_slug = $space_slug,
                github_url = $github_url,
                branch = $branch,
                direction = $direction,
                auto_sync = $auto_sync,
                path_prefix = $path_prefix,
                status = 'connected',
                last_synced_at = NONE,
                created_by = $uid,
                is_deleted = false,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("space_slug", &req.space_slug))
        .bind(("github_url", &req.github_url))
        .bind(("branch", req.branch.as_deref().unwrap_or("main")))
        .bind((
            "direction",
            req.direction.as_deref().unwrap_or("bidirectional"),
        ))
        .bind(("auto_sync", req.auto_sync.unwrap_or(false)))
        .bind(("path_prefix", req.path_prefix.as_deref().unwrap_or("docs/")))
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

async fn get_repo(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let item: Option<Value> = db
        .select(("git_repo", id.as_str()))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    match item {
        Some(v) => Ok(Json(json!({ "success": true, "data": v }))),
        None => Err(crate::error::ApiError::NotFound(
            "Repository not found".into(),
        )),
    }
}

async fn update_repo(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE $id MERGE $data SET updated_at = $now RETURN AFTER")
        .bind(("id", format!("git_repo:{}", id)))
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

async fn delete_repo(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE $id SET is_deleted = true, updated_at = $now")
        .bind(("id", format!("git_repo:{}", id)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true })))
}

async fn trigger_sync(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();

    // Record sync log
    db.query(
        "CREATE git_sync_log SET
            repo_id = $repo_id,
            triggered_by = $uid,
            direction = 'souldoc_to_github',
            status = 'pending',
            created_at = $now",
    )
    .bind(("repo_id", format!("git_repo:{}", id)))
    .bind(("uid", &user.id))
    .bind(("now", &now))
    .await
    .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    // Update repo last_synced_at
    db.query("UPDATE $id SET last_synced_at = $now, updated_at = $now")
        .bind(("id", format!("git_repo:{}", id)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    Ok(Json(json!({
        "success": true,
        "data": { "message": "同步任务已提交，请稍后查看日志" }
    })))
}

async fn sync_logs(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query(
            "SELECT * FROM git_sync_log WHERE repo_id = $repo_id ORDER BY created_at DESC LIMIT 50",
        )
        .bind(("repo_id", format!("git_repo:{}", id)))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}
