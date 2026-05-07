use crate::models::document::Document;
use crate::services::database::Database;
use base64::{engine::general_purpose, Engine as _};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepoRef {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    pub path: String,
    pub content: String,
}

pub fn parse_github_repo_url(url: &str) -> Result<GitHubRepoRef, String> {
    let trimmed = url.trim().trim_end_matches(".git").trim_end_matches('/');
    let path = if let Some(path) = trimmed.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = trimmed.strip_prefix("git@github.com:") {
        path
    } else {
        return Err("Only github.com repository URLs are supported".to_string());
    };

    let mut parts = path.split('/');
    let owner = parts.next().unwrap_or_default().trim();
    let repo = parts.next().unwrap_or_default().trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Err("GitHub repository URL must be owner/repo".to_string());
    }

    Ok(GitHubRepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

pub fn normalize_path_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

pub fn export_documents_to_markdown(prefix: &str, documents: &[Document]) -> Vec<ExportedFile> {
    let prefix = normalize_path_prefix(prefix);
    documents
        .iter()
        .filter(|doc| !doc.is_deleted)
        .map(|doc| {
            let slug = if doc.slug.trim().is_empty() {
                doc.id.as_deref().unwrap_or("untitled")
            } else {
                doc.slug.as_str()
            };
            ExportedFile {
                path: format!("{prefix}{slug}.md"),
                content: format_document_markdown(doc),
            }
        })
        .collect()
}

fn format_document_markdown(doc: &Document) -> String {
    let mut output = format!("# {}\n\n", doc.title.trim());
    if let Some(excerpt) = doc.excerpt.as_deref().filter(|value| !value.trim().is_empty()) {
        output.push_str("> ");
        output.push_str(excerpt.trim());
        output.push_str("\n\n");
    }
    output.push_str(doc.content.trim());
    output.push('\n');
    output
}

#[derive(Clone)]
pub struct GitSyncService {
    db: Arc<Database>,
    client: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct GitSyncInput {
    pub github_url: String,
    pub branch: String,
    pub path_prefix: String,
    pub space_id: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitSyncResult {
    pub files_changed: usize,
    pub commit_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubContentResponse {
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubPutResponse {
    commit: GitHubCommit,
}

#[derive(Debug, Deserialize)]
struct GitHubCommit {
    sha: String,
}

#[derive(Debug, Serialize)]
struct GitHubPutRequest {
    message: String,
    content: String,
    branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<String>,
}

impl GitSyncService {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            client: reqwest::Client::new(),
        }
    }

    pub async fn sync_space_to_github(&self, input: GitSyncInput) -> Result<GitSyncResult, String> {
        let repo = parse_github_repo_url(&input.github_url)?;
        let documents = self.load_space_documents(&input.space_id).await?;
        let files = export_documents_to_markdown(&input.path_prefix, &documents);
        if files.is_empty() {
            return Ok(GitSyncResult {
                files_changed: 0,
                commit_sha: None,
            });
        }

        let mut last_commit_sha = None;
        for file in files.iter() {
            let existing_sha = self
                .get_existing_file_sha(&repo, &file.path, &input.branch, &input.token)
                .await?;
            let commit_sha = self
                .put_file(&repo, file, &input.branch, &input.token, existing_sha)
                .await?;
            last_commit_sha = Some(commit_sha);
        }

        Ok(GitSyncResult {
            files_changed: files.len(),
            commit_sha: last_commit_sha,
        })
    }

    async fn load_space_documents(&self, space_id: &str) -> Result<Vec<Document>, String> {
        let mut result = self
            .db
            .client
            .query(
                "SELECT
                    string::replace(type::string(id), 'document:', '') AS id,
                    string::replace(type::string(space_id), 'space:', '') AS space_id,
                    title, slug, content, excerpt, is_public, status,
                    (IF parent_id = NONE THEN NONE ELSE string::replace(type::string(parent_id), 'document:', '') END) AS parent_id,
                    order_index,
                    (IF author_id = NONE THEN '' ELSE type::string(author_id) END) AS author_id,
                    (IF last_editor_id = NONE THEN NONE ELSE type::string(last_editor_id) END) AS last_editor_id,
                    view_count, word_count, reading_time,
                    (metadata ?? {}) AS metadata,
                    (IF updated_by = NONE THEN NONE ELSE type::string(updated_by) END) AS updated_by,
                    is_deleted, deleted_at,
                    (IF deleted_by = NONE THEN NONE ELSE type::string(deleted_by) END) AS deleted_by,
                    created_at, updated_at
                 FROM document
                 WHERE space_id = type::record($space_id)
                   AND is_deleted = false
                 ORDER BY order_index ASC, created_at ASC",
            )
            .bind(("space_id", format!("space:{}", normalize_record_key(space_id))))
            .await
            .map_err(|e| e.to_string())?;

        result.take::<Vec<Document>>(0).map_err(|e| e.to_string())
    }

    async fn get_existing_file_sha(
        &self,
        repo: &GitHubRepoRef,
        path: &str,
        branch: &str,
        token: &str,
    ) -> Result<Option<String>, String> {
        let url = github_contents_url(repo, path);
        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "SoulBook")
            .query(&[("ref", branch)])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!(
                "GitHub get file failed: HTTP {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
        }

        let body = response
            .json::<GitHubContentResponse>()
            .await
            .map_err(|e| e.to_string())?;
        Ok(body.sha)
    }

    async fn put_file(
        &self,
        repo: &GitHubRepoRef,
        file: &ExportedFile,
        branch: &str,
        token: &str,
        existing_sha: Option<String>,
    ) -> Result<String, String> {
        let request = GitHubPutRequest {
            message: format!("sync: update {}", file.path),
            content: general_purpose::STANDARD.encode(file.content.as_bytes()),
            branch: branch.to_string(),
            sha: existing_sha,
        };

        let response = self
            .client
            .put(github_contents_url(repo, &file.path))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "SoulBook")
            .json(&request)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            return Err(format!(
                "GitHub put file failed: HTTP {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
        }

        let body = response
            .json::<GitHubPutResponse>()
            .await
            .map_err(|e| e.to_string())?;
        Ok(body.commit.sha)
    }
}

fn normalize_record_key(value: &str) -> String {
    value
        .trim()
        .strip_prefix("space:")
        .unwrap_or(value.trim())
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`')
        .to_string()
}

fn github_contents_url(repo: &GitHubRepoRef, path: &str) -> String {
    format!(
        "https://api.github.com/repos/{}/{}/contents/{}",
        repo.owner, repo.repo, path
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::document::{DocumentMetadata, SeoMetadata};

    #[test]
    fn parses_https_and_ssh_github_urls() {
        assert_eq!(
            parse_github_repo_url("https://github.com/3W1shes/awesome-design-md.git").unwrap(),
            GitHubRepoRef {
                owner: "3W1shes".to_string(),
                repo: "awesome-design-md".to_string()
            }
        );
        assert_eq!(
            parse_github_repo_url("git@github.com:RcityHunter/SoulBook.git").unwrap(),
            GitHubRepoRef {
                owner: "RcityHunter".to_string(),
                repo: "SoulBook".to_string()
            }
        );
    }

    #[test]
    fn normalizes_path_prefix() {
        assert_eq!(normalize_path_prefix("docs"), "docs/");
        assert_eq!(normalize_path_prefix("/docs/"), "docs/");
        assert_eq!(normalize_path_prefix(""), "");
    }

    #[test]
    fn exports_documents_to_markdown_files() {
        let docs = vec![Document {
            id: Some("doc1".to_string()),
            space_id: "space1".to_string(),
            title: "Hello".to_string(),
            slug: "hello".to_string(),
            content: "Body".to_string(),
            excerpt: Some("Summary".to_string()),
            is_public: false,
            status: Some("draft".to_string()),
            parent_id: None,
            order_index: 0,
            author_id: "user1".to_string(),
            last_editor_id: None,
            view_count: 0,
            word_count: 1,
            reading_time: 1,
            metadata: DocumentMetadata {
                tags: vec![],
                custom_fields: Default::default(),
                seo: SeoMetadata::default(),
                reading_time: None,
            },
            updated_by: None,
            is_deleted: false,
            deleted_at: None,
            deleted_by: None,
            created_at: None,
            updated_at: None,
        }];

        let files = export_documents_to_markdown("/docs/", &docs);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "docs/hello.md");
        assert_eq!(files[0].content, "# Hello\n\n> Summary\n\nBody\n");
    }
}
