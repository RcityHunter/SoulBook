use axum::{extract::State, response::Json, routing::get, Extension, Router};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;

use crate::{error::Result, services::auth::User};

#[derive(Serialize)]
pub struct SearchStats {
    pub total_documents: i64,
    pub total_searches_today: i64,
    pub most_searched_terms: Vec<SearchTerm>,
    pub recent_searches: Vec<RecentSearch>,
}

#[derive(Serialize)]
pub struct SearchTerm {
    pub term: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct RecentSearch {
    pub query: String,
    pub results_count: i64,
    pub timestamp: String,
}

#[derive(Serialize)]
pub struct DocumentStats {
    pub total_documents: i64,
    pub total_spaces: i64,
    pub total_comments: i64,
    pub documents_created_today: i64,
    pub most_active_spaces: Vec<SpaceActivity>,
}

#[derive(Serialize)]
pub struct SpaceActivity {
    pub space_id: String,
    pub space_name: String,
    pub document_count: i64,
    pub recent_activity: i64,
}

pub async fn get_search_stats(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    _user: User,
) -> Result<Json<serde_json::Value>> {
    // 暂时返回模拟数据
    let stats = SearchStats {
        total_documents: 156,
        total_searches_today: 42,
        most_searched_terms: vec![
            SearchTerm {
                term: "API documentation".to_string(),
                count: 15,
            },
            SearchTerm {
                term: "authentication".to_string(),
                count: 12,
            },
            SearchTerm {
                term: "database".to_string(),
                count: 8,
            },
        ],
        recent_searches: vec![
            RecentSearch {
                query: "user management".to_string(),
                results_count: 7,
                timestamp: "2024-01-15T10:30:00Z".to_string(),
            },
            RecentSearch {
                query: "deployment guide".to_string(),
                results_count: 3,
                timestamp: "2024-01-15T10:25:00Z".to_string(),
            },
        ],
    };

    Ok(Json(json!({
        "success": true,
        "data": stats
    })))
}

pub async fn get_document_stats(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    _user: User,
) -> Result<Json<serde_json::Value>> {
    let db = &app_state.db.client;

    // 获取文档总数
    let doc_count_query =
        "SELECT count() as total FROM document WHERE is_deleted = false GROUP ALL";
    let mut doc_result = db
        .query(doc_count_query)
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let doc_records: Vec<serde_json::Value> = doc_result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let total_documents = doc_records
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // 获取空间总数
    let space_count_query = "SELECT count() as total FROM space WHERE is_deleted = false GROUP ALL";
    let mut space_result = db
        .query(space_count_query)
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let space_records: Vec<serde_json::Value> = space_result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let total_spaces = space_records
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // 获取评论总数（暂时返回0）
    let total_comments = 0;

    // 获取今天创建的文档数
    let today_docs_query = "SELECT count() as total FROM document 
        WHERE is_deleted = false 
        AND created_at >= time::floor(time::now(), 1d) 
        GROUP ALL";

    let mut today_result = db
        .query(today_docs_query)
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let today_records: Vec<serde_json::Value> = today_result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let documents_created_today = today_records
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // 获取最活跃的空间
    let active_spaces_query = "SELECT id, name FROM space WHERE is_deleted = false LIMIT 5";

    let mut active_result = db
        .query(active_spaces_query)
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let space_records: Vec<serde_json::Value> = active_result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    // 为每个空间获取文档数量
    let mut most_active_spaces = Vec::new();
    for record in space_records {
        let space_id = record
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let space_name = record
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("未知空间")
            .to_string();

        // 获取该空间的文档数量
        let doc_count_query = "SELECT count() as total FROM document WHERE space_id = $space_id AND is_deleted = false GROUP ALL";
        let mut doc_count_result = db
            .query(doc_count_query)
            .bind(("space_id", space_id.clone()))
            .await
            .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

        let doc_count_records: Vec<serde_json::Value> = doc_count_result
            .take(0)
            .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

        let document_count = doc_count_records
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        most_active_spaces.push(SpaceActivity {
            space_id,
            space_name,
            document_count,
            recent_activity: 0, // 暂时返回0
        });
    }

    // 按文档数量排序
    most_active_spaces.sort_by(|a, b| b.document_count.cmp(&a.document_count));

    let stats = DocumentStats {
        total_documents,
        total_spaces,
        total_comments,
        documents_created_today,
        most_active_spaces,
    };

    Ok(Json(json!({
        "success": true,
        "data": stats
    })))
}

pub async fn get_overview_stats(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    _user: User,
) -> Result<Json<serde_json::Value>> {
    let db = &app_state.db.client;

    let mut doc_r = db
        .query("SELECT count() as total FROM document WHERE is_deleted = false GROUP ALL")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let doc_rec: Vec<serde_json::Value> = doc_r.take(0).unwrap_or_default();
    let doc_count = doc_rec
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let mut space_r = db
        .query("SELECT count() as total FROM space WHERE is_deleted = false GROUP ALL")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let space_rec: Vec<serde_json::Value> = space_r.take(0).unwrap_or_default();
    let space_count = space_rec
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let mut tag_r = db
        .query("SELECT count() as total FROM tag GROUP ALL")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let tag_rec: Vec<serde_json::Value> = tag_r.take(0).unwrap_or_default();
    let tag_count = tag_rec
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let mut cr_r = db.query("SELECT count() as total FROM change_request WHERE is_deleted = false AND status = 'open' GROUP ALL").await.map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let cr_rec: Vec<serde_json::Value> = cr_r.take(0).unwrap_or_default();
    let open_crs = cr_rec
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let mut at_r = db.query("SELECT count() as total FROM ai_task WHERE is_deleted = false AND status IN ['pending','running'] GROUP ALL").await.map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let at_rec: Vec<serde_json::Value> = at_r.take(0).unwrap_or_default();
    let active_ai_tasks = at_rec
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "success": true,
        "data": {
            "space_count": space_count,
            "document_count": doc_count,
            "tag_count": tag_count,
            "open_change_requests": open_crs,
            "active_ai_tasks": active_ai_tasks,
        }
    })))
}

pub fn router() -> Router {
    Router::new()
        .route("/search", get(get_search_stats))
        .route("/documents", get(get_document_stats))
        .route("/overview", get(get_overview_stats))
}
