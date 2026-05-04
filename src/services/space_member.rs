use crate::config::Config;
use crate::error::{AppError, Result};
use crate::models::space_member::{
    AcceptInvitationRequest, InviteMemberRequest, MemberRole, MemberStatus, SpaceInvitation,
    SpaceInvitationDb, SpaceMember, SpaceMemberDb, SpaceMemberResponse, UpdateMemberRequest,
};
use crate::services::auth::User;
use crate::services::database::{record_id_key, Database};
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;
use surrealdb::types::RecordId as Thing;
use tracing::{error, info, warn};
use uuid::Uuid;
use validator::Validate;

/// 清理用户ID格式，确保和数据库存储格式一致
fn clean_user_id_format(user_id: &str) -> String {
    // 统一成裸 UUID：移除前缀、包裹符、引号与反引号
    let trimmed = user_id.trim();
    let without_prefix = trimmed
        .strip_prefix("user:")
        .or_else(|| trimmed.strip_prefix("users:"))
        .unwrap_or(trimmed)
        .trim();

    without_prefix
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string()
}

fn normalize_space_id(space_id: &str) -> String {
    let trimmed = space_id.trim();

    if let Some(inner) = trimmed.strip_prefix("space:⟨⟨space:") {
        return inner
            .trim_end_matches("⟩⟩")
            .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
            .to_string();
    }

    trimmed
        .strip_prefix("space:")
        .unwrap_or(trimmed)
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string()
}

fn space_id_match_candidates(space_id: &str) -> Vec<String> {
    let actual_space_id = normalize_space_id(space_id);
    vec![
        format!("space:{}", actual_space_id),
        format!("space:⟨{}⟩", actual_space_id),
        format!("space:⟨space:{}⟩", actual_space_id),
        format!("space:⟨⟨space:{}⟩⟩", actual_space_id),
    ]
}

fn space_owner_where_clause() -> &'static str {
    "type::string(id) IN [$space_id_plain, $space_id_bracketed, $space_id_prefixed, $space_id_nested]
              AND (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) IN [$user_id_bracketed, $user_id_plain, $user_id_raw]"
}

fn invitation_optional_assignments(request: &InviteMemberRequest) -> String {
    let mut assignments = String::new();

    if request.email.is_some() {
        assignments.push_str(",\n                email = $email");
    }
    if request.user_id.is_some() {
        assignments.push_str(",\n                user_id = $user_id");
    }
    if request.message.is_some() {
        assignments.push_str(",\n                message = $message");
    }

    assignments
}

fn local_user_id_from_lookup_row(row: &Value) -> Option<String> {
    row.get("id")
        .and_then(Value::as_str)
        .map(|id| {
            id.replace("local_user:", "")
                .trim_matches(|c: char| {
                    c == '`' || c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == ' '
                })
                .to_string()
        })
        .filter(|id| !id.is_empty())
}

fn inviter_display_name_from_local_user(row: &Value) -> Option<String> {
    row.get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            row.get("email")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "unknown@example.com")
                .map(ToString::to_string)
        })
}

fn space_name_lookup_query() -> &'static str {
    "SELECT name FROM space
     WHERE type::string(id) = $space_id_prefixed
        OR string::replace(type::string(id), 'space:', '') = $space_id
     LIMIT 1"
}

fn invitation_lookup_query() -> &'static str {
    "SELECT * FROM space_invitation
     WHERE invite_token = $invite_token
       AND expires_at > time::now()
     LIMIT 1"
}

fn create_accepted_member_query() -> &'static str {
    "CREATE space_member SET
        space_id = type::record($space_id),
        user_id = $user_id,
        role = $role,
        permissions = $permissions,
        invited_by = $invited_by,
        invited_at = time::now(),
        accepted_at = time::now(),
        status = 'accepted',
        expires_at = NONE,
        created_at = time::now(),
        updated_at = time::now()"
}

fn update_invitation_usage_query() -> &'static str {
    "UPDATE type::record($invitation_id) SET
        used_count = used_count + 1,
        updated_at = time::now()"
}

pub struct SpaceMemberService {
    db: Arc<Database>,
    config: Config,
}

impl SpaceMemberService {
    pub fn new(db: Arc<Database>, config: Config) -> Self {
        Self { db, config }
    }

    /// 检查用户是否为空间成员或所有者
    pub async fn can_access_space(&self, space_id: &str, user_id: Option<&str>) -> Result<bool> {
        let Some(uid) = user_id else {
            return Ok(false);
        };

        // 清理user_id格式，确保和数据库存储格式一致
        let clean_user_id = clean_user_id_format(uid);
        info!(
            "Checking space access for clean_user_id: {} (original: {})",
            clean_user_id, uid
        );

        let space_id_candidates = space_id_match_candidates(space_id);
        let space_id_plain = space_id_candidates[0].clone();
        let space_id_bracketed = space_id_candidates[1].clone();
        let space_id_prefixed = space_id_candidates[2].clone();
        let space_id_nested = space_id_candidates[3].clone();

        // 检查是否为空间所有者（数据库内比较，避免反序列化形态差异）
        let user_id_bracketed = format!("user:⟨{}⟩", clean_user_id);
        let user_id_plain = format!("user:{}", clean_user_id);
        let owner_query = format!(
            r#"
            SELECT count() AS count
            FROM space
            WHERE {}
            GROUP ALL
        "#,
            space_owner_where_clause()
        );
        let owner_count: Vec<serde_json::Value> = self
            .db
            .client
            .query(owner_query)
            .bind(("space_id_plain", space_id_plain.clone()))
            .bind(("space_id_bracketed", space_id_bracketed.clone()))
            .bind(("space_id_prefixed", space_id_prefixed.clone()))
            .bind(("space_id_nested", space_id_nested.clone()))
            .bind(("user_id_bracketed", user_id_bracketed.clone()))
            .bind(("user_id_plain", user_id_plain.clone()))
            .bind(("user_id_raw", clean_user_id.clone()))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let is_owner = owner_count
            .first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0;
        if is_owner {
            info!("User is space owner, granting access");
            return Ok(true);
        }

        // 检查是否为空间成员 - 只需要检查存在性，避免返回 Thing 导致反序列化错误
        let member_query = r#"
            SELECT count() AS count
            FROM space_member
            WHERE type::string(space_id) IN [$space_id_plain, $space_id_bracketed, $space_id_prefixed, $space_id_nested]
              AND type::string(user_id) IN [$user_id_bracketed, $user_id_plain, $user_id_raw]
              AND status = 'accepted'
            GROUP ALL
        "#;
        let member_result: Vec<serde_json::Value> = self
            .db
            .client
            .query(member_query)
            .bind(("space_id_plain", space_id_plain))
            .bind(("space_id_bracketed", space_id_bracketed))
            .bind(("space_id_prefixed", space_id_prefixed))
            .bind(("space_id_nested", space_id_nested))
            .bind(("user_id_bracketed", user_id_bracketed))
            .bind(("user_id_plain", user_id_plain))
            .bind(("user_id_raw", clean_user_id.clone()))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let has_access = member_result
            .first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0;
        if has_access {
            info!("User found as space member, granting access");
        } else {
            info!("User not found as space member or owner");
        }
        Ok(has_access)
    }

    /// 检查用户在空间中的权限
    pub async fn check_permission(
        &self,
        space_id: &str,
        user_id: &str,
        permission: &str,
    ) -> Result<bool> {
        let actual_space_id = normalize_space_id(space_id);
        let space_id_candidates = space_id_match_candidates(space_id);
        let space_id_plain = space_id_candidates[0].clone();
        let space_id_bracketed = space_id_candidates[1].clone();
        let space_id_prefixed = space_id_candidates[2].clone();
        let space_id_nested = space_id_candidates[3].clone();

        // 清理user_id格式，确保和数据库存储格式一致
        let clean_user_id = clean_user_id_format(user_id);
        info!(
            "Checking permission '{}' for clean_user_id: {} (original: {}) in space: {}",
            permission, clean_user_id, user_id, actual_space_id
        );

        let user_id_bracketed = format!("user:⟨{}⟩", clean_user_id);
        let user_id_plain = format!("user:{}", clean_user_id);

        // 先检查是否为空间所有者（数据库内比较）
        let owner_query = format!(
            r#"
            SELECT count() AS count
            FROM space
            WHERE {}
            GROUP ALL
        "#,
            space_owner_where_clause()
        );
        let owner_count: Vec<serde_json::Value> = self
            .db
            .client
            .query(owner_query)
            .bind(("space_id_plain", space_id_plain.clone()))
            .bind(("space_id_bracketed", space_id_bracketed.clone()))
            .bind(("space_id_prefixed", space_id_prefixed.clone()))
            .bind(("space_id_nested", space_id_nested.clone()))
            .bind(("user_id_bracketed", user_id_bracketed.clone()))
            .bind(("user_id_plain", user_id_plain.clone()))
            .bind(("user_id_raw", clean_user_id.clone()))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let is_owner = owner_count
            .first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0;
        if is_owner {
            info!("User is space owner, granting permission");
            return Ok(true); // 所有者拥有所有权限
        }

        // 检查成员权限
        let member_query = r#"
            SELECT role, permissions
            FROM space_member
            WHERE type::string(space_id) IN [$space_id_plain, $space_id_bracketed, $space_id_prefixed, $space_id_nested]
              AND type::string(user_id) IN [$user_id_bracketed, $user_id_plain, $user_id_raw]
              AND status = 'accepted'
        "#;
        let members: Vec<serde_json::Value> = self
            .db
            .client
            .query(member_query)
            .bind(("space_id_plain", space_id_plain))
            .bind(("space_id_bracketed", space_id_bracketed))
            .bind(("space_id_prefixed", space_id_prefixed))
            .bind(("space_id_nested", space_id_nested))
            .bind(("user_id_bracketed", user_id_bracketed))
            .bind(("user_id_plain", user_id_plain))
            .bind(("user_id_raw", clean_user_id.clone()))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        if let Some(member) = members.first() {
            let role_str = member
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let permissions_array = member.get("permissions").and_then(|v| v.as_array());

            info!(
                "Found space member with role: {}, permissions: {:?}",
                role_str, permissions_array
            );

            // 解析角色
            let member_role = match role_str {
                "owner" => MemberRole::Owner,
                "admin" => MemberRole::Admin,
                "editor" => MemberRole::Editor,
                "viewer" => MemberRole::Viewer,
                "member" => MemberRole::Member,
                _ => MemberRole::Member,
            };

            // 检查角色默认权限
            if member_role.can_perform(permission) {
                info!("Permission granted by role: {:?}", member_role);
                return Ok(true);
            }

            // 检查自定义权限
            if let Some(perms) = permissions_array {
                for perm in perms {
                    if let Some(perm_str) = perm.as_str() {
                        if perm_str == permission {
                            info!("Permission granted by custom permissions");
                            return Ok(true);
                        }
                    }
                }
            }

            info!(
                "Permission denied: role {:?} does not have permission '{}'",
                member_role, permission
            );
        } else {
            info!(
                "No space member record found for user_id: {}",
                clean_user_id
            );
        }

        Ok(false)
    }

    /// 邀请用户加入空间
    pub async fn invite_member(
        &self,
        space_id: &str,
        inviter: &User,
        request: InviteMemberRequest,
    ) -> Result<SpaceInvitation> {
        request
            .validate()
            .map_err(|e| AppError::Validation(e.to_string()))?;

        // 检查邀请权限
        if !self
            .check_permission(space_id, &inviter.id, "members.invite")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: members.invite required".to_string(),
            ));
        }

        // 如果通过user_id邀请，检查用户是否已经是成员
        if let Some(user_id) = &request.user_id {
            if self.can_access_space(space_id, Some(user_id)).await? {
                return Err(AppError::Conflict(
                    "User is already a member of this space".to_string(),
                ));
            }
        }

        // 生成邀请令牌
        let invite_token = Uuid::new_v4().to_string();
        let expires_in_days = request.expires_in_days.unwrap_or(7);

        // 提取纯净的space_id，避免嵌套Thing
        let clean_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        info!(
            "Creating invitation with clean_space_id: {}",
            clean_space_id
        );

        let optional_assignments = invitation_optional_assignments(&request);

        // 使用 SQL 查询创建邀请记录，避免把空可选字段写成 SurrealDB 不接受的 NULL
        let query = format!(
            r#"
            CREATE space_invitation SET
                space_id = type::record($space_id){},
                invite_token = $invite_token,
                role = $role,
                permissions = $permissions,
                invited_by = $invited_by,
                max_uses = $max_uses,
                used_count = $used_count,
                expires_at = time::now() + {}d,
                created_at = time::now(),
                updated_at = time::now()
        "#,
            optional_assignments, expires_in_days
        );

        let mut create_query = self
            .db
            .client
            .query(query)
            .bind(("space_id", format!("space:{}", clean_space_id)))
            .bind(("invite_token", invite_token.clone()))
            .bind(("role", request.role.clone()))
            .bind(("permissions", request.role.default_permissions()))
            .bind(("invited_by", inviter.id.clone()))
            .bind(("max_uses", 1))
            .bind(("used_count", 0));

        if let Some(email) = &request.email {
            create_query = create_query.bind(("email", email.clone()));
        }
        if let Some(user_id) = &request.user_id {
            create_query = create_query.bind(("user_id", user_id.clone()));
        }
        if let Some(message) = &request.message {
            create_query = create_query.bind(("message", message.clone()));
        }

        let created: Vec<SpaceInvitationDb> = create_query
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let created_invitation = created
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Failed to create invitation")))?;

        info!(
            "User {} invited {} to space {}",
            inviter.id,
            request
                .email
                .as_deref()
                .unwrap_or(request.user_id.as_deref().unwrap_or("unknown")),
            space_id
        );

        let inviter_name = self.resolve_inviter_display_name(inviter).await;

        info!(
            "Inviter info - ID: {}, Email: {}, Display name: {}",
            inviter.id, inviter.email, inviter_name
        );

        let notification_user_id = if request.user_id.is_some() {
            request.user_id.clone()
        } else if let Some(email) = request.email.as_deref() {
            match self.find_local_user_id_by_email(email).await {
                Ok(user_id) => user_id,
                Err(e) => {
                    error!(
                        "Failed to resolve invitee email {} to local user: {}",
                        email, e
                    );
                    None
                }
            }
        } else {
            None
        };

        // 发送邮件和通知
        self.send_invitation_notifications(
            request.email.as_deref(),
            notification_user_id.as_deref(),
            space_id,
            &inviter_name,
            &invite_token,
            &request.role.to_string(),
            request.message.as_deref(),
            expires_in_days.into(),
        )
        .await
        .unwrap_or_else(|e| {
            error!("Failed to send invitation notifications: {}", e);
        });

        Ok(created_invitation.into())
    }

    async fn find_local_user_id_by_email(&self, email: &str) -> Result<Option<String>> {
        let normalized_email = email.trim().to_lowercase();
        if normalized_email.is_empty() {
            return Ok(None);
        }

        let mut result = self
            .db
            .client
            .query("SELECT type::string(id) AS id FROM local_user WHERE email = $email LIMIT 1")
            .bind(("email", normalized_email.clone()))
            .await
            .map_err(|e| {
                error!(
                    "Failed to look up local_user by email {}: {}",
                    normalized_email, e
                );
                AppError::Database(e)
            })?;

        let users: Vec<Value> = result.take(0).map_err(|e| {
            error!(
                "Failed to parse local_user lookup for email {}: {}",
                normalized_email, e
            );
            AppError::Database(e.into())
        })?;

        let user_id = users.first().and_then(local_user_id_from_lookup_row);
        if let Some(user_id) = &user_id {
            info!(
                "Resolved invitee email {} to local user {} for station notification",
                normalized_email, user_id
            );
        } else {
            info!(
                "Invitee email {} does not match a local user; station notification skipped",
                normalized_email
            );
        }

        Ok(user_id)
    }

    async fn resolve_inviter_display_name(&self, inviter: &User) -> String {
        if let Some(name) = inviter
            .profile
            .as_ref()
            .and_then(|profile| profile.display_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            return name.to_string();
        }

        if inviter.email != "unknown@example.com" && !inviter.email.trim().is_empty() {
            return inviter.email.clone();
        }

        match self.find_local_user_display_name(&inviter.id).await {
            Ok(Some(name)) => name,
            Ok(None) => inviter.id.clone(),
            Err(e) => {
                error!(
                    "Failed to resolve inviter display name for {}: {}",
                    inviter.id, e
                );
                inviter.id.clone()
            }
        }
    }

    async fn find_local_user_display_name(&self, user_id: &str) -> Result<Option<String>> {
        let clean_user_id = clean_user_id_format(user_id);
        if clean_user_id.is_empty() {
            return Ok(None);
        }

        let mut result = self
            .db
            .client
            .query(
                "SELECT username, email FROM local_user
                 WHERE type::string(id) = $local_user_prefixed
                    OR string::replace(type::string(id), 'local_user:', '') = $local_user_id
                 LIMIT 1",
            )
            .bind(("local_user_id", clean_user_id.clone()))
            .bind(("local_user_prefixed", format!("local_user:{}", clean_user_id)))
            .await
            .map_err(|e| {
                error!(
                    "Failed to look up local_user display name for {}: {}",
                    clean_user_id, e
                );
                AppError::Database(e)
            })?;

        let users: Vec<Value> = result.take(0).map_err(|e| {
            error!(
                "Failed to parse local_user display name lookup for {}: {}",
                clean_user_id, e
            );
            AppError::Database(e.into())
        })?;

        Ok(users.first().and_then(inviter_display_name_from_local_user))
    }

    /// 接受邀请
    pub async fn accept_invitation(
        &self,
        user_id: &str,
        request: AcceptInvitationRequest,
    ) -> Result<SpaceMember> {
        // 查找邀请 - 使用更简单的查询方法
        info!(
            "Searching for invitation with token: {}",
            &request.invite_token
        );

        let mut invitation_result = self
            .db
            .client
            .query(invitation_lookup_query())
            .bind(("invite_token", request.invite_token.clone()))
            .await
            .map_err(|e| {
                error!("Failed to query invitation by token: {}", e);
                AppError::Database(e)
            })?;

        let invitations: Vec<SpaceInvitationDb> = invitation_result.take(0).map_err(|e| {
            error!("Failed to parse invitation lookup result: {}", e);
            AppError::Database(e.into())
        })?;

        let invitation = invitations
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound("Invitation not found or expired".to_string()))?;

        // 检查邀请是否已用完
        if invitation.used_count >= invitation.max_uses {
            return Err(AppError::Conflict(
                "Invitation has been used up".to_string(),
            ));
        }

        let invitation_space_id = invitation.space_id.clone().into_key_string();

        // 检查是否已经是成员
        if self
            .can_access_space(&invitation_space_id, Some(user_id))
            .await?
        {
            return Err(AppError::Conflict(
                "User is already a member of this space".to_string(),
            ));
        }

        // 提取纯净的space_id和user_id，避免嵌套Thing
        let raw_space_id = invitation_space_id.clone();
        info!("Raw space_id from invitation: {}", raw_space_id);

        // 处理可能的嵌套Thing格式 space:⟨⟨space:xxxxx⟩⟩
        let clean_space_id = if raw_space_id.contains("⟨⟨space:") {
            // 提取最内层的ID，从 space:⟨⟨space:xxxxx⟩⟩ 中提取 xxxxx
            if let Some(start) = raw_space_id.find("⟨⟨space:") {
                let after_start = &raw_space_id[start + 8..]; // 跳过 "⟨⟨space:"
                if let Some(end) = after_start.find("⟩⟩") {
                    &after_start[..end]
                } else {
                    &raw_space_id
                }
            } else {
                &raw_space_id
            }
        } else if raw_space_id.starts_with("space:") {
            raw_space_id.strip_prefix("space:").unwrap()
        } else {
            &raw_space_id
        };

        // 清理user_id格式，确保存储的是纯净的UUID
        let clean_user_id = clean_user_id_format(user_id);

        info!(
            "Creating space member with clean_space_id: {}, clean_user_id: {}",
            clean_space_id, clean_user_id
        );

        let mut create_result = self
            .db
            .client
            .query(create_accepted_member_query())
            .bind(("space_id", format!("space:{}", clean_space_id)))
            .bind(("user_id", clean_user_id))
            .bind(("role", invitation.role.clone()))
            .bind(("permissions", invitation.permissions.clone()))
            .bind(("invited_by", invitation.invited_by.clone()))
            .await
            .map_err(|e| {
                error!("Failed to create space member: {}", e);
                AppError::Database(e)
            })?;

        let created_members: Vec<SpaceMemberDb> = create_result.take(0).map_err(|e| {
            error!("Failed to take created member: {}", e);
            AppError::Database(e.into())
        })?;

        let created_member = created_members
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Failed to create member")))?;

        // 更新邀请使用次数 - 使用简单的更新方法
        if let Some(invitation_id) = &invitation.id {
            let invitation_record_id = invitation_id
                .clone()
                .into_string()
                .strip_prefix("space_invitation:")
                .map(str::to_string)
                .unwrap_or_else(|| invitation_id.clone().into_string());

            let mut update_result = self
                .db
                .client
                .query(update_invitation_usage_query())
                .bind(("invitation_id", format!("space_invitation:{}", invitation_record_id)))
                .await
                .map_err(|e| {
                    error!("Failed to update invitation used_count: {}", e);
                    AppError::Database(e)
                })?;

            let _: Vec<serde_json::Value> = update_result.take(0).map_err(|e| {
                error!("Failed to take update results: {}", e);
                AppError::Database(e.into())
            })?;
        }

        info!(
            "User {} accepted invitation to space {}",
            user_id, invitation_space_id
        );

        Ok(created_member.into())
    }

    /// 获取空间成员列表
    pub async fn list_space_members(
        &self,
        space_id: &str,
        _requester: &User,
    ) -> Result<Vec<SpaceMemberResponse>> {
        // 提取实际的空间ID（去掉"space:"前缀，如果存在）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        let query = "SELECT * FROM space_member WHERE space_id = $space_id ORDER BY created_at ASC";
        let members: Vec<SpaceMemberDb> = self
            .db
            .client
            .query(query)
            .bind(("space_id", Thing::new("space", actual_space_id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let member_responses = members
            .into_iter()
            .map(|member| SpaceMemberResponse::from(SpaceMember::from(member)))
            .collect();

        Ok(member_responses)
    }

    /// 更新成员权限
    pub async fn update_member(
        &self,
        space_id: &str,
        member_user_id: &str,
        updater: &User,
        request: UpdateMemberRequest,
    ) -> Result<SpaceMemberResponse> {
        request
            .validate()
            .map_err(|e| AppError::Validation(e.to_string()))?;

        // 检查管理权限
        if !self
            .check_permission(space_id, &updater.id, "members.manage")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: members.manage required".to_string(),
            ));
        }

        // 提取实际的空间ID（去掉"space:"前缀，如果存在）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        // 获取当前成员信息
        let query = "SELECT * FROM space_member WHERE space_id = $space_id AND user_id = $user_id";
        let members: Vec<SpaceMemberDb> = self
            .db
            .client
            .query(query)
            .bind(("space_id", Thing::new("space", actual_space_id)))
            .bind(("user_id", member_user_id))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let mut member: SpaceMember = members
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound("Member not found".to_string()))?
            .into();

        // 更新字段
        if let Some(role) = request.role {
            member.role = role.clone();
            member.permissions = role.default_permissions();
        }

        if let Some(permissions) = request.permissions {
            member.permissions = permissions;
        }

        member.updated_at = Utc::now();

        // 保存更新
        let updated: Option<SpaceMemberDb> = self.db.client
            .query("UPDATE space_member SET role = $role, permissions = $permissions, updated_at = $updated_at WHERE space_id = $space_id AND user_id = $user_id RETURN AFTER")
            .bind(("role", &member.role))
            .bind(("permissions", &member.permissions))
            .bind(("updated_at", member.updated_at))
            .bind(("space_id", Thing::new("space", actual_space_id)))
            .bind(("user_id", member_user_id))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "AFTER"))?;

        let updated_member = updated
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Failed to update member")))?;

        info!(
            "User {} updated member {} in space {}",
            updater.id, member_user_id, space_id
        );

        Ok(SpaceMemberResponse::from(SpaceMember::from(updated_member)))
    }

    /// 移除成员
    pub async fn remove_member(
        &self,
        space_id: &str,
        member_user_id: &str,
        remover: &User,
    ) -> Result<()> {
        // 检查移除权限
        if !self
            .check_permission(space_id, &remover.id, "members.remove")
            .await?
        {
            return Err(AppError::Authorization(
                "Permission denied: members.remove required".to_string(),
            ));
        }

        // 不能移除自己
        if member_user_id == remover.id {
            return Err(AppError::Conflict(
                "Cannot remove yourself from space".to_string(),
            ));
        }

        // 提取实际的空间ID（去掉"space:"前缀，如果存在）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        // 删除成员记录
        let _: Option<SpaceMemberDb> = self
            .db
            .client
            .query("DELETE space_member WHERE space_id = $space_id AND user_id = $user_id")
            .bind(("space_id", Thing::new("space", actual_space_id)))
            .bind(("user_id", member_user_id))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        info!(
            "User {} removed member {} from space {}",
            remover.id, member_user_id, space_id
        );

        Ok(())
    }

    /// 获取用户参与的空间列表
    pub async fn get_user_spaces(&self, user_id: &str) -> Result<Vec<String>> {
        let query =
            "SELECT space_id FROM space_member WHERE user_id = $user_id AND status = 'accepted'";
        let members: Vec<SpaceMemberDb> = self
            .db
            .client
            .query(query)
            .bind(("user_id", user_id))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let space_ids = members
            .into_iter()
            .map(|member| record_id_key(&member.space_id))
            .collect();

        Ok(space_ids)
    }

    /// 发送邀请通知（邮件和站内通知）
    async fn send_invitation_notifications(
        &self,
        to_email: Option<&str>,
        to_user_id: Option<&str>,
        space_id: &str,
        inviter_name: &str,
        invite_token: &str,
        role: &str,
        message: Option<&str>,
        expires_in_days: u64,
    ) -> Result<()> {
        // 获取空间名称
        let space_name = self.get_space_name(space_id).await?;

        // 如果提供了用户ID，创建站内通知
        if let Some(user_id) = to_user_id {
            self.create_space_invitation_notification(
                user_id,
                &space_name,
                inviter_name,
                invite_token,
                role,
                message,
            )
            .await?;
        }

        // 如果提供了邮箱，发送邮件通知
        if let Some(email) = to_email {
            self.send_invitation_email(
                email,
                &space_name,
                inviter_name,
                invite_token,
                role,
                message,
                expires_in_days,
            )
            .await?;
        }

        Ok(())
    }

    /// 获取空间名称
    async fn get_space_name(&self, space_id: &str) -> Result<String> {
        // 提取实际的空间ID（去掉"space:"前缀，如果存在）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        let mut response = self
            .db
            .client
            .query(space_name_lookup_query())
            .bind(("space_id", actual_space_id.to_string()))
            .bind(("space_id_prefixed", format!("space:{}", actual_space_id)))
            .await
            .map_err(|e| {
                error!("Failed to get space name for {}: {}", space_id, e);
                AppError::Database(e)
            })?;

        let spaces: Vec<serde_json::Value> = response.take(0)?;
        match spaces.into_iter().next() {
            Some(space_data) => {
                let name = space_data["name"]
                    .as_str()
                    .unwrap_or("未知空间")
                    .to_string();
                info!("Found space name: {} for space: {}", name, space_id);
                Ok(name)
            }
            None => {
                warn!("No space found for ID: {}", space_id);
                Ok("未知空间".to_string())
            }
        }
    }

    /// 创建站内通知
    async fn create_space_invitation_notification(
        &self,
        user_id: &str,
        space_name: &str,
        inviter_name: &str,
        invite_token: &str,
        role: &str,
        message: Option<&str>,
    ) -> Result<()> {
        use serde_json::json;

        // 创建通知数据
        let notification_data = json!({
            "space_name": space_name,
            "invite_token": invite_token,
            "role": role,
            "inviter_name": inviter_name,
        });

        info!("Creating notification with data: {}", notification_data);

        let title = format!("{} 邀请您加入 {} 空间", inviter_name, space_name);
        let content = format!(
            "{} 邀请您以 {} 的身份加入 {} 空间。{}",
            inviter_name,
            role,
            space_name,
            message.unwrap_or(""),
        );

        // 最终解决方案：将invite_token作为独立字段存储，完全绕过data字段的问题
        info!("Storing invite_token as separate field: {}", invite_token);

        let query = r#"
            CREATE notification SET
                user_id = $user_id,
                type = $type,
                title = $title,
                content = $content,
                data = NONE,
                invite_token = $invite_token,
                space_name = $space_name,
                role = $role,
                inviter_name = $inviter_name,
                is_read = false,
                created_at = time::now(),
                updated_at = time::now()
        "#;

        let mut result = self
            .db
            .client
            .query(query)
            .bind(("user_id", user_id))
            .bind(("type", "space_invitation"))
            .bind(("title", &title))
            .bind(("content", &content))
            .bind(("invite_token", invite_token))
            .bind(("space_name", space_name))
            .bind(("role", role))
            .bind(("inviter_name", inviter_name))
            .await
            .map_err(|e| {
                error!("Failed to create notification: {}", e);
                AppError::Database(e)
            })?;

        // 获取创建的通知记录
        let created_notifications: Vec<serde_json::Value> = result.take(0).map_err(|e| {
            error!("Failed to retrieve created notification: {}", e);
            AppError::Database(e.into())
        })?;

        if created_notifications.is_empty() {
            error!("No notification was created for user {}", user_id);
            return Err(AppError::Internal(anyhow::anyhow!(
                "Failed to create notification"
            )));
        }

        // 记录创建的通知详情
        if let Some(created_notification) = created_notifications.first() {
            info!(
                "Successfully created notification: {}",
                serde_json::to_string_pretty(created_notification).unwrap_or_default()
            );
        }

        info!("Created space invitation notification for user {}", user_id);
        Ok(())
    }

    /// 发送邀请邮件
    async fn send_invitation_email(
        &self,
        to_email: &str,
        space_name: &str,
        inviter_name: &str,
        invite_token: &str,
        role: &str,
        message: Option<&str>,
        expires_in_days: u64,
    ) -> Result<()> {
        /*  use serde_json::json;

        // 调用 Rainbow-Auth 的邮件服务
        let rainbow_auth_url = self.config.auth.rainbow_auth_url
            .as_ref()
            .ok_or_else(|| AppError::Configuration("Rainbow-Auth URL not configured".to_string()))?;

        let url = format!("{}/api/internal/email/notification", rainbow_auth_url);

        let email_data = json!({
            "to": to_email,
            "notification_type": "space_invitation",
            "data": {
                "space_name": space_name,
                "inviter_name": inviter_name,
                "invite_token": invite_token,
                "role": role,
                "message": message,
                "expires_in_days": expires_in_days,
            }
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("X-Internal-API-Key", "todo-implement-api-key") // TODO: 实现内部API密钥
            .json(&email_data)
            .send()
            .await
            .map_err(|e| AppError::External(format!("Failed to send email: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!("Failed to send email notification: {}", error_text);
            return Err(AppError::External(format!("Email service error: {}", error_text)));
        } */

        info!("Sent invitation email to {}", to_email);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        invitation_optional_assignments, inviter_display_name_from_local_user,
        invitation_lookup_query, local_user_id_from_lookup_row, normalize_space_id,
        create_accepted_member_query, space_id_match_candidates, space_name_lookup_query,
        space_owner_where_clause, update_invitation_usage_query,
    };
    use crate::models::space_member::{InviteMemberRequest, MemberRole};
    use serde_json::json;

    #[test]
    fn normalizes_nested_space_record_shapes() {
        assert_eq!(normalize_space_id("test"), "test");
        assert_eq!(normalize_space_id("space:test"), "test");
        assert_eq!(normalize_space_id("space:⟨test⟩"), "test");
        assert_eq!(normalize_space_id("space:⟨⟨space:test⟩⟩"), "test");
    }

    #[test]
    fn builds_compatible_space_id_match_candidates() {
        assert_eq!(
            space_id_match_candidates("space:⟨⟨space:test⟩⟩"),
            vec![
                "space:test".to_string(),
                "space:⟨test⟩".to_string(),
                "space:⟨space:test⟩".to_string(),
                "space:⟨⟨space:test⟩⟩".to_string(),
            ]
        );
    }

    #[test]
    fn owner_lookup_matches_space_id_string_shapes() {
        let clause = space_owner_where_clause();
        assert!(clause.contains("type::string(id) IN"));
        assert!(clause.contains("$space_id_plain"));
        assert!(clause.contains("$space_id_bracketed"));
        assert!(clause.contains("$space_id_prefixed"));
        assert!(clause.contains("$space_id_nested"));
        assert!(!clause.contains("id = $space_id"));
    }

    #[test]
    fn invitation_create_omits_empty_optional_fields() {
        let request = InviteMemberRequest {
            email: None,
            user_id: Some("user-1".to_string()),
            role: MemberRole::Viewer,
            message: None,
            expires_in_days: None,
        };

        let assignments = invitation_optional_assignments(&request);
        assert!(!assignments.contains("email = $email"));
        assert!(assignments.contains("user_id = $user_id"));
        assert!(!assignments.contains("message = $message"));
    }

    #[test]
    fn extracts_clean_local_user_id_from_lookup_row() {
        assert_eq!(
            local_user_id_from_lookup_row(&json!({ "id": "local_user:abc-123" })),
            Some("abc-123".to_string())
        );
        assert_eq!(
            local_user_id_from_lookup_row(&json!({ "id": "local_user:⟨abc-123⟩" })),
            Some("abc-123".to_string())
        );
        assert_eq!(local_user_id_from_lookup_row(&json!({ "id": "" })), None);
    }

    #[test]
    fn inviter_display_name_prefers_local_username_then_email() {
        assert_eq!(
            inviter_display_name_from_local_user(&json!({
                "username": "Trantor Labs",
                "email": "owner@example.com"
            })),
            Some("Trantor Labs".to_string())
        );
        assert_eq!(
            inviter_display_name_from_local_user(&json!({
                "username": "",
                "email": "owner@example.com"
            })),
            Some("owner@example.com".to_string())
        );
    }

    #[test]
    fn space_name_lookup_matches_projected_record_id_strings() {
        let query = space_name_lookup_query();

        assert!(query.contains("type::string(id) = $space_id_prefixed"));
        assert!(query.contains("string::replace(type::string(id), 'space:', '') = $space_id"));
    }

    #[test]
    fn invitation_lookup_uses_token_filter_not_full_table_select() {
        let query = invitation_lookup_query();

        assert!(query.contains("FROM space_invitation"));
        assert!(query.contains("invite_token = $invite_token"));
        assert!(query.contains("expires_at > time::now()"));
        assert!(query.contains("LIMIT 1"));
    }

    #[test]
    fn accept_invitation_writes_record_ids_with_type_record() {
        let create_query = create_accepted_member_query();
        let update_query = update_invitation_usage_query();

        assert!(create_query.contains("space_id = type::record($space_id)"));
        assert!(update_query.contains("UPDATE type::record($invitation_id)"));
    }
}
