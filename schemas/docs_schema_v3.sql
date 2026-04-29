-- SoulBook Database Schema
-- This file should be executed before starting the application
-- Compatible with SurrealDB

-- =====================================
-- 核心业务表
-- =====================================

-- 文档空间表 (类似GitBook的Space)
DEFINE TABLE space SCHEMAFULL;
DEFINE FIELD name ON space TYPE string ASSERT $value != NONE AND string::len($value) > 0 AND string::len($value) <= 255;
DEFINE FIELD slug ON space TYPE string ASSERT $value != NONE AND string::len($value) > 0 AND string::len($value) <= 100;
DEFINE FIELD description ON space TYPE option<string>;
DEFINE FIELD avatar_url ON space TYPE option<string>;
DEFINE FIELD is_public ON space TYPE bool DEFAULT false;
DEFINE FIELD is_deleted ON space TYPE bool DEFAULT false;
DEFINE FIELD owner_id ON space TYPE string ASSERT $value != NONE; -- Rainbow-Auth用户ID
DEFINE FIELD settings ON space TYPE object DEFAULT {};
DEFINE FIELD theme_config ON space TYPE object DEFAULT {};
DEFINE FIELD member_count ON space TYPE number DEFAULT 0;
DEFINE FIELD document_count ON space TYPE number DEFAULT 0;
DEFINE FIELD created_at ON space TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON space TYPE datetime DEFAULT time::now();
DEFINE FIELD created_by ON space TYPE option<string>;
DEFINE FIELD updated_by ON space TYPE option<string>;

-- 空间索引 - slug全局唯一（类似GitHub仓库名）
DEFINE INDEX space_slug_unique_idx ON space COLUMNS slug UNIQUE;
-- 保留owner索引用于查询
DEFINE INDEX space_owner_slug_idx ON space COLUMNS owner_id, slug;
DEFINE INDEX space_owner_idx ON space COLUMNS owner_id;
DEFINE INDEX space_public_idx ON space COLUMNS is_public;
DEFINE INDEX space_deleted_idx ON space COLUMNS is_deleted;

-- 文档表
DEFINE TABLE document SCHEMAFULL;
DEFINE FIELD space_id ON document TYPE record<space> ASSERT $value != NONE;
DEFINE FIELD title ON document TYPE string ASSERT $value != NONE AND string::len($value) > 0 AND string::len($value) <= 255;
DEFINE FIELD slug ON document TYPE string ASSERT $value != NONE AND string::len($value) > 0 AND string::len($value) <= 100;
DEFINE FIELD content ON document TYPE string DEFAULT "";
DEFINE FIELD excerpt ON document TYPE option<string>;
DEFINE FIELD description ON document TYPE option<string>;
DEFINE FIELD is_public ON document TYPE bool DEFAULT false;
DEFINE FIELD is_deleted ON document TYPE bool DEFAULT false;
DEFINE FIELD parent_id ON document TYPE option<record<document>>;
DEFINE FIELD order_index ON document TYPE number DEFAULT 0;
DEFINE FIELD depth_level ON document TYPE number DEFAULT 0;
DEFINE FIELD author_id ON document TYPE string ASSERT $value != NONE;
DEFINE FIELD updated_by ON document TYPE option<string>;
DEFINE FIELD deleted_by ON document TYPE option<string>;
DEFINE FIELD view_count ON document TYPE number DEFAULT 0;
DEFINE FIELD word_count ON document TYPE number DEFAULT 0;
DEFINE FIELD reading_time ON document TYPE number DEFAULT 0; -- 阅读时间(分钟)
DEFINE FIELD cover_image ON document TYPE option<string>;
DEFINE FIELD status ON document TYPE string DEFAULT "draft" ASSERT $value INSIDE ["draft", "published", "archived"];
DEFINE FIELD template_id ON document TYPE option<record<document>>; -- 模板文档ID
DEFINE FIELD metadata ON document TYPE object DEFAULT {};
DEFINE FIELD created_at ON document TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON document TYPE datetime DEFAULT time::now();
DEFINE FIELD deleted_at ON document TYPE option<datetime>;
DEFINE FIELD published_at ON document TYPE option<datetime>;

-- 文档索引
DEFINE INDEX document_space_slug_idx ON document COLUMNS space_id, slug UNIQUE;
DEFINE INDEX document_parent_idx ON document COLUMNS parent_id;
DEFINE INDEX document_author_idx ON document COLUMNS author_id;
DEFINE INDEX document_status_idx ON document COLUMNS status;
DEFINE INDEX document_public_idx ON document COLUMNS is_public;
DEFINE INDEX document_deleted_idx ON document COLUMNS is_deleted;
DEFINE INDEX document_updated_idx ON document COLUMNS updated_at;
DEFINE INDEX document_order_idx ON document COLUMNS space_id, parent_id, order_index;

-- 文档版本表
DEFINE TABLE document_version SCHEMAFULL;
DEFINE FIELD document_id ON document_version TYPE record<document> ASSERT $value != NONE;
DEFINE FIELD version_number ON document_version TYPE number ASSERT $value != NONE AND $value > 0;
DEFINE FIELD title ON document_version TYPE string ASSERT $value != NONE;
DEFINE FIELD content ON document_version TYPE string DEFAULT "";
DEFINE FIELD summary ON document_version TYPE string;
DEFINE FIELD change_type ON document_version TYPE string DEFAULT "Updated" ASSERT $value INSIDE ["Created", "Updated", "Restored", "Merged"];
DEFINE FIELD author_id ON document_version TYPE string ASSERT $value != NONE;
DEFINE FIELD parent_version_id ON document_version TYPE option<record<document_version>>;
DEFINE FIELD is_current ON document_version TYPE bool DEFAULT false;
DEFINE FIELD word_count ON document_version TYPE number DEFAULT 0;
DEFINE FIELD created_at ON document_version TYPE datetime DEFAULT time::now();

-- 版本索引
DEFINE INDEX version_document_idx ON document_version COLUMNS document_id;
DEFINE INDEX version_number_idx ON document_version COLUMNS document_id, version_number UNIQUE;
DEFINE INDEX version_current_idx ON document_version COLUMNS document_id, is_current;
DEFINE INDEX version_author_idx ON document_version COLUMNS author_id;

-- 文档权限表 (扩展Rainbow-Auth的RBAC)
DEFINE TABLE document_permission SCHEMAFULL;
DEFINE FIELD resource_type ON document_permission TYPE string DEFAULT "Document" ASSERT $value INSIDE ["Space", "Document", "Comment"];
DEFINE FIELD resource_id ON document_permission TYPE string ASSERT $value != NONE;
DEFINE FIELD user_id ON document_permission TYPE option<string>;
DEFINE FIELD role_id ON document_permission TYPE option<string>; -- Rainbow-Auth角色ID
DEFINE FIELD permissions ON document_permission TYPE array<string> DEFAULT [];
DEFINE FIELD granted_by ON document_permission TYPE string ASSERT $value != NONE;
DEFINE FIELD granted_at ON document_permission TYPE datetime DEFAULT time::now();
DEFINE FIELD expires_at ON document_permission TYPE option<datetime>;
DEFINE FIELD is_inherited ON document_permission TYPE bool DEFAULT false;

-- 权限索引
DEFINE INDEX permission_resource_idx ON document_permission COLUMNS resource_type, resource_id;
DEFINE INDEX permission_user_idx ON document_permission COLUMNS user_id;
DEFINE INDEX permission_role_idx ON document_permission COLUMNS role_id;
DEFINE INDEX permission_expires_idx ON document_permission COLUMNS expires_at;

-- 评论表
DEFINE TABLE comment SCHEMAFULL;
DEFINE FIELD document_id ON comment TYPE record<document> ASSERT $value != NONE;
DEFINE FIELD parent_id ON comment TYPE option<record<comment>>; -- 回复支持
DEFINE FIELD author_id ON comment TYPE string ASSERT $value != NONE;
DEFINE FIELD content ON comment TYPE string ASSERT $value != NONE AND string::len($value) > 0;
DEFINE FIELD is_deleted ON comment TYPE bool DEFAULT false;
DEFINE FIELD is_resolved ON comment TYPE bool DEFAULT false;
DEFINE FIELD like_count ON comment TYPE number DEFAULT 0;
DEFINE FIELD liked_by ON comment TYPE array<string> DEFAULT [];
DEFINE FIELD edited_at ON comment TYPE datetime;
DEFINE FIELD edited_by ON comment TYPE string;
DEFINE FIELD deleted_at ON comment TYPE datetime;
DEFINE FIELD deleted_by ON comment TYPE string;
DEFINE FIELD metadata ON comment TYPE object DEFAULT {};
DEFINE FIELD created_at ON comment TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON comment TYPE datetime DEFAULT time::now();

-- 评论索引
DEFINE INDEX comment_document_idx ON comment COLUMNS document_id;
DEFINE INDEX comment_author_idx ON comment COLUMNS author_id;
DEFINE INDEX comment_parent_idx ON comment COLUMNS parent_id;
DEFINE INDEX comment_deleted_idx ON comment COLUMNS is_deleted;
DEFINE INDEX comment_resolved_idx ON comment COLUMNS is_resolved;

-- =====================================
-- 标签系统
-- =====================================

-- 标签表
DEFINE TABLE tag SCHEMAFULL;
DEFINE FIELD name ON tag TYPE string ASSERT $value != NONE AND string::len($value) > 0 AND string::len($value) <= 50;
DEFINE FIELD slug ON tag TYPE string ASSERT $value != NONE AND string::len($value) > 0 AND string::len($value) <= 50;
DEFINE FIELD description ON tag TYPE string;
DEFINE FIELD color ON tag TYPE string DEFAULT "#3b82f6" ASSERT string::len($value) = 7;
DEFINE FIELD space_id ON tag TYPE option<record<space>>; -- null表示全局标签
DEFINE FIELD usage_count ON tag TYPE number DEFAULT 0;
DEFINE FIELD created_by ON tag TYPE string ASSERT $value != NONE;
DEFINE FIELD created_at ON tag TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON tag TYPE datetime DEFAULT time::now();

-- 标签索引
DEFINE INDEX tag_space_name_idx ON tag COLUMNS space_id, name UNIQUE;
DEFINE INDEX tag_space_slug_idx ON tag COLUMNS space_id, slug UNIQUE;
DEFINE INDEX tag_usage_idx ON tag COLUMNS usage_count;

-- 文档标签关联表
DEFINE TABLE document_tag SCHEMAFULL;
DEFINE FIELD document_id ON document_tag TYPE record<document> ASSERT $value != NONE;
DEFINE FIELD tag_id ON document_tag TYPE record<tag> ASSERT $value != NONE;
DEFINE FIELD tagged_by ON document_tag TYPE string ASSERT $value != NONE;
DEFINE FIELD tagged_at ON document_tag TYPE datetime DEFAULT time::now();

-- 文档标签关联索引
DEFINE INDEX document_tag_unique_idx ON document_tag COLUMNS document_id, tag_id UNIQUE;
DEFINE INDEX document_tag_doc_idx ON document_tag COLUMNS document_id;
DEFINE INDEX document_tag_tag_idx ON document_tag COLUMNS tag_id;

-- =====================================
-- 搜索和索引系统
-- =====================================

-- 搜索索引表 (全文搜索)
DEFINE TABLE search_index SCHEMAFULL;
DEFINE FIELD document_id ON search_index TYPE record<document> ASSERT $value != NONE;
DEFINE FIELD space_id ON search_index TYPE record<space> ASSERT $value != NONE;
DEFINE FIELD title ON search_index TYPE string;
DEFINE FIELD content ON search_index TYPE string; -- 纯文本内容
DEFINE FIELD excerpt ON search_index TYPE string;
DEFINE FIELD tags ON search_index TYPE array<string> DEFAULT [];
DEFINE FIELD author_id ON search_index TYPE string ASSERT $value != NONE;
DEFINE FIELD is_public ON search_index TYPE bool DEFAULT false;
DEFINE FIELD last_updated ON search_index TYPE datetime DEFAULT time::now();

-- 搜索索引
DEFINE INDEX search_document_idx ON search_index COLUMNS document_id UNIQUE;
DEFINE INDEX search_space_idx ON search_index COLUMNS space_id;
DEFINE INDEX search_public_idx ON search_index COLUMNS is_public;
DEFINE INDEX search_author_idx ON search_index COLUMNS author_id;

-- =====================================
-- 用户交互系统
-- =====================================

-- 用户收藏表
DEFINE TABLE user_favorite SCHEMAFULL;
DEFINE FIELD user_id ON user_favorite TYPE string ASSERT $value != NONE;
DEFINE FIELD resource_type ON user_favorite TYPE string DEFAULT "document" ASSERT $value INSIDE ["document", "space"];
DEFINE FIELD resource_id ON user_favorite TYPE string ASSERT $value != NONE;
DEFINE FIELD created_at ON user_favorite TYPE datetime DEFAULT time::now();

-- 收藏索引
DEFINE INDEX favorite_user_resource_idx ON user_favorite COLUMNS user_id, resource_type, resource_id UNIQUE;
DEFINE INDEX favorite_user_idx ON user_favorite COLUMNS user_id;

-- 文档访问记录表
DEFINE TABLE document_view SCHEMAFULL;
DEFINE FIELD document_id ON document_view TYPE record<document> ASSERT $value != NONE;
DEFINE FIELD user_id ON document_view TYPE string ASSERT $value != NONE;
DEFINE FIELD ip_address ON document_view TYPE string;
DEFINE FIELD user_agent ON document_view TYPE string;
DEFINE FIELD duration ON document_view TYPE number DEFAULT 0; -- 阅读时长(秒)
DEFINE FIELD viewed_at ON document_view TYPE datetime DEFAULT time::now();

-- 访问记录索引
DEFINE INDEX view_document_idx ON document_view COLUMNS document_id;
DEFINE INDEX view_user_idx ON document_view COLUMNS user_id;
DEFINE INDEX view_date_idx ON document_view COLUMNS viewed_at;

-- =====================================
-- 系统日志和审计
-- =====================================

-- 活动日志表
DEFINE TABLE activity_log SCHEMAFULL;
DEFINE FIELD user_id ON activity_log TYPE string ASSERT $value != NONE;
DEFINE FIELD action ON activity_log TYPE string ASSERT $value != NONE;
DEFINE FIELD resource_type ON activity_log TYPE string ASSERT $value INSIDE ["space", "document", "comment", "tag", "version"];
DEFINE FIELD resource_id ON activity_log TYPE string ASSERT $value != NONE;
DEFINE FIELD ip_address ON activity_log TYPE string;
DEFINE FIELD user_agent ON activity_log TYPE string;
DEFINE FIELD details ON activity_log TYPE object DEFAULT {};
DEFINE FIELD created_at ON activity_log TYPE datetime DEFAULT time::now();

-- 活动日志索引
DEFINE INDEX activity_user_idx ON activity_log COLUMNS user_id;
DEFINE INDEX activity_resource_idx ON activity_log COLUMNS resource_type, resource_id;
DEFINE INDEX activity_created_idx ON activity_log COLUMNS created_at;
DEFINE INDEX activity_action_idx ON activity_log COLUMNS action;

-- =====================================
-- 文件和媒体管理
-- =====================================

-- 文件上传表
DEFINE TABLE file_upload SCHEMAFULL;
DEFINE FIELD filename ON file_upload TYPE string ASSERT $value != NONE;
DEFINE FIELD original_name ON file_upload TYPE string ASSERT $value != NONE;
DEFINE FIELD file_path ON file_upload TYPE string ASSERT $value != NONE;
DEFINE FIELD file_size ON file_upload TYPE number ASSERT $value > 0;
DEFINE FIELD file_type ON file_upload TYPE string ASSERT $value != NONE;
DEFINE FIELD mime_type ON file_upload TYPE string ASSERT $value != NONE;
DEFINE FIELD uploaded_by ON file_upload TYPE string ASSERT $value != NONE;
DEFINE FIELD space_id ON file_upload TYPE option<record<space>>;
DEFINE FIELD document_id ON file_upload TYPE option<record<document>>;
DEFINE FIELD is_deleted ON file_upload TYPE bool DEFAULT false;
DEFINE FIELD deleted_at ON file_upload TYPE datetime;
DEFINE FIELD deleted_by ON file_upload TYPE string;
DEFINE FIELD created_at ON file_upload TYPE datetime DEFAULT time::now();

-- 文件索引
DEFINE INDEX file_uploader_idx ON file_upload COLUMNS uploaded_by;
DEFINE INDEX file_space_idx ON file_upload COLUMNS space_id;
DEFINE INDEX file_document_idx ON file_upload COLUMNS document_id;
DEFINE INDEX file_deleted_idx ON file_upload COLUMNS is_deleted;
DEFINE INDEX file_type_idx ON file_upload COLUMNS file_type;

-- =====================================
-- 通知系统
-- =====================================

-- 通知表
DEFINE TABLE notification SCHEMAFULL;
DEFINE FIELD user_id ON notification TYPE string ASSERT $value != NONE;
DEFINE FIELD type ON notification TYPE string ASSERT $value INSIDE ["space_invitation", "document_shared", "comment_mention", "document_update", "system"];
DEFINE FIELD title ON notification TYPE string ASSERT $value != NONE;
DEFINE FIELD content ON notification TYPE string ASSERT $value != NONE;
DEFINE FIELD data ON notification TYPE option<object>; -- 额外的数据，如邀请令牌、文档ID等
DEFINE FIELD invite_token ON notification TYPE option<string>; -- 空间邀请令牌
DEFINE FIELD space_name ON notification TYPE option<string>; -- 空间名称
DEFINE FIELD role ON notification TYPE option<string>; -- 邀请角色
DEFINE FIELD inviter_name ON notification TYPE option<string>; -- 邀请者名称
DEFINE FIELD is_read ON notification TYPE bool DEFAULT false;
DEFINE FIELD read_at ON notification TYPE option<datetime>;
DEFINE FIELD created_at ON notification TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON notification TYPE datetime DEFAULT time::now();

-- 索引
DEFINE INDEX notification_user_idx ON notification COLUMNS user_id;
DEFINE INDEX notification_user_unread_idx ON notification COLUMNS user_id, is_read;
DEFINE INDEX notification_created_idx ON notification COLUMNS created_at;
DEFINE INDEX notification_invite_token_idx ON notification COLUMNS invite_token;




-- SoulBook 发布功能数据库表结构
-- 用于实现 GitBook 风格的文档集发布功能

-- 空间发布记录表
-- 存储每次发布的快照信息
DEFINE TABLE space_publication SCHEMAFULL;

DEFINE FIELD space_id ON TABLE space_publication TYPE string ASSERT $value != NONE;
DEFINE FIELD slug ON TABLE space_publication TYPE string ASSERT $value != NONE;
DEFINE FIELD version ON TABLE space_publication TYPE number DEFAULT 1;
DEFINE FIELD title ON TABLE space_publication TYPE string ASSERT $value != NONE;
DEFINE FIELD description ON TABLE space_publication TYPE option<string>;
DEFINE FIELD cover_image ON TABLE space_publication TYPE option<string>;
DEFINE FIELD theme ON TABLE space_publication TYPE string DEFAULT 'default';

-- 发布设置
DEFINE FIELD include_private_docs ON TABLE space_publication TYPE bool DEFAULT false;
DEFINE FIELD enable_search ON TABLE space_publication TYPE bool DEFAULT true;
DEFINE FIELD enable_comments ON TABLE space_publication TYPE bool DEFAULT false;
DEFINE FIELD custom_css ON TABLE space_publication TYPE option<string>;
DEFINE FIELD custom_js ON TABLE space_publication TYPE option<string>;

-- SEO 设置
DEFINE FIELD seo_title ON TABLE space_publication TYPE option<string>;
DEFINE FIELD seo_description ON TABLE space_publication TYPE option<string>;
DEFINE FIELD seo_keywords ON TABLE space_publication TYPE array<string> DEFAULT [];

-- 状态和时间戳
DEFINE FIELD is_active ON TABLE space_publication TYPE bool DEFAULT true;
DEFINE FIELD is_deleted ON TABLE space_publication TYPE bool DEFAULT false;
DEFINE FIELD published_by ON TABLE space_publication TYPE string ASSERT $value != NONE;
DEFINE FIELD published_at ON TABLE space_publication TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON TABLE space_publication TYPE datetime DEFAULT time::now();
DEFINE FIELD deleted_at ON TABLE space_publication TYPE option<datetime>;

-- 索引
DEFINE INDEX idx_space_publication_slug ON TABLE space_publication COLUMNS slug UNIQUE;
DEFINE INDEX idx_space_publication_space_id ON TABLE space_publication COLUMNS space_id;
DEFINE INDEX idx_space_publication_active ON TABLE space_publication COLUMNS is_active;

-- 发布的文档快照表
-- 存储发布时文档的完整快照
DEFINE TABLE publication_document SCHEMAFULL;

DEFINE FIELD publication_id ON TABLE publication_document TYPE string ASSERT $value != NONE;
DEFINE FIELD original_doc_id ON TABLE publication_document TYPE string ASSERT $value != NONE;

-- 文档内容快照
DEFINE FIELD title ON TABLE publication_document TYPE string ASSERT $value != NONE;
DEFINE FIELD slug ON TABLE publication_document TYPE string ASSERT $value != NONE;
DEFINE FIELD content ON TABLE publication_document TYPE string ASSERT $value != NONE;
DEFINE FIELD excerpt ON TABLE publication_document TYPE option<string>;

-- 文档结构
DEFINE FIELD parent_id ON TABLE publication_document TYPE option<string>;
DEFINE FIELD order_index ON TABLE publication_document TYPE number DEFAULT 0;

-- 文档元数据
DEFINE FIELD word_count ON TABLE publication_document TYPE number DEFAULT 0;
DEFINE FIELD reading_time ON TABLE publication_document TYPE number DEFAULT 0;

-- 时间戳
DEFINE FIELD created_at ON TABLE publication_document TYPE datetime DEFAULT time::now();

-- 索引
DEFINE INDEX idx_publication_document_publication_id ON TABLE publication_document COLUMNS publication_id;
DEFINE INDEX idx_publication_document_original_doc_id ON TABLE publication_document COLUMNS original_doc_id;
DEFINE INDEX idx_publication_document_parent_id ON TABLE publication_document COLUMNS parent_id;
DEFINE INDEX idx_publication_document_slug ON TABLE publication_document COLUMNS publication_id, slug UNIQUE;

-- 发布访问统计表
DEFINE TABLE publication_analytics SCHEMAFULL;

DEFINE FIELD publication_id ON TABLE publication_analytics TYPE string ASSERT $value != NONE;

-- 访问统计
DEFINE FIELD total_views ON TABLE publication_analytics TYPE number DEFAULT 0;
DEFINE FIELD unique_visitors ON TABLE publication_analytics TYPE number DEFAULT 0;

-- 按时间段统计
DEFINE FIELD views_today ON TABLE publication_analytics TYPE number DEFAULT 0;
DEFINE FIELD views_week ON TABLE publication_analytics TYPE number DEFAULT 0;
DEFINE FIELD views_month ON TABLE publication_analytics TYPE number DEFAULT 0;

-- 最热门文档
DEFINE FIELD popular_documents ON TABLE publication_analytics TYPE array<object> DEFAULT [];

-- 更新时间
DEFINE FIELD updated_at ON TABLE publication_analytics TYPE datetime DEFAULT time::now();

-- 索引
DEFINE INDEX idx_publication_analytics_publication_id ON TABLE publication_analytics COLUMNS publication_id UNIQUE;

-- 自定义域名表
DEFINE TABLE publication_domain SCHEMAFULL;

DEFINE FIELD publication_id ON TABLE publication_domain TYPE string ASSERT $value != NONE;
DEFINE FIELD domain ON TABLE publication_domain TYPE string ASSERT $value != NONE;

-- SSL 证书信息
DEFINE FIELD ssl_status ON TABLE publication_domain TYPE string DEFAULT 'pending';
DEFINE FIELD ssl_issued_at ON TABLE publication_domain TYPE option<datetime>;
DEFINE FIELD ssl_expires_at ON TABLE publication_domain TYPE option<datetime>;

-- 状态
DEFINE FIELD is_verified ON TABLE publication_domain TYPE bool DEFAULT false;
DEFINE FIELD is_active ON TABLE publication_domain TYPE bool DEFAULT false;

-- 时间戳
DEFINE FIELD created_at ON TABLE publication_domain TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON TABLE publication_domain TYPE datetime DEFAULT time::now();

-- 索引
DEFINE INDEX idx_publication_domain_publication_id ON TABLE publication_domain COLUMNS publication_id;
DEFINE INDEX idx_publication_domain_domain ON TABLE publication_domain COLUMNS domain UNIQUE;

-- 创建发布历史记录表（用于版本管理）
DEFINE TABLE publication_history SCHEMAFULL;

DEFINE FIELD publication_id ON TABLE publication_history TYPE string ASSERT $value != NONE;
DEFINE FIELD version ON TABLE publication_history TYPE number ASSERT $value != NONE;

-- 变更信息
DEFINE FIELD change_summary ON TABLE publication_history TYPE option<string>;
DEFINE FIELD changed_documents ON TABLE publication_history TYPE array<object> DEFAULT [];

-- 操作信息
DEFINE FIELD published_by ON TABLE publication_history TYPE string ASSERT $value != NONE;
DEFINE FIELD published_at ON TABLE publication_history TYPE datetime DEFAULT time::now();

-- 索引
DEFINE INDEX idx_publication_history_publication_id ON TABLE publication_history COLUMNS publication_id;
DEFINE INDEX idx_publication_history_version ON TABLE publication_history COLUMNS publication_id, version UNIQUE;



-- 空间成员表扩展
-- 添加到现有的docs_schema.sql中

-- 空间成员表
DEFINE TABLE space_member SCHEMAFULL;
DEFINE FIELD space_id ON space_member TYPE record<space> ASSERT $value != NONE;
DEFINE FIELD user_id ON space_member TYPE string ASSERT $value != NONE; -- Rainbow-Auth用户ID
DEFINE FIELD role ON space_member TYPE string DEFAULT "member" ASSERT $value INSIDE ["owner", "admin", "editor", "viewer", "member"];
DEFINE FIELD permissions ON space_member TYPE array<string> DEFAULT ["docs.read"];
DEFINE FIELD invited_by ON space_member TYPE string ASSERT $value != NONE;
DEFINE FIELD invited_at ON space_member TYPE datetime DEFAULT time::now();
DEFINE FIELD accepted_at ON space_member TYPE option<datetime>;
DEFINE FIELD status ON space_member TYPE string DEFAULT "pending" ASSERT $value INSIDE ["pending", "accepted", "rejected", "removed"];
DEFINE FIELD expires_at ON space_member TYPE option<datetime>; -- 邀请过期时间
DEFINE FIELD created_at ON space_member TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON space_member TYPE datetime DEFAULT time::now();

-- 空间成员索引
DEFINE INDEX space_member_unique_idx ON space_member COLUMNS space_id, user_id UNIQUE;
DEFINE INDEX space_member_space_idx ON space_member COLUMNS space_id;
DEFINE INDEX space_member_user_idx ON space_member COLUMNS user_id;
DEFINE INDEX space_member_status_idx ON space_member COLUMNS status;
DEFINE INDEX space_member_role_idx ON space_member COLUMNS role;

-- 空间邀请表（用于邀请链接等）
DEFINE TABLE space_invitation SCHEMAFULL;
DEFINE FIELD space_id ON space_invitation TYPE record<space> ASSERT $value != NONE;
DEFINE FIELD email ON space_invitation TYPE option<string>; -- 被邀请人邮箱
DEFINE FIELD user_id ON space_invitation TYPE option<string>; -- 被邀请人用户ID（如果已注册）
DEFINE FIELD invite_token ON space_invitation TYPE string ASSERT $value != NONE; -- 唯一邀请令牌
DEFINE FIELD role ON space_invitation TYPE string DEFAULT "member" ASSERT $value INSIDE ["admin", "editor", "viewer", "member"];
DEFINE FIELD permissions ON space_invitation TYPE array<string> DEFAULT ["docs.read"];
DEFINE FIELD invited_by ON space_invitation TYPE string ASSERT $value != NONE;
DEFINE FIELD message ON space_invitation TYPE option<string>; -- 邀请消息
DEFINE FIELD max_uses ON space_invitation TYPE number DEFAULT 1; -- 最大使用次数
DEFINE FIELD used_count ON space_invitation TYPE number DEFAULT 0; -- 已使用次数
DEFINE FIELD expires_at ON space_invitation TYPE datetime ASSERT $value != NONE;
DEFINE FIELD created_at ON space_invitation TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON space_invitation TYPE datetime DEFAULT time::now();

-- 邀请索引
DEFINE INDEX space_invitation_token_idx ON space_invitation COLUMNS invite_token UNIQUE;
DEFINE INDEX space_invitation_space_idx ON space_invitation COLUMNS space_id;
DEFINE INDEX space_invitation_inviter_idx ON space_invitation COLUMNS invited_by;
DEFINE INDEX space_invitation_email_idx ON space_invitation COLUMNS email;

-- =====================================
-- 初始数据插入
-- =====================================

-- 插入默认标签
INSERT INTO tag (name, slug, description, color, space_id, created_by) VALUES ("API", "api", "API相关文档", "#10b981", NONE, "system");
INSERT INTO tag (name, slug, description, color, space_id, created_by) VALUES ("Tutorial", "tutorial", "教程文档", "#3b82f6", NONE, "system");
INSERT INTO tag (name, slug, description, color, space_id, created_by) VALUES ("Guide", "guide", "指南文档", "#8b5cf6", NONE, "system");
INSERT INTO tag (name, slug, description, color, space_id, created_by) VALUES ("Reference", "reference", "参考文档", "#f59e0b", NONE, "system");
INSERT INTO tag (name, slug, description, color, space_id, created_by) VALUES ("FAQ", "faq", "常见问题", "#ef4444", NONE, "system");

-- 设置初始使用统计
UPDATE tag SET usage_count = 0 WHERE created_by = "system";

-- =====================================
-- 向量存储系统
-- =====================================

-- 文档向量存储表
-- 用于存储文档的向量嵌入，支持语义搜索
DEFINE TABLE document_vector SCHEMAFULL;
DEFINE FIELD document_id ON document_vector TYPE record<document> ASSERT $value != NONE;
DEFINE FIELD space_id ON document_vector TYPE record<space> ASSERT $value != NONE;
DEFINE FIELD embedding ON document_vector TYPE array<float> ASSERT $value != NONE;
DEFINE FIELD embedding_model ON document_vector TYPE string;
DEFINE FIELD dimension ON document_vector TYPE int ASSERT $value > 0;
DEFINE FIELD metadata ON document_vector TYPE object DEFAULT {};
DEFINE FIELD created_at ON document_vector TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON document_vector TYPE datetime DEFAULT time::now();

-- 向量表索引
DEFINE INDEX idx_document_vector_doc ON document_vector FIELDS document_id;
DEFINE INDEX idx_document_vector_space ON document_vector FIELDS space_id;
DEFINE INDEX idx_document_vector_created ON document_vector FIELDS created_at;
