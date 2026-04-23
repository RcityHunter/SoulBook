use chrono::Utc;
use std::sync::Arc;
use surrealdb::types::RecordId as Thing;
use validator::Validate;

use crate::{
    error::ApiError,
    models::document::{
        CreateDocumentRequest, Document, DocumentMetadata, DocumentTreeNode, UpdateDocumentRequest,
    },
    models::version::{CreateVersionRequest, VersionChangeType},
    services::{
        auth::AuthService, database::Database, search::SearchService, versions::VersionService,
    },
    utils::markdown::MarkdownProcessor,
};

#[derive(Clone)]
pub struct DocumentService {
    db: Arc<Database>,
    auth_service: Arc<AuthService>,
    markdown_processor: Arc<MarkdownProcessor>,
    search_service: Option<Arc<SearchService>>,
    version_service: Option<Arc<VersionService>>,
}

impl DocumentService {
    fn normalize_document_id(raw: &str) -> String {
        let trimmed = raw.trim();
        let no_prefix = trimmed.strip_prefix("document:").unwrap_or(trimmed).trim();
        no_prefix
            .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
            .to_string()
    }

    fn document_select_fields() -> &'static str {
        "string::replace(type::string(id), 'document:', '') AS id, \
         string::replace(type::string(space_id), 'space:', '') AS space_id, \
         title, slug, content, excerpt, is_public, status, \
         (IF parent_id = NONE THEN NONE ELSE string::replace(type::string(parent_id), 'document:', '') END) AS parent_id, \
         order_index, author_id, last_editor_id, view_count, word_count, reading_time, \
         (metadata ?? {}) AS metadata, updated_by, is_deleted, deleted_at, deleted_by, created_at, updated_at"
    }

    fn normalize_status(status: Option<&str>, is_public: bool) -> String {
        match status {
            Some("published") => "published".to_string(),
            Some("archived") => "archived".to_string(),
            Some("draft") => "draft".to_string(),
            _ if is_public => "published".to_string(),
            _ => "draft".to_string(),
        }
    }

    fn map_document_write_error(err: surrealdb::Error) -> ApiError {
        let msg = err.to_string();
        if msg.contains("document_space_slug_idx") || msg.contains("already contains") {
            return ApiError::Conflict("Document slug already exists in this space".to_string());
        }
        ApiError::DatabaseError(msg)
    }

    pub fn new(
        db: Arc<Database>,
        auth_service: Arc<AuthService>,
        markdown_processor: Arc<MarkdownProcessor>,
    ) -> Self {
        Self {
            db,
            auth_service,
            markdown_processor,
            search_service: None,
            version_service: None,
        }
    }

    pub fn with_search_service(mut self, search_service: Arc<SearchService>) -> Self {
        self.search_service = Some(search_service);
        self
    }

    pub fn with_version_service(mut self, version_service: Arc<VersionService>) -> Self {
        self.version_service = Some(version_service);
        self
    }

    pub async fn list_documents(
        &self,
        space_id: &str,
        query: crate::models::document::DocumentQuery,
        _user: Option<&crate::services::auth::User>,
    ) -> Result<serde_json::Value, ApiError> {
        use crate::models::document::{DocumentListItem, DocumentListResponse, DocumentQuery};

        // 提取实际的空间ID（去掉"space:"前缀，如果存在）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        // 注意：权限检查已经在路由层完成，这里不再重复检查

        // 构建查询条件
        let page = query.page.unwrap_or(1);
        let limit = query.limit.unwrap_or(20);
        let offset = (page - 1) * limit;

        let space_record = format!("space:{}", actual_space_id);

        // 查询文档列表
        let base_query = format!(
            "SELECT {} FROM document
             WHERE space_id = type::record($space_id)
             AND is_deleted = false
             ORDER BY order_index ASC, created_at DESC
             LIMIT $limit START $offset",
            Self::document_select_fields()
        );
        let mut documents_query = self.db.client.query(base_query);

        documents_query = documents_query
            .bind(("space_id", space_record.clone()))
            .bind(("limit", limit))
            .bind(("offset", offset));

        // 添加搜索条件
        if let Some(search) = &query.search {
            let search_query = format!(
                "SELECT {} FROM document
                 WHERE space_id = type::record($space_id)
                 AND is_deleted = false
                 AND (title CONTAINS $search OR content CONTAINS $search)
                 ORDER BY order_index ASC, created_at DESC
                 LIMIT $limit START $offset",
                Self::document_select_fields()
            );
            documents_query = self
                .db
                .client
                .query(search_query)
                .bind(("space_id", space_record.clone()))
                .bind(("search", search))
                .bind(("limit", limit))
                .bind(("offset", offset));
        }

        let mut result = documents_query
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let documents_raw: Vec<Document> = result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        // 转换为DocumentListItem
        let documents: Vec<DocumentListItem> =
            documents_raw.into_iter().map(|doc| doc.into()).collect();

        // 暂时使用简单的总数计算 - 由于分页问题，暂时查询所有文档获取总数
        let all_query = format!(
            "SELECT {} FROM document
             WHERE space_id = type::record($space_id)
             AND is_deleted = false",
            Self::document_select_fields()
        );
        let all_docs_query = self
            .db
            .client
            .query(all_query)
            .bind(("space_id", space_record.clone()));

        let mut all_docs_result = all_docs_query
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let all_docs: Vec<Document> = all_docs_result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let total = all_docs.len() as u32;

        let total_pages = (total + limit - 1) / limit;

        let response = DocumentListResponse {
            documents,
            total,
            page,
            limit,
            total_pages,
        };

        Ok(serde_json::to_value(response)
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Serialization error: {}", e)))?)
    }

    pub async fn create_document(
        &self,
        space_id: &str,
        author_id: &str,
        request: CreateDocumentRequest,
    ) -> Result<Document, ApiError> {
        request.validate()?;

        // 检查slug在空间内是否唯一
        if self.document_slug_exists(space_id, &request.slug).await? {
            return Err(ApiError::Conflict(
                "Document slug already exists in this space".to_string(),
            ));
        }

        // 提取space_id的实际ID部分（去掉"space:"前缀）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        // 验证父文档存在性（使用清理后的space_id）
        if let Some(parent_id) = &request.parent_id {
            self.verify_parent_document(actual_space_id, parent_id)
                .await?;
        }

        // 处理Markdown内容
        let content = request.content.as_deref().unwrap_or("");
        let processed = self.markdown_processor.process(content).await?;
        let is_public = request.is_public.unwrap_or(false);
        let status = Self::normalize_status(request.status.as_deref(), is_public);

        // 使用 SurrealQL 创建记录 - 不设置 metadata，让它使用默认值
        let query = if request.parent_id.is_some() {
            r#"
                CREATE document SET
                    space_id = type::record($space_id),
                    title = $title,
                    slug = $slug,
                    author_id = $author_id,
                    content = $content,
                    excerpt = $excerpt,
                    word_count = $word_count,
                    reading_time = $reading_time,
                    is_public = $is_public,
                    is_deleted = false,
                    status = $status,
                    view_count = 0,
                    parent_id = type::record($parent_id),
                    order_index = $order_index
            "#
        } else {
            r#"
                CREATE document SET
                    space_id = type::record($space_id),
                    title = $title,
                    slug = $slug,
                    author_id = $author_id,
                    content = $content,
                    excerpt = $excerpt,
                    word_count = $word_count,
                    reading_time = $reading_time,
                    is_public = $is_public,
                    is_deleted = false,
                    status = $status,
                    view_count = 0,
                    parent_id = NONE,
                    order_index = $order_index
            "#
        };

        let mut query_builder = self.db.client.query(query);
        query_builder = query_builder
            .bind(("space_id", format!("space:{}", actual_space_id)))
            .bind(("title", request.title.clone()))
            .bind(("slug", request.slug.clone()))
            .bind(("author_id", author_id.to_string()))
            .bind(("content", content.to_string()))
            .bind(("excerpt", processed.excerpt.clone()))
            .bind(("word_count", processed.word_count))
            .bind(("reading_time", processed.reading_time))
            .bind(("is_public", is_public))
            .bind(("status", status))
            .bind(("order_index", request.order_index.unwrap_or(0)));

        if let Some(parent_id) = &request.parent_id {
            let actual_parent_id = parent_id
                .strip_prefix("document:")
                .unwrap_or(parent_id)
                .trim_matches(|c| c == '⟨' || c == '⟩');
            query_builder =
                query_builder.bind(("parent_id", format!("document:{}", actual_parent_id)));
        }

        let mut result = query_builder
            .await
            .map_err(Self::map_document_write_error)?;

        // Consume the raw CREATE result (RecordId fields can't deserialize directly as String)
        let _: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        // Fetch the created document using the proper SELECT with string field transformations
        let created_document = self
            .get_document_by_slug(actual_space_id, &request.slug)
            .await
            .map_err(|e| {
                ApiError::InternalServerError(format!("Failed to retrieve created document: {}", e))
            })?;

        // 更新搜索索引
        if let Some(search_service) = &self.search_service {
            let _ = search_service
                .update_document_index(
                    &created_document.id.as_ref().unwrap().to_string(),
                    space_id,
                    &created_document.title,
                    &created_document.content,
                    &created_document.excerpt.clone().unwrap_or_default(),
                    Vec::new(), // 标签将在后续更新
                    author_id,
                    created_document.is_public,
                )
                .await;
        }

        // 创建初始版本
        if let Some(version_service) = &self.version_service {
            let version_request = CreateVersionRequest {
                title: created_document.title.clone(),
                content: created_document.content.clone(),
                summary: Some("Initial version".to_string()),
                change_type: VersionChangeType::Created,
            };

            let _ = version_service
                .create_version(
                    &created_document.id.as_ref().unwrap().to_string(),
                    author_id,
                    version_request,
                )
                .await;
        }

        Ok(created_document)
    }

    pub async fn get_document(&self, document_id: &str) -> Result<Document, ApiError> {
        // Reuse the normalized query path to avoid wrapper-level
        // select deserialization mismatches with Surreal tagged values.
        self.get_document_by_id(document_id).await
    }

    pub async fn update_document(
        &self,
        document_id: &str,
        editor_id: &str,
        request: UpdateDocumentRequest,
    ) -> Result<Document, ApiError> {
        request.validate()?;

        let mut document = self
            .get_document(document_id)
            .await
            .map_err(|e| ApiError::DatabaseError(format!("update_document/get_document: {}", e)))?;

        if let Some(title) = request.title {
            document.title = title;
        }

        if let Some(content) = request.content {
            let processed = self.markdown_processor.process(&content).await?;
            document.content = content;
            document.excerpt = Some(processed.excerpt);
            document.word_count = processed.word_count;
            document.reading_time = processed.reading_time;
        }

        if let Some(excerpt) = request.excerpt {
            document.excerpt = Some(excerpt);
        }

        if let Some(is_public) = request.is_public {
            document.is_public = is_public;
        }

        if let Some(status) = request.status {
            document.status = Some(Self::normalize_status(Some(&status), document.is_public));
            document.is_public = document.status.as_deref() == Some("published");
        } else if request.is_public.is_some() {
            document.status = Some(Self::normalize_status(
                document.status.as_deref(),
                document.is_public,
            ));
        }

        if let Some(metadata) = request.metadata {
            document.metadata = metadata;
        }

        document.updated_by = Some(editor_id.to_string());
        document.updated_at = Some(chrono::Utc::now());

        let clean_id = Self::normalize_document_id(document_id);
        let update_sql = format!(
            r#"
                UPDATE document:{} SET
                    title = $title,
                    content = $content,
                    excerpt = $excerpt,
                    is_public = $is_public,
                    status = $status,
                    metadata = $metadata,
                    word_count = $word_count,
                    reading_time = $reading_time,
                    updated_by = $updated_by,
                    updated_at = time::now()
                WHERE is_deleted = false
            "#,
            clean_id
        );

        let mut result = self
            .db
            .client
            .query(update_sql)
            .bind(("title", document.title.clone()))
            .bind(("content", document.content.clone()))
            .bind(("excerpt", document.excerpt.clone()))
            .bind(("is_public", document.is_public))
            .bind((
                "status",
                document
                    .status
                    .clone()
                    .unwrap_or_else(|| "draft".to_string()),
            ))
            .bind(("metadata", document.metadata.clone()))
            .bind(("word_count", document.word_count))
            .bind(("reading_time", document.reading_time))
            .bind(("updated_by", document.updated_by.clone()))
            .await
            .map_err(|e| ApiError::DatabaseError(format!("update_document/query: {}", e)))?;

        let _updated_rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(format!("update_document/take: {}", e)))?;

        // Re-read via normalized select-fields path to avoid deserialization mismatch
        // between raw UPDATE result and `Document` model.
        let updated_document = self
            .get_document_by_id(clean_id.as_str())
            .await
            .map_err(|e| ApiError::DatabaseError(format!("update_document/reread: {}", e)))?;

        // 更新搜索索引
        if let Some(search_service) = &self.search_service {
            let _ = search_service
                .update_document_index(
                    document_id,
                    &updated_document.space_id.to_string(),
                    &updated_document.title,
                    &updated_document.content,
                    &updated_document.excerpt.clone().unwrap_or_default(),
                    Vec::new(), // 标签将在后续更新
                    &updated_document.author_id,
                    updated_document.is_public,
                )
                .await;
        }

        // 创建新版本
        if let Some(version_service) = &self.version_service {
            let version_request = CreateVersionRequest {
                title: updated_document.title.clone(),
                content: updated_document.content.clone(),
                summary: Some("Document updated".to_string()),
                change_type: VersionChangeType::Updated,
            };

            let _ = version_service
                .create_version(clean_id.as_str(), editor_id, version_request)
                .await;
        }

        Ok(updated_document)
    }

    pub async fn delete_document(
        &self,
        document_id: &str,
        deleter_id: &str,
    ) -> Result<(), ApiError> {
        let clean_id = Self::normalize_document_id(document_id);
        // Soft-delete only specific fields; never write the whole record back,
        // otherwise datetime fields (e.g. created_at) may be coerced from strings.
        let update_sql = r#"
            UPDATE ONLY type::record($record_id) SET
                is_deleted = true,
                deleted_at = time::now(),
                deleted_by = $deleted_by,
                updated_by = $deleted_by,
                updated_at = time::now()
            WHERE is_deleted = false
        "#;

        let mut result = self
            .db
            .client
            .query(update_sql)
            .bind(("record_id", format!("document:{}", clean_id)))
            .bind(("deleted_by", deleter_id.to_string()))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let affected: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        if affected.is_empty() {
            return Err(ApiError::NotFound("Document not found".to_string()));
        }

        // 从搜索索引中删除
        if let Some(search_service) = &self.search_service {
            let _ = search_service.delete_index(clean_id.as_str()).await;
        }

        Ok(())
    }

    pub async fn get_space_documents(
        &self,
        space_id: &str,
        page: i64,
        per_page: i64,
    ) -> Result<Vec<Document>, ApiError> {
        let offset = (page - 1) * per_page;

        let query = format!(
            "SELECT {} FROM document
             WHERE space_id = type::record($space_id)
             AND is_deleted = false
             ORDER BY created_at DESC
             LIMIT $limit START $offset",
            Self::document_select_fields()
        );

        let documents: Vec<Document> = self
            .db
            .client
            .query(query)
            .bind(("space_id", format!("space:{}", space_id)))
            .bind(("limit", per_page))
            .bind(("offset", offset))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(documents)
    }

    pub async fn get_document_children(&self, parent_id: &str) -> Result<Vec<Document>, ApiError> {
        let query = format!(
            "SELECT {} FROM document
             WHERE parent_id = type::record($parent_id)
             AND is_deleted = false
             ORDER BY order_index ASC, created_at ASC",
            Self::document_select_fields()
        );

        let children: Vec<Document> = self
            .db
            .client
            .query(query)
            .bind(("parent_id", format!("document:{}", parent_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(children)
    }

    pub async fn get_document_children_by_id(
        &self,
        parent_id: &str,
    ) -> Result<Vec<Document>, ApiError> {
        let actual_id = Self::normalize_document_id(parent_id);
        self.get_document_children(&actual_id).await
    }

    pub async fn get_document_tree(
        &self,
        space_id: &str,
    ) -> Result<Vec<DocumentTreeNode>, ApiError> {
        tracing::debug!("Getting document tree for space_id: {}", space_id);

        // 提取实际的空间ID（去掉"space:"前缀，如果存在）
        let actual_space_id = if space_id.starts_with("space:") {
            space_id.strip_prefix("space:").unwrap()
        } else {
            space_id
        };

        // 获取空间内所有文档
        let query = format!(
            "SELECT {} FROM document
             WHERE space_id = type::record($space_id)
             AND is_deleted = false
             ORDER BY order_index ASC, created_at ASC",
            Self::document_select_fields()
        );

        let space_record = format!("space:{}", actual_space_id);
        tracing::debug!("Querying with space_record: {}", space_record);

        let all_documents: Vec<Document> = self
            .db
            .client
            .query(query)
            .bind(("space_id", space_record))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        tracing::debug!("Found {} documents in database", all_documents.len());

        tracing::debug!("Converted to {} Document objects", all_documents.len());

        // 构建文档映射
        let mut doc_map = std::collections::HashMap::new();
        let mut children_map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut root_docs = Vec::new();

        // 第一次遍历：创建所有节点并识别父子关系
        for doc in all_documents {
            if let Some(doc_id) = &doc.id {
                let node = DocumentTreeNode {
                    id: doc_id.clone(),
                    title: doc.title.clone(),
                    slug: doc.slug.clone(),
                    status: doc.status.clone(),
                    is_public: doc.is_public,
                    order_index: doc.order_index,
                    children: Vec::new(),
                };

                doc_map.insert(doc_id.clone(), node);

                if let Some(parent_id) = &doc.parent_id {
                    children_map
                        .entry(parent_id.clone())
                        .or_insert_with(Vec::new)
                        .push(doc_id.clone());
                } else {
                    root_docs.push(doc_id.clone());
                }
            }
        }

        // 第二次遍历：构建树结构
        fn build_tree_recursive(
            doc_id: &str,
            doc_map: &mut std::collections::HashMap<String, DocumentTreeNode>,
            children_map: &std::collections::HashMap<String, Vec<String>>,
        ) -> Option<DocumentTreeNode> {
            if let Some(mut node) = doc_map.remove(doc_id) {
                // 递归构建子节点
                if let Some(child_ids) = children_map.get(doc_id) {
                    for child_id in child_ids {
                        if let Some(child_node) =
                            build_tree_recursive(child_id, doc_map, children_map)
                        {
                            node.children.push(child_node);
                        }
                    }
                    // 按order_index排序子节点
                    node.children.sort_by_key(|child| child.order_index);
                }
                Some(node)
            } else {
                None
            }
        }

        // 构建最终的树结构
        let mut result = Vec::new();
        for root_id in &root_docs {
            if let Some(root_node) = build_tree_recursive(root_id, &mut doc_map, &children_map) {
                result.push(root_node);
            }
        }

        // 按order_index排序根节点
        result.sort_by_key(|node| node.order_index);

        tracing::debug!("Returning {} root documents in tree", result.len());
        Ok(result)
    }

    pub async fn move_document(
        &self,
        document_id: &str,
        new_parent_id: Option<String>,
        new_order_index: Option<i32>,
        mover_id: &str,
    ) -> Result<Document, ApiError> {
        let mut document = self.get_document(document_id).await?;

        if let Some(parent_id) = new_parent_id {
            self.verify_parent_document(&document.space_id.to_string(), &parent_id)
                .await?;
            document.parent_id = Some(parent_id);
        } else {
            document.parent_id = None;
        }

        if let Some(order_index) = new_order_index {
            document.order_index = order_index;
        }

        document.updated_by = Some(mover_id.to_string());
        document.updated_at = Some(chrono::Utc::now());

        let updated: Option<Document> = self
            .db
            .client
            .update(("document", document_id))
            .content(document.clone())
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        updated.ok_or_else(|| ApiError::InternalServerError("Failed to move document".to_string()))
    }

    pub async fn get_document_breadcrumbs(
        &self,
        document_id: &str,
    ) -> Result<Vec<Document>, ApiError> {
        let mut breadcrumbs = Vec::new();
        let mut current_id = Some(document_id.to_string());

        while let Some(id) = current_id {
            let document = self.get_document(&id).await?;
            current_id = document.parent_id.as_ref().map(|p| p.to_string());
            breadcrumbs.push(document);
        }

        breadcrumbs.reverse();
        Ok(breadcrumbs)
    }

    pub async fn get_document_breadcrumbs_by_id(
        &self,
        document_id: &str,
    ) -> Result<Vec<Document>, ApiError> {
        let actual_id = Self::normalize_document_id(document_id);

        let mut breadcrumbs = Vec::new();
        let mut current_id = Some(actual_id.to_string());

        while let Some(id) = current_id {
            let document = self.get_document_by_id(&id).await?;
            // 从parent_id中提取实际ID
            current_id = document
                .parent_id
                .as_ref()
                .map(|p| Self::normalize_document_id(p));
            breadcrumbs.push(document);
        }

        breadcrumbs.reverse();
        Ok(breadcrumbs)
    }

    pub async fn duplicate_document(
        &self,
        document_id: &str,
        new_title: Option<String>,
        new_slug: Option<String>,
        duplicator_id: &str,
    ) -> Result<Document, ApiError> {
        let original = self.get_document(document_id).await?;

        let title = new_title.unwrap_or_else(|| format!("{} (Copy)", original.title));
        let slug = new_slug.unwrap_or_else(|| format!("{}-copy", original.slug));

        // 检查新slug是否唯一
        if self
            .document_slug_exists(&original.space_id.to_string(), &slug)
            .await?
        {
            return Err(ApiError::Conflict("New slug already exists".to_string()));
        }

        let mut new_document = Document::new(
            original.space_id.clone(),
            title,
            slug,
            duplicator_id.to_string(),
        );
        new_document.content = original.content.clone();

        new_document.excerpt = original.excerpt.clone();
        new_document.word_count = original.word_count;
        new_document.reading_time = original.reading_time;
        new_document.is_public = original.is_public;

        let created: Vec<Document> = self
            .db
            .client
            .create("document")
            .content(new_document)
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let created_document = created.into_iter().next().ok_or_else(|| {
            ApiError::InternalServerError("Failed to duplicate document".to_string())
        })?;

        // 更新搜索索引
        if let Some(search_service) = &self.search_service {
            let _ = search_service
                .update_document_index(
                    &created_document.id.as_ref().unwrap().to_string(),
                    &created_document.space_id.to_string(),
                    &created_document.title,
                    &created_document.content,
                    &created_document.excerpt.clone().unwrap_or_default(),
                    Vec::new(),
                    duplicator_id,
                    created_document.is_public,
                )
                .await;
        }

        Ok(created_document)
    }

    pub async fn get_document_by_slug(
        &self,
        space_id: &str,
        slug: &str,
    ) -> Result<Document, ApiError> {
        let query = format!(
            "SELECT {} FROM document
             WHERE space_id = type::record($space_id)
             AND slug = $slug
             AND is_deleted = false",
            Self::document_select_fields()
        );

        let mut result = self
            .db
            .client
            .query(query)
            .bind(("space_id", format!("space:{}", space_id)))
            .bind(("slug", slug))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let documents: Vec<Document> = result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        documents
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::NotFound("Document not found".to_string()))
    }

    pub async fn get_document_by_id(&self, id: &str) -> Result<Document, ApiError> {
        let actual_id = Self::normalize_document_id(id);
        let record_id = format!("document:{}", actual_id);

        let query = format!(
            "SELECT {} FROM ONLY type::record($record_id)
             WHERE is_deleted = false",
            Self::document_select_fields()
        );

        let mut result = self
            .db
            .client
            .query(query)
            .bind(("record_id", record_id))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let raw = rows
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::NotFound("Document not found".to_string()))?;

        let document: Document = serde_json::from_value(raw.clone()).map_err(|e| {
            ApiError::DatabaseError(format!(
                "get_document_by_id/decode failed: {} | raw={}",
                e, raw
            ))
        })?;
        Ok(document)
    }

    async fn document_slug_exists(&self, space_id: &str, slug: &str) -> Result<bool, ApiError> {
        let query = "
            SELECT count() FROM document
            WHERE space_id = type::record($space_id)
            AND slug = $slug
            AND is_deleted = false
            GROUP ALL
        ";

        let result: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("space_id", format!("space:{}", space_id)))
            .bind(("slug", slug))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let count = result
            .first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        Ok(count > 0)
    }

    async fn verify_parent_document(
        &self,
        space_id: &str,
        parent_id: &str,
    ) -> Result<(), ApiError> {
        let query = "
            SELECT id FROM document
            WHERE id = type::record($parent_id)
            AND space_id = type::record($space_id)
            AND is_deleted = false
        ";

        let mut response = self
            .db
            .client
            .query(query)
            .bind(("parent_id", format!("document:{}", parent_id)))
            .bind(("space_id", format!("space:{}", space_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let result: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        if result.is_empty() {
            return Err(ApiError::NotFound("Parent document not found".to_string()));
        }

        Ok(())
    }
}
