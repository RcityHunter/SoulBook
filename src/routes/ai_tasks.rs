use axum::{
    extract::{Path, Query},
    response::Json,
    routing::{get, post},
    Extension, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

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
    _user: User,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let (page, per_page, offset) = normalize_pagination(q.page, q.per_page);

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
    let items: Vec<Value> = match db.query(sql).await {
        Ok(mut result) => match result.take(0) {
            Ok(items) => items,
            Err(e) => {
                warn!("ai_tasks list result parse failed: {}", e);
                Vec::new()
            }
        },
        Err(e) => {
            warn!("ai_tasks list query failed: {}", e);
            Vec::new()
        }
    };

    // counts by status
    let counts: Vec<Value> = match db
        .query(
            "SELECT count() as total, status FROM ai_task WHERE is_deleted = false GROUP BY status",
        )
        .await
    {
        Ok(mut cnt) => cnt.take(0).unwrap_or_else(|e| {
            warn!("ai_tasks stats result parse failed: {}", e);
            Vec::new()
        }),
        Err(e) => {
            warn!("ai_tasks stats query failed: {}", e);
            Vec::new()
        }
    };

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

fn normalize_pagination(page: Option<i64>, per_page: Option<i64>) -> (i64, i64, i64) {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;
    (page, per_page, offset)
}

async fn create_task(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<Value>> {
    let db = &app_state.db.client;
    let now = chrono::Utc::now().to_rfc3339();
    let model = req.model.as_deref().unwrap_or("claude-3.5");
    let task_document = load_ai_task_document(&app_state, &user, &req)
        .await
        .unwrap_or_else(|e| {
            warn!(
                "ai task source document lookup failed for user {} doc {}: {}",
                user.id, req.document_id, e
            );
            AiTaskDocument {
                title: req
                    .document_title
                    .clone()
                    .unwrap_or_else(|| req.document_id.clone()),
                content: String::new(),
                target_language: req.target_language.clone(),
            }
        });
    let output = generate_task_result(&req.task_type, &task_document);

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
                status = 'completed',
                progress = 100,
                result = $result,
                created_by = $uid,
                is_deleted = false,
                created_at = $now,
                updated_at = $now,
                completed_at = $now",
        )
        .bind(("task_type", &req.task_type))
        .bind(("doc_id", &req.document_id))
        .bind(("doc_title", &task_document.title))
        .bind(("space_id", req.space_id.as_deref().unwrap_or("")))
        .bind(("model", model))
        .bind(("lang", req.target_language.as_deref().unwrap_or("")))
        .bind(("extra", req.extra.as_deref().unwrap_or("")))
        .bind(("result", output))
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

#[derive(Debug, Clone)]
struct AiTaskDocument {
    title: String,
    content: String,
    target_language: Option<String>,
}

async fn load_ai_task_document(
    app_state: &Arc<AppState>,
    user: &User,
    req: &CreateTaskRequest,
) -> Result<AiTaskDocument> {
    let document = if let Some(space_slug) = req.space_id.as_deref().filter(|s| !s.is_empty()) {
        let space = app_state
            .space_service
            .get_space_by_slug(space_slug, Some(user))
            .await?;
        app_state
            .document_service
            .get_document_by_slug(&space.id, &req.document_id)
            .await?
    } else {
        app_state
            .document_service
            .get_document(&req.document_id)
            .await?
    };

    Ok(AiTaskDocument {
        title: document.title,
        content: document.content,
        target_language: req.target_language.clone(),
    })
}

fn generate_task_result(task_type: &str, doc: &AiTaskDocument) -> Value {
    let title = doc.title.trim();
    let content = normalize_markdown_text(&doc.content);
    let paragraphs = markdown_paragraphs(&content);
    let first_paragraph = paragraphs
        .first()
        .cloned()
        .unwrap_or_else(|| "当前文档内容较少，建议先补充正文后再生成。".to_string());
    let key_points = paragraphs.iter().take(4).cloned().collect::<Vec<_>>();

    let (label, body) = match task_type {
        "summarize" => (
            "文档摘要",
            format!(
                "## 文档摘要\n\n{}\n\n### 关键要点\n{}",
                truncate_chars(&first_paragraph, 260),
                markdown_bullets(&key_points)
            ),
        ),
        "outline" => (
            "优化大纲",
            format!(
                "## AI 优化大纲\n\n{}\n\n## 建议结构\n\n1. 背景与目标\n2. 核心内容\n3. 操作步骤\n4. 注意事项\n5. FAQ",
                heading_overview(title, &key_points)
            ),
        ),
        "extract_tags" => {
            let tags = extract_keywords(title, &content);
            return json!({
                "format": "tags",
                "title": "智能提取标签",
                "content": format!("建议标签：{}", tags.join("、")),
                "tags": tags,
            });
        }
        "faq" => (
            "FAQ",
            format!(
                "## FAQ\n\n### Q1：{}主要讲什么？\n\n{}\n\n### Q2：使用时应该优先关注什么？\n\n建议优先关注文档中的核心概念、操作步骤和权限边界。\n\n### Q3：后续可以如何完善？\n\n可以补充示例、异常处理、常见问题和相关链接。",
                if title.is_empty() { "这篇文档" } else { title },
                truncate_chars(&first_paragraph, 220)
            ),
        ),
        "proofread" => (
            "审校建议",
            format!(
                "## AI 审校检查\n\n### 可读性\n{}\n\n### 建议\n- 检查标题层级是否连续。\n- 为关键步骤补充示例。\n- 避免同一段落承载过多信息。",
                if content.chars().count() > 1200 {
                    "正文较长，建议拆分为多个小节。"
                } else {
                    "正文长度适中。"
                }
            ),
        ),
        "translate" => (
            "翻译草稿",
            format!(
                "## 翻译草稿\n\n目标语言：{}\n\n当前内置执行器只生成任务结果占位。接入真实模型后会在这里返回完整译文。\n\n---\n\n{}",
                doc.target_language.as_deref().unwrap_or("未指定"),
                truncate_chars(&content, 800)
            ),
        ),
        "polish" => (
            "润色建议",
            format!("## 润色建议\n\n{}", improve_readability(&first_paragraph)),
        ),
        "expand" => (
            "扩写草稿",
            format!(
                "## 扩写草稿\n\n{}\n\n可以进一步补充背景、具体示例、操作路径和风险说明，让内容更完整。",
                first_paragraph
            ),
        ),
        "compress" => (
            "压缩版本",
            format!("## 压缩版本\n\n{}", truncate_chars(&first_paragraph, 160)),
        ),
        "dialog_to_doc" => (
            "对话转文档",
            format!(
                "## 对话整理文档\n\n### 主题\n{}\n\n### 内容摘要\n{}\n\n### 后续行动\n- 明确结论。\n- 补充负责人和时间节点。\n- 整理为正式文档。",
                if title.is_empty() { "未命名主题" } else { title },
                truncate_chars(&first_paragraph, 220)
            ),
        ),
        other => (
            "AI 任务结果",
            format!(
                "## AI 任务结果\n\n任务类型：{}\n\n{}",
                other,
                truncate_chars(&first_paragraph, 260)
            ),
        ),
    };

    json!({
        "format": "markdown",
        "title": label,
        "content": body,
    })
}

fn normalize_markdown_text(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches('#')
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn markdown_paragraphs(content: &str) -> Vec<String> {
    content
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

fn markdown_bullets(items: &[String]) -> String {
    if items.is_empty() {
        "- 暂无可提取要点。".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- {}", truncate_chars(item, 120)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn heading_overview(title: &str, items: &[String]) -> String {
    if items.is_empty() {
        format!(
            "围绕「{}」补充背景、目标、步骤和结果。",
            if title.is_empty() { "当前主题" } else { title }
        )
    } else {
        format!(
            "围绕「{}」重组内容，突出：{}",
            if title.is_empty() { "当前主题" } else { title },
            items
                .iter()
                .take(3)
                .map(|item| truncate_chars(item, 36))
                .collect::<Vec<_>>()
                .join("；")
        )
    }
}

fn extract_keywords(title: &str, content: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for raw in title
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .chain(content.split(|c: char| !c.is_alphanumeric() && c != '_'))
    {
        let tag = raw.trim_matches(|c: char| c.is_ascii_punctuation()).trim();
        if tag.chars().count() < 2 || tag.chars().count() > 24 {
            continue;
        }
        if tags.iter().any(|existing| existing == tag) {
            continue;
        }
        tags.push(tag.to_string());
        if tags.len() >= 8 {
            break;
        }
    }
    if tags.is_empty() {
        tags.push("知识库".to_string());
    }
    tags
}

fn improve_readability(text: &str) -> String {
    format!(
        "{}\n\n建议：压缩重复表达，补充明确结论，并将长句拆分为更容易阅读的短句。",
        truncate_chars(text, 360)
    )
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out = input.chars().take(max_chars).collect::<String>();
    if input.chars().count() > max_chars {
        out.push('…');
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_pagination_clamps_page_and_per_page() {
        assert_eq!(normalize_pagination(Some(-3), Some(-10)), (1, 1, 0));
        assert_eq!(normalize_pagination(Some(2), Some(500)), (2, 100, 100));
        assert_eq!(normalize_pagination(None, None), (1, 20, 0));
    }

    #[test]
    fn generate_task_result_creates_viewable_outputs() {
        let input = AiTaskDocument {
            title: "SoulBook 协作空间".to_string(),
            content: "# SoulBook 协作空间\n\nSoulBook 支持多人协作文档、空间成员邀请和 AI 写作任务。\n\n编辑者可以维护知识库内容。".to_string(),
            target_language: None,
        };

        let summary = generate_task_result("summarize", &input);
        assert_eq!(summary["format"], "markdown");
        assert!(summary["content"].as_str().unwrap().contains("SoulBook 支持多人协作文档"));

        let faq = generate_task_result("faq", &input);
        assert!(faq["content"].as_str().unwrap().contains("## FAQ"));
        assert!(faq["content"].as_str().unwrap().contains("SoulBook 协作空间"));

        let tags = generate_task_result("extract_tags", &input);
        assert!(tags["tags"].as_array().unwrap().iter().any(|tag| tag == "SoulBook"));
    }
}
