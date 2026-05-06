use crate::config::Config;
use crate::error::{AppError, Result};
use axum::{
    async_trait,
    extract::FromRequestParts,
    headers::{authorization::Bearer, Authorization},
    http::{request::Parts, StatusCode},
    Extension, RequestPartsExt, TypedHeader,
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct AuthService {
    config: Config,
    http_client: Client,
    user_cache: Arc<RwLock<HashMap<String, CachedUser>>>,
    permission_cache: Arc<RwLock<HashMap<String, CachedPermission>>>,
}

#[derive(Debug, Clone)]
struct CachedUser {
    user: User,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct CachedPermission {
    has_permission: bool,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    #[allow(dead_code)]
    message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,                // 用户ID
    pub exp: i64,                   // 过期时间
    pub iat: i64,                   // 签发时间
    pub session_id: Option<String>, // 会话ID
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
    pub profile: Option<UserProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RainbowAuthUserResponse {
    pub id: String,
    pub email: String,
    pub email_verified: Option<bool>,
    pub created_at: Option<String>,
    // 其他字段根据Rainbow-Auth实际返回调整
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RainbowAuthPermissionResponse {
    pub success: bool,
    pub data: PermissionData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PermissionData {
    pub has_permission: bool,
    pub user_id: String,
    pub permission: String,
}

impl AuthService {
    pub fn new(config: Config) -> Self {
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            http_client,
            user_cache: Arc::new(RwLock::new(HashMap::new())),
            permission_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn verify_jwt(&self, token: &str) -> Result<Claims> {
        let decoding_key = DecodingKey::from_secret(self.config.auth.jwt_secret.as_ref());
        let validation = Validation::new(Algorithm::HS256);

        match decode::<Claims>(token, &decoding_key, &validation) {
            Ok(token_data) => {
                info!("JWT token verified for user: {}", token_data.claims.sub);
                Ok(token_data.claims)
            }
            Err(e) => {
                warn!("JWT verification failed: {}", e);
                Err(AppError::Authentication("Invalid token".to_string()))
            }
        }
    }

    pub async fn get_user_from_rainbow_auth(&self, user_id: &str, token: &str) -> Result<User> {
        if !self.config.auth.integration_mode {
            return Err(AppError::Authentication(
                "Rainbow-Auth integration not enabled".to_string(),
            ));
        }

        // 检查缓存
        if let Some(cached_user) = self.get_cached_user(user_id).await {
            debug!("Using cached user data for user: {}", user_id);
            return Ok(cached_user);
        }

        let rainbow_auth_url = self.config.auth.rainbow_auth_url.as_ref().ok_or_else(|| {
            AppError::Authentication("Rainbow-Auth URL not configured".to_string())
        })?;

        let url = format!("{}/api/auth/me", rainbow_auth_url);

        let response = match self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                error!("Failed to fetch user from Rainbow-Auth: {}", e);
                warn!(
                    "Rainbow-Auth request failed for user {}, fallback to local JWT identity",
                    user_id
                );
                let fallback = self.build_fallback_user(user_id);
                self.cache_user(user_id, fallback.clone()).await;
                return Ok(fallback);
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            warn!(
                "Rainbow-Auth returned error status: {} body: {}",
                status, body
            );
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Err(AppError::Authentication("Invalid credentials".to_string()));
            }
            if status.is_server_error() {
                warn!(
                    "Rainbow-Auth upstream {} for user {}, fallback to local JWT identity",
                    status, user_id
                );
                let fallback = self.build_fallback_user(user_id);
                self.cache_user(user_id, fallback.clone()).await;
                return Ok(fallback);
            }
            return Err(AppError::Authentication(format!(
                "Rainbow-Auth unavailable: upstream status {}",
                status
            )));
        }

        let user_data: RainbowAuthUserResponse = response.json().await.map_err(|e| {
            error!("Failed to parse Rainbow-Auth response: {}", e);
            AppError::Authentication("Invalid response from Rainbow-Auth".to_string())
        })?;

        // 获取用户角色和权限
        let (roles, permissions) = self.get_user_permissions(&user_data.id, token).await?;

        let user = User {
            id: user_data.id.clone(),
            email: user_data.email,
            roles,
            permissions,
            profile: None, // 可以后续扩展获取用户档案
        };

        // 缓存用户数据
        self.cache_user(&user_data.id, user.clone()).await;

        Ok(user)
    }

    fn build_fallback_user(&self, user_id: &str) -> User {
        User {
            id: user_id.to_string(),
            email: "unknown@example.com".to_string(),
            roles: vec!["user".to_string()],
            permissions: vec![
                "docs.read".to_string(),
                "docs.write".to_string(),
                "spaces.read".to_string(),
                "spaces.write".to_string(),
            ],
            profile: None,
        }
    }

    async fn get_cached_user(&self, user_id: &str) -> Option<User> {
        let cache = self.user_cache.read().await;
        if let Some(cached) = cache.get(user_id) {
            if cached.expires_at > Utc::now() {
                return Some(cached.user.clone());
            }
        }
        None
    }

    async fn cache_user(&self, user_id: &str, user: User) {
        let mut cache = self.user_cache.write().await;
        cache.insert(
            user_id.to_string(),
            CachedUser {
                user,
                expires_at: Utc::now() + Duration::minutes(15), // 缓存15分钟
            },
        );
    }

    pub async fn get_user_permissions(
        &self,
        user_id: &str,
        token: &str,
    ) -> Result<(Vec<String>, Vec<String>)> {
        if !self.config.auth.integration_mode {
            // 独立模式：返回默认权限
            return Ok((vec!["user".to_string()], vec!["docs.read".to_string()]));
        }

        let rainbow_auth_url = self.config.auth.rainbow_auth_url.as_ref().ok_or_else(|| {
            AppError::Authentication("Rainbow-Auth URL not configured".to_string())
        })?;

        // 获取用户角色
        let roles_url = format!("{}/api/rbac/users/{}/roles", rainbow_auth_url, user_id);
        let roles_response = self
            .http_client
            .get(&roles_url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|_| AppError::Authentication("Failed to fetch user roles".to_string()))?;

        let roles = if roles_response.status().is_success() {
            match roles_response
                .json::<ApiResponse<serde_json::Value>>()
                .await
            {
                Ok(resp) => {
                    if resp.success {
                        // We don't depend on roles content today; keep a safe default.
                        vec!["user".to_string()]
                    } else {
                        vec!["user".to_string()]
                    }
                }
                Err(_) => vec!["user".to_string()],
            }
        } else {
            vec!["user".to_string()]
        };

        // 获取用户权限
        let permissions_url = format!(
            "{}/api/rbac/users/{}/permissions",
            rainbow_auth_url, user_id
        );
        let permissions_response = self
            .http_client
            .get(&permissions_url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|_| {
                AppError::Authentication("Failed to fetch user permissions".to_string())
            })?;

        let permissions = if permissions_response.status().is_success() {
            match permissions_response
                .json::<ApiResponse<Vec<String>>>()
                .await
            {
                Ok(resp) => {
                    if resp.success {
                        resp.data.unwrap_or_else(|| vec!["docs.read".to_string()])
                    } else {
                        vec!["docs.read".to_string()]
                    }
                }
                Err(_) => vec!["docs.read".to_string()],
            }
        } else {
            vec!["docs.read".to_string()]
        };

        Ok((roles, permissions))
    }

    pub async fn check_permission(
        &self,
        user_id: &str,
        permission: &str,
        resource_id: Option<&str>,
    ) -> Result<bool> {
        if !self.config.auth.integration_mode {
            // 独立模式：简单权限检查
            debug!(
                "Independent mode: Checking permission {} for user {} on resource {:?}",
                permission, user_id, resource_id
            );
            return match permission {
                "docs.read" | "docs.comment.read" | "spaces.read" => Ok(true),
                "docs.write" | "docs.create" | "docs.update" | "docs.comment.write"
                | "spaces.write" | "spaces.create" => Ok(true),
                "docs.delete" => Ok(true),
                _ => Ok(false),
            };
        }

        // 检查权限缓存
        let cache_key = format!("{}:{}", user_id, permission);
        if let Some(cached_permission) = self.get_cached_permission(&cache_key).await {
            debug!(
                "Using cached permission for {}: {}",
                cache_key, cached_permission
            );
            return Ok(cached_permission);
        }

        // 如果没有Rainbow-Auth集成，使用简单权限检查
        let has_permission = match permission {
            "docs.read" | "docs.comment.read" | "spaces.read" => true,
            "docs.write" | "docs.create" | "docs.update" | "docs.comment.write"
            | "spaces.write" | "spaces.create" => true,
            "docs.delete" => true,
            _ => false,
        };

        // 缓存权限结果
        self.cache_permission(&cache_key, has_permission).await;

        Ok(has_permission)
    }

    async fn get_cached_permission(&self, cache_key: &str) -> Option<bool> {
        let cache = self.permission_cache.read().await;
        if let Some(cached) = cache.get(cache_key) {
            if cached.expires_at > Utc::now() {
                return Some(cached.has_permission);
            }
        }
        None
    }

    async fn cache_permission(&self, cache_key: &str, has_permission: bool) {
        let mut cache = self.permission_cache.write().await;
        cache.insert(
            cache_key.to_string(),
            CachedPermission {
                has_permission,
                expires_at: Utc::now() + Duration::minutes(10), // 权限缓存10分钟
            },
        );
    }

    // 批量权限检查
    pub async fn check_multiple_permissions(
        &self,
        user_id: &str,
        permissions: &[&str],
        token: &str,
    ) -> Result<HashMap<String, bool>> {
        let mut results = HashMap::new();

        for permission in permissions {
            let has_permission = self.check_permission(user_id, permission, None).await?;
            results.insert(permission.to_string(), has_permission);
        }

        Ok(results)
    }

    // 清理过期缓存
    pub async fn cleanup_cache(&self) {
        let now = Utc::now();

        // 清理用户缓存
        {
            let mut user_cache = self.user_cache.write().await;
            user_cache.retain(|_, cached| cached.expires_at > now);
        }

        // 清理权限缓存
        {
            let mut permission_cache = self.permission_cache.write().await;
            permission_cache.retain(|_, cached| cached.expires_at > now);
        }

        debug!("Cache cleanup completed");
    }
}

fn is_api_key_token(token: &str) -> bool {
    token.starts_with("sk-sd-") || token.starts_with("sk-sb-")
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(input.as_bytes());
    format!("{:x}", digest)
}

fn created_by_from_api_key_row(row: &Value) -> Result<String> {
    row.get("created_by")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| AppError::Authentication("Invalid token".to_string()))
}

fn ensure_api_key_not_expired(row: &Value) -> Result<()> {
    let Some(expires_at) = row.get("expires_at").and_then(|value| value.as_str()) else {
        return Ok(());
    };

    if expires_at.trim().is_empty() {
        return Ok(());
    }

    let expires_at = DateTime::parse_from_rfc3339(expires_at)
        .map_err(|_| AppError::Authentication("Invalid token".to_string()))?
        .with_timezone(&Utc);

    if expires_at <= Utc::now() {
        return Err(AppError::Authentication("Invalid token".to_string()));
    }

    Ok(())
}

// Axum extractor for authentication
#[async_trait]
impl<S> FromRequestParts<S> for User
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        // Extract the authorization header
        let TypedHeader(Authorization(bearer)) = parts
            .extract::<TypedHeader<Authorization<Bearer>>>()
            .await
            .map_err(|_| AppError::Authentication("Missing authorization header".to_string()))?;

        // Get auth service from app state
        let Extension(auth_service): Extension<Arc<AuthService>> = parts
            .extract::<Extension<Arc<AuthService>>>()
            .await
            .map_err(|_| AppError::Internal(anyhow::anyhow!("Auth service not found")))?;

        let token = bearer.token();

        if is_api_key_token(token) {
            let Extension(app_state): Extension<Arc<crate::AppState>> = parts
                .extract::<Extension<Arc<crate::AppState>>>()
                .await
                .map_err(|_| AppError::Internal(anyhow::anyhow!("App state not found")))?;

            let key_hash = sha256_hex(token);
            let mut result = app_state
                .db
                .client
                .query(
                    "SELECT created_by, scopes, expires_at FROM api_key
                     WHERE key_hash = $key_hash AND is_deleted = false LIMIT 1",
                )
                .bind(("key_hash", key_hash))
                .await
                .map_err(|e| {
                    warn!("API key lookup failed: {}", e);
                    AppError::Authentication("Invalid token".to_string())
                })?;

            let rows: Vec<Value> = result.take(0).map_err(|e| {
                warn!("API key row parse failed: {}", e);
                AppError::Authentication("Invalid token".to_string())
            })?;

            let row = rows
                .into_iter()
                .next()
                .ok_or_else(|| AppError::Authentication("Invalid token".to_string()))?;
            ensure_api_key_not_expired(&row)?;
            let user_id = created_by_from_api_key_row(&row)?;

            let _ = app_state
                .db
                .client
                .query("UPDATE api_key SET last_used_at = time::now() WHERE key_hash = $key_hash")
                .bind(("key_hash", sha256_hex(token)))
                .await
                .map_err(|e| warn!("failed to update API key last_used_at: {}", e));

            return Ok(auth_service.build_fallback_user(&user_id));
        }

        // Verify JWT token
        let claims = auth_service.verify_jwt(token)?;

        // Get user details from Rainbow-Auth if integration is enabled
        if auth_service.config.auth.integration_mode {
            auth_service
                .get_user_from_rainbow_auth(&claims.sub, bearer.token())
                .await
        } else {
            // Standalone mode: create user from JWT claims with default values
            Ok(User {
                id: claims.sub,
                email: "unknown@example.com".to_string(), // 默认邮箱，因为JWT中没有
                roles: vec!["user".to_string()],          // 默认角色
                permissions: vec![
                    "docs.read".to_string(),
                    "docs.write".to_string(),
                    "spaces.read".to_string(),
                    "spaces.write".to_string(),
                ], // 默认权限
                profile: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_token_detection_accepts_developer_and_agent_prefixes() {
        assert!(is_api_key_token("sk-sd-abc"));
        assert!(is_api_key_token("sk-sb-abc"));
        assert!(!is_api_key_token("eyJhbGciOiJIUzI1NiJ9"));
        assert!(!is_api_key_token("sk-other-abc"));
    }

    #[test]
    fn api_key_hash_matches_known_sha256_hex() {
        assert_eq!(
            sha256_hex("sk-sd-test"),
            "93964d5317248b26c35085a28cbc52079404f82d4936ac28a46b6e3438e5be3e"
        );
    }

    #[test]
    fn api_key_row_requires_created_by() {
        let row = serde_json::json!({ "created_by": "user-1" });
        assert_eq!(created_by_from_api_key_row(&row).unwrap(), "user-1");

        let row = serde_json::json!({ "created_by": "" });
        assert!(created_by_from_api_key_row(&row).is_err());
    }

    #[test]
    fn api_key_expiration_allows_missing_or_future_expiry_only() {
        assert!(ensure_api_key_not_expired(&serde_json::json!({})).is_ok());
        assert!(ensure_api_key_not_expired(&serde_json::json!({ "expires_at": "" })).is_ok());

        let future = (Utc::now() + Duration::minutes(5)).to_rfc3339();
        assert!(ensure_api_key_not_expired(&serde_json::json!({ "expires_at": future })).is_ok());

        let past = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        assert!(ensure_api_key_not_expired(&serde_json::json!({ "expires_at": past })).is_err());
    }
}

// Optional authentication extractor
pub struct OptionalUser(pub Option<User>);

#[async_trait]
impl<S> FromRequestParts<S> for OptionalUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        match User::from_request_parts(parts, state).await {
            Ok(user) => Ok(OptionalUser(Some(user))),
            Err(_) => Ok(OptionalUser(None)),
        }
    }
}
