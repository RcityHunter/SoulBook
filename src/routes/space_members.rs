use crate::models::space_member::{
    AcceptInvitationRequest, InviteMemberRequest, UpdateMemberRequest,
};
use crate::services::auth::User;
use crate::{
    error::{AppError, Result},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Extension, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

pub fn router() -> Router {
    Router::new()
        .route("/:space_slug/members", get(list_members))
        .route("/:space_slug/invite", post(invite_member))
        .route(
            "/:space_slug/members/:user_id",
            put(update_member).delete(remove_member),
        )
        .route("/invitations/accept", post(accept_invitation))
}

/// 获取空间成员列表
/// GET /api/docs/spaces/:space_slug/members
async fn list_members(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    user: User,
) -> Result<Json<Value>> {
    // 获取space_id
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    let members = app_state
        .space_member_service
        .list_space_members(&space.id, &user)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": members,
        "message": "Members retrieved successfully"
    })))
}

/// 邀请新成员
/// POST /api/docs/spaces/:space_slug/invite  
async fn invite_member(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_slug): Path<String>,
    user: User,
    Json(request): Json<InviteMemberRequest>,
) -> Result<Json<Value>> {
    // 获取space_id
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    let invitation = app_state
        .space_member_service
        .invite_member(&space.id, &user, request)
        .await?;

    info!(
        "User {} invited new member to space: {}",
        user.id, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": invitation,
        "message": "Invitation sent successfully"
    })))
}

/// 接受邀请
/// POST /api/docs/spaces/invitations/accept
async fn accept_invitation(
    Extension(app_state): Extension<Arc<AppState>>,
    user: User,
    Json(request): Json<AcceptInvitationRequest>,
) -> Result<Json<Value>> {
    let member = app_state
        .space_member_service
        .accept_invitation(&user.id, request)
        .await?;

    info!("User {} accepted invitation to space", user.id);

    Ok(Json(json!({
        "success": true,
        "data": member,
        "message": "Invitation accepted successfully"
    })))
}

/// 更新成员权限
/// PUT /api/docs/spaces/:space_slug/members/:user_id
async fn update_member(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, member_user_id)): Path<(String, String)>,
    user: User,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<Value>> {
    // 获取space_id
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    let updated_member = app_state
        .space_member_service
        .update_member(&space.id, &member_user_id, &user, request)
        .await?;

    info!(
        "User {} updated member {} in space: {}",
        user.id, member_user_id, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": updated_member,
        "message": "Member updated successfully"
    })))
}

/// 移除成员
/// DELETE /api/docs/spaces/:space_slug/members/:user_id
async fn remove_member(
    Extension(app_state): Extension<Arc<AppState>>,
    Path((space_slug, member_user_id)): Path<(String, String)>,
    user: User,
) -> Result<Json<Value>> {
    // 获取space_id
    let space = app_state
        .space_service
        .get_space_by_slug(&space_slug, Some(&user))
        .await?;

    app_state
        .space_member_service
        .remove_member(&space.id, &member_user_id, &user)
        .await?;

    info!(
        "User {} removed member {} from space: {}",
        user.id, member_user_id, space_slug
    );

    Ok(Json(json!({
        "success": true,
        "data": null,
        "message": "Member removed successfully"
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::space_member::{InviteMemberRequest, MemberRole};
    use validator::Validate;

    #[test]
    fn test_invite_member_validation() {
        let valid_request = InviteMemberRequest {
            email: Some("test@example.com".to_string()),
            user_id: None,
            role: MemberRole::Member,
            message: Some("Welcome to our space!".to_string()),
            expires_in_days: Some(7),
        };

        assert!(valid_request.validate().is_ok());

        let invalid_request = InviteMemberRequest {
            email: Some("invalid-email".to_string()), // 无效邮箱格式
            user_id: None,
            role: MemberRole::Member,
            message: None,
            expires_in_days: Some(7),
        };

        assert!(invalid_request.validate().is_err());
    }
}
