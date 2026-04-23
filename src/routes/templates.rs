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
        .route("/", get(list_templates).post(create_template))
        .route(
            "/:id",
            get(get_template)
                .put(update_template)
                .delete(delete_template),
        )
        .route("/:id/use", post(use_template))
        .route("/categories", get(list_categories))
}

#[derive(Deserialize)]
struct CreateTemplateRequest {
    name: String,
    category: String,
    icon: Option<String>,
    description: Option<String>,
    content: Option<String>,
}

async fn list_templates(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM doc_template WHERE is_deleted = false ORDER BY usage_count DESC")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let mut items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    if items.is_empty() {
        items = default_templates();
    }

    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn get_template(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let item: Option<Value> = db
        .select(("doc_template", id.as_str()))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    match item {
        Some(v) => Ok(Json(json!({ "success": true, "data": v }))),
        None => Err(crate::error::ApiError::NotFound(
            "Template not found".into(),
        )),
    }
}

async fn create_template(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateTemplateRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query(
            "CREATE doc_template SET
                name = $name,
                category = $category,
                icon = $icon,
                description = $desc,
                content = $content,
                usage_count = 0,
                created_by = $uid,
                is_deleted = false,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("name", &req.name))
        .bind(("category", &req.category))
        .bind(("icon", req.icon.as_deref().unwrap_or("📋")))
        .bind(("desc", req.description.as_deref().unwrap_or("")))
        .bind(("content", req.content.as_deref().unwrap_or("")))
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

async fn update_template(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE $id MERGE $data SET updated_at = $now RETURN AFTER")
        .bind(("id", format!("doc_template:{}", id)))
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

async fn delete_template(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE $id SET is_deleted = true, updated_at = $now")
        .bind(("id", format!("doc_template:{}", id)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true })))
}

async fn use_template(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE $id SET usage_count += 1, updated_at = $now")
        .bind(("id", format!("doc_template:{}", id)))
        .bind(("now", &now))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    let item: Option<Value> = db
        .select(("doc_template", id.as_str()))
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    Ok(Json(json!({ "success": true, "data": item })))
}

async fn list_categories(
    Extension(_app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let categories = vec!["产品", "技术", "帮助", "发布", "运营", "设计", "其他"];
    Ok(Json(
        json!({ "success": true, "data": { "items": categories } }),
    ))
}

fn default_templates() -> Vec<Value> {
    vec![
        json!({
            "id": "doc_template:prd",
            "name": "产品需求文档 PRD",
            "category": "产品",
            "icon": "📋",
            "description": "包含背景、目标、功能需求、验收标准等标准章节",
            "usage_count": 24,
            "content": "# 产品需求文档\n\n## 背景\n\n## 目标\n\n## 功能需求\n\n## 验收标准\n"
        }),
        json!({
            "id": "doc_template:api",
            "name": "API 接口文档",
            "category": "技术",
            "icon": "🔌",
            "description": "REST API 接口规范模板，含请求/响应示例",
            "usage_count": 18,
            "content": "# API 文档\n\n## 接口概述\n\n## 请求参数\n\n## 响应示例\n"
        }),
        json!({
            "id": "doc_template:faq",
            "name": "FAQ 文档",
            "category": "帮助",
            "icon": "❓",
            "description": "常见问题解答模板，适合帮助中心和知识库",
            "usage_count": 32,
            "content": "# 常见问题\n\n## Q: 问题一？\n\nA: 回答一\n\n## Q: 问题二？\n\nA: 回答二\n"
        }),
        json!({
            "id": "doc_template:release",
            "name": "发布说明",
            "category": "发布",
            "icon": "🚀",
            "description": "版本发布说明模板，含功能列表和变更日志",
            "usage_count": 15,
            "content": "# 发布说明 vX.X.X\n\n## 新功能\n\n## 修复问题\n\n## 已知问题\n"
        }),
        json!({
            "id": "doc_template:design",
            "name": "技术方案设计",
            "category": "技术",
            "icon": "🏗️",
            "description": "技术方案评审文档，含背景、方案对比和实现计划",
            "usage_count": 9,
            "content": "# 技术方案设计\n\n## 背景\n\n## 方案对比\n\n## 实现计划\n"
        }),
        json!({
            "id": "doc_template:report",
            "name": "运营报告",
            "category": "运营",
            "icon": "📊",
            "description": "月度/季度运营报告模板，含数据图表占位符",
            "usage_count": 6,
            "content": "# 运营报告\n\n## 数据概览\n\n## 核心指标\n\n## 总结与展望\n"
        }),
    ]
}
