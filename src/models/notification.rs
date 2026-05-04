use crate::services::database::record_id_to_string;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId as Thing;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    SpaceInvitation,
    DocumentShared,
    CommentMention,
    DocumentUpdate,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlexibleNotificationId {
    RecordId(Thing),
    String(String),
}

impl FlexibleNotificationId {
    pub fn into_string(self) -> String {
        match self {
            FlexibleNotificationId::RecordId(thing) => record_id_to_string(&thing),
            FlexibleNotificationId::String(value) => value,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotificationDb {
    pub id: Option<FlexibleNotificationId>,
    pub user_id: String,
    #[serde(rename = "type")]
    pub notification_type: NotificationType,
    pub title: String,
    pub content: String,
    pub data: Option<serde_json::Value>,
    pub invite_token: Option<String>,
    pub space_name: Option<String>,
    pub role: Option<String>,
    pub inviter_name: Option<String>,
    pub is_read: bool,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: Option<String>,
    pub user_id: String,
    #[serde(rename = "type")]
    pub notification_type: NotificationType,
    pub title: String,
    pub content: String,
    pub data: Option<serde_json::Value>,
    pub invite_token: Option<String>,
    pub space_name: Option<String>,
    pub role: Option<String>,
    pub inviter_name: Option<String>,
    pub is_read: bool,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateNotificationRequest {
    pub user_id: String,
    #[serde(rename = "type")]
    pub notification_type: NotificationType,
    pub title: String,
    pub content: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateNotificationRequest {
    pub is_read: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationListQuery {
    pub page: Option<u32>,
    pub limit: Option<u32>,
    pub unread_only: Option<bool>,
}

impl From<NotificationDb> for Notification {
    fn from(db: NotificationDb) -> Self {
        Self {
            id: db.id.map(FlexibleNotificationId::into_string),
            user_id: db.user_id,
            notification_type: db.notification_type,
            title: db.title,
            content: db.content,
            data: db.data,
            invite_token: db.invite_token,
            space_name: db.space_name,
            role: db.role,
            inviter_name: db.inviter_name,
            is_read: db.is_read,
            read_at: db.read_at,
            created_at: db.created_at,
            updated_at: db.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FlexibleNotificationId, NotificationDb};

    #[test]
    fn notification_db_accepts_string_record_ids() {
        let notification: NotificationDb = serde_json::from_str(
            r#"{
                "id": "notification:p1tufjserris6owmfsun",
                "user_id": "user-1",
                "type": "space_invitation",
                "title": "Invite",
                "content": "Join",
                "data": null,
                "invite_token": "token-1",
                "space_name": "Space",
                "role": "成员",
                "inviter_name": "Owner",
                "is_read": false,
                "read_at": null,
                "created_at": "2026-05-04T08:42:08Z",
                "updated_at": "2026-05-04T08:42:08Z"
            }"#,
        )
        .unwrap();

        assert!(matches!(
            notification.id,
            Some(FlexibleNotificationId::String(id)) if id == "notification:p1tufjserris6owmfsun"
        ));
    }
}
