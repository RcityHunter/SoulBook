use axum::{
    extract::{Extension, Query},
    http::{
        header::{COOKIE, SET_COOKIE},
        HeaderMap,
    },
    response::{IntoResponse, Redirect, Response},
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

const STATE_COOKIE_NAME: &str = "soulbook_soulauth_oidc_csrf";
const STATE_COOKIE_PATH: &str = "/api/docs/auth/soulauth";

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
    csrf: String,
    exp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoginCookie {
    csrf: String,
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
) -> Result<Response> {
    let oauth = app_state
        .config
        .oauth
        .soulauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("SoulAuth OIDC login is not configured".into()))?;

    let nonce = Uuid::new_v4().to_string();
    let csrf = Uuid::new_v4().to_string();
    let code_verifier = generate_code_verifier();
    let code_challenge = pkce_challenge(&code_verifier);
    let state_claims = OidcState {
        nonce: nonce.clone(),
        next: Some(sanitize_next(params.next, &app_state.config.server.app_url)),
        csrf: csrf.clone(),
        exp: (Utc::now() + Duration::minutes(10)).timestamp(),
    };
    let login_cookie = LoginCookie {
        csrf,
        code_verifier,
        exp: (Utc::now() + Duration::minutes(10)).timestamp(),
    };
    let state = encode_state(&state_claims, &app_state.config.auth.jwt_secret)?;
    let cookie_token = encode_login_cookie(&login_cookie, &app_state.config.auth.jwt_secret)?;
    let url = build_authorize_url(oauth, &state, &nonce, &code_challenge);
    let mut response = Redirect::to(&url).into_response();
    response.headers_mut().append(
        SET_COOKIE,
        state_cookie(&cookie_token)
            .parse()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("state cookie failed: {}", e)))?,
    );

    Ok(response)
}

async fn callback(
    Extension(app_state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Result<Response> {
    if params.error.is_some() {
        let login_url = format!("/docs/login?error={}", safe_login_error());
        if let Some(state_token) = params.state.as_deref() {
            validate_error_callback_state(
                Some(state_token),
                &headers,
                &app_state.config.auth.jwt_secret,
            )?;
            return redirect_with_clear_cookie(&login_url);
        }
        return Ok(Redirect::to(&login_url).into_response());
    }

    let code = params
        .code
        .ok_or_else(|| AppError::BadRequest("missing code".into()))?;
    let state_token = params
        .state
        .ok_or_else(|| AppError::BadRequest("missing state".into()))?;
    let state_claims = decode_state(&state_token, &app_state.config.auth.jwt_secret)?;
    let login_cookie = login_cookie_from_headers(&headers, &app_state.config.auth.jwt_secret)?;
    let login_cookie = validate_login_cookie(login_cookie, &state_claims)?;

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
            ("code_verifier", login_cookie.code_verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("SoulAuth token request failed: {}", e)))?;

    if !token_resp.status().is_success() {
        return Err(AppError::BadRequest("SoulAuth login failed".into()));
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
        return Err(AppError::BadRequest("SoulAuth login failed".into()));
    }

    let userinfo: UserInfo = userinfo_resp.json().await.map_err(|e| {
        AppError::Internal(anyhow::anyhow!("SoulAuth userinfo parse failed: {}", e))
    })?;

    let user_id = find_or_create_local_user(&app_state, userinfo).await?;
    let token = issue_soulbook_token(&app_state, &user_id)?;
    let next = crate::sanitize_sso_next(state_claims.next, &app_state.config.server.app_url);
    let bridge_binding = crate::create_sso_bridge_binding();
    let bridge = crate::create_sso_bridge_token(
        &token,
        Some(next.clone()),
        &app_state.config.server.app_url,
        &app_state.config.auth.jwt_secret,
        &bridge_binding,
    )?;
    let sso_url = format!(
        "/sso?bridge={}&next={}",
        urlencoding::encode(&bridge),
        urlencoding::encode(&next),
    );

    let mut response = redirect_with_clear_cookie(&sso_url)?;
    response.headers_mut().append(
        SET_COOKIE,
        crate::sso_bridge_binding_cookie(&bridge_binding)
            .parse()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("bridge cookie failed: {}", e)))?,
    );
    Ok(response)
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
             WHERE provider = 'soulauth' AND external_subject = $sub",
        )
        .bind(("sub", &sub))
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("local_user subject lookup failed: {}", e))
        })?;
    let users: Vec<Value> = by_subject.take(0).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("local_user subject parse failed: {}", e))
    })?;
    if let Some(user_id) = single_subject_user_id(users)? {
        return Ok(user_id);
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

fn encode_login_cookie(cookie: &LoginCookie, secret: &str) -> Result<String> {
    encode(
        &Header::default(),
        cookie,
        &EncodingKey::from_secret(secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("login cookie encode failed: {}", e)))
}

fn decode_login_cookie(token: &str, secret: &str) -> Result<LoginCookie> {
    let validation = Validation::new(Algorithm::HS256);
    decode::<LoginCookie>(
        token,
        &DecodingKey::from_secret(secret.as_ref()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::BadRequest("invalid or expired state".into()))
}

fn sanitize_next(next: Option<String>, default: &str) -> String {
    crate::sanitize_sso_next(next, default)
}

fn validate_login_cookie(cookie: Option<LoginCookie>, state: &OidcState) -> Result<LoginCookie> {
    match cookie {
        Some(cookie) if cookie.csrf == state.csrf => Ok(cookie),
        _ => Err(AppError::BadRequest("invalid or expired state".into())),
    }
}

fn validate_error_callback_state(
    state_token: Option<&str>,
    headers: &HeaderMap,
    secret: &str,
) -> Result<()> {
    if let Some(state_token) = state_token {
        let state = decode_state(state_token, secret)?;
        let cookie = login_cookie_from_headers(headers, secret)?;
        validate_login_cookie(cookie, &state)?;
    }

    Ok(())
}

fn safe_login_error() -> &'static str {
    "login_failed"
}

fn state_cookie(cookie_token: &str) -> String {
    format!(
        "{}={}; Max-Age=600; Path={}; HttpOnly; Secure; SameSite=Lax",
        STATE_COOKIE_NAME, cookie_token, STATE_COOKIE_PATH
    )
}

fn clear_state_cookie() -> String {
    format!(
        "{}=; Max-Age=0; Path={}; HttpOnly; Secure; SameSite=Lax",
        STATE_COOKIE_NAME, STATE_COOKIE_PATH
    )
}

fn redirect_with_clear_cookie(url: &str) -> Result<Response> {
    let mut response = Redirect::to(url).into_response();
    response.headers_mut().append(
        SET_COOKIE,
        clear_state_cookie()
            .parse()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("clear state cookie failed: {}", e)))?,
    );
    Ok(response)
}

fn login_cookie_from_headers(headers: &HeaderMap, secret: &str) -> Result<Option<LoginCookie>> {
    let Some(cookie_header) = headers.get(COOKIE) else {
        return Ok(None);
    };
    let Ok(cookie_header) = cookie_header.to_str() else {
        return Ok(None);
    };
    let cookie_token = cookie_header.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == STATE_COOKIE_NAME).then(|| value.to_string())
    });

    cookie_token
        .map(|token| decode_login_cookie(&token, secret))
        .transpose()
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

fn single_subject_user_id(users: Vec<Value>) -> Result<Option<String>> {
    match users.len() {
        0 => Ok(None),
        1 => Ok(users.first().map(local_user_id_from_row)),
        _ => Err(AppError::Conflict(
            "Multiple local users are linked to the same SoulAuth subject".into(),
        )),
    }
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
    fn state_round_trip_preserves_only_non_secret_fields() {
        let state = OidcState {
            nonce: "nonce-value".to_string(),
            next: Some("/docs".to_string()),
            csrf: "csrf-value".to_string(),
            exp: (Utc::now() + Duration::minutes(10)).timestamp(),
        };

        let token = encode_state(&state, "test-secret").expect("state should encode");
        let decoded = decode_state(&token, "test-secret").expect("state should decode");
        let decoded_json = serde_json::to_value(&decoded).expect("state should serialize");

        assert_eq!(decoded.nonce, "nonce-value");
        assert_eq!(decoded.next.as_deref(), Some("/docs"));
        assert_eq!(decoded.csrf, "csrf-value");
        assert!(decoded_json.get("code_verifier").is_none());
    }

    #[test]
    fn signed_login_cookie_preserves_verifier_and_csrf() {
        let cookie = LoginCookie {
            csrf: "csrf-value".to_string(),
            code_verifier: "verifier-value".to_string(),
            exp: (Utc::now() + Duration::minutes(10)).timestamp(),
        };

        let token = encode_login_cookie(&cookie, "test-secret").expect("cookie should encode");
        let decoded = decode_login_cookie(&token, "test-secret").expect("cookie should decode");

        assert_eq!(decoded.csrf, "csrf-value");
        assert_eq!(decoded.code_verifier, "verifier-value");
    }

    #[test]
    fn safe_relative_next_is_accepted() {
        assert_eq!(
            sanitize_next(Some("/docs/login?tab=oidc#top".to_string()), "/"),
            "/docs/login?tab=oidc#top"
        );
        assert_eq!(sanitize_next(Some("/search?q=a".to_string()), "/"), "/search?q=a");
    }

    #[test]
    fn unsafe_next_values_fall_back_to_default() {
        for next in [
            "javascript:alert(1)",
            "http://evil.test",
            "https://evil.test",
            "//evil.test/path",
            "/docs\n<script>",
            "",
            "   ",
        ] {
            assert_eq!(sanitize_next(Some(next.to_string()), "/"), "/");
        }
    }

    #[test]
    fn nested_sso_next_values_fall_back_to_default() {
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
            assert_eq!(sanitize_next(Some(next.to_string()), "/docs"), "/docs");
        }
    }

    #[test]
    fn callback_state_cookie_must_match_state_csrf() {
        let state = OidcState {
            nonce: "nonce-value".to_string(),
            next: None,
            csrf: "csrf-value".to_string(),
            exp: (Utc::now() + Duration::minutes(10)).timestamp(),
        };
        let cookie = LoginCookie {
            csrf: "csrf-value".to_string(),
            code_verifier: "verifier-value".to_string(),
            exp: (Utc::now() + Duration::minutes(10)).timestamp(),
        };

        assert!(validate_login_cookie(Some(cookie), &state).is_ok());
        assert!(validate_login_cookie(None, &state).is_err());
        assert!(validate_login_cookie(
            Some(LoginCookie {
                csrf: "other-value".to_string(),
                code_verifier: "verifier-value".to_string(),
                exp: (Utc::now() + Duration::minutes(10)).timestamp(),
            }),
            &state
        )
        .is_err());
    }

    #[test]
    fn error_callback_without_valid_state_does_not_clear_cookie() {
        let headers = HeaderMap::new();

        assert!(validate_error_callback_state(None, &headers, "test-secret").is_ok());
        assert!(
            validate_error_callback_state(Some("invalid-state"), &headers, "test-secret").is_err()
        );
        assert_eq!(safe_login_error(), "login_failed");
    }

    #[test]
    fn duplicate_external_subject_rows_fail_instead_of_selecting_first() {
        let rows = vec![
            serde_json::json!({ "id": "local_user:one" }),
            serde_json::json!({ "id": "local_user:two" }),
        ];

        assert!(single_subject_user_id(rows).is_err());
    }
}
