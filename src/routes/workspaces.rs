use crate::models::workspace::CreateTeamRequest;
use crate::services::auth::User;
use crate::{AppState, error::Result};
use axum::{Extension, Router, response::Json, routing::get};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn router() -> Router {
    Router::new()
        .route("/", get(list_workspaces))
        .route("/teams", axum::routing::post(create_team))
}

async fn list_workspaces(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
) -> Result<Json<Value>> {
    let workspaces = app_state.workspace_service.list_workspaces(&user).await?;

    Ok(Json(json!({
        "success": true,
        "data": workspaces,
        "message": "Workspaces retrieved successfully"
    })))
}

async fn create_team(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(request): Json<CreateTeamRequest>,
) -> Result<Json<Value>> {
    let team = app_state
        .workspace_service
        .create_team(&user, request)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": team,
        "message": "Team workspace created successfully"
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_is_constructible() {
        let _router = router();
    }
}
