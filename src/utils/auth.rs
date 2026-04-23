use crate::error::{AppError, Result};
use crate::services::auth::{AuthService, User};
use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};
use std::collections::HashSet;
use std::sync::Arc;

/// 检查用户是否有指定权限
pub fn has_permission(user: &User, permission: &str) -> bool {
    user.permissions.contains(&permission.to_string())
}

/// 检查用户是否有指定角色
pub fn has_role(user: &User, role: &str) -> bool {
    user.roles.contains(&role.to_string())
}

/// 管理员权限检查
pub fn require_admin(user: &User) -> Result<()> {
    if user.roles.contains(&"admin".to_string())
        || user.permissions.contains(&"docs.admin".to_string())
    {
        Ok(())
    } else {
        Err(AppError::Authorization(
            "Admin permission required".to_string(),
        ))
    }
}

/// 检查用户是否有文档读取权限
pub fn can_read_document(user: &User) -> bool {
    user.permissions.contains(&"docs.read".to_string())
        || user.permissions.contains(&"docs.write".to_string())
        || user.permissions.contains(&"docs.admin".to_string())
}

/// 检查用户是否有文档写入权限
pub fn can_write_document(user: &User) -> bool {
    user.permissions.contains(&"docs.write".to_string())
        || user.permissions.contains(&"docs.admin".to_string())
}

/// 检查用户是否有文档管理权限
pub fn can_admin_document(user: &User) -> bool {
    user.permissions.contains(&"docs.admin".to_string())
}

/// 检查用户是否是空间所有者或有管理权限
pub fn can_manage_space(user: &User, space_owner_id: &str) -> bool {
    user.id == space_owner_id || can_admin_document(user)
}

/// 文档权限类型
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentPermission {
    Read,
    Write,
    Admin,
}

/// 检查用户是否有特定文档权限
pub fn has_document_permission(
    user: &User,
    permission: DocumentPermission,
    document_owner_id: Option<&str>,
) -> bool {
    match permission {
        DocumentPermission::Read => {
            can_read_document(user) || document_owner_id.map_or(false, |owner| user.id == owner)
        }
        DocumentPermission::Write => {
            can_write_document(user) || document_owner_id.map_or(false, |owner| user.id == owner)
        }
        DocumentPermission::Admin => can_admin_document(user),
    }
}

/// 从请求头中提取用户ID
pub async fn extract_user_from_header(
    headers: &HeaderMap,
    auth_service: &Arc<AuthService>,
) -> crate::error::Result<String> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| AppError::unauthorized("Authorization header missing"))?;

    if !auth_header.starts_with("Bearer ") {
        return Err(AppError::unauthorized(
            "Invalid authorization header format",
        ));
    }

    let token = &auth_header[7..]; // Remove "Bearer " prefix

    // Validate token and extract user ID
    let claims = auth_service
        .verify_jwt(token)
        .map_err(|_| AppError::unauthorized("Invalid token"))?;

    // Extract user ID from claims
    let user_id = claims.sub;

    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_user(id: &str, roles: Vec<&str>, permissions: Vec<&str>) -> User {
        User {
            id: id.to_string(),
            email: format!("{}@example.com", id),
            roles: roles.into_iter().map(|s| s.to_string()).collect(),
            permissions: permissions.into_iter().map(|s| s.to_string()).collect(),
            profile: None,
        }
    }

    #[test]
    fn test_permission_checks() {
        let admin_user = create_test_user("1", vec!["admin"], vec!["docs.admin"]);
        let writer_user = create_test_user("2", vec!["writer"], vec!["docs.write"]);
        let reader_user = create_test_user("3", vec!["reader"], vec!["docs.read"]);

        // Test admin permissions
        assert!(can_read_document(&admin_user));
        assert!(can_write_document(&admin_user));
        assert!(can_admin_document(&admin_user));

        // Test writer permissions
        assert!(can_read_document(&writer_user));
        assert!(can_write_document(&writer_user));
        assert!(!can_admin_document(&writer_user));

        // Test reader permissions
        assert!(can_read_document(&reader_user));
        assert!(!can_write_document(&reader_user));
        assert!(!can_admin_document(&reader_user));
    }
}
