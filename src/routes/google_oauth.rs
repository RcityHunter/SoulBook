use axum::{
    extract::{Extension, Query},
    response::Redirect,
    routing::get,
    Router,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    error::{AppError, Result},
    services::auth::Claims,
    AppState,
};

pub fn router() -> Router {
    Router::new()
        .route("/start", get(start))
        .route("/callback", get(callback))
}

#[derive(Debug, Deserialize)]
struct StartParams {
    next: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OAuthState {
    nonce: String,
    next: Option<String>,
    exp: i64,
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn start(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(params): Query<StartParams>,
) -> Result<Redirect> {
    let oauth = app_state
        .config
        .oauth
        .google
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("Google login is not configured".into()))?;

    // Build short-lived state JWT (10 min) for CSRF protection + carry post-login `next`
    let state_claims = OAuthState {
        nonce: Uuid::new_v4().to_string(),
        next: params.next,
        exp: (Utc::now() + Duration::minutes(10)).timestamp(),
    };
    let state = encode(
        &Header::default(),
        &state_claims,
        &EncodingKey::from_secret(app_state.config.auth.jwt_secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("state encode failed: {}", e)))?;

    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?response_type=code\
         &client_id={}\
         &redirect_uri={}\
         &scope={}\
         &state={}\
         &access_type=online\
         &prompt=select_account",
        urlencoding::encode(&oauth.client_id),
        urlencoding::encode(&oauth.redirect_uri),
        urlencoding::encode("openid email profile"),
        urlencoding::encode(&state),
    );

    Ok(Redirect::to(&url))
}

async fn callback(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> Result<Redirect> {
    if let Some(err) = params.error {
        return Err(AppError::BadRequest(format!("Google OAuth error: {}", err)));
    }
    let code = params
        .code
        .ok_or_else(|| AppError::BadRequest("missing code".into()))?;
    let state_token = params
        .state
        .ok_or_else(|| AppError::BadRequest("missing state".into()))?;

    // Verify state (CSRF + replay protection via exp)
    let state_claims = decode::<OAuthState>(
        &state_token,
        &DecodingKey::from_secret(app_state.config.auth.jwt_secret.as_ref()),
        &Validation::default(),
    )
    .map_err(|_| AppError::BadRequest("invalid or expired state".into()))?
    .claims;

    let oauth = app_state
        .config
        .oauth
        .google
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("Google login is not configured".into()))?;

    let http = reqwest::Client::new();

    // Step 1: exchange authorization code for access_token
    let token_resp = http
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code.as_str()),
            ("client_id", oauth.client_id.as_str()),
            ("client_secret", oauth.client_secret.as_str()),
            ("redirect_uri", oauth.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Google token request failed: {}", e)))?;

    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(AppError::BadRequest(format!(
            "Google token exchange failed: {}",
            body
        )));
    }

    let token_json: Value = token_resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Google token parse failed: {}", e)))?;

    let access_token = token_json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Google did not return access_token")))?;

    // Step 2: fetch user profile
    let userinfo: Value = http
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Google userinfo request failed: {}", e)))?
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Google userinfo parse failed: {}", e)))?;

    let email = userinfo
        .get("email")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("Google account has no email".into()))?
        .trim()
        .to_lowercase();
    let name = userinfo
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let picture = userinfo
        .get("picture")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Step 3: find or create local_user by email
    let db = &app_state.db.client;
    let mut select_result = db
        .query("SELECT type::string(id) AS id FROM local_user WHERE email = $email LIMIT 1")
        .bind(("email", &email))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("local_user lookup failed: {}", e)))?;
    let existing: Vec<Value> = select_result
        .take(0)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("local_user parse failed: {}", e)))?;

    let user_id = if let Some(row) = existing.into_iter().next() {
        let raw_id = row
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .replace("local_user:", "");
        raw_id
            .trim_matches(|c: char| c == '`' || c == '⟨' || c == '⟩' || c == '"' || c == ' ')
            .to_string()
    } else {
        let new_id = Uuid::new_v4().to_string();
        db.query(
            "CREATE local_user SET
                id = type::record('local_user', $user_id),
                email = $email,
                username = $username,
                password_hash = '',
                avatar_url = $avatar,
                provider = 'google',
                created_at = time::now()",
        )
        .bind(("user_id", &new_id))
        .bind(("email", &email))
        .bind(("username", name.unwrap_or_default()))
        .bind(("avatar", picture.unwrap_or_default()))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("local_user create failed: {}", e)))?;
        new_id
    };

    // Step 4: issue JWT
    let claims = Claims {
        sub: user_id,
        exp: (Utc::now() + Duration::seconds(app_state.config.auth.jwt_expiration as i64))
            .timestamp(),
        iat: Utc::now().timestamp(),
        session_id: None,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(app_state.config.auth.jwt_secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("jwt encode failed: {}", e)))?;

    // Step 5: redirect to /sso bridge — it will inject token into localStorage
    let next = state_claims
        .next
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| app_state.config.server.app_url.clone());
    let sso_url = format!(
        "/sso?token={}&next={}",
        urlencoding::encode(&token),
        urlencoding::encode(&next),
    );

    Ok(Redirect::to(&sso_url))
}
