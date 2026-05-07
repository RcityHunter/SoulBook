use crate::services::database::record_id_to_string;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId as Thing;
use validator::Validate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamDb {
    pub id: Option<Thing>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub owner_id: String,
    #[serde(default)]
    pub is_deleted: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub owner_id: String,
    #[serde(default)]
    pub is_deleted: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
}

impl Default for TeamRole {
    fn default() -> Self {
        Self::Member
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemberRow {
    pub team_id: String,
    pub role: TeamRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamWorkspaceResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub role: TeamRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalWorkspaceResponse {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacesResponse {
    pub personal: PersonalWorkspaceResponse,
    pub teams: Vec<TeamWorkspaceResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CreateTeamRequest {
    #[validate(length(
        min = 1,
        max = 80,
        message = "Team name must be between 1 and 80 characters"
    ))]
    pub name: String,

    #[validate(length(
        min = 1,
        max = 50,
        message = "Team slug must be between 1 and 50 characters"
    ))]
    #[validate(regex(
        path = "crate::models::workspace::TEAM_SLUG_REGEX",
        message = "Team slug can only contain lowercase letters, numbers, and hyphens"
    ))]
    pub slug: String,

    #[validate(length(max = 300, message = "Team description cannot exceed 300 characters"))]
    pub description: Option<String>,
}

lazy_static::lazy_static! {
    pub static ref TEAM_SLUG_REGEX: regex::Regex = regex::Regex::new(r"^[a-z0-9-]+$").unwrap();
}

impl From<TeamDb> for Team {
    fn from(db: TeamDb) -> Self {
        Self {
            id: db
                .id
                .map(|thing| record_id_to_string(&thing))
                .unwrap_or_default(),
            name: db.name,
            slug: db.slug,
            description: db.description,
            owner_id: db.owner_id,
            is_deleted: db.is_deleted,
            created_at: db.created_at,
            updated_at: db.updated_at,
        }
    }
}

impl Team {
    pub fn into_workspace(self, role: TeamRole) -> TeamWorkspaceResponse {
        TeamWorkspaceResponse {
            id: self
                .id
                .strip_prefix("team:")
                .unwrap_or(&self.id)
                .to_string(),
            name: self.name,
            slug: self.slug,
            description: self.description,
            role,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    #[test]
    fn validates_team_slug() {
        let valid = CreateTeamRequest {
            name: "Team 514".to_string(),
            slug: "team-514".to_string(),
            description: None,
        };
        assert!(valid.validate().is_ok());

        let invalid = CreateTeamRequest {
            name: "Team".to_string(),
            slug: "Team_514".to_string(),
            description: None,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn strips_team_prefix_for_workspace_id() {
        let team = Team {
            id: "team:abc".to_string(),
            name: "ABC".to_string(),
            slug: "abc".to_string(),
            description: None,
            owner_id: "user-1".to_string(),
            is_deleted: Some(false),
            created_at: None,
            updated_at: None,
        };

        let workspace = team.into_workspace(TeamRole::Owner);
        assert_eq!(workspace.id, "abc");
        assert_eq!(workspace.role, TeamRole::Owner);
    }
}
