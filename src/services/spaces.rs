use crate::config::Config;
use crate::error::{AppError, Result};
use crate::models::space::{
    CreateSpaceRequest, Space, SpaceListQuery, SpaceListResponse, SpaceResponse, SpaceStats,
    UpdateSpaceRequest,
};
use crate::services::auth::User;
use crate::services::database::{record_id_key, Database};
use serde_json::Value;
use std::sync::Arc;
use surrealdb::types::RecordId as Thing;
use tracing::{debug, error, info, warn};
use validator::Validate;

pub struct SpaceService {
    db: Arc<Database>,
}

impl SpaceService {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 创建新的文档空间
    pub async fn create_space(
        &self,
        request: CreateSpaceRequest,
        user: &User,
    ) -> Result<SpaceResponse> {
        fn map_create_space_db_error(err: surrealdb::Error) -> AppError {
            let msg = err.to_string();
            if msg.contains("space_slug_unique_idx") || msg.contains("already contains") {
                return AppError::Conflict(
                    "Space slug already exists globally. Please choose a different slug."
                        .to_string(),
                );
            }
            AppError::Database(err)
        }

        // 验证输入
        request
            .validate()
            .map_err(|e| AppError::Validation(e.to_string()))?;

        // 检查slug是否已存在（全局唯一）
        if self.slug_exists(&request.slug).await? {
            return Err(AppError::Conflict(
                "Space slug already exists globally. Please choose a different slug.".to_string(),
            ));
        }

        // 创建空间对象
        let mut space = Space::new(request.name, request.slug.clone(), user.id.clone());

        if let Some(description) = request.description {
            space.description = Some(description);
        }

        if let Some(avatar_url) = request.avatar_url {
            space.avatar_url = Some(avatar_url);
        }

        if let Some(is_public) = request.is_public {
            space.is_public = is_public;
        }

        if let Some(settings) = request.settings {
            space.settings = settings;
        }

        // 使用原生SQL创建，绕开封装 create(content) 在 SurrealDB 3 上的类型兼容问题。
        let create_sql = r#"
            CREATE space CONTENT {
                name: $name,
                slug: $slug,
                is_public: $is_public,
                is_deleted: false,
                owner_id: $owner_id,
                settings: {},
                theme_config: {},
                member_count: 0,
                document_count: 0,
                created_by: $created_by,
                updated_by: $updated_by,
                created_at: time::now(),
                updated_at: time::now()
            } RETURN *;
        "#;

        let mut response = self
            .db
            .client
            .query(create_sql)
            .bind(("name", space.name.clone()))
            .bind(("slug", space.slug.clone()))
            .bind(("is_public", space.is_public))
            .bind(("owner_id", space.owner_id.clone()))
            .bind(("created_by", user.id.clone()))
            .bind(("updated_by", user.id.clone()))
            .await
            .map_err(|e| {
                error!("Failed to create space: {}", e);
                map_create_space_db_error(e)
            })?;

        let created_spaces: Vec<Space> = response.take(0).map_err(|e| {
            error!("Failed to decode created space: {}", e);
            map_create_space_db_error(e)
        })?;

        let created_space = created_spaces.into_iter().next();

        let created_space = created_space.ok_or_else(|| {
            error!("Failed to get created space from database");
            AppError::Internal(anyhow::anyhow!("Failed to create space"))
        })?;

        info!("Created new space: {} by user: {}", request.slug, user.id);

        // 记录活动日志
        self.log_activity(
            &user.id,
            "space_created",
            "space",
            &created_space.id.as_ref().unwrap_or(&String::new()),
        )
        .await?;

        Ok(SpaceResponse::from(created_space))
    }

    /// 获取空间列表
    pub async fn list_spaces(
        &self,
        query: SpaceListQuery,
        user: Option<&User>,
    ) -> Result<SpaceListResponse> {
        let page = query.page.unwrap_or(1);
        let limit = query.limit.unwrap_or(20);
        let offset = (page - 1) * limit;

        // 构建查询条件
        let mut where_conditions = Vec::new();
        let mut params: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();

        // 权限过滤：只显示用户拥有的空间或用户加入的空间
        // 公开空间应该通过直接链接访问，而不是在列表中显示
        if let Some(user) = user {
            info!("Listing spaces for user: {}", user.id);

            // 由于SurrealDB子查询语法问题，分两步获取：
            // 1. 先获取用户拥有的空间
            // 2. 再获取用户加入的空间，然后合并

            // 这里先查询用户拥有的空间（owner_id 可能是 Thing，需要转成字符串比较）
            let user_id_raw = user.id.clone();
            let user_id_bracketed = format!("user:{}", user_id_raw);
            let user_id_plain = user_id_raw
                .trim_matches(|c| c == '⟨' || c == '⟩')
                .to_string();
            let user_id_plain_prefixed = format!("user:{}", user_id_plain);

            where_conditions.push("(IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) IN [$user_id_raw, $user_id_bracketed, $user_id_plain_prefixed]");
            params.insert("user_id_raw".to_string(), user_id_raw.into());
            params.insert("user_id_bracketed".to_string(), user_id_bracketed.into());
            params.insert(
                "user_id_plain_prefixed".to_string(),
                user_id_plain_prefixed.into(),
            );
        } else {
            // 未登录用户看不到任何空间列表
            where_conditions.push("1 = 0");
        }

        // 基础过滤条件
        where_conditions.push("is_deleted = false");

        // 搜索过滤
        if let Some(search) = &query.search {
            where_conditions.push("(string::lowercase(name) CONTAINS string::lowercase($search) OR string::lowercase(description) CONTAINS string::lowercase($search))");
            params.insert("search".to_string(), search.clone().into());
        }

        // 所有者过滤
        if let Some(owner_id) = &query.owner_id {
            // 同样处理 owner_id 过滤
            let owner_raw = owner_id.clone();
            let owner_bracketed = format!("user:{}", owner_raw);
            let owner_plain = owner_raw.trim_matches(|c| c == '⟨' || c == '⟩').to_string();
            let owner_plain_prefixed = format!("user:{}", owner_plain);

            where_conditions.push("(IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) IN [$owner_raw, $owner_bracketed, $owner_plain_prefixed]");
            params.insert("owner_raw".to_string(), owner_raw.into());
            params.insert("owner_bracketed".to_string(), owner_bracketed.into());
            params.insert(
                "owner_plain_prefixed".to_string(),
                owner_plain_prefixed.into(),
            );
        }

        // 公开性过滤
        if let Some(is_public) = query.is_public {
            where_conditions.push("is_public = $is_public");
            params.insert("is_public".to_string(), is_public.into());
        }

        let where_clause = if where_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_conditions.join(" AND "))
        };

        // 排序
        let sort_field = query.sort.unwrap_or_else(|| "updated_at".to_string());
        let sort_order = query.order.unwrap_or_else(|| "desc".to_string());
        let order_clause = format!("ORDER BY {} {}", sort_field, sort_order);

        // 查询总数
        let count_query = format!(
            "SELECT count() AS total FROM space {} GROUP ALL",
            where_clause
        );
        let count_result: Vec<serde_json::Value> = self
            .db
            .client
            .query(&count_query)
            .bind(params.clone())
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        let total = count_result
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let total_pages = (total + limit - 1) / limit;

        // 查询数据（将 id 转为字符串，避免 Thing 反序列化问题）
        let data_query = format!(
            "SELECT string::replace(type::string(id), 'space:', '') AS id, \
                    name, slug, description, avatar_url, is_public, is_deleted, \
                    (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) as owner_id, \
                    settings, theme_config, member_count, document_count, created_at, updated_at, \
                    (IF created_by = NONE THEN '' ELSE type::string(created_by) END) as created_by, \
                    (IF updated_by = NONE THEN '' ELSE type::string(updated_by) END) as updated_by \
             FROM space {} {} LIMIT {} START {}",
            where_clause, order_clause, limit, offset
        );

        info!("Executing space list query: {}", data_query);
        info!("Query params: {:?}", params);

        // 直接获取为 Space（id 已是字符串）
        let mut spaces: Vec<Space> = self
            .db
            .client
            .query(&data_query)
            .bind(params)
            .await
            .map_err(|e| {
                error!("Failed to execute space list query: {}", e);
                AppError::Database(e)
            })?
            .take(0)?;

        // 如果是登录用户，还需要添加用户作为成员的空间
        if let Some(user) = user {
            let member_spaces = self.get_user_member_spaces(&user.id).await?;
            info!(
                "Found {} member spaces for user {}",
                member_spaces.len(),
                user.id
            );

            // 合并空间，避免重复
            let existing_ids: std::collections::HashSet<String> =
                spaces.iter().filter_map(|s| s.id.clone()).collect();

            for member_space in member_spaces {
                if let Some(space_id) = &member_space.id {
                    if !existing_ids.contains(space_id) {
                        spaces.push(member_space);
                    }
                }
            }
        }

        // 转换为响应格式
        let mut space_responses = Vec::new();
        for space in spaces {
            let mut response = SpaceResponse::from(space);
            // 获取空间统计信息
            if let Ok(stats) = self.get_space_stats(&response.id).await {
                response.stats = Some(stats);
            }
            space_responses.push(response);
        }

        debug!(
            "Listed {} spaces for user: {:?}",
            space_responses.len(),
            user.map(|u| &u.id)
        );

        Ok(SpaceListResponse {
            spaces: space_responses,
            total,
            page,
            limit,
            total_pages,
        })
    }

    /// 根据slug获取空间详情
    pub async fn get_space_by_slug(
        &self,
        slug: &str,
        user: Option<&User>,
    ) -> Result<SpaceResponse> {
        // SurrealDB 3 + current client stack can intermittently miss rows on bound slug filters
        // (WHERE slug = $slug). Use a validated literal to keep behavior stable.
        let slug_literal = sanitize_slug_for_query(slug)?;
        let mut response = self
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
                 FROM space
                 WHERE slug = '{slug_literal}' AND is_deleted = false
                 LIMIT 1"
            ))
            .await
            .map_err(AppError::Database)?;
        let spaces: Vec<Space> = response.take(0)?;
        let space = if let Some(space) = spaces.into_iter().next() {
            space
        } else {
            warn!(
                "Primary slug query missed for slug={}, trying fallback scan",
                slug
            );
            let mut fallback = self.db.client
                .query(
                    "SELECT
                        string::replace(type::string(id), 'space:', '') AS id,
                        name, slug, description, avatar_url, is_public, is_deleted,
                        (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
                        settings, theme_config, member_count, document_count,
                        created_at, updated_at,
                        (IF created_by = NONE THEN '' ELSE type::string(created_by) END) AS created_by,
                        (IF updated_by = NONE THEN '' ELSE type::string(updated_by) END) AS updated_by
                     FROM space
                     WHERE is_deleted = false
                     LIMIT 5000"
                )
                .await
                .map_err(AppError::Database)?;
            let all_spaces: Vec<Space> = fallback.take(0)?;
            all_spaces
                .into_iter()
                .find(|s| s.slug == slug)
                .ok_or_else(|| AppError::NotFound("Space not found".to_string()))?
        };

        // 匿名访问私有空间才拒绝；已认证用户的成员权限由各路由自行检查
        if user.is_none() && !space.is_public {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }

        let mut response = SpaceResponse::from(space);

        // 获取统计信息
        if let Ok(stats) = self.get_space_stats(&response.id).await {
            response.stats = Some(stats);
        }

        debug!(
            "Retrieved space: {} for user: {:?}",
            slug,
            user.map(|u| &u.id)
        );

        Ok(response)
    }

    /// 根据ID获取空间详情
    pub async fn get_space_by_id(&self, id: &str, user: Option<&User>) -> Result<SpaceResponse> {
        let clean_id = sanitize_space_id_for_query(id)?;
        let query_id = format!("space:{}", clean_id);
        info!("get_space_by_id: searching for id = {}", query_id);

        let mut response = self
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
        let spaces: Vec<Space> = response.take(0)?;
        info!("get_space_by_id: query result = {:?}", !spaces.is_empty());
        let space = if let Some(space) = spaces.into_iter().next() {
            space
        } else {
            warn!(
                "Primary id query missed for space_id={}, trying fallback scan",
                clean_id
            );
            let mut fallback = self.db.client
                .query(
                    "SELECT
                        string::replace(type::string(id), 'space:', '') AS id,
                        name, slug, description, avatar_url, is_public, is_deleted,
                        (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
                        settings, theme_config, member_count, document_count,
                        created_at, updated_at,
                        (IF created_by = NONE THEN '' ELSE type::string(created_by) END) AS created_by,
                        (IF updated_by = NONE THEN '' ELSE type::string(updated_by) END) AS updated_by
                     FROM space
                     WHERE is_deleted = false
                     LIMIT 5000"
                )
                .await
                .map_err(AppError::Database)?;
            let all_spaces: Vec<Space> = fallback.take(0)?;
            all_spaces
                .into_iter()
                .find(|s| s.id.as_deref() == Some(clean_id.as_str()))
                .ok_or_else(|| AppError::NotFound("Space not found".to_string()))?
        };

        // 匿名访问私有空间才拒绝；已认证用户的成员权限由各路由自行检查
        if user.is_none() && !space.is_public {
            return Err(AppError::Authorization(
                "Access denied to this space".to_string(),
            ));
        }

        let mut response = SpaceResponse::from(space);

        // 获取统计信息
        if let Ok(stats) = self.get_space_stats(&response.id).await {
            response.stats = Some(stats);
        }

        debug!(
            "Retrieved space by ID: {} for user: {:?}",
            id,
            user.map(|u| &u.id)
        );

        Ok(response)
    }

    /// 更新空间信息
    pub async fn update_space(
        &self,
        slug: &str,
        request: UpdateSpaceRequest,
        user: &User,
    ) -> Result<SpaceResponse> {
        // 验证输入
        request
            .validate()
            .map_err(|e| AppError::Validation(e.to_string()))?;

        // 获取现有空间
        let existing_space = self.get_space_by_slug(slug, Some(user)).await?;

        // 检查权限：只有所有者可以更新
        if existing_space.owner_id != user.id {
            return Err(AppError::Authorization(
                "Only space owner can update space".to_string(),
            ));
        }

        // 构建更新数据
        let mut update_data = std::collections::HashMap::new();

        if let Some(name) = request.name {
            update_data.insert("name", Value::String(name));
        }

        if let Some(description) = request.description {
            update_data.insert("description", Value::String(description));
        }

        if let Some(avatar_url) = request.avatar_url {
            update_data.insert("avatar_url", Value::String(avatar_url));
        }

        if let Some(is_public) = request.is_public {
            update_data.insert("is_public", Value::Bool(is_public));
        }

        if request.settings.is_some() {
            // Keep compatibility with current SCHEMAFULL schema (no settings.* subfield defs).
            update_data.insert("settings", serde_json::json!({}));
            update_data.insert("theme_config", serde_json::json!({}));
        }

        // 执行更新
        let mut response = self
            .db
            .client
            .query(
                "UPDATE space
                 SET $data, updated_at = time::now()
                 WHERE slug = $slug
                 RETURN AFTER",
            )
            .bind(("data", update_data))
            .bind(("slug", slug))
            .await
            .map_err(AppError::Database)?;
        let updated_spaces: Vec<Space> = response.take((0, "AFTER"))?;

        let updated_space = updated_spaces
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Failed to update space")))?;

        info!("Updated space: {} by user: {}", slug, user.id);

        // 记录活动日志
        self.log_activity(
            &user.id,
            "space_updated",
            "space",
            &updated_space.id.as_ref().unwrap_or(&String::new()),
        )
        .await?;

        Ok(SpaceResponse::from(updated_space))
    }

    /// 删除空间
    pub async fn delete_space(&self, slug: &str, user: &User) -> Result<()> {
        // 获取现有空间
        let existing_space = self.get_space_by_slug(slug, Some(user)).await?;

        // 检查权限：只有所有者可以删除
        if existing_space.owner_id != user.id {
            return Err(AppError::Authorization(
                "Only space owner can delete space".to_string(),
            ));
        }

        // 检查空间是否有文档
        let doc_count: Option<u32> = self
            .db
            .client
            .query("SELECT count() FROM document WHERE space_id = $space_id")
            .bind(("space_id", format!("space:{}", existing_space.id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "count"))?;

        if let Some(count) = doc_count {
            if count > 0 {
                return Err(AppError::Conflict(
                    "Cannot delete space with existing documents".to_string(),
                ));
            }
        }

        // 删除空间
        let _: Option<serde_json::Value> = self
            .db
            .client
            .query("DELETE space WHERE slug = $slug")
            .bind(("slug", slug))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        info!("Deleted space: {} by user: {}", slug, user.id);

        // 记录活动日志
        self.log_activity(&user.id, "space_deleted", "space", &existing_space.id)
            .await?;

        Ok(())
    }

    pub async fn is_slug_available(&self, slug: &str) -> Result<bool> {
        Ok(!self.slug_exists(slug).await?)
    }

    /// 检查slug是否已存在（全局检查）
    async fn slug_exists(&self, slug: &str) -> Result<bool> {
        let existing: Option<surrealdb::types::RecordId> = self
            .db
            .client
            .query("SELECT VALUE id FROM space WHERE slug = $slug AND is_deleted = false LIMIT 1")
            .bind(("slug", slug))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        Ok(existing.is_some())
    }

    /// 获取空间统计信息
    pub async fn get_space_stats(&self, space_id: &str) -> Result<SpaceStats> {
        // 查询文档数量
        let doc_count: Option<u32> = self
            .db
            .client
            .query("SELECT count() FROM document WHERE space_id = $space_id")
            .bind(("space_id", format!("space:{}", space_id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "count"))?;

        // 查询公开文档数量
        let public_doc_count: Option<u32> = self
            .db
            .client
            .query("SELECT count() FROM document WHERE space_id = $space_id AND is_public = true")
            .bind(("space_id", format!("space:{}", space_id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "count"))?;

        // 查询评论数量
        let comment_count: Option<u32> = self.db.client
            .query("SELECT count() FROM comment WHERE document_id IN (SELECT id FROM document WHERE space_id = $space_id)")
            .bind(("space_id", format!("space:{}", space_id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "count"))?;

        // 查询总浏览量
        let view_count: Option<u32> = self.db.client
            .query("SELECT math::sum(view_count) AS total_views FROM document WHERE space_id = $space_id")
            .bind(("space_id", format!("space:{}", space_id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "total_views"))?;

        // 查询最后活动时间
        let last_activity: Option<String> = self.db.client
            .query("SELECT updated_at FROM document WHERE space_id = $space_id ORDER BY updated_at DESC LIMIT 1")
            .bind(("space_id", format!("space:{}", space_id)))
            .await
            .map_err(|e| AppError::Database(e))?
            .take((0, "updated_at"))?;

        let last_activity = last_activity
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        Ok(SpaceStats {
            document_count: doc_count.unwrap_or(0),
            public_document_count: public_doc_count.unwrap_or(0),
            comment_count: comment_count.unwrap_or(0),
            view_count: view_count.unwrap_or(0),
            last_activity,
        })
    }

    /// 获取用户作为成员的空间列表
    async fn get_user_member_spaces(&self, user_id: &str) -> Result<Vec<Space>> {
        info!("Getting member spaces for user: {}", user_id);

        // 统一成裸 UUID，避免 user:⟨...⟩ / user:... / ⟨...⟩ 三种格式不一致
        let clean_user_id = user_id
            .trim()
            .strip_prefix("user:")
            .unwrap_or(user_id)
            .trim_matches(|c| c == '⟨' || c == '⟩')
            .to_string();
        info!(
            "Querying member spaces with cleaned user_id: {} (original: {})",
            clean_user_id, user_id
        );

        // 查询用户是成员的space_id列表
        let user_id_bracketed = format!("user:⟨{}⟩", clean_user_id);
        let user_id_plain = format!("user:{}", clean_user_id);

        let member_query = r#"
            SELECT space_id
            FROM space_member
            WHERE type::string(user_id) IN [$user_id_bracketed, $user_id_plain, $user_id_raw]
              AND status = 'accepted'
        "#;
        let member_results: Vec<surrealdb::types::Value> = self
            .db
            .client
            .query(member_query)
            .bind(("user_id_bracketed", user_id_bracketed))
            .bind(("user_id_plain", user_id_plain))
            .bind(("user_id_raw", clean_user_id.clone()))
            .await
            .map_err(|e| {
                error!("Failed to query space members: {}", e);
                AppError::Database(e)
            })?
            .take(0)?;

        // 如果没有找到结果，尝试查看所有space_member记录进行调试
        if member_results.is_empty() {
            info!("No member spaces found for cleaned user_id: {} (original: {}), checking all space_member records for debugging", clean_user_id, user_id);
            let all_members: Vec<surrealdb::types::Value> = self
                .db
                .client
                .query("SELECT user_id, space_id, status FROM space_member LIMIT 5")
                .await
                .map_err(|e| AppError::Database(e))?
                .take(0)?;

            for member in &all_members {
                info!("Found space_member record: {:?}", member);
            }
        }

        info!(
            "Found {} space member records for user {}",
            member_results.len(),
            user_id
        );

        // 提取space_id列表
        let mut space_ids = Vec::new();
        for result in member_results {
            let obj = match result {
                surrealdb::types::Value::Object(o) => o,
                _ => continue,
            };

            if let Some(space_id_value) = obj.get("space_id") {
                info!("Processing space_id value: {:?}", space_id_value);

                match space_id_value {
                    surrealdb::types::Value::RecordId(t) => {
                        let id_str = record_id_key(t);
                        let clean_id = id_str.strip_prefix("space:").unwrap_or(&id_str);
                        space_ids.push(clean_id.to_string());
                        info!("Found member space_id (thing): {}", clean_id);
                    }
                    surrealdb::types::Value::String(s) => {
                        let space_id_str = s.as_str();
                        let clean_id = space_id_str.strip_prefix("space:").unwrap_or(space_id_str);
                        space_ids.push(clean_id.to_string());
                        info!("Found member space_id (string): {}", clean_id);
                    }
                    _ => {}
                }
            }
        }

        if space_ids.is_empty() {
            info!("No member spaces found for user {}", user_id);
            return Ok(Vec::new());
        }

        // 查询对应的space记录
        let mut spaces = Vec::new();
        for space_id in space_ids {
            let space_results: Vec<Space> = self
                .db
                .client
                .query(
                    "SELECT
                        string::replace(type::string(id), 'space:', '') AS id,
                        name, slug, description, avatar_url, is_public, is_deleted,
                        owner_id, settings, theme_config, member_count, document_count,
                        created_at, updated_at, created_by, updated_by
                     FROM space
                     WHERE id = $space_id AND is_deleted = false",
                )
                .bind(("space_id", Thing::new("space", space_id.as_str())))
                .await
                .map_err(|e| {
                    error!("Failed to query space: {}", e);
                    AppError::Database(e)
                })?
                .take(0)?;

            for space in space_results {
                spaces.push(space);
            }
        }

        info!(
            "Retrieved {} actual spaces for user {}",
            spaces.len(),
            user_id
        );
        Ok(spaces)
    }

    /// 记录活动日志
    async fn log_activity(
        &self,
        user_id: &str,
        action: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<()> {
        let mut response = self
            .db
            .client
            .query(
                "CREATE activity_log SET
                    user_id = $user_id,
                    action = $action,
                    resource_type = $resource_type,
                    resource_id = $resource_id,
                    details = {},
                    created_at = time::now()",
            )
            .bind(("user_id", user_id))
            .bind(("action", action))
            .bind(("resource_type", resource_type))
            .bind(("resource_id", resource_id))
            .await
            .map_err(|e| {
                warn!("Failed to log activity: {}", e);
                e
            })
            .ok();
        let _ignored: Option<Value> = response.as_mut().and_then(|r| r.take(0).ok());

        Ok(())
    }
}

fn sanitize_slug_for_query(slug: &str) -> Result<String> {
    if slug.is_empty() {
        return Err(AppError::Validation(
            "space slug cannot be empty".to_string(),
        ));
    }
    if slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Ok(slug.to_string());
    }
    Err(AppError::Validation(
        "invalid space slug format".to_string(),
    ))
}

fn sanitize_space_id_for_query(id: &str) -> Result<String> {
    let clean = id
        .strip_prefix("space:")
        .unwrap_or(id)
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string();
    if clean.is_empty() {
        return Err(AppError::Validation("space id cannot be empty".to_string()));
    }
    if clean
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(clean);
    }
    Err(AppError::Validation("invalid space id format".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::space::CreateSpaceRequest;

    // 注意：实际测试需要数据库连接，这里只是示例结构
    #[tokio::test]
    async fn test_create_space_validation() {
        let request = CreateSpaceRequest {
            name: "".to_string(), // 无效：空名称
            slug: "test-space".to_string(),
            description: None,
            avatar_url: None,
            is_public: None,
            settings: None,
        };

        assert!(request.validate().is_err());
    }

    #[tokio::test]
    async fn test_slug_validation() {
        let valid_request = CreateSpaceRequest {
            name: "Test Space".to_string(),
            slug: "test-space".to_string(),
            description: None,
            avatar_url: None,
            is_public: None,
            settings: None,
        };

        assert!(valid_request.validate().is_ok());

        let invalid_request = CreateSpaceRequest {
            name: "Test Space".to_string(),
            slug: "Test Space".to_string(), // 无效：包含空格和大写
            description: None,
            avatar_url: None,
            is_public: None,
            settings: None,
        };

        assert!(invalid_request.validate().is_err());
    }
}
