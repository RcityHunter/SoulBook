use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub server: ServerConfig,
    pub features: FeatureConfig,
    pub oauth: OAuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuthConfig {
    pub google: Option<GoogleOAuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub user: String,
    pub pass: String,
    pub namespace: String,
    pub database: String,
    pub connection_timeout: u64,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub jwt_expiration: u64,
    pub rainbow_auth_url: Option<String>, // Rainbow-Auth服务地址
    pub integration_mode: bool,           // 是否集成Rainbow-Auth
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub app_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureConfig {
    pub enable_pdf_export: bool,
    pub enable_notifications: bool,
    pub enable_comments: bool,
    pub enable_versioning: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database = DatabaseConfig {
            url: env::var("DATABASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string()),
            user: env::var("DATABASE_USER").unwrap_or_else(|_| "root".to_string()),
            pass: env::var("DATABASE_PASS").unwrap_or_else(|_| "root".to_string()),
            namespace: env::var("DATABASE_NAMESPACE").unwrap_or_else(|_| "docs".to_string()),
            database: env::var("DATABASE_DB").unwrap_or_else(|_| "main".to_string()),
            connection_timeout: env::var("DATABASE_CONNECTION_TIMEOUT")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap_or(30),
            max_connections: env::var("DATABASE_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .unwrap_or(10),
        };

        let auth = AuthConfig {
            jwt_secret: env::var("JWT_SECRET")
                .map_err(|_| anyhow::anyhow!("JWT_SECRET environment variable is required"))?,
            jwt_expiration: env::var("JWT_EXPIRATION")
                .unwrap_or_else(|_| "86400".to_string())
                .parse()
                .unwrap_or(86400),
            rainbow_auth_url: env::var("RAINBOW_AUTH_URL").ok(),
            integration_mode: env::var("RAINBOW_AUTH_INTEGRATION")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
        };

        let server = ServerConfig {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .unwrap_or(3000),
            app_url: env::var("APP_URL").unwrap_or_else(|_| "http://localhost:3000".to_string()),
        };

        let features = FeatureConfig {
            enable_pdf_export: env::var("ENABLE_PDF_EXPORT")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            enable_notifications: env::var("ENABLE_NOTIFICATIONS")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
            enable_comments: env::var("ENABLE_COMMENTS")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
            enable_versioning: env::var("ENABLE_VERSIONING")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
        };

        let oauth = OAuthConfig {
            google: match (
                env::var("GOOGLE_CLIENT_ID"),
                env::var("GOOGLE_CLIENT_SECRET"),
                env::var("GOOGLE_REDIRECT_URI"),
            ) {
                (Ok(client_id), Ok(client_secret), Ok(redirect_uri))
                    if !client_id.is_empty()
                        && !client_secret.is_empty()
                        && !redirect_uri.is_empty() =>
                {
                    Some(GoogleOAuthConfig {
                        client_id,
                        client_secret,
                        redirect_uri,
                    })
                }
                _ => None,
            },
        };

        Ok(Config {
            database,
            auth,
            server,
            features,
            oauth,
        })
    }
}
