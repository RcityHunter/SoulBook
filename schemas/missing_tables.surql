-- Missing tables supplemental schema
-- Run with: surreal import --conn http://localhost:8000 --ns rainbow --db docs --user root --pass root missing_tables.surql

-- change_request
DEFINE TABLE change_request SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD title ON change_request TYPE string;
DEFINE FIELD description ON change_request TYPE option<string>;
DEFINE FIELD space_id ON change_request TYPE string;
DEFINE FIELD document_id ON change_request TYPE string;
DEFINE FIELD document_title ON change_request TYPE option<string>;
DEFINE FIELD diff_content ON change_request TYPE option<string>;
DEFINE FIELD reviewer_id ON change_request TYPE option<string>;
DEFINE FIELD author_id ON change_request TYPE string;
DEFINE FIELD status ON change_request TYPE string DEFAULT 'open';
DEFINE FIELD is_deleted ON change_request TYPE bool DEFAULT false;
DEFINE FIELD created_at ON change_request TYPE string;
DEFINE FIELD updated_at ON change_request TYPE string;

-- ai_task
DEFINE TABLE ai_task SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD task_type ON ai_task TYPE string;
DEFINE FIELD document_id ON ai_task TYPE string;
DEFINE FIELD document_title ON ai_task TYPE option<string>;
DEFINE FIELD space_id ON ai_task TYPE option<string>;
DEFINE FIELD model ON ai_task TYPE option<string>;
DEFINE FIELD target_language ON ai_task TYPE option<string>;
DEFINE FIELD extra ON ai_task TYPE option<string>;
DEFINE FIELD status ON ai_task TYPE string DEFAULT 'pending';
DEFINE FIELD progress ON ai_task TYPE int DEFAULT 0;
DEFINE FIELD created_by ON ai_task TYPE string;
DEFINE FIELD is_deleted ON ai_task TYPE bool DEFAULT false;
DEFINE FIELD created_at ON ai_task TYPE string;
DEFINE FIELD updated_at ON ai_task TYPE string;

-- tool_config
DEFINE TABLE tool_config SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD family ON tool_config TYPE string;
DEFINE FIELD title ON tool_config TYPE string;
DEFINE FIELD icon ON tool_config TYPE option<string>;
DEFINE FIELD description ON tool_config TYPE option<string>;
DEFINE FIELD model ON tool_config TYPE option<string>;
DEFINE FIELD approval_required ON tool_config TYPE bool DEFAULT false;
DEFINE FIELD enabled ON tool_config TYPE bool DEFAULT true;
DEFINE FIELD max_tokens ON tool_config TYPE int DEFAULT 4096;
DEFINE FIELD timeout_secs ON tool_config TYPE int DEFAULT 60;
DEFINE FIELD system_prompt ON tool_config TYPE option<string>;
DEFINE FIELD actions ON tool_config TYPE array DEFAULT [];
DEFINE FIELD actions.* ON tool_config TYPE string;
DEFINE FIELD created_at ON tool_config TYPE string;
DEFINE FIELD updated_at ON tool_config TYPE string;

-- git_repo
DEFINE TABLE git_repo SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD space_slug ON git_repo TYPE string;
DEFINE FIELD github_url ON git_repo TYPE string;
DEFINE FIELD branch ON git_repo TYPE string DEFAULT 'main';
DEFINE FIELD direction ON git_repo TYPE string DEFAULT 'bidirectional';
DEFINE FIELD auto_sync ON git_repo TYPE bool DEFAULT false;
DEFINE FIELD path_prefix ON git_repo TYPE option<string>;
DEFINE FIELD status ON git_repo TYPE string DEFAULT 'connected';
DEFINE FIELD last_synced_at ON git_repo TYPE option<string>;
DEFINE FIELD created_by ON git_repo TYPE string;
DEFINE FIELD is_deleted ON git_repo TYPE bool DEFAULT false;
DEFINE FIELD created_at ON git_repo TYPE string;
DEFINE FIELD updated_at ON git_repo TYPE string;

-- git_sync_log
DEFINE TABLE git_sync_log SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD repo_id ON git_sync_log TYPE string;
DEFINE FIELD triggered_by ON git_sync_log TYPE string;
DEFINE FIELD direction ON git_sync_log TYPE string;
DEFINE FIELD status ON git_sync_log TYPE string DEFAULT 'pending';
DEFINE FIELD message ON git_sync_log TYPE option<string>;
DEFINE FIELD created_at ON git_sync_log TYPE string;

-- api_key
DEFINE TABLE api_key SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD name ON api_key TYPE string;
DEFINE FIELD key_hash ON api_key TYPE string;
DEFINE FIELD key_prefix ON api_key TYPE string;
DEFINE FIELD scopes ON api_key TYPE array DEFAULT [];
DEFINE FIELD scopes.* ON api_key TYPE string;
DEFINE FIELD expires_at ON api_key TYPE option<string>;
DEFINE FIELD created_by ON api_key TYPE string;
DEFINE FIELD is_deleted ON api_key TYPE bool DEFAULT false;
DEFINE FIELD last_used_at ON api_key TYPE option<string>;
DEFINE FIELD created_at ON api_key TYPE string;
DEFINE FIELD updated_at ON api_key TYPE string;

-- webhook
DEFINE TABLE webhook SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD name ON webhook TYPE string;
DEFINE FIELD url ON webhook TYPE string;
DEFINE FIELD events ON webhook TYPE array DEFAULT [];
DEFINE FIELD events.* ON webhook TYPE string;
DEFINE FIELD secret ON webhook TYPE option<string>;
DEFINE FIELD enabled ON webhook TYPE bool DEFAULT true;
DEFINE FIELD created_by ON webhook TYPE string;
DEFINE FIELD is_deleted ON webhook TYPE bool DEFAULT false;
DEFINE FIELD created_at ON webhook TYPE string;
DEFINE FIELD updated_at ON webhook TYPE string;

-- webhook_log
DEFINE TABLE webhook_log SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD webhook_id ON webhook_log TYPE string;
DEFINE FIELD status ON webhook_log TYPE int DEFAULT 200;
DEFINE FIELD response_time_ms ON webhook_log TYPE int DEFAULT 0;
DEFINE FIELD message ON webhook_log TYPE option<string>;
DEFINE FIELD created_at ON webhook_log TYPE string;

-- doc_template
DEFINE TABLE doc_template SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD name ON doc_template TYPE string;
DEFINE FIELD category ON doc_template TYPE string;
DEFINE FIELD icon ON doc_template TYPE option<string>;
DEFINE FIELD description ON doc_template TYPE option<string>;
DEFINE FIELD content ON doc_template TYPE option<string>;
DEFINE FIELD usage_count ON doc_template TYPE int DEFAULT 0;
DEFINE FIELD created_by ON doc_template TYPE option<string>;
DEFINE FIELD is_deleted ON doc_template TYPE bool DEFAULT false;
DEFINE FIELD created_at ON doc_template TYPE string;
DEFINE FIELD updated_at ON doc_template TYPE string;
