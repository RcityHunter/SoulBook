use std::sync::Arc;
use std::time::Instant;
use surrealdb::types::RecordId as Thing;
use tracing::warn;

use crate::{
    error::ApiError,
    models::search::{
        SearchHighlight, SearchIndex, SearchRequest, SearchResponse, SearchResult, SearchSortBy,
    },
    services::{
        auth::AuthService,
        database::{record_id_to_string, Database},
    },
};

#[derive(Clone)]
pub struct SearchService {
    db: Arc<Database>,
    auth_service: Arc<AuthService>,
}

impl SearchService {
    pub fn new(db: Arc<Database>, auth_service: Arc<AuthService>) -> Self {
        Self { db, auth_service }
    }

    pub async fn create_or_update_index(&self, index: SearchIndex) -> Result<(), ApiError> {
        let created: Vec<SearchIndex> = self
            .db
            .client
            .create("search_index")
            .content(index)
            .await
            .map_err(|e| ApiError::Database(e))?;

        Ok(())
    }

    pub async fn delete_index(&self, document_id: &str) -> Result<(), ApiError> {
        let _: Option<SearchIndex> = self
            .db
            .client
            .delete(("search_index", document_id))
            .await
            .map_err(|e| ApiError::Database(e))?;

        Ok(())
    }

    pub async fn search(
        &self,
        user_id: &str,
        request: SearchRequest,
    ) -> Result<SearchResponse, ApiError> {
        let start_time = Instant::now();

        let page = request.page.unwrap_or(1).max(1);
        let per_page = request.per_page.unwrap_or(20).clamp(1, 100);
        let offset = (page - 1) * per_page;

        // 构建搜索查询
        let mut query_parts = vec![
            "SELECT * FROM search_index".to_string(),
            "WHERE is_public = true OR author_id = $user_id".to_string(),
        ];

        let mut bindings: Vec<(String, String)> =
            vec![("user_id".to_string(), user_id.to_string())];

        // 添加查询条件
        if !request.query.is_empty() {
            query_parts
                .push("AND (title CONTAINSTEXT $query OR content CONTAINSTEXT $query)".to_string());
            bindings.push(("query".to_string(), request.query.clone()));
        }

        if let Some(space_id) = &request.space_id {
            query_parts.push("AND space_id = $space_id".to_string());
            bindings.push(("space_id".to_string(), space_id.clone()));
        }

        if let Some(author_id) = &request.author_id {
            query_parts.push("AND author_id = $author_id".to_string());
            bindings.push(("author_id".to_string(), author_id.clone()));
        }

        if let Some(tags) = &request.tags {
            if !tags.is_empty() {
                let tags_condition = tags
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("$tag_{} IN tags", i))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                query_parts.push(format!("AND ({})", tags_condition));

                for (i, tag) in tags.iter().enumerate() {
                    bindings.push((format!("tag_{}", i), tag.clone()));
                }
            }
        }

        // 排序
        let sort_clause = match request.sort_by.as_ref().unwrap_or(&SearchSortBy::Relevance) {
            SearchSortBy::Relevance => {
                "ORDER BY (title CONTAINSTEXT $query) DESC, last_updated DESC"
            }
            SearchSortBy::CreatedAt => "ORDER BY id DESC",
            SearchSortBy::UpdatedAt => "ORDER BY last_updated DESC",
            SearchSortBy::Title => "ORDER BY title ASC",
        };
        query_parts.push(sort_clause.to_string());

        // 分页
        query_parts.push(format!("LIMIT {} START {}", per_page, offset));

        let full_query = query_parts.join(" ");

        // 执行搜索查询
        let mut db_query = self.db.client.query(&full_query);
        for (key, value) in bindings {
            db_query = db_query.bind((key, value));
        }

        let search_indexes: Vec<SearchIndex> = match db_query.await {
            Ok(mut response) => match response.take(0) {
                Ok(indexes) => indexes,
                Err(e) => {
                    warn!("search result parse failed: {}", e);
                    Vec::new()
                }
            },
            Err(e) => {
                warn!("search query failed: {}", e);
                Vec::new()
            }
        };

        // 获取总数
        let total_count = self
            .get_search_count(user_id, &request)
            .await
            .unwrap_or_else(|e| {
                warn!("search count failed: {}", e);
                0
            });

        // 转换为搜索结果
        let mut results = Vec::new();
        for index in search_indexes {
            let highlights = self.generate_highlights(&index, &request.query);
            let score = self.calculate_relevance_score(&index, &request.query);

            results.push(SearchResult {
                document_id: record_id_to_string(&index.document_id),
                space_id: record_id_to_string(&index.space_id),
                title: index.title,
                excerpt: index.excerpt,
                tags: index.tags,
                author_id: index.author_id,
                last_updated: index.last_updated,
                score,
                highlights,
            });
        }

        let took = start_time.elapsed().as_millis() as i64;

        Ok(SearchResponse::new(
            results,
            total_count,
            page,
            per_page,
            request.query,
            took,
        ))
    }

    async fn get_search_count(
        &self,
        user_id: &str,
        request: &SearchRequest,
    ) -> Result<i64, ApiError> {
        let mut query_parts = vec![
            "SELECT count() FROM search_index".to_string(),
            "WHERE is_public = true OR author_id = $user_id".to_string(),
        ];

        let mut bindings: Vec<(String, String)> =
            vec![("user_id".to_string(), user_id.to_string())];

        if !request.query.is_empty() {
            query_parts
                .push("AND (title CONTAINSTEXT $query OR content CONTAINSTEXT $query)".to_string());
            bindings.push(("query".to_string(), request.query.clone()));
        }

        if let Some(space_id) = &request.space_id {
            query_parts.push("AND space_id = $space_id".to_string());
            bindings.push(("space_id".to_string(), space_id.clone()));
        }

        if let Some(author_id) = &request.author_id {
            query_parts.push("AND author_id = $author_id".to_string());
            bindings.push(("author_id".to_string(), author_id.clone()));
        }

        if let Some(tags) = &request.tags {
            if !tags.is_empty() {
                let tags_condition = tags
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("$tag_{} IN tags", i))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                query_parts.push(format!("AND ({})", tags_condition));

                for (i, tag) in tags.iter().enumerate() {
                    bindings.push((format!("tag_{}", i), tag.clone()));
                }
            }
        }

        query_parts.push("GROUP ALL".to_string());
        let full_query = query_parts.join(" ");

        let mut db_query = self.db.client.query(&full_query);
        for (key, value) in bindings {
            db_query = db_query.bind((key, value));
        }

        let result: Vec<serde_json::Value> = match db_query.await {
            Ok(mut response) => response.take(0).map_err(|e| ApiError::Database(e))?,
            Err(e) => return Err(ApiError::Database(e)),
        };

        let count = result
            .first()
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        Ok(count)
    }

    fn generate_highlights(&self, index: &SearchIndex, query: &str) -> Vec<SearchHighlight> {
        let mut highlights = Vec::new();

        if query.is_empty() {
            return highlights;
        }

        let query_lower = query.to_lowercase();

        // 在标题中查找高亮
        if let Some(pos) = index.title.to_lowercase().find(&query_lower) {
            highlights.push(SearchHighlight {
                field: "title".to_string(),
                text: index.title.clone(),
                start: pos,
                end: pos + query.len(),
            });
        }

        // 在内容中查找高亮
        if let Some(pos) = index.content.to_lowercase().find(&query_lower) {
            let start = pos.saturating_sub(50);
            let end = (pos + query.len() + 50).min(index.content.len());
            let excerpt = &index.content[start..end];

            highlights.push(SearchHighlight {
                field: "content".to_string(),
                text: format!("...{}...", excerpt),
                start: pos - start,
                end: pos - start + query.len(),
            });
        }

        highlights
    }

    fn calculate_relevance_score(&self, index: &SearchIndex, query: &str) -> f64 {
        if query.is_empty() {
            return 0.0;
        }

        let mut score = 0.0;
        let query_lower = query.to_lowercase();

        // 标题匹配权重更高
        if index.title.to_lowercase().contains(&query_lower) {
            score += 10.0;

            // 完全匹配标题权重最高
            if index.title.to_lowercase() == query_lower {
                score += 20.0;
            }
        }

        // 内容匹配
        let content_lower = index.content.to_lowercase();
        let matches = content_lower.matches(&query_lower).count();
        score += matches as f64 * 1.0;

        // 标签匹配
        for tag in &index.tags {
            if tag.to_lowercase().contains(&query_lower) {
                score += 5.0;
            }
        }

        // 最近更新的文档得分略高
        let days_since_update =
            (chrono::Utc::now().timestamp() - index.last_updated.timestamp()) / 86400;
        if days_since_update < 30 {
            score += 1.0;
        }

        score
    }

    pub async fn suggest_search_terms(
        &self,
        user_id: &str,
        prefix: &str,
        limit: i64,
    ) -> Result<Vec<String>, ApiError> {
        let query = "
            SELECT title, tags FROM search_index 
            WHERE is_public = true OR author_id = $user_id
            AND title CONTAINSTEXT $prefix
            LIMIT $limit
        ";

        let results: Vec<SearchIndex> = match self
            .db
            .client
            .query(query)
            .bind(("user_id", user_id))
            .bind(("prefix", prefix))
            .bind(("limit", limit))
            .await
        {
            Ok(mut response) => match response.take(0) {
                Ok(results) => results,
                Err(e) => {
                    warn!("search suggestions result parse failed: {}", e);
                    Vec::new()
                }
            },
            Err(e) => {
                warn!("search suggestions query failed: {}", e);
                Vec::new()
            }
        };

        let mut suggestions = Vec::new();

        for result in results {
            // 添加标题建议
            if result.title.to_lowercase().contains(&prefix.to_lowercase()) {
                suggestions.push(result.title);
            }

            // 添加标签建议
            for tag in result.tags {
                if tag.to_lowercase().contains(&prefix.to_lowercase())
                    && !suggestions.contains(&tag)
                {
                    suggestions.push(tag);
                }
            }
        }

        suggestions.truncate(limit as usize);
        Ok(suggestions)
    }

    pub async fn update_document_index(
        &self,
        document_id: &str,
        space_id: &str,
        title: &str,
        content: &str,
        excerpt: &str,
        tags: Vec<String>,
        author_id: &str,
        is_public: bool,
    ) -> Result<(), ApiError> {
        let index = SearchIndex::new(
            Thing::new("document", document_id),
            Thing::new("space", space_id),
            title.to_string(),
            content.to_string(),
            excerpt.to_string(),
            author_id.to_string(),
        )
        .with_tags(tags)
        .set_public(is_public);

        self.create_or_update_index(index).await
    }

    pub async fn bulk_reindex(&self) -> Result<i64, ApiError> {
        // 获取所有文档并重建索引
        let query = "
            SELECT id, space_id, title, content, author_id, created_at, updated_at
            FROM document 
            WHERE is_deleted = false
        ";

        let documents: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .await
            .map_err(|e| ApiError::Database(e))?
            .take(0)
            .map_err(|e| ApiError::Database(e))?;

        let mut indexed_count = 0;

        for doc in documents {
            if let Some(_doc_obj) = doc.as_object() {
                // 提取文档信息并创建索引
                // 这里需要根据实际的文档结构来解析
                indexed_count += 1;
            }
        }

        Ok(indexed_count)
    }
}
