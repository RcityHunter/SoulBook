use crate::{
    config::Config,
    services::database::Database,
    services::{
        auth::AuthService, comments::CommentService, documents::DocumentService,
        file_upload::FileUploadService, publication::PublicationService, search::SearchService,
        space_member::SpaceMemberService, spaces::SpaceService, tags::TagService,
        versions::VersionService, workspace::WorkspaceService,
    },
};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Config,
    pub auth_service: Arc<AuthService>,
    pub space_service: Arc<SpaceService>,
    pub space_member_service: Arc<SpaceMemberService>,
    pub workspace_service: Arc<WorkspaceService>,
    pub file_upload_service: Arc<FileUploadService>,
    pub tag_service: Arc<TagService>,
    pub document_service: Arc<DocumentService>,
    pub comment_service: Arc<CommentService>,
    pub publication_service: Arc<PublicationService>,
    pub search_service: Arc<SearchService>,
    pub version_service: Arc<VersionService>,
}
