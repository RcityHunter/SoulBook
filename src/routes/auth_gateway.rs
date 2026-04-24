use std::sync::Arc;

use axum::{
    Router,
    extract::Extension,
    http::HeaderMap,
    response::Json,
    routing::{get, post, put},
};
use reqwest::Method;
use serde_json::{Value, json};

use crate::{
    AppState,
    config::Config,
    error::{AppError, Result},
    services::auth::User,
};

pub fn router() -> Router {
    Router::new()
        .route("/login", post(login))
        .route("/register", post(register))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/profile", put(update_profile))
        .route("/change-password", post(change_password))
}

async fn login(
    Extension(app_state): Extension<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let upstream = proxy_json(
        &app_state.config,
        Method::POST,
        "/api/auth/login",
        Some(body),
        None,
    )
    .await?;
    Ok(Json(wrap_rainbow_auth_login(upstream)?))
}

async fn register(
    Extension(app_state): Extension<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let upstream = proxy_json(
        &app_state.config,
        Method::POST,
        "/api/auth/register",
        Some(body),
        None,
    )
    .await?;

    Ok(Json(json!({
        "success": true,
        "data": upstream,
        "message": "Registration successful"
    })))
}

async fn logout(
    Extension(app_state): Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok().map(str::to_string));

    let _ = proxy_json(
        &app_state.config,
        Method::POST,
        "/api/auth/logout",
        None,
        auth_header,
    )
    .await;

    Ok(Json(json!({
        "success": true,
        "message": "Logged out"
    })))
}

async fn me(user: User) -> Result<Json<Value>> {
    Ok(Json(wrap_authenticated_user(user)))
}

async fn update_profile(
    Extension(app_state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok().map(str::to_string))
        .ok_or_else(|| AppError::Authentication("Missing authorization header".to_string()))?;

    let profile_body = json!({
        "display_name": body.get("username").cloned().unwrap_or(Value::Null)
    });

    let upstream = proxy_json(
        &app_state.config,
        Method::PUT,
        "/api/users/profile",
        Some(profile_body),
        Some(auth_header),
    )
    .await?;

    Ok(Json(json!({
        "success": true,
        "data": upstream,
        "message": "Profile updated"
    })))
}

async fn change_password(
    Extension(_app_state): Extension<Arc<AppState>>,
    Json(_body): Json<Value>,
) -> Result<Json<Value>> {
    Err(AppError::Validation(
        "Password changes are managed by Rainbow-Auth".to_string(),
    ))
}

async fn proxy_json(
    config: &Config,
    method: Method,
    path: &str,
    body: Option<Value>,
    auth_header: Option<String>,
) -> Result<Value> {
    let base =
        config.auth.rainbow_auth_url.as_deref().ok_or_else(|| {
            AppError::Configuration("RAINBOW_AUTH_URL not configured".to_string())
        })?;
    let url = format!("{}{}", base.trim_end_matches('/'), path);

    let client = reqwest::Client::new();
    let mut request = client.request(method, &url);
    if let Some(auth) = auth_header {
        request = request.header("Authorization", auth);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request
        .send()
        .await
        .map_err(|e| AppError::External(format!("Rainbow-Auth request failed: {}", e)))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| AppError::External(format!("Rainbow-Auth response parse failed: {}", e)))?;

    if status.is_success() {
        return Ok(payload);
    }

    let message = extract_error_message(&payload);
    Err(match status.as_u16() {
        400 => AppError::Validation(message),
        401 => AppError::Authentication(message),
        403 => AppError::Authorization(message),
        404 => AppError::NotFound(message),
        409 => AppError::Conflict(message),
        _ => AppError::External(message),
    })
}

fn extract_error_message(payload: &Value) -> String {
    payload
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| payload.get("error").and_then(Value::as_str))
        .unwrap_or("Rainbow-Auth request failed")
        .to_string()
}

pub fn wrap_rainbow_auth_login(upstream: Value) -> Result<Value> {
    let token = upstream.get("token").cloned().ok_or_else(|| {
        AppError::External("Rainbow-Auth login response missing token".to_string())
    })?;
    let user = upstream.get("user").cloned().ok_or_else(|| {
        AppError::External("Rainbow-Auth login response missing user".to_string())
    })?;

    Ok(json!({
        "success": true,
        "data": {
            "token": token,
            "user": {
                "id": user.get("id").cloned().unwrap_or(Value::Null),
                "email": user.get("email").cloned().unwrap_or(Value::Null),
                "username": user.get("username").cloned().unwrap_or(Value::Null),
                "avatar_url": user.get("avatar_url").cloned().unwrap_or(Value::Null)
            }
        },
        "message": "Login successful"
    }))
}

pub fn wrap_rainbow_auth_me(upstream: Value) -> Result<Value> {
    Ok(json!({
        "success": true,
        "data": {
            "id": upstream.get("id").cloned().unwrap_or(Value::Null),
            "email": upstream.get("email").cloned().unwrap_or(Value::Null),
            "username": upstream.get("username").cloned().unwrap_or(Value::Null),
            "avatar_url": upstream.get("avatar_url").cloned().unwrap_or(Value::Null)
        },
        "message": "User retrieved"
    }))
}

fn wrap_authenticated_user(user: User) -> Value {
    let username = user
        .profile
        .as_ref()
        .and_then(|profile| profile.display_name.clone());
    let avatar_url = user.profile.and_then(|profile| profile.avatar_url);

    json!({
        "success": true,
        "data": {
            "id": user.id,
            "email": user.email,
            "username": username,
            "avatar_url": avatar_url
        },
        "message": "User retrieved"
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{wrap_rainbow_auth_login, wrap_rainbow_auth_me};

    #[test]
    fn wraps_rainbow_auth_login_into_soulbook_shape() {
        let upstream = json!({
            "token": "jwt-123",
            "user": {
                "id": "user-1",
                "email": "user@example.com",
                "username": "tester"
            }
        });

        let wrapped = wrap_rainbow_auth_login(upstream).expect("login response should wrap");

        assert_eq!(
            wrapped,
            json!({
                "success": true,
                "data": {
                    "token": "jwt-123",
                    "user": {
                        "id": "user-1",
                        "email": "user@example.com",
                        "username": "tester",
                        "avatar_url": null
                    }
                },
                "message": "Login successful"
            })
        );
    }

    #[test]
    fn wraps_rainbow_auth_me_into_soulbook_shape() {
        let upstream = json!({
            "id": "user-1",
            "email": "user@example.com",
            "username": "tester"
        });

        let wrapped = wrap_rainbow_auth_me(upstream).expect("me response should wrap");

        assert_eq!(
            wrapped,
            json!({
                "success": true,
                "data": {
                    "id": "user-1",
                    "email": "user@example.com",
                    "username": "tester",
                    "avatar_url": null
                },
                "message": "User retrieved"
            })
        );
    }
}
