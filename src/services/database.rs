use crate::config::Config;
use crate::error::{AppError, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value as JsonValue};
use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use surrealdb::types::{RecordId as Thing, RecordIdKey, Value as SurrealValue};
use surrealdb::IndexedResults;
use tracing::{error, info};

/// 客户端包装器，提供完全兼容的 SurrealDB API
#[derive(Clone)]
pub struct ClientWrapper {
    storage: Arc<soulcore::engines::storage::StorageEngine>,
}

pub fn record_key_to_string(key: &RecordIdKey) -> String {
    match key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        other => format!("{:?}", other),
    }
}

pub fn record_id_to_string(id: &Thing) -> String {
    format!("{}:{}", id.table.as_str(), record_key_to_string(&id.key))
}

pub fn record_id_key(id: &Thing) -> String {
    record_key_to_string(&id.key)
}

pub struct Response {
    inner: surrealdb::IndexedResults,
}

pub enum TakeIndex {
    Statement(usize),
    Field(usize, String),
}

impl From<usize> for TakeIndex {
    fn from(value: usize) -> Self {
        TakeIndex::Statement(value)
    }
}

impl From<&str> for TakeIndex {
    fn from(value: &str) -> Self {
        TakeIndex::Field(0, value.to_string())
    }
}

impl From<(usize, &str)> for TakeIndex {
    fn from((idx, field): (usize, &str)) -> Self {
        TakeIndex::Field(idx, field.to_string())
    }
}

impl From<(usize, String)> for TakeIndex {
    fn from((idx, field): (usize, String)) -> Self {
        TakeIndex::Field(idx, field)
    }
}

impl Response {
    pub fn new(inner: surrealdb::IndexedResults) -> Self {
        Self { inner }
    }

    pub fn take<T: DeserializeOwned + 'static>(
        &mut self,
        index: impl Into<TakeIndex>,
    ) -> std::result::Result<T, surrealdb::Error> {
        match index.into() {
            TakeIndex::Statement(idx) => {
                let surreal_val: surrealdb::types::Value = self
                    .inner
                    .take(idx)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                let raw = surreal_val.into_json_value();
                decode_take_value::<T>(raw)
            }
            TakeIndex::Field(stmt_idx, field) => {
                let surreal_val: surrealdb::types::Value = self
                    .inner
                    .take(stmt_idx)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                let raw = surreal_val.into_json_value();
                let extracted = raw.get(&field).cloned().unwrap_or(serde_json::Value::Null);
                decode_take_value::<T>(extracted)
            }
        }
    }
}

fn decode_take_value<T: DeserializeOwned + 'static>(
    json_raw: JsonValue,
) -> std::result::Result<T, surrealdb::Error> {
    // Keep original payload for callers explicitly asking for JSON.
    if TypeId::of::<T>() == TypeId::of::<JsonValue>() {
        return serde_json::from_value(json_raw)
            .map_err(|e| surrealdb::Error::thrown(e.to_string()));
    }

    let json = normalize_query_json(json_raw);

    match serde_json::from_value(json.clone()) {
        Ok(v) => Ok(v),
        Err(first_err) => {
            // Surreal3 + wrapper normalization may yield `[]` for "no row".
            // For `Option<U>` call sites, interpreting empty array as `null` avoids
            // false 500s and correctly returns `None`.
            if matches!(json, JsonValue::Array(ref arr) if arr.is_empty()) {
                if let Ok(v) = serde_json::from_value(JsonValue::Null) {
                    return Ok(v);
                }
            }

            // object -> [object] for Vec<T>
            if json.is_object() {
                let wrapped = JsonValue::Array(vec![json.clone()]);
                if let Ok(v) = serde_json::from_value(wrapped) {
                    return Ok(v);
                }
            }

            // [object] -> object for T
            if let Some(arr) = json.as_array() {
                if arr.len() == 1 {
                    if let Ok(v) = serde_json::from_value(arr[0].clone()) {
                        return Ok(v);
                    }
                }
            }

            Err(surrealdb::Error::thrown(first_err.to_string()))
        }
    }
}

fn normalize_query_json(mut v: JsonValue) -> JsonValue {
    v = detag_surreal_json(v);
    loop {
        let Some(obj) = v.as_object() else {
            break;
        };
        if let Some(next) = obj.get("result").cloned() {
            v = next;
            continue;
        }
        if let Some(next) = obj.get("data").cloned() {
            v = next;
            continue;
        }
        if let Some(next) = obj.get("value").cloned() {
            v = next;
            continue;
        }
        if obj.len() == 1 {
            if let Some(next) = obj.get("0").cloned() {
                v = next;
                continue;
            }
        }
        break;
    }
    v
}

fn detag_surreal_json(v: JsonValue) -> JsonValue {
    match v {
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.into_iter().map(detag_surreal_json).collect())
        }
        JsonValue::Object(mut obj) => {
            // Surreal / soulcore may emit single-key tagged objects with variant key names.
            // Handle fuzzy matches first to avoid leaking tagged datetime/uuid payloads.
            if obj.len() == 1 {
                let (k, raw) = obj.into_iter().next().unwrap();
                let key_lower = k.to_ascii_lowercase();
                if key_lower.contains("datetime") {
                    return match raw {
                        JsonValue::String(s) => JsonValue::String(s),
                        other => detag_surreal_json(other),
                    };
                }
                if key_lower.contains("uuid") || key_lower == "string" {
                    return match raw {
                        JsonValue::String(s) => JsonValue::String(s),
                        other => detag_surreal_json(other),
                    };
                }
                if key_lower == "array" {
                    return match raw {
                        JsonValue::Array(arr) => {
                            JsonValue::Array(arr.into_iter().map(detag_surreal_json).collect())
                        }
                        other => detag_surreal_json(other),
                    };
                }
                if key_lower == "object" {
                    return match raw {
                        JsonValue::Object(map) => JsonValue::Object(
                            map.into_iter()
                                .map(|(kk, vv)| (kk, detag_surreal_json(vv)))
                                .collect(),
                        ),
                        other => detag_surreal_json(other),
                    };
                }
                if key_lower == "bool" {
                    return JsonValue::Bool(raw.as_bool().unwrap_or(false));
                }
                if key_lower == "null" {
                    return JsonValue::Null;
                }
                if key_lower.contains("number") {
                    if let JsonValue::Object(mut n) = raw {
                        if let Some(i) = n.remove("Int").and_then(|x| x.as_i64()) {
                            return JsonValue::Number(Number::from(i));
                        }
                        if let Some(u) = n.remove("Uint").and_then(|x| x.as_u64()) {
                            return JsonValue::Number(Number::from(u));
                        }
                        if let Some(f) = n.remove("Float").and_then(|x| x.as_f64()) {
                            if let Some(num) = Number::from_f64(f) {
                                return JsonValue::Number(num);
                            }
                        }
                        if let Some(dec) = n.remove("Decimal") {
                            if let Some(s) = dec.as_str() {
                                if let Ok(f) = s.parse::<f64>() {
                                    if let Some(num) = Number::from_f64(f) {
                                        return JsonValue::Number(num);
                                    }
                                }
                                return JsonValue::String(s.to_string());
                            }
                        }
                        return JsonValue::Object(
                            n.into_iter()
                                .map(|(kk, vv)| (kk, detag_surreal_json(vv)))
                                .collect(),
                        );
                    }
                    return detag_surreal_json(raw);
                }

                // Not a known tag wrapper, rebuild and continue standard recursion.
                obj = Map::from_iter([(k, raw)]);
            }

            if obj.len() == 1 {
                if let Some(inner) = obj.remove("Array") {
                    if let JsonValue::Array(arr) = inner {
                        return JsonValue::Array(arr.into_iter().map(detag_surreal_json).collect());
                    }
                    return detag_surreal_json(inner);
                }
                if let Some(inner) = obj.remove("Object") {
                    if let JsonValue::Object(map) = inner {
                        let mapped: Map<String, JsonValue> = map
                            .into_iter()
                            .map(|(k, v)| (k, detag_surreal_json(v)))
                            .collect();
                        return JsonValue::Object(mapped);
                    }
                    return detag_surreal_json(inner);
                }
                if let Some(inner) = obj.remove("String") {
                    return JsonValue::String(inner.as_str().unwrap_or_default().to_string());
                }
                if let Some(inner) = obj.remove("Bool") {
                    return JsonValue::Bool(inner.as_bool().unwrap_or(false));
                }
                if obj.contains_key("Null") {
                    return JsonValue::Null;
                }
                if let Some(inner) = obj.remove("Datetime") {
                    return JsonValue::String(inner.as_str().unwrap_or_default().to_string());
                }
                if let Some(inner) = obj.remove("Uuid") {
                    return JsonValue::String(inner.as_str().unwrap_or_default().to_string());
                }
                if let Some(inner) = obj.remove("Number") {
                    if let JsonValue::Object(mut n) = inner {
                        if let Some(i) = n.remove("Int").and_then(|x| x.as_i64()) {
                            return JsonValue::Number(Number::from(i));
                        }
                        if let Some(u) = n.remove("Uint").and_then(|x| x.as_u64()) {
                            return JsonValue::Number(Number::from(u));
                        }
                        if let Some(f) = n.remove("Float").and_then(|x| x.as_f64()) {
                            if let Some(num) = Number::from_f64(f) {
                                return JsonValue::Number(num);
                            }
                        }
                        if let Some(dec) = n.remove("Decimal") {
                            if let Some(s) = dec.as_str() {
                                if let Ok(f) = s.parse::<f64>() {
                                    if let Some(num) = Number::from_f64(f) {
                                        return JsonValue::Number(num);
                                    }
                                }
                                return JsonValue::String(s.to_string());
                            }
                        }
                        return JsonValue::Object(
                            n.into_iter()
                                .map(|(k, v)| (k, detag_surreal_json(v)))
                                .collect(),
                        );
                    }
                    return detag_surreal_json(inner);
                }
            }

            JsonValue::Object(
                obj.into_iter()
                    .map(|(k, v)| (k, detag_surreal_json(v)))
                    .collect(),
            )
        }
        other => other,
    }
}

impl ClientWrapper {
    /// 执行原始SQL查询
    pub fn query(&self, sql: impl Into<String>) -> QueryBuilder {
        QueryBuilder::new(self.storage.clone(), sql.into())
    }

    /// Select - 兼容原有API，直接返回Future
    pub async fn select<T>(
        &self,
        resource: impl Into<ResourceId>,
    ) -> std::result::Result<T, surrealdb::Error>
    where
        T: for<'de> serde::Deserialize<'de> + Serialize + std::fmt::Debug,
    {
        let resource_id = resource.into();
        match resource_id {
            ResourceId::Record(table, id) => {
                // 单条记录查询，必须按 record-id 读取
                let mut res = self
                    .storage
                    .query_with_params(
                        "SELECT * FROM ONLY type::record($rid)",
                        serde_json::json!({
                            "rid": format!("{}:{}", table, id)
                        }),
                    )
                    .await
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;

                let surreal_val: surrealdb::types::Value = res
                    .take(0usize)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                let raw = surreal_val.into_json_value();
                let rows: Vec<serde_json::Value> = match raw {
                    serde_json::Value::Array(arr) => arr,
                    serde_json::Value::Null => vec![],
                    other => vec![other],
                };
                let result: Option<serde_json::Value> =
                    rows.into_iter().next().map(detag_surreal_json);

                serde_json::to_value(result)
                    .and_then(serde_json::from_value)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))
            }
            ResourceId::Table(table) => {
                let result: Vec<serde_json::Value> = self
                    .storage
                    .select(&table)
                    .await
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                let result: Vec<serde_json::Value> =
                    result.into_iter().map(detag_surreal_json).collect();

                serde_json::to_value(result)
                    .and_then(serde_json::from_value)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))
            }
        }
    }

    /// Create - 返回兼容的builder
    pub fn create(&self, resource: impl Into<String>) -> CreateWrapper {
        CreateWrapper::new(self.storage.clone(), resource.into())
    }

    /// Update - 返回兼容的builder
    pub fn update(&self, resource: impl Into<ResourceId>) -> UpdateWrapper {
        UpdateWrapper::new(self.storage.clone(), resource.into())
    }

    /// Delete - 直接执行
    pub async fn delete<T>(
        &self,
        resource: impl Into<ResourceId>,
    ) -> std::result::Result<Option<T>, surrealdb::Error>
    where
        T: for<'de> serde::Deserialize<'de> + std::fmt::Debug,
    {
        let resource_id = resource.into();
        let (table, id) = match resource_id {
            ResourceId::Record(table, id) => (table, id),
            _ => {
                return Err(surrealdb::Error::thrown(
                    "Delete requires a specific record ID".to_string(),
                ));
            }
        };
        let storage_id = soulcore::engines::storage::RecordId { table, id };

        let deleted: Option<serde_json::Value> = self
            .storage
            .delete(storage_id)
            .await
            .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;

        match deleted {
            Some(v) => {
                let item: T = serde_json::from_value(v)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                Ok(Some(item))
            }
            None => Ok(None),
        }
    }
}

/// 资源标识符
pub enum ResourceId {
    Table(String),
    Record(String, String),
}

impl From<&str> for ResourceId {
    fn from(s: &str) -> Self {
        ResourceId::Table(s.to_string())
    }
}

impl From<String> for ResourceId {
    fn from(s: String) -> Self {
        ResourceId::Table(s)
    }
}

impl From<(&str, &str)> for ResourceId {
    fn from((table, id): (&str, &str)) -> Self {
        ResourceId::Record(table.to_string(), id.to_string())
    }
}

impl From<(String, String)> for ResourceId {
    fn from((table, id): (String, String)) -> Self {
        ResourceId::Record(table, id)
    }
}

impl From<(&str, String)> for ResourceId {
    fn from((table, id): (&str, String)) -> Self {
        ResourceId::Record(table.to_string(), id)
    }
}

impl From<(&str, RecordIdKey)> for ResourceId {
    fn from((table, id): (&str, RecordIdKey)) -> Self {
        let id = match id {
            RecordIdKey::String(s) => s,
            RecordIdKey::Number(n) => n.to_string(),
            RecordIdKey::Uuid(u) => u.to_string(),
            other => format!("{:?}", other),
        };
        ResourceId::Record(table.to_string(), id)
    }
}

/// Query构建器 - 兼容原有的链式调用
pub struct QueryBuilder {
    storage: Arc<soulcore::engines::storage::StorageEngine>,
    sql: String,
    bindings: Vec<(String, serde_json::Value)>,
}

impl QueryBuilder {
    fn new(storage: Arc<soulcore::engines::storage::StorageEngine>, sql: String) -> Self {
        Self {
            storage,
            sql,
            bindings: Vec::new(),
        }
    }

    // 灵活的bind方法，可以接受多种类型
    pub fn bind<B>(mut self, binding: B) -> Self
    where
        B: IntoBinding,
    {
        binding.add_to(&mut self.bindings);
        self
    }
}

// Trait用于各种类型转换为绑定
pub trait IntoBinding {
    fn add_to(self, bindings: &mut Vec<(String, serde_json::Value)>);
}

// 实现元组绑定
impl<K, V> IntoBinding for (K, V)
where
    K: Into<String>,
    V: Serialize,
{
    fn add_to(self, bindings: &mut Vec<(String, serde_json::Value)>) {
        let json_value = serde_json::to_value(self.1).unwrap_or(serde_json::Value::Null);
        bindings.push((self.0.into(), json_value));
    }
}

// 实现HashMap绑定
impl IntoBinding for std::collections::HashMap<String, serde_json::Value> {
    fn add_to(self, bindings: &mut Vec<(String, serde_json::Value)>) {
        for (key, value) in self {
            bindings.push((key, value));
        }
    }
}

// 实现引用的HashMap绑定
impl IntoBinding for &std::collections::HashMap<String, serde_json::Value> {
    fn add_to(self, bindings: &mut Vec<(String, serde_json::Value)>) {
        for (key, value) in self {
            bindings.push((key.clone(), value.clone()));
        }
    }
}

impl std::future::IntoFuture for QueryBuilder {
    type Output = std::result::Result<Response, surrealdb::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let mut params = serde_json::Map::new();
            for (key, value) in self.bindings {
                params.insert(key, value);
            }

            self.storage
                .query_with_params(&self.sql, serde_json::Value::Object(params))
                .await
                .map(Response::new)
                .map_err(|e| surrealdb::Error::thrown(e.to_string()))
        })
    }
}

/// Create包装器 - 类型擦除版本
pub struct CreateWrapper {
    storage: Arc<soulcore::engines::storage::StorageEngine>,
    table: String,
}

impl CreateWrapper {
    fn new(storage: Arc<soulcore::engines::storage::StorageEngine>, table: String) -> Self {
        Self { storage, table }
    }

    /// content方法，能够处理引用和值
    pub fn content<T>(self, content: T) -> TypedCreateFuture<T>
    where
        T: Serialize + for<'de> serde::Deserialize<'de> + Clone + std::fmt::Debug + Send + 'static,
    {
        TypedCreateFuture {
            storage: self.storage,
            table: self.table,
            content,
        }
    }
}

/// Trait来处理content的各种输入类型
pub trait ContentProvider<T> {
    fn provide(self) -> T;
}

// 实现值类型
impl<T> ContentProvider<T> for T {
    fn provide(self) -> T {
        self
    }
}

// 实现引用类型（需要Clone）
impl<T: Clone> ContentProvider<T> for &T {
    fn provide(self) -> T {
        self.clone()
    }
}

/// 有类型的Create Future
pub struct TypedCreateFuture<T> {
    storage: Arc<soulcore::engines::storage::StorageEngine>,
    table: String,
    content: T,
}

impl<T> std::future::IntoFuture for TypedCreateFuture<T>
where
    T: Serialize + for<'de> serde::Deserialize<'de> + Clone + std::fmt::Debug + Send + 'static,
{
    type Output = std::result::Result<Vec<T>, surrealdb::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let payload = serde_json::to_value(self.content)
                .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;

            let created: Option<serde_json::Value> = self
                .storage
                .create(&self.table, payload)
                .await
                .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;

            let mut out = Vec::new();
            if let Some(v) = created {
                let item: T = serde_json::from_value(v)
                    .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                out.push(item);
            }
            Ok(out)
        })
    }
}

/// Update包装器
pub struct UpdateWrapper {
    storage: Arc<soulcore::engines::storage::StorageEngine>,
    resource: ResourceId,
}

impl UpdateWrapper {
    fn new(storage: Arc<soulcore::engines::storage::StorageEngine>, resource: ResourceId) -> Self {
        Self { storage, resource }
    }

    /// content方法，能够处理引用和值
    pub fn content<T>(self, content: T) -> TypedUpdateFuture<T>
    where
        T: Serialize + for<'de> serde::Deserialize<'de> + std::fmt::Debug + Send + 'static,
    {
        TypedUpdateFuture {
            storage: self.storage,
            resource: self.resource,
            content,
        }
    }
}

/// 有类型的Update Future
pub struct TypedUpdateFuture<T> {
    storage: Arc<soulcore::engines::storage::StorageEngine>,
    resource: ResourceId,
    content: T,
}

impl<T> std::future::IntoFuture for TypedUpdateFuture<T>
where
    T: Serialize + for<'de> serde::Deserialize<'de> + std::fmt::Debug + Send + 'static,
{
    type Output = std::result::Result<Option<T>, surrealdb::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (table, id) = match self.resource {
                ResourceId::Record(table, id) => (table, id),
                _ => {
                    return Err(surrealdb::Error::thrown(
                        "Update requires a specific record ID".to_string(),
                    ));
                }
            };
            let storage_id = soulcore::engines::storage::RecordId { table, id };

            let payload = serde_json::to_value(self.content)
                .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;

            let updated: Option<serde_json::Value> = self
                .storage
                .update(storage_id, payload)
                .await
                .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;

            match updated {
                Some(v) => {
                    let item: T = serde_json::from_value(v)
                        .map_err(|e| surrealdb::Error::thrown(e.to_string()))?;
                    Ok(Some(item))
                }
                None => Ok(None),
            }
        })
    }
}

#[derive(Clone)]
pub struct Database {
    pub client: ClientWrapper,
    pub config: Config,
    storage: Arc<soulcore::engines::storage::StorageEngine>,
}

impl Database {
    pub async fn new(config: &Config) -> Result<Self> {
        let soulcore_config = soulcore::config::StorageConfig {
            connection_mode: soulcore::config::ConnectionMode::Http,
            url: config.database.url.clone(),
            username: config.database.user.clone(),
            password: config.database.pass.clone(),
            namespace: config.database.namespace.clone(),
            database: config.database.database.clone(),
            pool_size: config.database.max_connections as usize,
            connection_timeout: config.database.connection_timeout,
            query_timeout: 60,
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        let storage = Arc::new(
            soulcore::engines::storage::StorageEngine::new(soulcore_config)
                .await
                .map_err(|e| {
                    error!("Failed to create soulcore storage engine: {}", e);
                    AppError::Internal(anyhow::anyhow!("Database initialization failed: {}", e))
                })?,
        );

        // Verify connection with a simple query
        storage
            .query("RETURN 1")
            .await
            .map_err(|e| {
                error!("Failed to verify database connection: {}", e);
                AppError::Internal(anyhow::anyhow!("Database connection failed: {}", e))
            })?;

        info!(
            "Successfully connected to SurrealDB via soulcore at {}",
            config.database.url
        );

        let client = ClientWrapper {
            storage: storage.clone(),
        };

        Ok(Database {
            client,
            config: config.clone(),
            storage,
        })
    }

    pub async fn verify_connection(&self) -> Result<()> {
        self.storage
            .query("RETURN 1")
            .await
            .map_err(|e| {
                error!("Database connection verification failed: {}", e);
                AppError::Internal(anyhow::anyhow!("Connection verification failed: {}", e))
            })?;

        info!("Database connection verified successfully");
        Ok(())
    }

    pub async fn health_check(&self) -> Result<DatabaseHealth> {
        let start = std::time::Instant::now();

        match self.verify_connection().await {
            Ok(_) => {
                let response_time = start.elapsed();
                Ok(DatabaseHealth {
                    connected: true,
                    response_time_ms: response_time.as_millis() as u64,
                    error: None,
                })
            }
            Err(e) => {
                error!("Database health check failed: {}", e);
                Ok(DatabaseHealth {
                    connected: false,
                    response_time_ms: 0,
                    error: Some(e.to_string()),
                })
            }
        }
    }

    pub fn storage(&self) -> &Arc<soulcore::engines::storage::StorageEngine> {
        &self.storage
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseHealth {
    pub connected: bool,
    pub response_time_ms: u64,
    pub error: Option<String>,
}
