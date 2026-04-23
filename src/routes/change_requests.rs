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
        .route("/", get(list_crs).post(create_cr))
        .route("/:id", get(get_cr).put(update_cr).delete(delete_cr))
        .route("/:id/approve", post(approve_cr))
        .route("/:id/reject", post(reject_cr))
        .route("/:id/merge", post(merge_cr))
}

#[derive(Deserialize)]
struct CrQuery {
    status: Option<String>,
    space_id: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(Deserialize, Serialize)]
struct CreateCrRequest {
    title: String,
    description: Option<String>,
    space_id: String,
    document_id: String,
    document_title: Option<String>,
    diff_content: Option<String>,
    reviewer_id: Option<String>,
}

#[derive(Deserialize)]
struct UpdateCrRequest {
    title: Option<String>,
    description: Option<String>,
    reviewer_id: Option<String>,
    diff_content: Option<String>,
}

#[derive(Deserialize)]
struct ReviewRequest {
    comment: Option<String>,
}

async fn list_crs(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(q): Query<CrQuery>,
    user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).min(100);
    let offset = (page - 1) * per_page;

    let mut sql = "SELECT * FROM change_request".to_string();
    let mut conditions = vec!["is_deleted = false".to_string()];
    if let Some(ref status) = q.status {
        conditions.push(format!("status = '{}'", status));
    }
    if let Some(ref sid) = q.space_id {
        conditions.push(format!("space_id = '{}'", sid));
    }
    sql.push_str(&format!(
        " WHERE {} ORDER BY created_at DESC LIMIT {} START {}",
        conditions.join(" AND "),
        per_page,
        offset
    ));

    let mut result = db
        .query(sql)
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    Ok(Json(json!({
        "success": true,
        "data": { "items": items, "page": page, "per_page": per_page },
        "message": "Change requests retrieved"
    })))
}

async fn create_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateCrRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();

    let mut result = db
        .query(
            "CREATE change_request SET
                title = $title,
                description = $desc,
                space_id = $space_id,
                document_id = $doc_id,
                document_title = $doc_title,
                diff_content = $diff,
                reviewer_id = $reviewer_id,
                author_id = $author_id,
                status = 'open',
                is_deleted = false,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("title", &req.title))
        .bind(("desc", req.description.as_deref().unwrap_or("")))
        .bind(("space_id", &req.space_id))
        .bind(("doc_id", &req.document_id))
        .bind(("doc_title", req.document_title.as_deref().unwrap_or("")))
        .bind(("diff", req.diff_content.as_deref().unwrap_or("")))
        .bind(("reviewer_id", req.reviewer_id.as_deref().unwrap_or("")))
        .bind(("author_id", &user.id))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let cr = items.into_iter().next().unwrap_or(Value::Null);

    Ok(Json(
        json!({ "success": true, "data": cr, "message": "Change request created" }),
    ))
}

async fn get_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let cr: Option<Value> = db
        .select(("change_request", id.as_str()))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    match cr {
        Some(v) => Ok(Json(json!({ "success": true, "data": v }))),
        None => Err(crate::error::ApiError::NotFound(
            "Change request not found".into(),
        )),
    }
}

async fn update_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
    Json(req): Json<UpdateCrRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query(
            "UPDATE change_request SET
                title = $title,
                description = $desc,
                reviewer_id = $reviewer_id,
                diff_content = $diff,
                updated_at = $now
             WHERE id = $id",
        )
        .bind(("id", format!("change_request:{}", id)))
        .bind(("title", req.title.as_deref().unwrap_or("")))
        .bind(("desc", req.description.as_deref().unwrap_or("")))
        .bind(("reviewer_id", req.reviewer_id.as_deref().unwrap_or("")))
        .bind(("diff", req.diff_content.as_deref().unwrap_or("")))
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

async fn delete_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    db.query("UPDATE change_request SET is_deleted = true WHERE id = $id")
        .bind(("id", format!("change_request:{}", id)))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "message": "Deleted" })))
}

async fn approve_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
    Json(req): Json<ReviewRequest>,
) -> Result<Json<Value>> {
    set_cr_status(&app_state, &id, "merged", &user.id).await
}

async fn reject_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
    Json(req): Json<ReviewRequest>,
) -> Result<Json<Value>> {
    set_cr_status(&app_state, &id, "closed", &user.id).await
}

async fn merge_cr(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    user: User,
    Json(req): Json<ReviewRequest>,
) -> Result<Json<Value>> {
    set_cr_status(&app_state, &id, "merged", &user.id).await
}

async fn set_cr_status(
    app_state: &Arc<AppState>,
    id: &str,
    status: &str,
    user_id: &str,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE change_request SET status = $status, reviewer_id = $uid, updated_at = $now WHERE id = $id")
        .bind(("status", status))
        .bind(("uid", user_id))
        .bind(("now", &now))
        .bind(("id", format!("change_request:{}", id)))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(
        json!({ "success": true, "data": items.into_iter().next() }),
    ))
}
