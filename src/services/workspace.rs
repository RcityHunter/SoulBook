use crate::error::{AppError, Result};
use crate::models::workspace::{
    CreateTeamRequest, PersonalWorkspaceResponse, Team, TeamMemberRow, TeamRole,
    TeamWorkspaceResponse, WorkspacesResponse,
};
use crate::services::auth::User;
use crate::services::database::Database;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{error, info};
use validator::Validate;

fn clean_user_id(user_id: &str) -> String {
    user_id
        .trim()
        .strip_prefix("user:")
        .or_else(|| user_id.trim().strip_prefix("users:"))
        .unwrap_or(user_id.trim())
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string()
}

fn user_id_candidates(user_id: &str) -> Vec<String> {
    let clean = clean_user_id(user_id);
    vec![
        clean.clone(),
        format!("user:{}", clean),
        format!("user:⟨{}⟩", clean),
    ]
}

fn create_team_optional_fields(description: &Option<String>) -> &'static str {
    if description.is_some() {
        "description: $description,"
    } else {
        ""
    }
}

fn team_projection_query() -> &'static str {
    "SELECT
        string::replace(type::string(id), 'team:', '') AS id,
        name,
        slug,
        description,
        (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
        is_deleted,
        created_at,
        updated_at
     FROM team
     WHERE string::replace(type::string(id), 'team:', '') = $team_id
       AND is_deleted = false
     LIMIT 1"
}

pub struct WorkspaceService {
    db: Arc<Database>,
}

impl WorkspaceService {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn list_workspaces(&self, user: &User) -> Result<WorkspacesResponse> {
        let mut teams_by_id: HashMap<String, TeamWorkspaceResponse> = HashMap::new();
        let user_ids = user_id_candidates(&user.id);

        let mut owner_response = self
            .db
            .client
            .query(
                "SELECT
                    string::replace(type::string(id), 'team:', '') AS id,
                    name,
                    slug,
                    description,
                    (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
                    is_deleted,
                    created_at,
                    updated_at
                 FROM team
                 WHERE (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) IN $user_ids
                   AND is_deleted = false
                 ORDER BY updated_at DESC",
            )
            .bind(("user_ids", user_ids.clone()))
            .await
            .map_err(|e| {
                error!("Failed to list owned teams: {}", e);
                AppError::Database(e)
            })?;

        let owned_teams: Vec<Team> = owner_response.take(0).map_err(|e| {
            error!("Failed to decode owned teams: {}", e);
            AppError::Database(e)
        })?;

        for team in owned_teams {
            teams_by_id.insert(team.id.clone(), team.into_workspace(TeamRole::Owner));
        }

        let mut member_response = self
            .db
            .client
            .query(
                "SELECT team_id, role
                 FROM team_member
                 WHERE user_id IN $user_ids
                   AND status = 'active'
                 ORDER BY joined_at DESC",
            )
            .bind(("user_ids", user_ids))
            .await
            .map_err(|e| {
                error!("Failed to list team memberships: {}", e);
                AppError::Database(e)
            })?;

        let member_rows: Vec<TeamMemberRow> = member_response.take(0).map_err(|e| {
            error!("Failed to decode team memberships: {}", e);
            AppError::Database(e)
        })?;

        let mut seen_team_ids: HashSet<String> = teams_by_id.keys().cloned().collect();
        for row in member_rows {
            let team_id = row
                .team_id
                .strip_prefix("team:")
                .unwrap_or(&row.team_id)
                .to_string();
            if !seen_team_ids.insert(team_id.clone()) {
                continue;
            }

            if let Some(team) = self.get_team_by_id(&team_id).await? {
                teams_by_id.insert(team_id, team.into_workspace(row.role));
            }
        }

        let mut teams: Vec<TeamWorkspaceResponse> = teams_by_id.into_values().collect();
        teams.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        Ok(WorkspacesResponse {
            personal: PersonalWorkspaceResponse {
                id: "personal".to_string(),
                name: "个人工作区".to_string(),
            },
            teams,
        })
    }

    pub async fn create_team(
        &self,
        user: &User,
        request: CreateTeamRequest,
    ) -> Result<TeamWorkspaceResponse> {
        request
            .validate()
            .map_err(|e| AppError::Validation(e.to_string()))?;

        if self.team_slug_exists(&request.slug).await? {
            return Err(AppError::Conflict(
                "Team slug already exists. Please choose a different slug.".to_string(),
            ));
        }

        let optional_fields = create_team_optional_fields(&request.description);
        let create_sql = format!(
            "CREATE team CONTENT {{
                name: $name,
                slug: $slug,
                {optional_fields}
                owner_id: $owner_id,
                is_deleted: false,
                created_by: $owner_id,
                updated_by: $owner_id,
                created_at: time::now(),
                updated_at: time::now()
            }};"
        );

        let mut create_query = self
            .db
            .client
            .query(create_sql)
            .bind(("name", request.name.clone()))
            .bind(("slug", request.slug.clone()))
            .bind(("owner_id", clean_user_id(&user.id)));

        if let Some(description) = &request.description {
            create_query = create_query.bind(("description", description.clone()));
        }

        let mut create_response = create_query.await.map_err(|e| {
            error!("Failed to create team: {}", e);
            AppError::Database(e)
        })?;
        let _: Value = create_response.take(0).map_err(|e| {
            error!("Failed to decode team create acknowledgement: {}", e);
            AppError::Database(e)
        })?;

        let team = self
            .get_team_by_slug(&request.slug)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Failed to fetch created team")))?;

        self.create_owner_membership(&team.id, user).await?;

        info!("User {} created team workspace {}", user.id, request.slug);
        Ok(team.into_workspace(TeamRole::Owner))
    }

    async fn team_slug_exists(&self, slug: &str) -> Result<bool> {
        let mut response = self
            .db
            .client
            .query("SELECT count() AS count FROM team WHERE slug = $slug AND is_deleted = false GROUP ALL")
            .bind(("slug", slug.to_string()))
            .await
            .map_err(AppError::Database)?;

        let rows: Vec<Value> = response.take(0).map_err(AppError::Database)?;
        Ok(rows
            .first()
            .and_then(|row| row.get("count"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0)
    }

    async fn get_team_by_slug(&self, slug: &str) -> Result<Option<Team>> {
        let mut response = self
            .db
            .client
            .query(
                "SELECT
                    string::replace(type::string(id), 'team:', '') AS id,
                    name,
                    slug,
                    description,
                    (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
                    is_deleted,
                    created_at,
                    updated_at
                 FROM team
                 WHERE slug = $slug AND is_deleted = false
                 LIMIT 1",
            )
            .bind(("slug", slug.to_string()))
            .await
            .map_err(AppError::Database)?;

        let teams: Vec<Team> = response.take(0).map_err(AppError::Database)?;
        Ok(teams.into_iter().next())
    }

    async fn get_team_by_id(&self, team_id: &str) -> Result<Option<Team>> {
        let mut response = self
            .db
            .client
            .query(team_projection_query())
            .bind(("team_id", team_id.to_string()))
            .await
            .map_err(AppError::Database)?;

        let teams: Vec<Team> = response.take(0).map_err(AppError::Database)?;
        Ok(teams.into_iter().next())
    }

    async fn create_owner_membership(&self, team_id: &str, user: &User) -> Result<()> {
        let user_id = clean_user_id(&user.id);
        let mut response = self
            .db
            .client
            .query(
                "CREATE team_member SET
                    team_id = $team_id,
                    user_id = $user_id,
                    role = 'owner',
                    status = 'active',
                    invited_by = $user_id,
                    joined_at = time::now(),
                    created_at = time::now(),
                    updated_at = time::now()",
            )
            .bind(("team_id", team_id.to_string()))
            .bind(("user_id", user_id))
            .await
            .map_err(|e| {
                error!("Failed to create team owner membership: {}", e);
                AppError::Database(e)
            })?;
        let _: Value = response.take(0).map_err(AppError::Database)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_user_id_candidates_from_record_id() {
        assert_eq!(
            user_id_candidates("user:⟨abc⟩"),
            vec![
                "abc".to_string(),
                "user:abc".to_string(),
                "user:⟨abc⟩".to_string()
            ]
        );
    }

    #[test]
    fn optional_description_field_only_when_present() {
        assert_eq!(create_team_optional_fields(&None), "");
        assert_eq!(
            create_team_optional_fields(&Some("desc".to_string())),
            "description: $description,"
        );
    }
}
