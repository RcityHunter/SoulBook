use axum::{
    Extension,
    extract::Query,
    http::{
        HeaderMap,
        header::{COOKIE, SET_COOKIE},
    },
    response::{Html, IntoResponse, Response},
    routing::{Router, delete, get, post},
};
use chrono::{Duration as ChronoDuration, Utc};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::time::{Duration, interval};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

mod agent;
mod config;
mod error;
mod models;
mod routes;
mod services;
mod state;
mod utils;

use crate::{
    config::Config,
    error::AppError,
    services::{
        auth::AuthService, comments::CommentService, database::Database,
        documents::DocumentService, file_upload::FileUploadService,
        publication::PublicationService, search::SearchService, space_member::SpaceMemberService,
        spaces::SpaceService, tags::TagService, versions::VersionService,
        workspace::WorkspaceService,
    },
    state::AppState,
    utils::markdown::MarkdownProcessor,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            "soulbook=debug,rainbow_docs=debug,tower_http=debug",
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting SoulBook service...");

    // 加载配置
    dotenv::dotenv().ok();
    let config = Config::from_env()?;

    // 检查是否需要跳过数据库连接（安装模式且未安装）
    #[cfg(feature = "installer")]
    {
        use crate::utils::installer::InstallationChecker;
        if let Ok(should_install) = InstallationChecker::should_show_installer() {
            if should_install {
                info!("System not installed, starting in installer-only mode");
                return start_installer_only_mode(config).await;
            }
        }
    }

    // 初始化数据库连接（已安装或非安装模式）
    // 如果数据库连接失败，尝试自动启动数据库
    let db = match Database::new(&config).await {
        Ok(db) => {
            match db.verify_connection().await {
                Ok(_) => {
                    info!("Database connection established successfully");
                    db
                }
                Err(e) => {
                    warn!("Database connection failed: {}", e);
                    info!("Attempting to auto-start database...");

                    // 尝试自动启动数据库
                    if let Err(start_err) = auto_start_database(&config).await {
                        return Err(anyhow::anyhow!(
                            "Failed to auto-start database: {}. Original error: {}",
                            start_err,
                            e
                        ));
                    }

                    // 重新尝试连接
                    let db = Database::new(&config).await?;
                    db.verify_connection().await?;
                    info!("Database auto-started and connected successfully");
                    db
                }
            }
        }
        Err(e) => {
            warn!("Failed to create database connection: {}", e);
            info!("Attempting to auto-start database...");

            // 尝试自动启动数据库
            if let Err(start_err) = auto_start_database(&config).await {
                return Err(anyhow::anyhow!(
                    "Failed to auto-start database: {}. Original error: {}",
                    start_err,
                    e
                ));
            }

            // 重新尝试连接
            let db = Database::new(&config).await?;
            db.verify_connection().await?;
            info!("Database auto-started and connected successfully");
            db
        }
    };

    info!(
        "Database connection established. Please ensure database schema is initialized with docs_schema.sql"
    );

    // 创建共享的数据库实例
    let shared_db = Arc::new(db.clone());

    // 创建认证服务
    let auth_service = Arc::new(AuthService::new(config.clone()));

    // 创建业务服务
    let space_service = Arc::new(SpaceService::new(shared_db.clone()));
    let space_member_service = Arc::new(SpaceMemberService::new(shared_db.clone(), config.clone()));
    let workspace_service = Arc::new(WorkspaceService::new(shared_db.clone()));
    workspace_service.ensure_schema().await?;
    let file_upload_service = Arc::new(FileUploadService::new(
        shared_db.clone(),
        auth_service.clone(),
    ));
    let tag_service = Arc::new(TagService::new(shared_db.clone(), auth_service.clone()));

    let markdown_processor = Arc::new(MarkdownProcessor::new());
    let search_service = Arc::new(SearchService::new(shared_db.clone(), auth_service.clone()));
    let version_service = Arc::new(VersionService::new(shared_db.clone(), auth_service.clone()));
    let document_service = Arc::new(
        DocumentService::new(
            shared_db.clone(),
            auth_service.clone(),
            markdown_processor.clone(),
        )
        .with_search_service(search_service.clone())
        .with_version_service(version_service.clone()),
    );
    let comment_service = Arc::new(CommentService::new(shared_db.clone(), auth_service.clone()));
    let publication_service = Arc::new(PublicationService::new(shared_db.clone()));

    // 启动缓存清理任务
    let cleanup_auth = auth_service.clone();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(1800)); // 每30分钟清理一次
        loop {
            interval.tick().await;
            cleanup_auth.cleanup_cache().await;
        }
    });

    // 创建 app state
    let app_state = AppState {
        db: shared_db.clone(),
        config: config.clone(),
        auth_service: auth_service.clone(),
        space_service: space_service.clone(),
        space_member_service: space_member_service.clone(),
        workspace_service: workspace_service.clone(),
        file_upload_service: file_upload_service.clone(),
        tag_service: tag_service.clone(),
        document_service: document_service.clone(),
        comment_service: comment_service.clone(),
        publication_service: publication_service.clone(),
        search_service: search_service.clone(),
        version_service: version_service.clone(),
    };
    let app_state = Arc::new(app_state);

    // 创建路由
    let auth_router = if config.auth.integration_mode {
        routes::auth_gateway::router()
    } else {
        routes::local_auth::router()
    };

    let mut app = Router::new()
        .nest("/api/auth", auth_router)
        .nest("/api/docs/agent", agent::router::router())
        .nest("/api/docs/auth/google", routes::google_oauth::router())
        .nest("/api/docs/auth/soulauth", routes::soulauth_oidc::router())
        .nest("/api/docs/auth", routes::auth::router())
        .nest("/api/docs/spaces", routes::spaces::router())
        .nest("/api/docs/spaces", routes::space_members::router())
        .nest("/api/docs/workspaces", routes::workspaces::router())
        .nest("/api/docs/files", routes::files::router())
        .nest("/api/docs/tags", routes::tags::router())
        .nest("/api/docs/documents", routes::documents::router())
        .nest("/api/docs/comments", routes::comments::router())
        .nest("/api/docs/notifications", routes::notifications::router())
        .nest("/api/docs/publications", routes::publication::router())
        .nest("/api/docs/search", routes::search::router())
        .nest("/api/docs/stats", routes::stats::router())
        .nest("/api/docs/versions", routes::versions::router())
        .nest(
            "/api/docs/change-requests",
            routes::change_requests::router(),
        )
        .nest("/api/docs/ai-tasks", routes::ai_tasks::router())
        .nest("/api/docs/language", routes::language::router())
        .nest("/api/docs/settings", routes::settings::router())
        .nest("/api/docs/tool-configs", routes::tool_configs::router())
        .nest("/api/docs/git-sync", routes::git_sync::router())
        .nest("/api/docs/developer", routes::developer::router())
        .nest("/api/docs/templates", routes::templates::router())
        .nest("/api/docs/publish", routes::publish::router())
        .nest("/api/docs", vectors_router())
        .nest("/agent/v1", agent::router::router())
        .route("/sso", get(sso_bridge));

    // 如果是安装模式，额外添加安装路由
    #[cfg(feature = "installer")]
    {
        app = app.nest("/api/install", routes::installer::installer_routes());
    }

    let app = app
        .layer(Extension(app_state.clone()))
        .layer(Extension(shared_db))
        .layer(Extension(config.clone()))
        .layer(Extension(auth_service.clone()))
        .layer(build_cors_layer());

    // 启动服务器
    let addr = format!("{}:{}", config.server.host, config.server.port);
    info!("SoulBook server listening on {}", addr);
    axum::Server::bind(&addr.parse()?)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

/// Build CORS layer from `CORS_ALLOWED_ORIGINS` env var.
/// Empty/unset → allow any origin (dev fallback). Comma-separated list → strict whitelist.
fn build_cors_layer() -> CorsLayer {
    let allow_origin = match std::env::var("CORS_ALLOWED_ORIGINS")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(raw) => {
            let origins: Vec<_> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            info!("CORS whitelist active: {} origins", origins.len());
            AllowOrigin::list(origins)
        }
        None => {
            warn!("CORS_ALLOWED_ORIGINS not set — allowing any origin (DEV ONLY)");
            AllowOrigin::any()
        }
    };
    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(Any)
        .allow_headers(Any)
}

#[derive(Deserialize)]
struct SsoParams {
    bridge: Option<String>,
    token: Option<String>,
}

async fn sso_bridge(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<SsoParams>,
) -> Response {
    let binding_cookie = sso_bridge_binding_from_headers(&headers);
    let payload = match resolve_sso_bridge_payload(
        params.bridge,
        params.token,
        binding_cookie,
        &state.config.server.app_url,
    ) {
        Ok(payload) => payload,
        Err(message) => return html_with_clear_sso_bridge_cookie(message.to_string()),
    };
    let token = payload.token;
    let next = payload.next;
    let token_js = serde_json::to_string(&token).unwrap_or_else(|_| "\"\"".into());
    let next_js = serde_json::to_string(&next).unwrap_or_else(|_| "\"/\"".into());
    let html = format!(
        r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>SSO Redirect</title>
  </head>
  <body>
    <script>
      const token = {token_js};
      const next = {next_js};
      try {{
        const storedToken = JSON.stringify(token);
        localStorage.setItem('jwt_token', storedToken);
        localStorage.setItem('auth_token', storedToken);
        localStorage.setItem('token', storedToken);
        localStorage.setItem('souldoc_token', storedToken);
        localStorage.setItem('soulbook_token', storedToken);
        localStorage.setItem('soulbook_token_raw', token);
      }} catch (e) {{
        // ignore storage errors
      }}
      window.location.replace(next);
    </script>
  </body>
</html>"#
    );
    html_with_clear_sso_bridge_cookie(html)
}

const SSO_BRIDGE_PURPOSE: &str = "soulbook_sso_bridge";
const SSO_BRIDGE_TTL_SECONDS: i64 = 60;
const SSO_BRIDGE_COOKIE_NAME: &str = "soulbook_sso_bridge_binding";
const SSO_BRIDGE_COOKIE_PATH: &str = "/sso";

#[derive(Debug, Clone)]
struct SsoBridgeEntry {
    token: String,
    next: String,
    purpose: String,
    binding: String,
    exp: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct SsoBridgePayload {
    token: String,
    next: String,
}

lazy_static::lazy_static! {
    static ref SSO_BRIDGE_STORE: Mutex<HashMap<String, SsoBridgeEntry>> = Mutex::new(HashMap::new());
}

pub(crate) fn create_sso_bridge_handle(
    token: &str,
    next: Option<String>,
    default_next: &str,
    binding: &str,
) -> std::result::Result<String, AppError> {
    if token.trim().is_empty() {
        return Err(AppError::BadRequest("missing token".into()));
    }
    if binding.trim().is_empty() {
        return Err(AppError::BadRequest("missing bridge binding".into()));
    }

    cleanup_expired_sso_bridge_entries(Utc::now().timestamp());

    let handle = Uuid::new_v4().simple().to_string();
    let entry = SsoBridgeEntry {
        token: token.to_string(),
        next: sanitize_sso_next(next, default_next),
        purpose: SSO_BRIDGE_PURPOSE.to_string(),
        binding: binding.to_string(),
        exp: (Utc::now() + ChronoDuration::seconds(SSO_BRIDGE_TTL_SECONDS)).timestamp(),
    };

    SSO_BRIDGE_STORE
        .lock()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("sso bridge store poisoned")))?
        .insert(handle.clone(), entry);

    Ok(handle)
}

fn resolve_sso_bridge_payload(
    bridge: Option<String>,
    raw_token: Option<String>,
    binding_cookie: Option<String>,
    default_next: &str,
) -> std::result::Result<SsoBridgePayload, &'static str> {
    if raw_token.is_some() {
        return Err("missing or invalid bridge");
    }

    let bridge = bridge
        .filter(|value| !value.trim().is_empty())
        .ok_or("missing or invalid bridge")?;

    let now = Utc::now().timestamp();
    let mut store = SSO_BRIDGE_STORE
        .lock()
        .map_err(|_| "missing or invalid bridge")?;
    let Some(entry) = store.get(&bridge) else {
        return Err("missing or invalid bridge");
    };

    if entry.exp <= now {
        store.remove(&bridge);
        return Err("missing or invalid bridge");
    }

    if entry.purpose != SSO_BRIDGE_PURPOSE
        || entry.token.trim().is_empty()
        || entry.binding.trim().is_empty()
    {
        store.remove(&bridge);
        return Err("missing or invalid bridge");
    }

    if binding_cookie.as_deref() != Some(entry.binding.as_str()) {
        return Err("missing or invalid bridge");
    }

    let entry = store.remove(&bridge).ok_or("missing or invalid bridge")?;

    Ok(SsoBridgePayload {
        token: entry.token,
        next: sanitize_sso_next(Some(entry.next), default_next),
    })
}

pub(crate) fn create_sso_bridge_binding() -> String {
    Uuid::new_v4().simple().to_string()
}

fn cleanup_expired_sso_bridge_entries(now: i64) {
    if let Ok(mut store) = SSO_BRIDGE_STORE.lock() {
        store.retain(|_, entry| entry.exp > now);
    }
}

#[cfg(test)]
fn insert_test_sso_bridge_entry(entry: SsoBridgeEntry) -> String {
    let handle = Uuid::new_v4().simple().to_string();
    SSO_BRIDGE_STORE
        .lock()
        .expect("sso bridge store should lock")
        .insert(handle.clone(), entry);
    handle
}

pub(crate) fn sso_bridge_binding_cookie(binding: &str) -> String {
    format!(
        "{}={}; Max-Age={}; Path={}; HttpOnly; Secure; SameSite=Lax",
        SSO_BRIDGE_COOKIE_NAME, binding, SSO_BRIDGE_TTL_SECONDS, SSO_BRIDGE_COOKIE_PATH
    )
}

fn clear_sso_bridge_binding_cookie() -> String {
    format!(
        "{}=; Max-Age=0; Path={}; HttpOnly; Secure; SameSite=Lax",
        SSO_BRIDGE_COOKIE_NAME, SSO_BRIDGE_COOKIE_PATH
    )
}

fn html_with_clear_sso_bridge_cookie(html: String) -> Response {
    let mut response = Html(html).into_response();
    if let Ok(header) = clear_sso_bridge_binding_cookie().parse() {
        response.headers_mut().append(SET_COOKIE, header);
    }
    response
}

fn sso_bridge_binding_from_headers(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == SSO_BRIDGE_COOKIE_NAME && !value.trim().is_empty()).then(|| value.to_string())
    })
}

pub(crate) fn sanitize_sso_next(next: Option<String>, default: &str) -> String {
    let Some(next) = next else {
        return default.to_string();
    };
    let next = next.trim();
    let Some(normalized_next) = normalize_sso_next(next) else {
        return default.to_string();
    };

    if is_sso_bridge_path(&normalized_next) {
        return default.to_string();
    }

    normalized_next
}

fn normalize_sso_next(next: &str) -> Option<String> {
    if next.is_empty()
        || !next.starts_with('/')
        || next.starts_with("//")
        || next.contains('\\')
        || next.chars().any(char::is_control)
    {
        return None;
    }

    let mut normalized = next.to_string();
    for _ in 0..3 {
        let decoded = urlencoding::decode(&normalized).ok()?.into_owned();
        if decoded == normalized {
            break;
        }
        normalized = decoded;
    }

    if normalized.is_empty()
        || !normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.contains('\\')
        || normalized.chars().any(char::is_control)
    {
        return None;
    }

    Some(normalized)
}

fn is_sso_bridge_path(next: &str) -> bool {
    let path_end = next.find(['?', '#']).unwrap_or(next.len());
    let path = &next[..path_end];
    let normalized_path = normalize_path_segments(path);
    normalized_path.eq_ignore_ascii_case("/sso")
        || normalized_path.to_ascii_lowercase().starts_with("/sso/")
}

fn normalize_path_segments(path: &str) -> String {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

// 自动启动数据库的函数
async fn auto_start_database(config: &Config) -> anyhow::Result<()> {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    info!("Auto-starting SurrealDB database service...");

    // 创建数据目录（如果不存在）
    let data_dir = "./data";
    if !Path::new(data_dir).exists() {
        fs::create_dir_all(data_dir)
            .map_err(|e| anyhow::anyhow!("Failed to create data directory: {}", e))?;
    }

    // 构建数据库文件路径
    let db_file = format!("{}/rainbow.db", data_dir);

    // 从配置中读取数据库认证信息
    let database_user = config.database.user.clone();
    let database_pass = config.database.pass.clone();
    let database_url = config.database.url.clone();

    // 构建启动命令
    // Strip scheme from bind URL — surreal start expects "host:port", not "http://host:port"
    let bind_addr = database_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();

    let mut cmd = Command::new("surreal");
    cmd.arg("start")
        .arg("--username")
        .arg(&database_user)
        .arg("--password")
        .arg(&database_pass)
        .arg("--bind")
        .arg(&bind_addr)
        .arg(format!("file:{}", db_file));

    info!(
        "Executing: surreal start --username {} --password *** --bind {} file:{}",
        database_user, bind_addr, db_file
    );

    let child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!(
            "Failed to start SurrealDB: {}. Please make sure SurrealDB is installed.",
            e
        )
    })?;

    // 保存进程ID
    let pid = child.id();
    fs::write(".surreal_pid", pid.to_string())
        .map_err(|e| anyhow::anyhow!("Failed to save database PID: {}", e))?;

    info!("SurrealDB process started (PID: {})", pid);

    // 等待数据库启动
    info!("Waiting for database service to be ready...");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    info!("Database service should be ready now");
    Ok(())
}

fn vectors_router() -> Router {
    Router::new()
        .route(
            "/documents/:id/vectors",
            post(routes::vectors::store_document_vector),
        )
        .route(
            "/documents/:id/vectors",
            get(routes::vectors::get_document_vectors),
        )
        .route(
            "/documents/:id/vectors/:vector_id",
            delete(routes::vectors::delete_document_vector),
        )
        .route("/search/vector", post(routes::vectors::vector_search))
        .route(
            "/documents/batch",
            post(routes::vectors::batch_get_documents),
        )
        .route(
            "/vectors/batch",
            post(routes::vectors::batch_update_vectors),
        )
}

#[cfg(feature = "installer")]
async fn start_installer_only_mode(config: Config) -> anyhow::Result<()> {
    use crate::routes::installer::installer_routes;

    info!("Starting installer-only mode (no database required)");

    // 创建仅包含安装路由的应用
    let app = Router::new()
        .nest("/api/install", installer_routes())
        .layer(Extension(config))
        .layer(build_cors_layer());

    // 启动服务器
    let addr = "0.0.0.0:3000";
    info!("SoulBook installer-only mode listening on {}", addr);
    axum::Server::bind(&addr.parse()?)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sso_safe_relative_next_is_accepted() {
        assert_eq!(
            sanitize_sso_next(
                Some("/docs/space?tab=read#top".to_string()),
                "https://book.test"
            ),
            "/docs/space?tab=read#top"
        );
        assert_eq!(
            sanitize_sso_next(Some("/search?q=a".to_string()), "https://book.test"),
            "/search?q=a"
        );
    }

    #[test]
    fn sso_unsafe_next_values_fall_back_to_app_default() {
        for next in [
            "javascript:alert(1)",
            "http://evil.test",
            "https://evil.test",
            "//evil.test/path",
            "/docs\n<script>",
            "",
            "   ",
        ] {
            assert_eq!(
                sanitize_sso_next(Some(next.to_string()), "https://book.test"),
                "https://book.test"
            );
        }
    }

    #[test]
    fn sso_nested_sso_next_values_fall_back_to_app_default() {
        for next in [
            "/sso",
            "/sso?bridge=attacker",
            "/sso#fragment",
            "/sso/",
            "/%73so?bridge=attacker",
            "/s%73o?bridge=attacker",
            "/sso%3Fbridge%3Dattacker",
            "/docs/../sso?bridge=attacker",
            "/./sso?bridge=attacker",
            "/%2e/sso?bridge=attacker",
            "/docs/%2e%2e/sso?bridge=attacker",
            "/docs/%2E%2E/%73so?bridge=attacker",
        ] {
            assert_eq!(
                sanitize_sso_next(Some(next.to_string()), "https://book.test"),
                "https://book.test"
            );
        }
    }

    #[test]
    fn sso_bridge_with_matching_binding_cookie_is_accepted() {
        let binding = create_sso_bridge_binding();
        let bridge = create_sso_bridge_handle(
            "jwt-value",
            Some("/docs/space".to_string()),
            "https://book.test",
            &binding,
        )
        .expect("bridge should be stored");

        let payload =
            resolve_sso_bridge_payload(Some(bridge), None, Some(binding), "https://book.test")
                .expect("bridge should resolve");

        assert_eq!(payload.token, "jwt-value");
        assert_eq!(payload.next, "/docs/space");
    }

    #[test]
    fn sso_bridge_nested_sso_next_falls_back_to_app_default() {
        let binding = create_sso_bridge_binding();
        let bridge = create_sso_bridge_handle(
            "jwt-value",
            Some("/sso?bridge=attacker".to_string()),
            "https://book.test",
            &binding,
        )
        .expect("bridge should be stored");

        let payload =
            resolve_sso_bridge_payload(Some(bridge), None, Some(binding), "https://book.test")
                .expect("bridge should resolve");

        assert_eq!(payload.token, "jwt-value");
        assert_eq!(payload.next, "https://book.test");
    }

    #[test]
    fn sso_raw_token_without_bridge_is_rejected() {
        let err = resolve_sso_bridge_payload(
            None,
            Some("attacker-token".to_string()),
            None,
            "https://book.test",
        )
        .expect_err("raw token should be rejected");

        assert_eq!(err, "missing or invalid bridge");
    }

    #[test]
    fn sso_expired_bridge_handle_is_rejected() {
        let bridge = insert_test_sso_bridge_entry(SsoBridgeEntry {
            token: "jwt-value".to_string(),
            next: "/docs".to_string(),
            purpose: SSO_BRIDGE_PURPOSE.to_string(),
            binding: "binding-value".to_string(),
            exp: (chrono::Utc::now() - chrono::Duration::seconds(1)).timestamp(),
        });

        let err = resolve_sso_bridge_payload(
            Some(bridge),
            None,
            Some("binding-value".to_string()),
            "https://book.test",
        )
        .expect_err("expired bridge should be rejected");

        assert_eq!(err, "missing or invalid bridge");
    }

    #[test]
    fn sso_bridge_without_binding_cookie_is_rejected() {
        let binding = create_sso_bridge_binding();
        let bridge = create_sso_bridge_handle(
            "jwt-value",
            Some("/docs".to_string()),
            "https://book.test",
            &binding,
        )
        .expect("bridge should be stored");

        let err = resolve_sso_bridge_payload(Some(bridge), None, None, "https://book.test")
            .expect_err("bridge replay without cookie should be rejected");

        assert_eq!(err, "missing or invalid bridge");
    }

    #[test]
    fn sso_bridge_with_wrong_binding_cookie_is_rejected() {
        let bridge = create_sso_bridge_handle(
            "jwt-value",
            Some("/docs".to_string()),
            "https://book.test",
            "binding-value",
        )
        .expect("bridge should be stored");

        let err = resolve_sso_bridge_payload(
            Some(bridge),
            None,
            Some("other-binding".to_string()),
            "https://book.test",
        )
        .expect_err("bridge replay with another browser cookie should be rejected");

        assert_eq!(err, "missing or invalid bridge");
    }

    #[test]
    fn sso_bridge_handle_payload_does_not_contain_raw_app_jwt() {
        let binding = create_sso_bridge_binding();
        let app_jwt = "header.payload.signature";
        let bridge = create_sso_bridge_handle(
            app_jwt,
            Some("/docs".to_string()),
            "https://book.test",
            &binding,
        )
        .expect("bridge should be stored");

        assert!(!bridge.contains(app_jwt));
        assert!(bridge.split('.').count() < 3);
        assert!(
            jsonwebtoken::decode::<serde_json::Value>(
                &bridge,
                &jsonwebtoken::DecodingKey::from_secret("test-secret".as_ref()),
                &jsonwebtoken::Validation::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn sso_bridge_handle_is_consumed_once() {
        let binding = create_sso_bridge_binding();
        let bridge = create_sso_bridge_handle(
            "jwt-value",
            Some("/docs".to_string()),
            "https://book.test",
            &binding,
        )
        .expect("bridge should be stored");

        let payload = resolve_sso_bridge_payload(
            Some(bridge.clone()),
            None,
            Some(binding.clone()),
            "https://book.test",
        )
        .expect("first use should resolve");

        assert_eq!(payload.token, "jwt-value");
        assert!(
            resolve_sso_bridge_payload(Some(bridge), None, Some(binding), "https://book.test",)
                .is_err()
        );
    }
}
