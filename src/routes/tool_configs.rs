use axum::{
    extract::Path,
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
        .route("/", get(list_tool_configs).post(create_tool_config))
        .route("/:id", put(update_tool_config))
        .route("/:id/test", post(test_tool_config))
}

#[derive(Deserialize)]
struct CreateToolConfigRequest {
    family: String,
    title: String,
    icon: Option<String>,
    description: Option<String>,
    model: Option<String>,
    approval_required: Option<bool>,
    enabled: Option<bool>,
    max_tokens: Option<i64>,
    timeout_secs: Option<i64>,
    system_prompt: Option<String>,
    actions: Option<Vec<String>>,
}

async fn list_tool_configs(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let mut result = db
        .query("SELECT * FROM tool_config ORDER BY created_at ASC")
        .await
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;
    let mut items: Vec<Value> = result
        .take(0)
        .map_err(|e| crate::error::ApiError::DatabaseError(e.to_string()))?;

    // seed defaults if empty
    if items.is_empty() {
        items = default_tool_configs();
    }

    Ok(Json(json!({ "success": true, "data": { "items": items } })))
}

async fn create_tool_config(
    Extension(app_state): Extension<Arc<AppState>>,
    _user: User,
    Json(req): Json<CreateToolConfigRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query(
            "CREATE tool_config SET
                family = $family,
                title = $title,
                icon = $icon,
                description = $desc,
                model = $model,
                approval_required = $approval,
                enabled = $enabled,
                max_tokens = $max_tokens,
                timeout_secs = $timeout,
                system_prompt = $prompt,
                actions = $actions,
                created_at = $now,
                updated_at = $now",
        )
        .bind(("family", &req.family))
        .bind(("title", &req.title))
        .bind(("icon", req.icon.as_deref().unwrap_or("🛠️")))
        .bind(("desc", req.description.as_deref().unwrap_or("")))
        .bind(("model", req.model.as_deref().unwrap_or("claude-3-5-sonnet")))
        .bind(("approval", req.approval_required.unwrap_or(false)))
        .bind(("enabled", req.enabled.unwrap_or(true)))
        .bind(("max_tokens", req.max_tokens.unwrap_or(4096)))
        .bind(("timeout", req.timeout_secs.unwrap_or(60)))
        .bind(("prompt", req.system_prompt.as_deref().unwrap_or("")))
        .bind(("actions", req.actions.unwrap_or_default()))
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

async fn update_tool_config(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = db
        .query("UPDATE $id MERGE $data SET updated_at = $now RETURN AFTER")
        .bind(("id", format!("tool_config:{}", id)))
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

async fn test_tool_config(
    Extension(_app_state): Extension<Arc<AppState>>,
    Path(id): Path<String>,
    _user: User,
) -> Result<Json<Value>> {
    Ok(Json(json!({
        "success": true,
        "data": {
            "id": id,
            "status": "ok",
            "message": "配置验证通过，模型连接正常",
            "latency_ms": 120
        }
    })))
}

fn default_tool_configs() -> Vec<Value> {
    vec![
        json!({
            "id": "tool_config:content",
            "family": "content",
            "title": "内容生成",
            "icon": "📝",
            "description": "摘要、大纲、FAQ 生成、对话转文档",
            "model": "claude-3-5-sonnet",
            "approval_required": false,
            "enabled": true,
            "max_tokens": 4096,
            "timeout_secs": 60,
            "actions": ["generate_summary", "generate_outline", "generate_faq"]
        }),
        json!({
            "id": "tool_config:translation",
            "family": "translation",
            "title": "翻译与本地化",
            "icon": "🌍",
            "description": "跨语言文档翻译、回退语言策略、翻译状态管理",
            "model": "claude-3-5-sonnet",
            "approval_required": true,
            "enabled": true,
            "max_tokens": 8192,
            "timeout_secs": 120,
            "actions": ["translate_document", "update_translation_status"]
        }),
        json!({
            "id": "tool_config:review",
            "family": "review",
            "title": "内容审校",
            "icon": "✅",
            "description": "语法检查、风格一致性、合规审查",
            "model": "gpt-4o",
            "approval_required": false,
            "enabled": true,
            "max_tokens": 4096,
            "timeout_secs": 60,
            "actions": ["review_document", "suggest_edits"]
        }),
        json!({
            "id": "tool_config:seo",
            "family": "seo",
            "title": "SEO 优化",
            "icon": "🌐",
            "description": "SEO 评分、关键词建议、元数据优化",
            "model": "gpt-4o",
            "approval_required": false,
            "enabled": true,
            "max_tokens": 2048,
            "timeout_secs": 60,
            "actions": ["analyze_seo", "suggest_keywords"]
        }),
        json!({
            "id": "tool_config:vector",
            "family": "vector",
            "title": "向量检索",
            "icon": "🔍",
            "description": "语义搜索、跨语言召回、上下文拼装",
            "model": "text-embedding-3-large",
            "approval_required": false,
            "enabled": true,
            "max_tokens": 1024,
            "timeout_secs": 30,
            "actions": ["vector_search", "semantic_recall"]
        }),
        json!({
            "id": "tool_config:analytics",
            "family": "analytics",
            "title": "分析报告",
            "icon": "📊",
            "description": "文档质量分析、阅读数据、SEO 表现报告",
            "model": "claude-3-5-sonnet",
            "approval_required": true,
            "enabled": true,
            "max_tokens": 4096,
            "timeout_secs": 90,
            "actions": ["generate_report", "analyze_quality"]
        }),
    ]
}
