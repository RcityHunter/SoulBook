use crate::{
    error::{AppError, Result},
    services::auth::{Claims, User},
    AppState,
};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::Extension,
    response::Json,
    routing::{get, post, put},
    Router,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router {
    Router::new()
        .route("/login", post(login))
        .route("/register", post(register))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/profile", put(update_profile))
        .route("/change-password", post(change_password))
}

#[derive(Debug, Serialize, Deserialize)]
struct LocalUser {
    pub id: Option<String>,
    pub email: String,
    pub username: Option<String>,
    pub password_hash: String,
    pub avatar_url: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    email: String,
    password: String,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateProfileRequest {
    username: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
}

fn issue_token(user_id: &str, secret: &str, expiry_seconds: i64) -> Result<String> {
    let claims = Claims {
        sub: user_id.to_string(),
        exp: (Utc::now() + Duration::seconds(expiry_seconds)).timestamp(),
        iat: Utc::now().timestamp(),
        session_id: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Token generation failed: {}", e)))
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Password hashing failed: {}", e)))
}

fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

async fn login(
    Extension(app_state): Extension<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<Value>> {
    let email = req.email.trim().to_lowercase();

    let mut result = app_state
        .db
        .client
        .query("SELECT type::string(id) AS id, email, username, password_hash, avatar_url, created_at FROM local_user WHERE email = $email LIMIT 1")
        .bind(("email", &email))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    let users: Vec<LocalUser> = result
        .take(0)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    let user = users
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Authentication("邮箱或密码不正确".to_string()))?;

    if !verify_password(&req.password, &user.password_hash) {
        return Err(AppError::Authentication("邮箱或密码不正确".to_string()));
    }

    let raw_id = user
        .id
        .as_deref()
        .unwrap_or("unknown")
        .replace("local_user:", "");
    let user_id = raw_id
        .trim_matches(|c: char| c == '`' || c == '⟨' || c == '⟩' || c == '"' || c == ' ')
        .to_string();
    let token = issue_token(
        &user_id,
        &app_state.config.auth.jwt_secret,
        app_state.config.auth.jwt_expiration as i64,
    )?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "token": token,
            "user": {
                "id": user_id,
                "email": user.email,
                "username": user.username,
                "avatar_url": user.avatar_url,
            }
        },
        "message": "Login successful"
    })))
}

async fn register(
    Extension(app_state): Extension<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<Value>> {
    let email = req.email.trim().to_lowercase();

    if req.password.len() < 6 {
        return Err(AppError::BadRequest("密码至少6位".to_string()));
    }

    // Check if email already exists
    let mut check = app_state
        .db
        .client
        .query("SELECT id FROM local_user WHERE email = $email LIMIT 1")
        .bind(("email", &email))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    let existing: Vec<Value> = check
        .take(0)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    if !existing.is_empty() {
        return Err(AppError::Conflict("该邮箱已被注册".to_string()));
    }

    let password_hash = hash_password(&req.password)?;
    let user_id = Uuid::new_v4().to_string();

    let mut result = app_state
        .db
        .client
        .query(
            "CREATE local_user SET
                id = type::record('local_user', $user_id),
                email = $email,
                username = $username,
                password_hash = $hash,
                created_at = time::now()",
        )
        .bind(("user_id", &user_id))
        .bind(("email", &email))
        .bind(("username", req.username.as_deref().unwrap_or("")))
        .bind(("hash", &password_hash))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("DB error: {}", e)))?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "id": user_id,
            "email": email,
            "username": req.username,
        },
        "message": "注册成功，请登录"
    })))
}

async fn logout() -> Json<Value> {
    Json(json!({ "success": true, "message": "Logged out" }))
}

async fn me(Extension(app_state): Extension<Arc<AppState>>, user: User) -> Result<Json<Value>> {
    // Try to fetch user details from local_user table
    let mut result = app_state
        .db
        .client
        .query("SELECT type::string(id) AS id, email, username, password_hash, avatar_url, created_at FROM local_user WHERE id = type::record('local_user', $id) LIMIT 1")
        .bind(("id", &user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    let users: Vec<LocalUser> = result.take(0).unwrap_or_default();

    let (email, username, avatar_url) = if let Some(u) = users.into_iter().next() {
        (u.email, u.username, u.avatar_url)
    } else {
        (user.email.clone(), None, None)
    };

    Ok(Json(json!({
        "success": true,
        "data": {
            "id": user.id,
            "email": email,
            "username": username,
            "avatar_url": avatar_url,
            "is_email_verified": true,
            "account_status": "active",
        },
        "message": "User retrieved"
    })))
}

async fn update_profile(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<Json<Value>> {
    app_state
        .db
        .client
        .query(
            "UPDATE local_user SET username = $username, avatar_url = $avatar_url
             WHERE id = type::record('local_user', $id)",
        )
        .bind(("id", &user.id))
        .bind(("username", req.username.as_deref().unwrap_or("")))
        .bind(("avatar_url", req.avatar_url.as_deref().unwrap_or("")))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    Ok(Json(json!({
        "success": true,
        "data": { "id": user.id, "username": req.username, "avatar_url": req.avatar_url },
        "message": "Profile updated"
    })))
}

async fn change_password(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<Value>> {
    let mut result = app_state
        .db
        .client
        .query("SELECT type::string(id) AS id, email, username, password_hash, avatar_url, created_at FROM local_user WHERE id = type::record('local_user', $id) LIMIT 1")
        .bind(("id", &user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    let users: Vec<LocalUser> = result.take(0).unwrap_or_default();
    let current_hash = users
        .into_iter()
        .next()
        .map(|u| u.password_hash)
        .unwrap_or_default();

    if !verify_password(&req.old_password, &current_hash) {
        return Err(AppError::Authentication("当前密码不正确".to_string()));
    }

    let new_hash = hash_password(&req.new_password)?;
    app_state
        .db
        .client
        .query(
            "UPDATE local_user SET password_hash = $hash WHERE id = type::record('local_user', $id)",
        )
        .bind(("id", &user.id))
        .bind(("hash", &new_hash))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    Ok(Json(json!({ "success": true, "message": "密码已修改" })))
}
