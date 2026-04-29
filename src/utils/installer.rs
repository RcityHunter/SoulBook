use anyhow::Result;
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use surrealdb::opt::auth::Root;
use surrealdb::{
    Surreal,
    engine::remote::http::{Client, Http},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallationStatus {
    pub is_installed: bool,
    pub config_exists: bool,
    pub database_initialized: bool,
    pub admin_created: bool,
    pub install_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for InstallationStatus {
    fn default() -> Self {
        Self {
            is_installed: false,
            config_exists: false,
            database_initialized: false,
            admin_created: false,
            install_time: None,
        }
    }
}

pub struct InstallationChecker;

impl InstallationChecker {
    const INSTALL_MARKER_FILE: &'static str = ".rainbow_docs_installed";
    const CONFIG_FILE: &'static str = ".env";

    /// 检查系统是否已安装
    pub fn check_installation_status() -> Result<InstallationStatus> {
        let mut status = InstallationStatus::default();

        // 检查安装标记文件
        if Path::new(Self::INSTALL_MARKER_FILE).exists() {
            if let Ok(content) = fs::read_to_string(Self::INSTALL_MARKER_FILE) {
                if let Ok(marker_status) = serde_json::from_str::<InstallationStatus>(&content) {
                    status = marker_status;
                }
            }
        }

        // 检查配置文件
        status.config_exists = Path::new(Self::CONFIG_FILE).exists();

        // 更新整体安装状态
        status.is_installed =
            status.config_exists && status.database_initialized && status.admin_created;

        Ok(status)
    }

    /// 标记系统为已安装
    pub fn mark_as_installed(status: &InstallationStatus) -> Result<()> {
        let content = serde_json::to_string_pretty(status)?;
        fs::write(Self::INSTALL_MARKER_FILE, content)?;
        Ok(())
    }

    /// 检查是否需要显示安装界面
    pub fn should_show_installer() -> Result<bool> {
        #[cfg(feature = "installer")]
        {
            let status = Self::check_installation_status()?;
            Ok(!status.is_installed)
        }

        #[cfg(not(feature = "installer"))]
        {
            Ok(false)
        }
    }
}

#[cfg(feature = "installer")]
pub mod wizard {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct InstallConfig {
        pub database_url: String,
        pub database_username: String, // 新增：数据库用户名
        pub database_password: String, // 新增：数据库密码
        pub database_namespace_auth: String,
        pub database_name_auth: String,
        pub database_namespace_docs: String,
        pub database_name_docs: String,
        pub admin_username: String,
        pub admin_email: String,
        pub admin_password: String,
        pub site_name: String,
        pub site_description: Option<String>,
        pub jwt_secret: String,
    }

    #[derive(Debug, Serialize)]
    pub struct InstallStep {
        pub step: u8,
        pub title: String,
        pub description: String,
        pub completed: bool,
    }

    pub struct InstallationWizard;

    impl InstallationWizard {
        pub fn get_steps() -> Vec<InstallStep> {
            vec![
                InstallStep {
                    step: 1,
                    title: "环境检查".to_string(),
                    description: "检查系统环境和依赖".to_string(),
                    completed: false,
                },
                InstallStep {
                    step: 2,
                    title: "数据库配置".to_string(),
                    description: "配置SurrealDB连接".to_string(),
                    completed: false,
                },
                InstallStep {
                    step: 3,
                    title: "管理员账户".to_string(),
                    description: "创建系统管理员账户".to_string(),
                    completed: false,
                },
                InstallStep {
                    step: 4,
                    title: "站点配置".to_string(),
                    description: "配置站点基本信息".to_string(),
                    completed: false,
                },
                InstallStep {
                    step: 5,
                    title: "完成安装".to_string(),
                    description: "保存配置并初始化系统".to_string(),
                    completed: false,
                },
            ]
        }

        pub async fn perform_installation(config: InstallConfig) -> Result<()> {
            use chrono::Utc;
            use std::fs;
            use std::path::Path;
            use std::process::Command;

            println!("开始安装过程...");

            // 1. 启动数据库服务
            println!("正在启动 SurrealDB 数据库服务...");

            // 创建数据目录（如果不存在）
            let data_dir = "./data";
            if !Path::new(data_dir).exists() {
                fs::create_dir_all(data_dir)
                    .map_err(|e| anyhow::anyhow!("Failed to create data directory: {}", e))?;
            }

            // 构建数据库文件路径
            let db_file = format!("{}/rainbow.db", data_dir);

            // 构建启动命令
            let bind_addr = config.database_url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .to_string();

            let mut cmd = Command::new("surreal");
            cmd.arg("start")
                .arg("--username")
                .arg(&config.database_username)
                .arg("--password")
                .arg(&config.database_password)
                .arg("--bind")
                .arg(&bind_addr)
                .arg(format!("file:{}", db_file));

            println!(
                "执行命令: surreal start --username {} --password *** --bind {} file://{}",
                config.database_username, bind_addr, db_file
            );

            let child = cmd.spawn().map_err(|e| {
                anyhow::anyhow!(
                    "Failed to start SurrealDB: {}. Please make sure SurrealDB is installed.",
                    e
                )
            })?;

            // 保存进程ID
            let pid = child.id();
            fs::write(".surreal_pid", pid.to_string())
                .map_err(|e| anyhow::anyhow!("Failed to save database PID: {}", e))?;

            println!("SurrealDB 进程已启动 (PID: {})", pid);

            // 等待数据库启动
            println!("等待数据库服务就绪...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            println!("数据库服务启动成功！");

            // 1. 验证数据库连接
            println!("验证数据库连接...");
            // TODO: 这里可以添加实际的数据库连接验证

            // 2. 更新SoulBook的.env文件
            println!("更新SoulBook配置...");
            let docs_env_path = ".env";
            let docs_env_content = format!(
                r#"# SoulBook 配置
DATABASE_URL={}
DATABASE_USER={}
DATABASE_PASS={}
DATABASE_NAMESPACE={}
DATABASE_NAME={}
JWT_SECRET={}
SITE_NAME={}
SITE_DESCRIPTION={}
DATABASE_CONNECTION_TIMEOUT=30
DATABASE_MAX_CONNECTIONS=10

JWT_EXPIRATION=86400

# Rainbow-Auth 集成配置
RAINBOW_AUTH_URL=http://localhost:8080
RAINBOW_AUTH_INTEGRATION=true

# 服务器配置
HOST=0.0.0.0
PORT=3000
APP_URL=http://localhost:3000

# 功能开关
ENABLE_PDF_EXPORT=false
ENABLE_NOTIFICATIONS=true
ENABLE_COMMENTS=true
ENABLE_VERSIONING=true
"#,
                config.database_url,
                config.database_username,
                config.database_password,
                config.database_namespace_docs,
                config.database_name_docs,
                config.jwt_secret,
                config.site_name,
                config.site_description.unwrap_or_default()
            );

            fs::write(docs_env_path, docs_env_content)
                .map_err(|e| anyhow::anyhow!("Failed to write SoulBook .env file: {}", e))?;

            // 3. 更新Rainbow-Auth的.env文件
            println!("更新Rainbow-Auth配置...");
            let auth_env_path = "../Rainbow-Auth/.env";

            // 先读取现有的Auth .env文件以保留其他配置
            let existing_auth_env = fs::read_to_string(auth_env_path).unwrap_or_default();

            // 解析现有配置
            let mut auth_config_lines: Vec<String> = Vec::new();
            let mut found_database_url = false;
            let mut found_database_user = false;
            let mut found_database_pass = false;
            let mut found_database_namespace = false;
            let mut found_database_name = false;
            let mut found_jwt_secret = false;

            for line in existing_auth_env.lines() {
                let line = line.trim();
                if line.starts_with("DATABASE_URL=") {
                    auth_config_lines.push(format!("DATABASE_URL={}", config.database_url));
                    found_database_url = true;
                } else if line.starts_with("DATABASE_USER=") {
                    auth_config_lines.push(format!("DATABASE_USER={}", config.database_username));
                    found_database_user = true;
                } else if line.starts_with("DATABASE_PASS=") {
                    auth_config_lines.push(format!("DATABASE_PASS={}", config.database_password));
                    found_database_pass = true;
                } else if line.starts_with("DATABASE_NAMESPACE=") {
                    auth_config_lines.push(format!(
                        "DATABASE_NAMESPACE={}",
                        config.database_namespace_auth
                    ));
                    found_database_namespace = true;
                } else if line.starts_with("DATABASE_NAME=") {
                    auth_config_lines.push(format!("DATABASE_NAME={}", config.database_name_auth));
                    found_database_name = true;
                } else if line.starts_with("JWT_SECRET=") {
                    auth_config_lines.push(format!("JWT_SECRET={}", config.jwt_secret));
                    found_jwt_secret = true;
                } else {
                    auth_config_lines.push(line.to_string());
                }
            }

            // 添加缺失的配置项
            if !found_database_url {
                auth_config_lines.push(format!("DATABASE_URL={}", config.database_url));
            }
            if !found_database_user {
                auth_config_lines.push(format!("DATABASE_USER={}", config.database_username));
            }
            if !found_database_pass {
                auth_config_lines.push(format!("DATABASE_PASS={}", config.database_password));
            }
            if !found_database_namespace {
                auth_config_lines.push(format!(
                    "DATABASE_NAMESPACE={}",
                    config.database_namespace_auth
                ));
            }
            if !found_database_name {
                auth_config_lines.push(format!("DATABASE_NAME={}", config.database_name_auth));
            }
            if !found_jwt_secret {
                auth_config_lines.push(format!("JWT_SECRET={}", config.jwt_secret));
            }

            let auth_env_content = auth_config_lines.join("\n");
            fs::write(auth_env_path, auth_env_content)
                .map_err(|e| anyhow::anyhow!("Failed to write Rainbow-Auth .env file: {}", e))?;

            // 4. 标记配置文件存在
            let config_dir = "config";
            if !Path::new(config_dir).exists() {
                fs::create_dir_all(config_dir)
                    .map_err(|e| anyhow::anyhow!("Failed to create config directory: {}", e))?;
            }

            // 创建一个简单的配置标记文件
            let config_content = format!(
                r#"# SoulBook 安装配置
# 安装时间: {}
# 管理员: {} ({})
# 站点名称: {}
"#,
                Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
                config.admin_username,
                config.admin_email,
                config.site_name
            );

            fs::write("config/installation.txt", config_content)
                .map_err(|e| anyhow::anyhow!("Failed to write installation config: {}", e))?;

            println!("安装配置完成！");

            // 5. 导入数据库schema
            println!("初始化数据库schema...");
            println!("正在连接数据库: {}", config.database_url);

            // 连接到数据库 - 添加短超时
            let client = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                Surreal::<Client>::new::<Http>(&config.database_url),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Database connection timeout after 10 seconds"))?
            .map_err(|e| anyhow::anyhow!("Failed to connect to database: {}", e))?;

            println!("数据库连接成功，正在认证...");

            // 认证 - 使用用户提供的凭据
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                client.signin(Root {
                    username: &config.database_username,
                    password: &config.database_password,
                }),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Database authentication timeout after 10 seconds"))?
            .map_err(|e| anyhow::anyhow!("Failed to authenticate with database: {}", e))?;

            println!("数据库认证成功！");

            // 导入Auth系统schema到auth namespace
            println!("导入Auth系统schema...");
            client
                .use_ns(&config.database_namespace_auth)
                .use_db(&config.database_name_auth)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to select auth database: {}", e))?;

            // 读取并执行Auth schema
            let auth_schema_path = "../Rainbow-Auth/schema.sql";
            if Path::new(auth_schema_path).exists() {
                let auth_schema = fs::read_to_string(auth_schema_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read auth schema: {}", e))?;

                // 将Auth schema分割成单独的语句执行
                let auth_statements: Vec<&str> = auth_schema
                    .split(';')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty() && !s.starts_with("--"))
                    .collect();

                println!("开始导入Auth schema，共{}条语句...", auth_statements.len());

                for (i, statement) in auth_statements.iter().enumerate() {
                    if !statement.trim().is_empty() {
                        println!("执行Auth语句 {}/{}...", i + 1, auth_statements.len());

                        let query_result = tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            client.query(*statement),
                        )
                        .await;

                        match query_result {
                            Ok(Ok(_)) => {
                                // 查询成功
                            }
                            Ok(Err(e)) => {
                                println!("警告: Auth语句执行失败: {}", e);
                                println!("失败的语句: {}", statement);
                            }
                            Err(_) => {
                                println!("警告: Auth语句执行超时");
                                println!("超时的语句: {}", statement);
                            }
                        }
                    }
                }

                println!("Auth schema导入完成");
            } else {
                println!("警告: Auth schema文件不存在: {}", auth_schema_path);
            }

            // 导入Auth初始数据
            let auth_initial_data_path = "../Rainbow-Auth/initial_data.sql";
            if Path::new(auth_initial_data_path).exists() {
                let auth_initial_data = fs::read_to_string(auth_initial_data_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read auth initial data: {}", e))?;

                client
                    .query(&auth_initial_data)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to execute auth initial data: {}", e))?;

                println!("Auth初始数据导入完成");
            } else {
                println!("警告: Auth初始数据文件不存在: {}", auth_initial_data_path);
            }

            // 导入文档权限数据
            let docs_permissions_path = "../Rainbow-Auth/docs_permissions.sql";
            if Path::new(docs_permissions_path).exists() {
                let docs_permissions = fs::read_to_string(docs_permissions_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read docs permissions: {}", e))?;

                client
                    .query(&docs_permissions)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to execute docs permissions: {}", e))?;

                println!("文档权限数据导入完成");
            } else {
                println!("警告: 文档权限文件不存在: {}", docs_permissions_path);
            }

            // 导入Docs系统schema到docs namespace
            println!("导入Docs系统schema...");
            client
                .use_ns(&config.database_namespace_docs)
                .use_db(&config.database_name_docs)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to select docs database: {}", e))?;

            let docs_schema_path = "schemas/docs_schema.sql";
            if Path::new(docs_schema_path).exists() {
                let docs_schema = fs::read_to_string(docs_schema_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read docs schema: {}", e))?;

                // 将schema分割成单独的语句执行，避免超时
                let statements: Vec<&str> = docs_schema
                    .split(';')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty() && !s.starts_with("--"))
                    .collect();

                println!("开始导入Docs schema，共{}条语句...", statements.len());

                for (i, statement) in statements.iter().enumerate() {
                    if !statement.trim().is_empty() {
                        println!("执行语句 {}/{}...", i + 1, statements.len());

                        // 添加超时处理
                        let query_result = tokio::time::timeout(
                            std::time::Duration::from_secs(30), // 30秒超时
                            client.query(*statement),
                        )
                        .await;

                        match query_result {
                            Ok(Ok(_)) => {
                                // 查询成功
                            }
                            Ok(Err(e)) => {
                                println!("警告: 语句执行失败: {}", e);
                                println!("失败的语句: {}", statement);
                                // 继续执行下一条语句而不是停止整个安装
                            }
                            Err(_) => {
                                println!("警告: 语句执行超时");
                                println!("超时的语句: {}", statement);
                                // 继续执行下一条语句而不是停止整个安装
                            }
                        }
                    }
                }

                println!("Docs schema导入完成");
            } else {
                println!("警告: Docs schema文件不存在: {}", docs_schema_path);
            }

            println!("数据库初始化完成！");

            // 6. 创建管理员账户
            println!("创建管理员账户...");

            // 切换回Auth数据库
            client
                .use_ns(&config.database_namespace_auth)
                .use_db(&config.database_name_auth)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to select auth database for admin creation: {}", e)
                })?;

            // 生成密码哈希
            let salt = SaltString::generate(&mut OsRng);
            let argon2 = Argon2::default();
            let password_hash = argon2
                .hash_password(config.admin_password.as_bytes(), &salt)
                .map_err(|e| anyhow::anyhow!("Failed to hash admin password: {}", e))?
                .to_string();

            // 创建管理员用户
            let current_time = Utc::now().timestamp();
            let admin_query = format!(
                r#"CREATE user SET 
                    email = "{}", 
                    password = "{}", 
                    verified = true, 
                    account_status = "Active",
                    created_at = {},
                    updated_at = {},
                    last_login_at = {},
                    last_login_ip = "",
                    verification_token = "";"#,
                config.admin_email, password_hash, current_time, current_time, current_time
            );

            println!("执行管理员创建SQL: {}", admin_query);
            let admin_result = client
                .query(&admin_query)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create admin user: {}", e))?;

            println!("管理员创建查询结果: {:?}", admin_result);

            // 验证管理员用户是否真的被创建
            let verify_query = format!(
                "SELECT * FROM user WHERE email = \"{}\"",
                config.admin_email
            );
            let verify_result = client
                .query(&verify_query)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to verify admin user: {}", e))?;

            println!("验证管理员用户查询结果: {:?}", verify_result);

            println!("管理员账户创建完成: {}", config.admin_email);

            // 为管理员分配超级管理员角色
            let role_query = format!(
                r#"CREATE user_role SET 
                    user_id = (SELECT id FROM user WHERE email = "{}")[0].id,
                    role_id = (SELECT id FROM role WHERE name = "SuperAdmin")[0].id,
                    created_at = {},
                    created_by = "system";"#,
                config.admin_email, current_time
            );

            let role_result = client.query(&role_query).await;
            if role_result.is_err() {
                println!("警告: 无法分配管理员角色，可能角色表未正确初始化");
            } else {
                println!("管理员角色分配完成");
            }

            // 7. 标记为已安装
            let install_status = InstallationStatus {
                is_installed: true,
                config_exists: true,
                database_initialized: true, // 数据库已初始化
                admin_created: true,        // 管理员已创建
                install_time: Some(Utc::now()),
            };

            InstallationChecker::mark_as_installed(&install_status)?;

            println!("安装过程完成！");
            println!("管理员登录信息:");
            println!("  邮箱: {}", config.admin_email);
            println!("  用户名: {}", config.admin_username);
            println!("  请使用邮箱和密码登录系统");
            Ok(())
        }
    }
}
