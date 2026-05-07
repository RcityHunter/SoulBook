use crate::error::{AppError, Result};
use crate::models::space::{
    CreateSpaceRequest, Space, SpaceListQuery, SpaceListResponse, SpaceResponse, SpaceStats,
    UpdateSpaceRequest,
};
use crate::services::auth::User;
use crate::services::database::Database;
use serde_json::Value;
use std::sync::Arc;
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

        let workspace_values = create_space_workspace_values(&request)?;

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

        let optional_fields = create_space_optional_fields(&space.description, &space.avatar_url);

        // 使用原生SQL创建，绕开封装 create(content) 在 SurrealDB 3 上的类型兼容问题。
        // 创建结果只作为执行确认；随后用字符串投影查询新空间，避免 RecordId 反序列化差异导致 500。
        let create_sql = r#"
            CREATE space CONTENT {
                name: $name,
                slug: $slug,
                __OPTIONAL_FIELDS__
                is_public: $is_public,
                is_deleted: false,
                workspace_kind: $workspace_kind,
                team_id: $team_id,
                owner_id: $owner_id,
                settings: {},
                theme_config: {},
                member_count: 0,
                document_count: 0,
                created_by: $created_by,
                updated_by: $updated_by,
                created_at: time::now(),
                updated_at: time::now()
            };
        "#
        .replace("__OPTIONAL_FIELDS__", &optional_fields);

        let mut create_query = self
            .db
            .client
            .query(create_sql)
            .bind(("name", space.name.clone()))
            .bind(("slug", space.slug.clone()))
            .bind(("is_public", space.is_public))
            .bind(("workspace_kind", workspace_values.workspace_kind))
            .bind(("team_id", workspace_values.team_id))
            .bind(("owner_id", space.owner_id.clone()))
            .bind(("created_by", user.id.clone()))
            .bind(("updated_by", user.id.clone()));

        if let Some(description) = &space.description {
            create_query = create_query.bind(("description", description.clone()));
        }
        if let Some(avatar_url) = &space.avatar_url {
            create_query = create_query.bind(("avatar_url", avatar_url.clone()));
        }

        let mut create_response = create_query.await.map_err(|e| {
            error!("Failed to create space: {}", e);
            map_create_space_db_error(e)
        })?;

        let _created: Value = create_response.take(0).map_err(|e| {
            error!("Failed to decode create acknowledgement: {}", e);
            map_create_space_db_error(e)
        })?;

        let mut response = self
            .db
            .client
            .query(
                "SELECT
                    string::replace(type::string(id), 'space:', '') AS id,
                    name, slug, description, avatar_url, is_public, is_deleted,
                    (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) as owner_id,
                    settings, theme_config, member_count, document_count,
                    created_at, updated_at,
                    (IF created_by = NONE THEN '' ELSE type::string(created_by) END) as created_by,
                    (IF updated_by = NONE THEN '' ELSE type::string(updated_by) END) as updated_by
                 FROM space
                 WHERE slug = $slug AND is_deleted = false
                 LIMIT 1",
            )
            .bind(("slug", request.slug.clone()))
            .await
            .map_err(|e| {
                error!("Failed to fetch created space: {}", e);
                map_create_space_db_error(e)
            })?;

        let created_spaces: Vec<Space> = response.take(0).map_err(|e| {
            error!("Failed to decode fetched created space: {}", e);
            map_create_space_db_error(e)
        })?;

        let created_space = created_spaces.into_iter().next().ok_or_else(|| {
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

            apply_user_space_visibility_scope(&query, user, &mut where_conditions, &mut params);
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

        apply_workspace_scope(&query, &mut where_conditions, &mut params)?;

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

        let owner_total = count_result
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

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
        if user.is_some() {
            let include_member_spaces = query.workspace.as_deref().unwrap_or("personal") != "team";
            if !include_member_spaces {
                info!(
                    "Skipping user member-space merge for team workspace query: {:?}",
                    query.team_id
                );
            }
        }

        if let Some(user) = user.filter(|_| query.workspace.as_deref().unwrap_or("personal") != "team") {
            let member_spaces = self.get_user_member_spaces(&user.id).await?;
            info!(
                "Found {} member spaces for user {}",
                member_spaces.len(),
                user.id
            );

            // 合并空间，避免重复。成员空间查询在部分 SurrealDB 返回形态下可能没有投影出 id，
            // 不能因此把已查询到的成员空间丢掉。
            let mut existing_keys: std::collections::HashSet<String> =
                spaces.iter().filter_map(space_identity_key).collect();

            for member_space in member_spaces {
                if let Some(key) = space_identity_key(&member_space) {
                    if existing_keys.insert(key) {
                        spaces.push(member_space);
                    }
                } else {
                    warn!(
                        "Member space has no usable id or slug; including it to avoid hiding accessible space"
                    );
                    spaces.push(member_space);
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

        let visible_total = space_responses.len() as u32;
        let total = owner_total.max(visible_total);
        let total_pages = if total == 0 {
            0
        } else {
            (total + limit - 1) / limit
        };

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
        let existing: Option<String> = self
            .db
            .client
            .query("SELECT VALUE type::string(id) FROM space WHERE slug = $slug AND is_deleted = false LIMIT 1")
            .bind(("slug", slug))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;

        Ok(existing.is_some())
    }

    /// 获取空间统计信息
    pub async fn get_space_stats(&self, space_id: &str) -> Result<SpaceStats> {
        let space_id = sanitize_space_id_for_query(space_id)?;
        let queries = space_stats_queries();
        let record_id = format!("space:{}", space_id);

        // 查询文档数量
        let doc_count = self.query_space_stats_count(queries.document_count, &record_id).await?;

        // 查询公开文档数量
        let public_doc_count = self
            .query_space_stats_count(queries.public_document_count, &record_id)
            .await?;

        let member_count = self
            .query_space_stats_count(queries.member_count, &record_id)
            .await?;

        let tag_count = self
            .query_space_stats_count(queries.tag_count, &record_id)
            .await?;

        // 查询评论数量
        let comment_count = self
            .query_space_stats_count(queries.comment_count, &record_id)
            .await?;

        // 查询总浏览量。SurrealDB 3 的 math::sum expects array<number>，
        // 对字段直接聚合会报类型错误，所以这里读取行后在 Rust 侧求和。
        let view_rows: Vec<Value> = self
            .db
            .client
            .query(queries.view_count)
            .bind(("space_id", record_id.clone()))
            .await
            .map_err(|e| AppError::Database(e))?
            .take(0)?;
        let view_count = view_rows
            .iter()
            .filter_map(|row| row.get("view_count").and_then(Value::as_u64))
            .sum::<u64>() as u32;

        // 查询最后活动时间
        let last_activity: Option<String> = self
            .db
            .client
            .query(queries.last_activity)
            .bind(("space_id", record_id))
            .await
            .map_err(|e| AppError::Database(e))?
            .take::<Vec<Value>>(0)?
            .into_iter()
            .find_map(|row| {
                row.get("updated_at")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            });

        let last_activity = last_activity
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        Ok(SpaceStats {
            document_count: doc_count,
            public_document_count: public_doc_count,
            member_count,
            tag_count,
            comment_count,
            view_count,
            last_activity,
        })
    }

    async fn query_space_stats_count(&self, query: &str, record_id: &str) -> Result<u32> {
        let rows: Vec<Value> = self
            .db
            .client
            .query(query)
            .bind(("space_id", record_id.to_string()))
            .await
            .map_err(AppError::Database)?
            .take(0)?;

        Ok(first_count_from_rows(&rows))
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
        let local_user_id_bracketed = format!("local_user:⟨{}⟩", clean_user_id);
        let local_user_id_plain = format!("local_user:{}", clean_user_id);

        let space_ids: Vec<String> = self
            .db
            .client
            .query(member_space_ids_query())
            .bind(("user_id_bracketed", user_id_bracketed))
            .bind(("user_id_plain", user_id_plain))
            .bind(("local_user_id_bracketed", local_user_id_bracketed))
            .bind(("local_user_id_plain", local_user_id_plain))
            .bind(("user_id_raw", clean_user_id.clone()))
            .await
            .map_err(|e| {
                error!("Failed to query space members: {}", e);
                AppError::Database(e)
            })?
            .take(0)?;

        // 如果没有找到结果，尝试查看所有space_member记录进行调试
        if space_ids.is_empty() {
            info!(
                "No member spaces found for cleaned user_id: {} (original: {}), checking all space_member records for debugging",
                clean_user_id, user_id
            );
            let all_members: Vec<Value> = self
                .db
                .client
                .query(
                    "SELECT
                        type::string(user_id) AS user_id,
                        type::string(space_id) AS space_id,
                        status
                     FROM space_member
                     LIMIT 5",
                )
                .await
                .map_err(|e| AppError::Database(e))?
                .take(0)?;

            for member in &all_members {
                info!("Found space_member record: {:?}", member);
            }
        }

        info!(
            "Found {} space member records for user {}",
            space_ids.len(),
            user_id
        );

        if space_ids.is_empty() {
            info!("No member spaces found for user {}", user_id);
            return Ok(Vec::new());
        }

        // 查询对应的space记录
        let mut spaces = Vec::new();
        for space_id in space_ids {
            let space_key = member_space_lookup_key(&space_id);
            let member_space_query = "SELECT
                        string::replace(type::string(id), 'space:', '') AS id,
                        name, slug, description, avatar_url, is_public, is_deleted,
                        (IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) AS owner_id,
                        settings, theme_config, member_count, document_count,
                        created_at, updated_at,
                        (IF created_by = NONE THEN '' ELSE type::string(created_by) END) AS created_by,
                        (IF updated_by = NONE THEN '' ELSE type::string(updated_by) END) AS updated_by
                     FROM space
                     WHERE __MEMBER_SPACE_LOOKUP__ AND is_deleted = false"
                .replace("__MEMBER_SPACE_LOOKUP__", member_space_lookup_where_clause());

            let space_results: Vec<Space> = self
                .db
                .client
                .query(&member_space_query)
                .bind(("space_id", format!("space:{}", space_key)))
                .bind(("space_key", space_key))
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

fn create_space_optional_fields(
    description: &Option<String>,
    avatar_url: &Option<String>,
) -> String {
    let mut fields = Vec::new();
    if description.is_some() {
        fields.push("description: $description,");
    }
    if avatar_url.is_some() {
        fields.push("avatar_url: $avatar_url,");
    }
    fields.join("\n                ")
}

struct CreateSpaceWorkspaceValues {
    workspace_kind: String,
    team_id: Option<String>,
}

fn create_space_workspace_values(
    request: &CreateSpaceRequest,
) -> Result<CreateSpaceWorkspaceValues> {
    match request.workspace.as_deref().unwrap_or("personal") {
        "team" => {
            let team_id = request.team_id.as_deref().ok_or_else(|| {
                AppError::Validation("team_id is required for team workspace".to_string())
            })?;

            Ok(CreateSpaceWorkspaceValues {
                workspace_kind: "team".to_string(),
                team_id: Some(team_id.to_string()),
            })
        }
        "personal" | "" => Ok(CreateSpaceWorkspaceValues {
            workspace_kind: "personal".to_string(),
            team_id: None,
        }),
        other => Err(AppError::Validation(format!(
            "unsupported workspace scope: {}",
            other
        ))),
    }
}

fn apply_workspace_scope(
    query: &SpaceListQuery,
    where_conditions: &mut Vec<&'static str>,
    params: &mut std::collections::HashMap<String, serde_json::Value>,
) -> Result<()> {
    match query.workspace.as_deref().unwrap_or("personal") {
        "team" => {
            let team_id = query.team_id.as_deref().ok_or_else(|| {
                AppError::Validation("team_id is required for team workspace".to_string())
            })?;
            where_conditions.push("workspace_kind = 'team'");
            where_conditions.push("team_id = $team_id");
            params.insert("team_id".to_string(), team_id.to_string().into());
        }
        "personal" | "" => {
            where_conditions
                .push("(!workspace_kind OR workspace_kind = NONE OR workspace_kind = 'personal')");
        }
        other => {
            return Err(AppError::Validation(format!(
                "unsupported workspace scope: {}",
                other
            )));
        }
    }

    Ok(())
}

fn apply_user_space_visibility_scope(
    query: &SpaceListQuery,
    user: &User,
    where_conditions: &mut Vec<&'static str>,
    params: &mut std::collections::HashMap<String, serde_json::Value>,
) {
    let user_id_raw = user.id.clone();
    let user_id_bracketed = format!("user:{}", user_id_raw);
    let user_id_plain = user_id_raw
        .trim_matches(|c| c == '⟨' || c == '⟩')
        .to_string();
    let user_id_plain_prefixed = format!("user:{}", user_id_plain);

    if query.workspace.as_deref() == Some("team") {
        where_conditions.push(
            "$team_id IN (
                SELECT VALUE string::replace(type::string(team_id), 'team:', '')
                FROM team_member
                WHERE user_id IN [$user_id_raw, $user_id_bracketed, $user_id_plain_prefixed]
                  AND status = 'active'
            )",
        );
    } else {
        where_conditions.push("(IF owner_id = NONE THEN '' ELSE type::string(owner_id) END) IN [$user_id_raw, $user_id_bracketed, $user_id_plain_prefixed]");
    }

    params.insert("user_id_raw".to_string(), user_id_raw.into());
    params.insert("user_id_bracketed".to_string(), user_id_bracketed.into());
    params.insert(
        "user_id_plain_prefixed".to_string(),
        user_id_plain_prefixed.into(),
    );
}

fn member_space_ids_query() -> &'static str {
    r#"
        SELECT VALUE type::string(space_id)
        FROM space_member
        WHERE type::string(user_id) IN [$user_id_bracketed, $user_id_plain, $local_user_id_bracketed, $local_user_id_plain, $user_id_raw]
          AND status = 'accepted'
    "#
}

fn member_space_lookup_where_clause() -> &'static str {
    "(type::string(id) = $space_id OR string::replace(type::string(id), 'space:', '') = $space_key OR slug = $space_key)"
}

struct SpaceStatsQueries {
    document_count: &'static str,
    public_document_count: &'static str,
    member_count: &'static str,
    tag_count: &'static str,
    comment_count: &'static str,
    view_count: &'static str,
    last_activity: &'static str,
}

fn space_stats_queries() -> SpaceStatsQueries {
    SpaceStatsQueries {
        document_count: "SELECT count() AS count FROM document WHERE space_id = type::record($space_id) AND is_deleted = false GROUP ALL",
        public_document_count: "SELECT count() AS count FROM document WHERE space_id = type::record($space_id) AND is_deleted = false AND is_public = true GROUP ALL",
        member_count: "SELECT count() AS count FROM space_member WHERE space_id = type::record($space_id) AND status = 'accepted' GROUP ALL",
        tag_count: "SELECT count() AS count FROM tag WHERE space_id = type::record($space_id) GROUP ALL",
        comment_count: "SELECT count() AS count FROM comment WHERE document_id IN (SELECT id FROM document WHERE space_id = type::record($space_id) AND is_deleted = false) GROUP ALL",
        view_count: "SELECT view_count FROM document WHERE space_id = type::record($space_id) AND is_deleted = false",
        last_activity: "SELECT type::string(updated_at) AS updated_at FROM document WHERE space_id = type::record($space_id) AND is_deleted = false ORDER BY updated_at DESC LIMIT 1",
    }
}

fn space_identity_key(space: &Space) -> Option<String> {
    if let Some(id) = space
        .id
        .as_ref()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
    {
        return Some(format!("id:{id}"));
    }

    let slug = space.slug.trim();
    if !slug.is_empty() {
        return Some(format!("slug:{slug}"));
    }

    None
}

fn member_space_lookup_key(space_id: &str) -> String {
    let mut clean = space_id
        .trim()
        .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
        .to_string();

    loop {
        let next = clean
            .strip_prefix("space:")
            .unwrap_or(&clean)
            .trim_matches(|c| c == '⟨' || c == '⟩' || c == '"' || c == '\'' || c == '`' || c == ' ')
            .to_string();

        if next == clean {
            return clean;
        }

        clean = next;
    }
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

fn first_count_from_rows(rows: &[Value]) -> u32 {
    rows.first()
        .and_then(|row| row.get("count"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32
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
            workspace: None,
            team_id: None,
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
            workspace: None,
            team_id: None,
        };

        assert!(valid_request.validate().is_ok());

        let invalid_request = CreateSpaceRequest {
            name: "Test Space".to_string(),
            slug: "Test Space".to_string(), // 无效：包含空格和大写
            description: None,
            avatar_url: None,
            is_public: None,
            settings: None,
            workspace: None,
            team_id: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_create_space_optional_fields_omit_none_values() {
        let fields = create_space_optional_fields(&None, &None);
        assert!(!fields.contains("description: $description"));
        assert!(!fields.contains("avatar_url: $avatar_url"));

        let fields = create_space_optional_fields(&Some("desc".to_string()), &None);
        assert!(fields.contains("description: $description"));
        assert!(!fields.contains("avatar_url: $avatar_url"));

        let fields = create_space_optional_fields(&None, &Some("avatar".to_string()));
        assert!(!fields.contains("description: $description"));
        assert!(fields.contains("avatar_url: $avatar_url"));
    }

    #[test]
    fn workspace_space_scope_personal_includes_legacy_personal_spaces() {
        let query = SpaceListQuery {
            page: None,
            limit: None,
            search: None,
            owner_id: None,
            workspace: Some("personal".to_string()),
            team_id: None,
            is_public: None,
            sort: None,
            order: None,
        };
        let mut conditions = Vec::new();
        let mut params = std::collections::HashMap::new();

        apply_workspace_scope(&query, &mut conditions, &mut params).unwrap();

        let clause = conditions.join(" AND ");
        assert!(clause.contains("!workspace_kind"));
        assert!(clause.contains("workspace_kind = 'personal'"));
        assert!(!params.contains_key("team_id"));
    }

    #[test]
    fn workspace_space_scope_team_filters_by_team_id() {
        let query = SpaceListQuery {
            page: None,
            limit: None,
            search: None,
            owner_id: None,
            workspace: Some("team".to_string()),
            team_id: Some("team-514".to_string()),
            is_public: None,
            sort: None,
            order: None,
        };
        let mut conditions = Vec::new();
        let mut params = std::collections::HashMap::new();

        apply_workspace_scope(&query, &mut conditions, &mut params).unwrap();

        let clause = conditions.join(" AND ");
        assert!(clause.contains("workspace_kind = 'team'"));
        assert!(clause.contains("team_id = $team_id"));
        assert_eq!(params.get("team_id").and_then(|v| v.as_str()), Some("team-514"));
    }

    #[test]
    fn create_space_workspace_values_require_team_id_for_team_spaces() {
        let request = CreateSpaceRequest {
            name: "Team Space".to_string(),
            slug: "team-space".to_string(),
            description: None,
            avatar_url: None,
            is_public: None,
            settings: None,
            workspace: Some("team".to_string()),
            team_id: Some("team-514".to_string()),
        };

        let values = create_space_workspace_values(&request).unwrap();

        assert_eq!(values.workspace_kind, "team");
        assert_eq!(values.team_id.as_deref(), Some("team-514"));

        let missing_team_id = CreateSpaceRequest {
            team_id: None,
            ..request
        };

        assert!(create_space_workspace_values(&missing_team_id).is_err());
    }

    #[test]
    fn team_workspace_visibility_uses_team_membership_not_space_owner() {
        let query = SpaceListQuery {
            page: None,
            limit: None,
            search: None,
            owner_id: None,
            workspace: Some("team".to_string()),
            team_id: Some("team-514".to_string()),
            is_public: None,
            sort: None,
            order: None,
        };
        let user = User {
            id: "user-1".to_string(),
            email: "user-1@example.com".to_string(),
            roles: vec![],
            permissions: vec![],
            profile: None,
        };
        let mut conditions = Vec::new();
        let mut params = std::collections::HashMap::new();

        apply_user_space_visibility_scope(&query, &user, &mut conditions, &mut params);

        let clause = conditions.join(" AND ");
        assert!(clause.contains("FROM team_member"));
        assert!(clause.contains("status = 'active'"));
        assert!(!clause.contains("owner_id"));
        assert_eq!(
            params.get("user_id_raw").and_then(|v| v.as_str()),
            Some("user-1")
        );
    }

    #[test]
    fn member_spaces_query_returns_value_strings_not_objects() {
        let query = member_space_ids_query();

        assert!(query.contains("SELECT VALUE"));
        assert!(query.contains("type::string(space_id)"));
        assert!(query.contains("$local_user_id_bracketed"));
        assert!(query.contains("$local_user_id_plain"));
        assert!(!query.contains("SELECT space_id"));
    }

    #[test]
    fn member_space_lookup_matches_record_id_or_slug() {
        let clause = member_space_lookup_where_clause();

        assert!(clause.contains("type::string(id) = $space_id"));
        assert!(clause.contains("string::replace(type::string(id), 'space:', '') = $space_key"));
        assert!(clause.contains("slug = $space_key"));
    }

    #[test]
    fn member_space_key_strips_record_wrappers_before_lookup() {
        assert_eq!(member_space_lookup_key("space:⟨asd⟩"), "asd");
        assert_eq!(member_space_lookup_key("⟨asd⟩"), "asd");
        assert_eq!(member_space_lookup_key("space:asd"), "asd");
        assert_eq!(member_space_lookup_key("⟨space:asd⟩"), "asd");
        assert_eq!(member_space_lookup_key("space:⟨space:asd⟩"), "asd");
        assert_eq!(member_space_lookup_key("space:⟨⟨space:asd⟩⟩"), "asd");
    }

    #[test]
    fn space_identity_key_falls_back_to_slug_when_id_missing() {
        let mut space = Space::new("514".to_string(), "514".to_string(), "owner".to_string());

        assert_eq!(space_identity_key(&space), Some("slug:514".to_string()));

        space.id = Some(String::new());
        assert_eq!(space_identity_key(&space), Some("slug:514".to_string()));

        space.id = Some("space-id".to_string());
        assert_eq!(space_identity_key(&space), Some("id:space-id".to_string()));
    }

    #[test]
    fn space_stats_queries_match_record_space_ids() {
        let queries = space_stats_queries();
        let queries = [
            queries.document_count,
            queries.public_document_count,
            queries.member_count,
            queries.tag_count,
            queries.comment_count,
            queries.view_count,
            queries.last_activity,
        ];

        for query in queries {
            assert!(query.contains("type::record($space_id)"));
            assert!(!query.contains("space_id = $space_id"));
        }
    }

    #[test]
    fn space_stats_count_queries_return_named_grouped_counts() {
        let queries = space_stats_queries();
        let count_queries = [
            queries.document_count,
            queries.public_document_count,
            queries.member_count,
            queries.tag_count,
            queries.comment_count,
        ];

        for query in count_queries {
            assert!(query.contains("count() AS count"));
            assert!(query.contains("GROUP ALL"));
        }
    }

    #[test]
    fn space_stats_count_rows_extract_first_count() {
        let rows = vec![serde_json::json!({ "count": 3 })];

        assert_eq!(first_count_from_rows(&rows), 3);
        assert_eq!(first_count_from_rows(&[]), 0);
        assert_eq!(first_count_from_rows(&[serde_json::json!({})]), 0);
    }

    #[test]
    fn space_stats_last_activity_returns_string_datetime() {
        let queries = space_stats_queries();

        assert!(queries.last_activity.contains("type::string(updated_at) AS updated_at"));
    }

    #[test]
    fn space_stats_view_count_query_returns_rows_for_rust_sum() {
        let queries = space_stats_queries();

        assert_eq!(
            queries.view_count,
            "SELECT view_count FROM document WHERE space_id = type::record($space_id) AND is_deleted = false"
        );
        assert!(!queries.view_count.contains("math::sum"));
    }

    #[test]
    fn space_stats_response_includes_member_and_tag_counts() {
        let stats = SpaceStats::default();

        assert_eq!(stats.member_count, 0);
        assert_eq!(stats.tag_count, 0);
    }
}
