use axum::{
    Extension, Router,
    extract::Path,
    response::Json,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use surrealdb::types::RecordId as Thing;
use tracing::warn;

use crate::{
    AppState,
    error::Result,
    services::{
        auth::User,
        git_sync::{GitSyncInput, GitSyncService},
    },
};

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
    let items = match db
        .query("SELECT * FROM git_repo WHERE is_deleted = false ORDER BY created_at DESC")
        .await
    {
        Ok(mut result) => match result.take::<Vec<Value>>(0) {
            Ok(items) => items,
            Err(e) => {
                warn!("failed to parse git repositories: {}", e);
                Vec::new()
            }
        },
        Err(e) => {
            warn!("failed to query git repositories: {}", e);
            Vec::new()
        }
    };

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
    let fallback = merge_git_repo_payload(&id, &body, &now);
    let data = match db
        .query("UPSERT $id MERGE $data SET updated_at = $now RETURN AFTER")
        .bind(("id", Thing::new("git_repo", id.clone())))
        .bind(("data", &body))
        .bind(("now", &now))
        .await
    {
        Ok(mut result) => match result.take::<Vec<Value>>(0) {
            Ok(items) => items.into_iter().next().unwrap_or(fallback),
            Err(e) => {
                warn!("failed to parse updated git repository {}: {}", id, e);
                fallback
            }
        },
        Err(e) => {
            warn!("failed to update git repository {}: {}", id, e);
            fallback
        }
    };

    Ok(Json(json!({ "success": true, "data": data })))
}

async fn delete_repo(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE $id SET is_deleted = true, updated_at = $now")
        .bind(("id", Thing::new("git_repo", id)))
        .bind(("now", &now))
        .await
        .map_err(|e| {
            warn!("failed to mark git repository as deleted: {}", e);
            crate::error::ApiError::DatabaseError(e.to_string())
        })?;
    Ok(Json(json!({ "success": true })))
}

async fn trigger_sync(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let repo = load_git_repo(&app_state, &id).await?;
    let token = std::env::var("GITHUB_TOKEN").map_err(|_| {
        crate::error::ApiError::BadRequest(
            "GitHub sync is not configured: missing GITHUB_TOKEN".to_string(),
        )
    })?;

    let service = GitSyncService::new(app_state.db.clone());
    let space = app_state
        .space_service
        .get_space_by_slug(&repo.space_slug, Some(&user))
        .await?;
    let sync_result = service
        .sync_space_to_github(GitSyncInput {
            github_url: repo.github_url.clone(),
            branch: repo.branch.clone(),
            path_prefix: repo.path_prefix.clone(),
            space_id: space.id.clone(),
            token,
        })
        .await;

    let (status, message, files_changed, commit_sha) = match sync_result {
        Ok(result) => (
            "success",
            format!("Synced {} file(s) to GitHub", result.files_changed),
            result.files_changed as i64,
            result.commit_sha,
        ),
        Err(error) => ("failed", error, 0, None),
    };

    record_sync_log(
        &app_state,
        &id,
        &user.id,
        status,
        &message,
        files_changed,
        commit_sha.as_deref(),
        &now,
    )
    .await;

    if status == "success" {
        if let Err(e) = db
            .query("UPDATE $id SET last_synced_at = $now, updated_at = $now, status = 'connected'")
            .bind(("id", Thing::new("git_repo", id.clone())))
            .bind(("now", &now))
            .await
        {
            warn!("failed to update git sync timestamp for {}: {}", id, e);
        }

        Ok(Json(json!({
            "success": true,
            "data": {
                "message": message,
                "files_changed": files_changed,
                "commit_sha": commit_sha
            }
        })))
    } else {
        if let Err(e) = db
            .query("UPDATE $id SET status = 'error', updated_at = $now")
            .bind(("id", Thing::new("git_repo", id.clone())))
            .bind(("now", &now))
            .await
        {
            warn!("failed to update git sync error status for {}: {}", id, e);
        }

        Err(crate::error::ApiError::Internal(anyhow::anyhow!(message)))
    }
}

async fn sync_logs(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let items = match db
        .query(
            "SELECT * FROM git_sync_log WHERE repo_id = $repo_id ORDER BY created_at DESC LIMIT 50",
        )
        .bind(("repo_id", format!("git_repo:{}", id)))
        .await
    {
        Ok(mut result) => match result.take::<Vec<Value>>(0) {
            Ok(items) => items,
            Err(e) => {
                warn!("failed to parse git sync logs for {}: {}", id, e);
                Vec::new()
            }
        },
        Err(e) => {
            warn!("failed to query git sync logs for {}: {}", id, e);
            Vec::new()
        }
    };

    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

#[derive(Debug, Deserialize, Serialize)]
struct GitRepoRecord {
    #[serde(default)]
    id: Option<Value>,
    space_slug: String,
    github_url: String,
    #[serde(default = "default_main_branch")]
    branch: String,
    #[serde(default = "default_path_prefix")]
    path_prefix: String,
}

impl GitRepoRecord {
    fn is_valid(&self) -> bool {
        !self.github_url.trim().is_empty() && !self.space_slug.trim().is_empty()
    }
}

fn default_main_branch() -> String {
    "main".to_string()
}

fn default_path_prefix() -> String {
    "docs/".to_string()
}

async fn load_git_repo(app_state: &AppState, id: &str) -> Result<GitRepoRecord> {
    let db = &app_state.db.client;
    let item: Option<GitRepoRecord> = db
        .select(("git_repo", id))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    item.filter(GitRepoRecord::is_valid)
        .ok_or_else(|| crate::error::ApiError::NotFound("Repository not found".to_string()))
}

async fn record_sync_log(
    app_state: &AppState,
    id: &str,
    user_id: &str,
    status: &str,
    message: &str,
    files_changed: i64,
    commit_sha: Option<&str>,
    now: &str,
) {
    let db = &app_state.db.client;
    if let Err(e) = db
        .query(
            "CREATE git_sync_log SET
                repo_id = $repo_id,
                triggered_by = $uid,
                direction = 'soulbook_to_github',
                status = $status,
                message = $message,
                files_changed = $files_changed,
                commit_sha = $commit_sha,
                created_at = $now",
        )
        .bind(("repo_id", format!("git_repo:{}", id)))
        .bind(("uid", user_id))
        .bind(("status", status))
        .bind(("message", message))
        .bind(("files_changed", files_changed))
        .bind(("commit_sha", commit_sha))
        .bind(("now", now))
        .await
    {
        warn!("failed to create git sync log for {}: {}", id, e);
    }
}

fn merge_git_repo_payload(id: &str, body: &Value, updated_at: &str) -> Value {
    let mut repo = json!({
        "id": format!("git_repo:{}", id),
        "space_slug": "",
        "github_url": "",
        "branch": "main",
        "direction": "bidirectional",
        "auto_sync": false,
        "path_prefix": "docs/",
        "status": "connected",
        "last_synced_at": Value::Null,
        "is_deleted": false,
    });

    if let (Some(target), Some(source)) = (repo.as_object_mut(), body.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
        target.insert(
            "updated_at".to_string(),
            Value::String(updated_at.to_string()),
        );
    }

    repo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_git_repo_payload_preserves_defaults_and_applies_update() {
        let merged = merge_git_repo_payload(
            "repo-1",
            &json!({ "branch": "docs", "auto_sync": true }),
            "2026-04-29T00:00:00Z",
        );
        assert_eq!(merged["id"], "git_repo:repo-1");
        assert_eq!(merged["branch"], "docs");
        assert_eq!(merged["auto_sync"], true);
        assert_eq!(merged["path_prefix"], "docs/");
        assert_eq!(merged["updated_at"], "2026-04-29T00:00:00Z");
    }
}
