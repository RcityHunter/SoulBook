use std::sync::Arc;

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    middleware::from_fn,
    response::Response,
    routing::get,
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::AppError,
    models::{
        document::{Document, DocumentQuery},
        search::{SearchRequest, SearchResponse, SearchSortBy},
        space::{SpaceListQuery, SpaceResponse},
    },
    services::auth::{OptionalUser, User},
    AppState,
};

use super::{
    request_id::{inject_request_id, RequestId},
    response::{err_response, ok_response},
};

#[derive(Debug, Clone, Serialize)]
struct AgentCapabilityInfo {
    name: &'static str,
    scope: &'static str,
    access: &'static str,
    notes: Option<&'static str>,
}

const CAP_SYSTEM_HEALTH: AgentCapabilityInfo = AgentCapabilityInfo {
    name: "system.health",
    scope: "system",
    access: "public",
    notes: Some("Liveness only; no dependency or config dump."),
};

const CAP_SPACE_LIST: AgentCapabilityInfo = AgentCapabilityInfo {
    name: "space.list",
    scope: "space",
    access: "user-or-optional",
    notes: Some("Returns only spaces visible through existing space access rules."),
};

const CAP_SPACE_GET: AgentCapabilityInfo = AgentCapabilityInfo {
    name: "space.get",
    scope: "space",
    access: "user-or-optional",
    notes: Some("Lookup by space id only for a stable agent-facing contract."),
};

const CAP_DOCUMENT_LIST: AgentCapabilityInfo = AgentCapabilityInfo {
    name: "document.list",
    scope: "space",
    access: "user-or-optional",
    notes: Some("Lists documents under one space id with existing docs.read checks."),
};

const CAP_DOCUMENT_GET: AgentCapabilityInfo = AgentCapabilityInfo {
    name: "document.get",
    scope: "document",
    access: "user-or-optional",
    notes: Some("Lookup by document id only; slug-based lookup stays on legacy REST routes."),
};

const CAP_SEARCH_DOCUMENTS: AgentCapabilityInfo = AgentCapabilityInfo {
    name: "search.documents",
    scope: "search",
    access: "user",
    notes: Some("Keyword search only; suggestions, tag search, and reindex are excluded from MVP."),
};

#[derive(Debug, Serialize)]
struct AgentSuccessEnvelope<T>
where
    T: Serialize,
{
    capability: &'static str,
    scope: AgentScope,
    data: T,
}

#[derive(Debug, Serialize)]
struct AgentScope {
    kind: &'static str,
    id: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthPayload {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    capabilities: Vec<AgentCapabilityInfo>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AgentSpaceListQuery {
    pub page: Option<u32>,
    pub limit: Option<u32>,
    pub search: Option<String>,
    pub owner_id: Option<String>,
    pub is_public: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AgentDocumentListQuery {
    pub page: Option<u32>,
    pub limit: Option<u32>,
    pub search: Option<String>,
    pub parent_id: Option<String>,
    pub is_public: Option<bool>,
    pub author_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentSearchQuery {
    pub q: String,
    pub space_id: Option<String>,
    pub tags: Option<String>,
    pub author_id: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub sort: Option<String>,
}

pub fn router() -> Router {
    Router::new()
        .route("/system/health", get(health))
        .route("/spaces", get(list_spaces))
        .route("/spaces/:space_id", get(get_space))
        .route("/spaces/:space_id/documents", get(list_documents))
        .route("/documents/:document_id", get(get_document))
        .route("/search/documents", get(search_documents))
        .layer(from_fn(inject_request_id))
}

async fn health(request_id: Option<Extension<RequestId>>) -> Response {
    ok_response(
        StatusCode::OK,
        request_id.map(|Extension(request_id)| request_id),
        agent_ok(
            CAP_SYSTEM_HEALTH.name,
            AgentScope {
                kind: CAP_SYSTEM_HEALTH.scope,
                id: None,
            },
            HealthPayload {
                status: "ok",
                service: "souldoc-agent",
                version: env!("CARGO_PKG_VERSION"),
                capabilities: vec![
                    CAP_SYSTEM_HEALTH.clone(),
                    CAP_SPACE_LIST.clone(),
                    CAP_SPACE_GET.clone(),
                    CAP_DOCUMENT_LIST.clone(),
                    CAP_DOCUMENT_GET.clone(),
                    CAP_SEARCH_DOCUMENTS.clone(),
                ],
            },
        ),
    )
}

async fn list_spaces(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(query): Query<AgentSpaceListQuery>,
    optional_user: OptionalUser,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let user = optional_user.0;
    let request_id = request_id.map(|Extension(request_id)| request_id);

    match app_state
        .space_service
        .list_spaces(
            SpaceListQuery {
                page: query.page,
                limit: query.limit,
                search: query.search,
                owner_id: query.owner_id,
                is_public: query.is_public,
                sort: query.sort,
                order: query.order,
            },
            user.as_ref(),
        )
        .await
    {
        Ok(spaces) => ok_response(
            StatusCode::OK,
            request_id.clone(),
            agent_ok(
                CAP_SPACE_LIST.name,
                AgentScope {
                    kind: CAP_SPACE_LIST.scope,
                    id: None,
                },
                spaces,
            ),
        ),
        Err(error) => agent_error_response::<Value>(
            &error,
            request_id.clone(),
            "space_list_failed",
            error.to_string(),
        ),
    }
}

async fn get_space(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_id): Path<String>,
    optional_user: OptionalUser,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let user = optional_user.0;
    let request_id = request_id.map(|Extension(request_id)| request_id);

    match load_space_for_read(&app_state, &space_id, user.as_ref()).await {
        Ok(space) => ok_response(
            StatusCode::OK,
            request_id.clone(),
            agent_ok(
                CAP_SPACE_GET.name,
                AgentScope {
                    kind: CAP_SPACE_GET.scope,
                    id: Some(space.id.clone()),
                },
                space,
            ),
        ),
        Err(error) => agent_error_response::<SpaceResponse>(
            &error,
            request_id.clone(),
            "space_get_failed",
            error.to_string(),
        ),
    }
}

async fn list_documents(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(space_id): Path<String>,
    Query(query): Query<AgentDocumentListQuery>,
    optional_user: OptionalUser,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let user = optional_user.0;
    let request_id = request_id.map(|Extension(request_id)| request_id);

    let space = match load_space_for_read(&app_state, &space_id, user.as_ref()).await {
        Ok(space) => space,
        Err(error) => {
            return agent_error_response::<Value>(
                &error,
                request_id.clone(),
                "space_get_failed",
                error.to_string(),
            )
        }
    };

    match app_state
        .document_service
        .list_documents(
            &space.id,
            DocumentQuery {
                page: query.page,
                limit: query.limit,
                search: query.search,
                parent_id: query.parent_id,
                is_public: query.is_public,
                author_id: query.author_id,
                tags: None,
                sort: query.sort,
                order: query.order,
            },
            user.as_ref(),
        )
        .await
    {
        Ok(documents) => ok_response(
            StatusCode::OK,
            request_id.clone(),
            agent_ok(
                CAP_DOCUMENT_LIST.name,
                AgentScope {
                    kind: CAP_DOCUMENT_LIST.scope,
                    id: Some(space.id),
                },
                documents,
            ),
        ),
        Err(error) => agent_error_response::<Value>(
            &error,
            request_id.clone(),
            "document_list_failed",
            error.to_string(),
        ),
    }
}

async fn get_document(
    Extension(app_state): Extension<Arc<AppState>>,
    Path(document_id): Path<String>,
    optional_user: OptionalUser,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let user = optional_user.0;
    let request_id = request_id.map(|Extension(request_id)| request_id);

    let document = match app_state
        .document_service
        .get_document_by_id(&document_id)
        .await
    {
        Ok(document) => document,
        Err(error) => {
            return agent_error_response::<Document>(
                &error,
                request_id.clone(),
                "document_get_failed",
                error.to_string(),
            )
        }
    };

    let space = match load_space_for_read(&app_state, &document.space_id, user.as_ref()).await {
        Ok(space) => space,
        Err(error) => {
            return agent_error_response::<Document>(
                &error,
                request_id.clone(),
                "space_get_failed",
                error.to_string(),
            )
        }
    };

    if !document.can_read(user.as_ref().map(|u| u.id.as_str()), space.is_public) {
        return agent_error_response::<Document>(
            &AppError::forbidden("Access denied to this document"),
            request_id.clone(),
            "document_get_forbidden",
            "Access denied to this document",
        );
    }

    ok_response(
        StatusCode::OK,
        request_id.clone(),
        agent_ok(
            CAP_DOCUMENT_GET.name,
            AgentScope {
                kind: CAP_DOCUMENT_GET.scope,
                id: document.id.clone(),
            },
            document,
        ),
    )
}

async fn search_documents(
    Extension(app_state): Extension<Arc<AppState>>,
    Query(query): Query<AgentSearchQuery>,
    optional_user: OptionalUser,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let request_id = request_id.map(|Extension(request_id)| request_id);
    let user = match optional_user.0 {
        Some(user) => user,
        None => {
            return err_response::<SearchResponse>(
                StatusCode::UNAUTHORIZED,
                request_id.clone(),
                "search_documents_unauthorized",
                "authorization required",
            )
        }
    };

    if let Err(error) = app_state
        .auth_service
        .check_permission(&user.id, "docs.read", None)
        .await
    {
        return agent_error_response::<SearchResponse>(
            &error,
            request_id.clone(),
            "search_documents_forbidden",
            error.to_string(),
        );
    }

    let search_request = SearchRequest {
        query: query.q,
        space_id: query.space_id,
        tags: query.tags.map(|value| {
            value
                .split(',')
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(|item| item.to_string())
                .collect()
        }),
        author_id: query.author_id,
        page: query.page,
        per_page: query.per_page,
        sort_by: Some(match query.sort.as_deref() {
            Some("created_at") => SearchSortBy::CreatedAt,
            Some("updated_at") => SearchSortBy::UpdatedAt,
            Some("title") => SearchSortBy::Title,
            _ => SearchSortBy::Relevance,
        }),
    };

    match app_state
        .search_service
        .search(&user.id, search_request)
        .await
    {
        Ok(result) => ok_response(
            StatusCode::OK,
            request_id.clone(),
            agent_ok(
                CAP_SEARCH_DOCUMENTS.name,
                AgentScope {
                    kind: CAP_SEARCH_DOCUMENTS.scope,
                    id: None,
                },
                result,
            ),
        ),
        Err(error) => agent_error_response::<SearchResponse>(
            &error,
            request_id.clone(),
            "search_documents_failed",
            error.to_string(),
        ),
    }
}

fn agent_ok<T>(capability: &'static str, scope: AgentScope, data: T) -> AgentSuccessEnvelope<T>
where
    T: Serialize,
{
    AgentSuccessEnvelope {
        capability,
        scope,
        data,
    }
}

fn agent_error_response<T>(
    error: &AppError,
    request_id: Option<RequestId>,
    code: &'static str,
    message: impl Into<String>,
) -> Response
where
    T: Serialize,
{
    err_response::<T>(map_status(error), request_id, code, message)
}

fn map_status(error: &AppError) -> StatusCode {
    match error {
        AppError::Authentication(_) => StatusCode::UNAUTHORIZED,
        AppError::Authorization(_) => StatusCode::FORBIDDEN,
        AppError::Validation(_) | AppError::ValidationErrors(_) => StatusCode::BAD_REQUEST,
        AppError::NotFound(_) => StatusCode::NOT_FOUND,
        AppError::Conflict(_) => StatusCode::CONFLICT,
        AppError::Http(_) | AppError::External(_) => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn load_space_for_read(
    app_state: &Arc<AppState>,
    space_id: &str,
    user: Option<&User>,
) -> Result<SpaceResponse, AppError> {
    let space = fetch_space_by_id_internal(app_state, space_id).await?;

    if let Some(user) = user {
        if !is_space_owner(&space.owner_id, &user.id) {
            if !app_state
                .space_member_service
                .can_access_space(&space.id, Some(&user.id))
                .await?
            {
                return Err(AppError::forbidden("Access denied to this space"));
            }
            if !app_state
                .space_member_service
                .check_permission(&space.id, &user.id, "docs.read")
                .await?
            {
                return Err(AppError::forbidden("Permission denied: docs.read required"));
            }
        }
    } else if !space.is_public {
        return Err(AppError::forbidden("Access denied to private space"));
    }

    Ok(space)
}

async fn fetch_space_by_id_internal(
    app_state: &Arc<AppState>,
    space_id: &str,
) -> Result<SpaceResponse, AppError> {
    let clean_id = sanitize_record_key(space_id.strip_prefix("space:").unwrap_or(space_id))?;
    let query_id = format!("space:{}", clean_id);
    let mut response = app_state
        .db
        .client
        .query(&format!(
            "SELECT
                string::replace(type::string(id), 'space:', '') AS id,
                name, slug, description, avatar_url, is_public, is_deleted,
                (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
                settings, theme_config, member_count, document_count,
                created_at, updated_at,
                (IF created_by = NONE THEN '' ELSE type::string(created_by) END) AS created_by,
                (IF updated_by = NONE THEN '' ELSE type::string(updated_by) END) AS updated_by
             FROM ONLY type::record('{query_id}')
             WHERE is_deleted = false
             LIMIT 1"
        ))
        .await
        .map_err(AppError::Database)?;

    let spaces = response.take::<Vec<crate::models::space::Space>>(0)?;
    let space = spaces
        .into_iter()
        .next()
        .ok_or_else(|| AppError::not_found("Space not found"))?;

    let mut response = SpaceResponse::from(space);
    if let Ok(stats) = app_state.space_service.get_space_stats(&response.id).await {
        response.stats = Some(stats);
    }

    Ok(response)
}

fn sanitize_record_key(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
    {
        return Err(AppError::bad_request("Invalid record id"));
    }
    Ok(trimmed.to_string())
}

fn normalize_user_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let no_prefix = trimmed
        .strip_prefix("user:")
        .or_else(|| trimmed.strip_prefix("users:"))
        .unwrap_or(trimmed)
        .trim();
    no_prefix
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string()
}

fn is_space_owner(space_owner_id: &str, user_id: &str) -> bool {
    normalize_user_id(space_owner_id) == normalize_user_id(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_user_id_handles_common_wrappers() {
        assert_eq!(normalize_user_id("user:⟨abc⟩"), "abc");
        assert_eq!(normalize_user_id("users:abc"), "abc");
        assert_eq!(normalize_user_id(" `abc` "), "abc");
    }

    #[test]
    fn sanitize_record_key_rejects_unsafe_input() {
        assert!(sanitize_record_key("space-123").is_ok());
        assert!(sanitize_record_key("space:123").is_ok());
        assert!(sanitize_record_key("space:123'; DROP TABLE space; --").is_err());
    }
}
