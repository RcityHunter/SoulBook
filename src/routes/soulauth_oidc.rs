use axum::{
    extract::{Extension, Query},
    response::Redirect,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    config::SoulAuthOidcConfig,
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
struct OidcState {
    nonce: String,
    next: Option<String>,
    code_verifier: String,
    exp: i64,
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    sub: Option<String>,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    username: Option<String>,
    picture: Option<String>,
    avatar_url: Option<String>,
}

async fn start(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(params): Query<StartParams>,
) -> Result<Redirect> {
    let oauth = app_state
        .config
        .oauth
        .soulauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("SoulAuth OIDC login is not configured".into()))?;

    let nonce = Uuid::new_v4().to_string();
    let code_verifier = generate_code_verifier();
    let code_challenge = pkce_challenge(&code_verifier);
    let state_claims = OidcState {
        nonce: nonce.clone(),
        next: params.next,
        code_verifier,
        exp: (Utc::now() + Duration::minutes(10)).timestamp(),
    };
    let state = encode_state(&state_claims, &app_state.config.auth.jwt_secret)?;
    let url = build_authorize_url(oauth, &state, &nonce, &code_challenge);

    Ok(Redirect::to(&url))
}

async fn callback(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> Result<Redirect> {
    if let Some(err) = params.error {
        let login_url = format!("/docs/login?error={}", urlencoding::encode(&err));
        return Ok(Redirect::to(&login_url));
    }

    let code = params
        .code
        .ok_or_else(|| AppError::BadRequest("missing code".into()))?;
    let state_token = params
        .state
        .ok_or_else(|| AppError::BadRequest("missing state".into()))?;
    let state_claims = decode_state(&state_token, &app_state.config.auth.jwt_secret)?;

    let oauth = app_state
        .config
        .oauth
        .soulauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("SoulAuth OIDC login is not configured".into()))?;

    let http = reqwest::Client::new();
    let token_url = oidc_endpoint(&oauth.issuer, "/api/oidc/token");
    let token_resp = http
        .post(token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", oauth.redirect_uri.as_str()),
            ("client_id", oauth.client_id.as_str()),
            ("client_secret", oauth.client_secret.as_str()),
            ("code_verifier", state_claims.code_verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("SoulAuth token request failed: {}", e)))?;

    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(AppError::BadRequest(format!(
            "SoulAuth token exchange failed: {}",
            body
        )));
    }

    let token_json: TokenResponse = token_resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("SoulAuth token parse failed: {}", e)))?;
    let access_token = token_json
        .access_token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!("SoulAuth did not return access_token"))
        })?;

    let userinfo_url = oidc_endpoint(&oauth.issuer, "/api/oidc/userinfo");
    let userinfo_resp = http
        .get(userinfo_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("SoulAuth userinfo request failed: {}", e))
        })?;

    if !userinfo_resp.status().is_success() {
        let body = userinfo_resp.text().await.unwrap_or_default();
        return Err(AppError::BadRequest(format!(
            "SoulAuth userinfo failed: {}",
            body
        )));
    }

    let userinfo: UserInfo = userinfo_resp.json().await.map_err(|e| {
        AppError::Internal(anyhow::anyhow!("SoulAuth userinfo parse failed: {}", e))
    })?;

    let user_id = find_or_create_local_user(&app_state, userinfo).await?;
    let token = issue_soulbook_token(&app_state, &user_id)?;
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

async fn find_or_create_local_user(app_state: &AppState, userinfo: UserInfo) -> Result<String> {
    let sub = userinfo.sub.unwrap_or_default();
    if sub.trim().is_empty() {
        return Err(AppError::BadRequest(
            "SoulAuth account has no subject".into(),
        ));
    }
    if userinfo.email_verified != Some(true) {
        return Err(AppError::BadRequest(
            "SoulAuth account email is not verified".into(),
        ));
    }

    let email = userinfo.email.unwrap_or_default().trim().to_lowercase();
    if email.is_empty() {
        return Err(AppError::BadRequest("SoulAuth account has no email".into()));
    }

    let db = &app_state.db.client;
    let mut by_subject = db
        .query(
            "SELECT type::string(id) AS id FROM local_user
             WHERE provider = 'soulauth' AND external_subject = $sub LIMIT 1",
        )
        .bind(("sub", &sub))
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("local_user subject lookup failed: {}", e))
        })?;
    let users: Vec<Value> = by_subject.take(0).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("local_user subject parse failed: {}", e))
    })?;
    if let Some(row) = users.into_iter().next() {
        return Ok(local_user_id_from_row(&row));
    }

    let mut by_email = db
        .query(
            "SELECT type::string(id) AS id, external_subject FROM local_user
             WHERE email = $email LIMIT 1",
        )
        .bind(("email", &email))
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("local_user email lookup failed: {}", e))
        })?;
    let existing_email_users: Vec<Value> = by_email
        .take(0)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("local_user email parse failed: {}", e)))?;

    if let Some(row) = existing_email_users.into_iter().next() {
        if has_external_subject(&row) {
            return Err(AppError::Conflict(
                "A local user with this verified email is already linked to another external identity"
                    .into(),
            ));
        }

        let user_id = local_user_id_from_row(&row);
        db.query(
            "UPDATE local_user SET
                provider = 'soulauth',
                external_subject = $sub,
                updated_at = time::now()
             WHERE id = type::record('local_user', $user_id)",
        )
        .bind(("sub", &sub))
        .bind(("user_id", &user_id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("local_user link failed: {}", e)))?;
        return Ok(user_id);
    }

    let user_id = Uuid::new_v4().to_string();
    let username = userinfo
        .name
        .or(userinfo.username)
        .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());
    let avatar = userinfo.picture.or(userinfo.avatar_url).unwrap_or_default();

    db.query(
        "CREATE local_user SET
            id = type::record('local_user', $user_id),
            email = $email,
            username = $username,
            password_hash = '',
            avatar_url = $avatar,
            provider = 'soulauth',
            external_subject = $sub,
            created_at = time::now(),
            updated_at = time::now()",
    )
    .bind(("user_id", &user_id))
    .bind(("email", &email))
    .bind(("username", username))
    .bind(("avatar", avatar))
    .bind(("sub", &sub))
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("local_user create failed: {}", e)))?;

    Ok(user_id)
}

fn issue_soulbook_token(app_state: &AppState, user_id: &str) -> Result<String> {
    let claims = Claims {
        sub: user_id.to_string(),
        exp: (Utc::now() + Duration::seconds(app_state.config.auth.jwt_expiration as i64))
            .timestamp(),
        iat: Utc::now().timestamp(),
        session_id: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(app_state.config.auth.jwt_secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("jwt encode failed: {}", e)))
}

fn encode_state(state: &OidcState, secret: &str) -> Result<String> {
    encode(
        &Header::default(),
        state,
        &EncodingKey::from_secret(secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("state encode failed: {}", e)))
}

fn decode_state(token: &str, secret: &str) -> Result<OidcState> {
    let validation = Validation::new(Algorithm::HS256);
    decode::<OidcState>(
        token,
        &DecodingKey::from_secret(secret.as_ref()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::BadRequest("invalid or expired state".into()))
}

fn build_authorize_url(
    config: &SoulAuthOidcConfig,
    state: &str,
    nonce: &str,
    code_challenge: &str,
) -> String {
    let params = serde_urlencoded::to_string(&[
        ("response_type", "code"),
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("scope", "openid profile email"),
        ("state", state),
        ("nonce", nonce),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
    ])
    .expect("static OIDC authorize parameters must encode");

    format!(
        "{}?{}",
        oidc_endpoint(&config.issuer, "/api/oidc/authorize"),
        params
    )
}

fn generate_code_verifier() -> String {
    format!("{}-{}", Uuid::new_v4(), Uuid::new_v4())
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn oidc_endpoint(issuer: &str, path: &str) -> String {
    format!("{}{}", issuer.trim_end_matches('/'), path)
}

fn local_user_id_from_row(row: &Value) -> String {
    let raw_id = row
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .replace("local_user:", "");
    raw_id
        .trim_matches(|c: char| c == '`' || c == '⟨' || c == '⟩' || c == '"' || c == ' ')
        .to_string()
}

fn has_external_subject(row: &Value) -> bool {
    row.get("external_subject")
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SoulAuthOidcConfig {
        SoulAuthOidcConfig {
            issuer: "https://auth.example.test".to_string(),
            client_id: "soulbook".to_string(),
            client_secret: "secret".to_string(),
            redirect_uri: "https://book.example.test/api/docs/auth/soulauth/callback".to_string(),
            post_logout_redirect_uri: None,
        }
    }

    #[test]
    fn authorize_url_contains_required_oidc_parameters() {
        let url = build_authorize_url(&test_config(), "state-token", "nonce-value", "pkce-hash");

        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=openid"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state="));
    }

    #[test]
    fn state_round_trip_preserves_nonce_next_and_code_verifier() {
        let state = OidcState {
            nonce: "nonce-value".to_string(),
            next: Some("/docs".to_string()),
            code_verifier: "verifier-value".to_string(),
            exp: (Utc::now() + Duration::minutes(10)).timestamp(),
        };

        let token = encode_state(&state, "test-secret").expect("state should encode");
        let decoded = decode_state(&token, "test-secret").expect("state should decode");

        assert_eq!(decoded.nonce, "nonce-value");
        assert_eq!(decoded.next.as_deref(), Some("/docs"));
        assert_eq!(decoded.code_verifier, "verifier-value");
    }
}
