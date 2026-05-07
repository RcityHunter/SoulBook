use std::sync::Arc;
use surrealdb::types::RecordId as Thing;
use validator::Validate;

use crate::{
    error::ApiError,
    models::tag::{CreateTagRequest, DocumentTag, Tag, TagDocumentRequest, UpdateTagRequest},
    services::{
        auth::AuthService,
        database::{record_id_to_string, Database},
    },
};

#[derive(Clone)]
pub struct TagService {
    db: Arc<Database>,
    auth_service: Arc<AuthService>,
}

impl TagService {
    pub fn new(db: Arc<Database>, auth_service: Arc<AuthService>) -> Self {
        Self { db, auth_service }
    }

    pub async fn create_tag(
        &self,
        creator_id: &str,
        request: CreateTagRequest,
    ) -> Result<Tag, ApiError> {
        request.validate()?;

        // 检查标签名在空间内是否唯一
        if self
            .tag_exists_in_space(&request.space_id, &request.name)
            .await?
        {
            return Err(ApiError::Conflict(
                "Tag name already exists in this space".to_string(),
            ));
        }

        let tag = Tag::new(request.name.clone(), request.color.clone(), creator_id.to_string());
        let create_sql = create_tag_sql(&request.space_id);
        let mut create_query = self
            .db
            .client
            .query(create_sql)
            .bind(("name", tag.name.clone()))
            .bind(("slug", tag.slug.clone()))
            .bind(("description", request.description.unwrap_or_default()))
            .bind(("color", tag.color.clone()))
            .bind(("created_by", creator_id.to_string()));

        if let Some(space_id) = &request.space_id {
            create_query = create_query.bind(("space_id", Thing::new("space", space_id.as_str())));
        }

        let mut created_response = create_query
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let created: Vec<serde_json::Value> = created_response
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let tag_id = created
            .first()
            .and_then(extract_created_tag_id)
            .ok_or_else(|| ApiError::InternalServerError("Failed to create tag".to_string()))?;

        self.get_tag(&tag_id).await
    }

    pub async fn get_tag(&self, tag_id: &str) -> Result<Tag, ApiError> {
        let tag: Option<Tag> = self
            .db
            .client
            .select(("tag", tag_id))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        tag.ok_or_else(|| ApiError::NotFound("Tag not found".to_string()))
    }

    pub async fn update_tag(
        &self,
        tag_id: &str,
        updater_id: &str,
        request: UpdateTagRequest,
    ) -> Result<Tag, ApiError> {
        request.validate()?;

        let mut tag = self.get_tag(tag_id).await?;

        if let Some(name) = request.name {
            // 检查新名称是否与其他标签冲突
            if name != tag.name
                && self
                    .tag_exists_in_space(&tag.space_id.as_ref().map(record_id_to_string), &name)
                    .await?
            {
                return Err(ApiError::Conflict(
                    "Tag name already exists in this space".to_string(),
                ));
            }
            tag.name = name;
            tag.slug = Tag::generate_slug(&tag.name);
        }

        if let Some(description) = request.description {
            tag.description = Some(description);
        }

        if let Some(color) = request.color {
            tag.color = color;
        }

        tag.updated_at = surrealdb::types::Datetime::default();

        let updated: Option<Tag> = self
            .db
            .client
            .update(("tag", tag_id))
            .content(tag)
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        updated.ok_or_else(|| ApiError::InternalServerError("Failed to update tag".to_string()))
    }

    pub async fn delete_tag(&self, tag_id: &str) -> Result<(), ApiError> {
        // 首先删除所有与此标签关联的文档标签关系
        let query = "DELETE document_tag WHERE tag_id = $tag_id";
        let _: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("tag_id", Thing::new("tag", tag_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        // 删除标签
        let _: Option<Tag> = self
            .db
            .client
            .delete(("tag", tag_id))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    pub async fn get_tags_by_space(
        &self,
        space_id: Option<&str>,
        page: i64,
        per_page: i64,
    ) -> Result<Vec<Tag>, ApiError> {
        let offset = (page - 1) * per_page;

        let query = if let Some(space_id) = space_id {
            "SELECT * FROM tag WHERE space_id = $space_id ORDER BY usage_count DESC, name ASC LIMIT $limit START $offset"
        } else {
            "SELECT * FROM tag WHERE space_id IS NULL ORDER BY usage_count DESC, name ASC LIMIT $limit START $offset"
        };

        let mut db_query = self.db.client.query(query);

        if let Some(space_id) = space_id {
            db_query = db_query.bind(("space_id", Thing::new("space", space_id)));
        }

        let tags: Vec<Tag> = db_query
            .bind(("limit", per_page))
            .bind(("offset", offset))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(tags)
    }

    pub async fn get_popular_tags(
        &self,
        space_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Tag>, ApiError> {
        let query = if let Some(space_id) = space_id {
            "SELECT * FROM tag WHERE space_id = $space_id AND usage_count > 0 ORDER BY usage_count DESC LIMIT $limit"
        } else {
            "SELECT * FROM tag WHERE space_id IS NULL AND usage_count > 0 ORDER BY usage_count DESC LIMIT $limit"
        };

        let mut db_query = self.db.client.query(query);

        if let Some(space_id) = space_id {
            db_query = db_query.bind(("space_id", Thing::new("space", space_id)));
        }

        let tags: Vec<Tag> = db_query
            .bind(("limit", limit))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(tags)
    }

    pub async fn search_tags(
        &self,
        space_id: Option<&str>,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Tag>, ApiError> {
        let search_query = if let Some(space_id) = space_id {
            "SELECT * FROM tag WHERE space_id = $space_id AND (name CONTAINSTEXT $query OR description CONTAINSTEXT $query) ORDER BY usage_count DESC LIMIT $limit"
        } else {
            "SELECT * FROM tag WHERE space_id IS NULL AND (name CONTAINSTEXT $query OR description CONTAINSTEXT $query) ORDER BY usage_count DESC LIMIT $limit"
        };

        let mut db_query = self.db.client.query(search_query);

        if let Some(space_id) = space_id {
            db_query = db_query.bind(("space_id", Thing::new("space", space_id)));
        }

        let tags: Vec<Tag> = db_query
            .bind(("query", query))
            .bind(("limit", limit))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(tags)
    }

    // 文档标签关联管理
    pub async fn tag_document(
        &self,
        tagger_id: &str,
        request: TagDocumentRequest,
    ) -> Result<Vec<DocumentTag>, ApiError> {
        let document_thing = Thing::new("document", request.document_id.as_str());
        let mut created_tags = Vec::new();

        for tag_id in request.tag_ids {
            // 检查关联是否已存在
            if !self
                .document_tag_exists(&request.document_id, &tag_id)
                .await?
            {
                let tag_thing = Thing::new("tag", tag_id.as_str());

                let document_tag = DocumentTag::new(
                    document_thing.clone(),
                    tag_thing.clone(),
                    tagger_id.to_string(),
                );

                let created: Vec<DocumentTag> = self
                    .db
                    .client
                    .create("document_tag")
                    .content(document_tag)
                    .await
                    .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

                if let Some(created_tag) = created.into_iter().next() {
                    created_tags.push(created_tag);

                    // 增加标签使用计数
                    self.increment_tag_usage(&tag_id).await?;
                }
            }
        }

        Ok(created_tags)
    }

    pub async fn untag_document(&self, document_id: &str, tag_id: &str) -> Result<(), ApiError> {
        let query = "DELETE document_tag WHERE document_id = $document_id AND tag_id = $tag_id";

        let _: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("document_id", Thing::new("document", document_id)))
            .bind(("tag_id", Thing::new("tag", tag_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        // 减少标签使用计数
        self.decrement_tag_usage(tag_id).await?;

        Ok(())
    }

    pub async fn get_document_tags(&self, document_id: &str) -> Result<Vec<Tag>, ApiError> {
        let query = "
            SELECT tag.* FROM tag, document_tag 
            WHERE document_tag.document_id = $document_id 
            AND document_tag.tag_id = tag.id
            ORDER BY tag.name ASC
        ";

        let tags: Vec<Tag> = self
            .db
            .client
            .query(query)
            .bind(("document_id", Thing::new("document", document_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(tags)
    }

    pub async fn get_documents_by_tag(
        &self,
        tag_id: &str,
        page: i64,
        per_page: i64,
    ) -> Result<Vec<String>, ApiError> {
        let offset = (page - 1) * per_page;

        let query = "
            SELECT document_tag.document_id FROM document_tag 
            WHERE document_tag.tag_id = $tag_id
            ORDER BY document_tag.tagged_at DESC
            LIMIT $limit START $offset
        ";

        let results: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("tag_id", Thing::new("tag", tag_id)))
            .bind(("limit", per_page))
            .bind(("offset", offset))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let document_ids: Vec<String> = results
            .into_iter()
            .filter_map(|v| {
                v.get("document_id")
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        Ok(document_ids)
    }

    pub async fn get_tag_statistics(
        &self,
        space_id: Option<&str>,
    ) -> Result<TagStatistics, ApiError> {
        let total_query = if let Some(space_id) = space_id {
            "SELECT count() as total FROM tag WHERE space_id = $space_id GROUP ALL"
        } else {
            "SELECT count() as total FROM tag WHERE space_id IS NULL GROUP ALL"
        };

        let mut db_query = self.db.client.query(total_query);
        if let Some(space_id) = space_id {
            db_query = db_query.bind(("space_id", Thing::new("space", space_id)));
        }

        let total_result: Vec<serde_json::Value> = db_query
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let total_tags = total_result
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let used_query = if let Some(space_id) = space_id {
            "SELECT count() as used FROM tag WHERE space_id = $space_id AND usage_count > 0 GROUP ALL"
        } else {
            "SELECT count() as used FROM tag WHERE space_id IS NULL AND usage_count > 0 GROUP ALL"
        };

        let mut db_query = self.db.client.query(used_query);
        if let Some(space_id) = space_id {
            db_query = db_query.bind(("space_id", Thing::new("space", space_id)));
        }

        let used_result: Vec<serde_json::Value> = db_query
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        let used_tags = used_result
            .first()
            .and_then(|v| v.get("used"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        Ok(TagStatistics {
            total_tags,
            used_tags,
            unused_tags: total_tags - used_tags,
            most_used_tags: self.get_popular_tags(space_id, 5).await?,
        })
    }

    // 私有辅助方法
    async fn tag_exists_in_space(
        &self,
        space_id: &Option<String>,
        name: &str,
    ) -> Result<bool, ApiError> {
        let query = if let Some(space_id) = space_id {
            "SELECT count() FROM tag WHERE space_id = $space_id AND name = $name GROUP ALL"
        } else {
            "SELECT count() FROM tag WHERE space_id IS NULL AND name = $name GROUP ALL"
        };

        let mut db_query = self.db.client.query(query);
        if let Some(space_id) = space_id {
            db_query = db_query.bind(("space_id", Thing::new("space", space_id.as_str())));
        }

        let result: Vec<serde_json::Value> = db_query
            .bind(("name", name))
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

    async fn document_tag_exists(&self, document_id: &str, tag_id: &str) -> Result<bool, ApiError> {
        let query = "SELECT count() FROM document_tag WHERE document_id = $document_id AND tag_id = $tag_id GROUP ALL";

        let result: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("document_id", Thing::new("document", document_id)))
            .bind(("tag_id", Thing::new("tag", tag_id)))
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

    async fn increment_tag_usage(&self, tag_id: &str) -> Result<(), ApiError> {
        let query = "UPDATE tag SET usage_count += 1 WHERE id = $tag_id";

        let _: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("tag_id", Thing::new("tag", tag_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn decrement_tag_usage(&self, tag_id: &str) -> Result<(), ApiError> {
        let query = "UPDATE tag SET usage_count = math::max(usage_count - 1, 0) WHERE id = $tag_id";

        let _: Vec<serde_json::Value> = self
            .db
            .client
            .query(query)
            .bind(("tag_id", Thing::new("tag", tag_id)))
            .await
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?
            .take(0)
            .map_err(|e| ApiError::DatabaseError(e.to_string()))?;

        Ok(())
    }
}

fn create_tag_sql(space_id: &Option<String>) -> &'static str {
    if space_id.is_some() {
        "CREATE tag CONTENT {
            name: $name,
            slug: $slug,
            description: $description,
            color: $color,
            space_id: $space_id,
            usage_count: 0,
            created_by: $created_by,
            created_at: time::now(),
            updated_at: time::now()
        }"
    } else {
        "CREATE tag CONTENT {
            name: $name,
            slug: $slug,
            description: $description,
            color: $color,
            usage_count: 0,
            created_by: $created_by,
            created_at: time::now(),
            updated_at: time::now()
        }"
    }
}

fn extract_created_tag_id(value: &serde_json::Value) -> Option<String> {
    let raw_id = value.get("id")?.as_str()?;

    Some(raw_id.strip_prefix("tag:").unwrap_or(raw_id).to_string())
}

#[derive(Debug, serde::Serialize)]
pub struct TagStatistics {
    pub total_tags: i64,
    pub used_tags: i64,
    pub unused_tags: i64,
    pub most_used_tags: Vec<Tag>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_tag_sql_omits_record_id_and_optional_none_space() {
        let sql = create_tag_sql(&None);

        assert!(sql.contains("CREATE tag CONTENT"));
        assert!(!sql.contains("\n            id:"));
        assert!(!sql.contains("space_id: $space_id"));
    }

    #[test]
    fn create_tag_sql_includes_space_when_provided() {
        let sql = create_tag_sql(&Some("space-id".to_string()));

        assert!(sql.contains("space_id: $space_id"));
        assert!(!sql.contains("\n            id:"));
    }

    #[test]
    fn extract_created_tag_id_accepts_surreal_string_id() {
        let value = serde_json::json!({ "id": "tag:abc123" });

        assert_eq!(extract_created_tag_id(&value), Some("abc123".to_string()));
    }
}
