use anyhow::Result;
use axum::extract::Multipart;
use image::ImageFormat;
use mime_guess::from_path;
use std::path::Path;
use std::sync::Arc;
use surrealdb::types::RecordId as Thing;
use tokio::fs as async_fs;
use tracing::{error, info, warn};
use uuid::Uuid;
use validator::Validate;

use crate::{
    error::ApiError,
    models::file::{FileListResponse, FileQuery, FileResponse, FileUpload, UploadFileRequest},
    services::{
        auth::AuthService,
        database::{record_id_key, Database},
    },
};

#[derive(Clone)]
pub struct FileUploadService {
    db: Arc<Database>,
    auth_service: Arc<AuthService>,
    upload_dir: String,
    max_file_size: usize,
}

impl FileUploadService {
    pub fn new(db: Arc<Database>, auth_service: Arc<AuthService>) -> Self {
        Self {
            db,
            auth_service,
            upload_dir: std::env::var("UPLOAD_DIR").unwrap_or_else(|_| "./uploads".to_string()),
            max_file_size: std::env::var("MAX_FILE_SIZE")
                .unwrap_or_else(|_| "10485760".to_string()) // 10MB default
                .parse()
                .unwrap_or(10485760),
        }
    }

    fn record_id_from_input(table: &str, value: &str) -> Thing {
        if let Some((tbl, key)) = value.split_once(':') {
            Thing::new(tbl, key)
        } else {
            Thing::new(table, value)
        }
    }

    pub async fn upload_file(
        &self,
        user_id: &str,
        mut multipart: Multipart,
        request: UploadFileRequest,
    ) -> Result<FileResponse, ApiError> {
        request.validate()?;

        // 确保上传目录存在
        self.ensure_upload_dir_exists().await?;

        let mut file_data = None;
        let mut filename = None;
        let mut content_type = None;

        // 处理 multipart 数据
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| ApiError::bad_request(format!("Failed to read multipart field: {}", e)))?
        {
            let field_name = field.name().unwrap_or("");

            if field_name == "file" {
                filename = field.file_name().map(|s| s.to_string());
                content_type = field.content_type().map(|s| s.to_string());

                let data = field.bytes().await.map_err(|e| {
                    ApiError::bad_request(format!("Failed to read file data: {}", e))
                })?;

                // 检查文件大小
                if data.len() > self.max_file_size {
                    return Err(ApiError::bad_request(format!(
                        "File size exceeds maximum allowed size of {} bytes",
                        self.max_file_size
                    )));
                }

                file_data = Some(data);
                break;
            }
        }

        let file_data = file_data
            .ok_or_else(|| ApiError::bad_request("No file found in request".to_string()))?;

        let original_name =
            filename.ok_or_else(|| ApiError::bad_request("No filename provided".to_string()))?;

        // 生成唯一文件名
        let file_extension = Path::new(&original_name)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        let unique_filename = if file_extension.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            format!("{}.{}", Uuid::new_v4(), file_extension)
        };

        // 确定 MIME 类型
        let mime_type = content_type.unwrap_or_else(|| {
            from_path(&original_name)
                .first_or_octet_stream()
                .to_string()
        });

        // 验证文件类型
        self.validate_file_type(&mime_type)?;

        // 创建文件路径
        let file_path = Path::new(&self.upload_dir).join(&unique_filename);

        // 保存文件
        async_fs::write(&file_path, &file_data).await.map_err(|e| {
            error!("Failed to save file: {}", e);
            ApiError::internal_server_error("Failed to save file".to_string())
        })?;

        // 如果是图片，生成缩略图
        if mime_type.starts_with("image/") {
            if let Err(e) = self.generate_thumbnail(&file_path, &unique_filename).await {
                warn!(
                    "Failed to generate thumbnail for {}: {}",
                    unique_filename, e
                );
            }
        }

        // 确定文件类型
        let file_type = self.determine_file_type(&mime_type);

        // 保存到数据库
        let mut file_upload = FileUpload::new(
            unique_filename.clone(),
            original_name,
            file_path.to_string_lossy().to_string(),
            file_data.len() as i64,
            file_type,
            mime_type,
            user_id.to_string(),
        );

        // 设置关联的空间或文档
        if let Some(space_id) = &request.space_id {
            file_upload = file_upload.with_space(Self::record_id_from_input("space", space_id));
        }

        if let Some(document_id) = &request.document_id {
            file_upload =
                file_upload.with_document(Self::record_id_from_input("document", document_id));
        }

        let created_files: Vec<FileUpload> = self
            .db
            .client
            .create("file_upload")
            .content(file_upload)
            .await
            .map_err(|e| {
                error!("Failed to save file to database: {}", e);
                ApiError::internal_server_error("Failed to save file metadata".to_string())
            })?;

        let created_file = created_files.into_iter().next();

        let created_file = created_file.ok_or_else(|| {
            ApiError::internal_server_error("Failed to create file record".to_string())
        })?;

        info!("File uploaded successfully: {}", unique_filename);
        Ok(created_file.into())
    }

    pub async fn upload_file_from_bytes(
        &self,
        user_id: &str,
        file_data: axum::body::Bytes,
        original_name: String,
        content_type: Option<String>,
        request: UploadFileRequest,
    ) -> Result<FileResponse, ApiError> {
        request.validate()?;

        // 确保上传目录存在
        self.ensure_upload_dir_exists().await?;

        // 检查文件大小
        if file_data.len() > self.max_file_size {
            return Err(ApiError::bad_request(format!(
                "File size exceeds maximum allowed size of {} bytes",
                self.max_file_size
            )));
        }

        // 生成唯一文件名
        let file_extension = Path::new(&original_name)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        let unique_filename = if file_extension.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            format!("{}.{}", Uuid::new_v4(), file_extension)
        };

        // 确定 MIME 类型
        let mime_type = content_type.unwrap_or_else(|| {
            from_path(&original_name)
                .first_or_octet_stream()
                .to_string()
        });

        // 验证文件类型
        self.validate_file_type(&mime_type)?;

        // 创建文件路径
        let file_path = Path::new(&self.upload_dir).join(&unique_filename);

        // 保存文件
        async_fs::write(&file_path, &file_data).await.map_err(|e| {
            error!("Failed to save file: {}", e);
            ApiError::internal_server_error("Failed to save file".to_string())
        })?;

        // 如果是图片，生成缩略图
        if mime_type.starts_with("image/") {
            if let Err(e) = self.generate_thumbnail(&file_path, &unique_filename).await {
                warn!(
                    "Failed to generate thumbnail for {}: {}",
                    unique_filename, e
                );
            }
        }

        // 确定文件类型
        let file_type = self.determine_file_type(&mime_type);

        // 保存到数据库
        let mut file_upload = FileUpload::new(
            unique_filename.clone(),
            original_name,
            file_path.to_string_lossy().to_string(),
            file_data.len() as i64,
            file_type,
            mime_type,
            user_id.to_string(),
        );

        // 设置关联的空间或文档
        if let Some(space_id) = &request.space_id {
            file_upload = file_upload.with_space(Self::record_id_from_input("space", space_id));
        }

        if let Some(document_id) = &request.document_id {
            file_upload =
                file_upload.with_document(Self::record_id_from_input("document", document_id));
        }

        let created_files: Vec<FileUpload> = self
            .db
            .client
            .create("file_upload")
            .content(file_upload)
            .await
            .map_err(|e| {
                error!("Failed to save file to database: {}", e);
                ApiError::internal_server_error("Failed to save file metadata".to_string())
            })?;

        let created_file = created_files.into_iter().next();

        let created_file = created_file.ok_or_else(|| {
            ApiError::internal_server_error("Failed to create file record".to_string())
        })?;

        info!("File uploaded successfully: {}", unique_filename);
        Ok(created_file.into())
    }

    pub async fn get_file(&self, file_id: &str) -> Result<FileUpload, ApiError> {
        let file: Option<FileUpload> = self
            .db
            .client
            .select(("file_upload", file_id))
            .await
            .map_err(|e| {
                error!("Failed to get file: {}", e);
                ApiError::internal_server_error("Failed to retrieve file".to_string())
            })?;

        let file = file.ok_or_else(|| ApiError::not_found("File not found".to_string()))?;

        if file.is_deleted {
            return Err(ApiError::not_found("File not found".to_string()));
        }

        Ok(file)
    }

    pub async fn list_files(
        &self,
        user_id: &str,
        query: FileQuery,
    ) -> Result<FileListResponse, ApiError> {
        let page = query.page.unwrap_or(1).max(1);
        let per_page = query.per_page.unwrap_or(20).min(100).max(1);
        let offset = (page - 1) * per_page;

        let mut sql = "SELECT * FROM file_upload WHERE is_deleted = false".to_string();
        let mut params: Vec<(&str, serde_json::Value)> = Vec::new();

        // 添加筛选条件
        if let Some(space_id) = &query.space_id {
            let space_thing = Self::record_id_from_input("space", space_id);
            sql.push_str(" AND space_id = $space_id");
            params.push((
                "space_id",
                serde_json::Value::String(format!("space:{}", record_id_key(&space_thing))),
            ));
        }

        if let Some(document_id) = &query.document_id {
            let doc_thing = Self::record_id_from_input("document", document_id);
            sql.push_str(" AND document_id = $document_id");
            params.push((
                "document_id",
                serde_json::Value::String(format!("document:{}", record_id_key(&doc_thing))),
            ));
        }

        if let Some(file_type) = &query.file_type {
            sql.push_str(" AND file_type = $file_type");
            params.push(("file_type", serde_json::Value::String(file_type.clone())));
        }

        sql.push_str(" ORDER BY created_at DESC");

        // 获取总数 (count query must not have ORDER BY)
        let count_sql = sql
            .replace("SELECT *", "SELECT count()")
            .replace(" ORDER BY created_at DESC", "");
        let mut query = self.db.client.query(count_sql);
        for (key, value) in &params {
            query = query.bind((*key, value));
        }
        let total_count: Option<i64> = query
            .await
            .map_err(|e| {
                error!("Failed to count files: {}", e);
                ApiError::internal_server_error("Failed to count files".to_string())
            })?
            .take(0)?;

        let total_count = total_count.unwrap_or(0);

        // 添加分页
        sql.push_str(&format!(" LIMIT {} START {}", per_page, offset));

        let mut files_query = self.db.client.query(sql);
        for (key, value) in params {
            files_query = files_query.bind((key, &value));
        }
        let files: Vec<FileUpload> = files_query
            .await
            .map_err(|e| {
                error!("Failed to list files: {}", e);
                ApiError::internal_server_error("Failed to list files".to_string())
            })?
            .take(0)?;

        let file_responses: Vec<FileResponse> = files.into_iter().map(|f| f.into()).collect();
        let total_pages = (total_count + per_page - 1) / per_page;

        Ok(FileListResponse {
            files: file_responses,
            total_count,
            page,
            per_page,
            total_pages,
        })
    }

    pub async fn delete_file(&self, user_id: &str, file_id: &str) -> Result<(), ApiError> {
        let mut file: FileUpload = self.get_file(file_id).await?;

        // 检查权限
        if file.uploaded_by != user_id {
            // 检查是否有空间管理权限
            if let Some(space_id) = &file.space_id {
                let space_id_str = record_id_key(space_id);

                // 检查用户是否有空间的管理权限
                match self
                    .auth_service
                    .check_permission(user_id, "docs.admin", Some(&space_id_str))
                    .await
                {
                    Ok(_) => {
                        // 用户有管理权限，可以删除
                    }
                    Err(_) => {
                        return Err(ApiError::forbidden("Permission denied: You can only delete your own files or need admin permission".to_string()));
                    }
                }
            } else {
                // 没有关联空间的文件，只有上传者可以删除
                return Err(ApiError::forbidden(
                    "Permission denied: You can only delete your own files".to_string(),
                ));
            }
        }

        // 标记为删除
        file.mark_deleted(user_id.to_string());

        // 更新数据库
        let _: Option<FileUpload> = self
            .db
            .client
            .update(("file_upload", file_id))
            .content(file)
            .await
            .map_err(|e| {
                error!("Failed to delete file: {}", e);
                ApiError::internal_server_error("Failed to delete file".to_string())
            })?;

        info!("File marked as deleted: {}", file_id);
        Ok(())
    }

    pub async fn get_file_content(
        &self,
        file_id: &str,
    ) -> Result<(Vec<u8>, String, String), ApiError> {
        let file = self.get_file(file_id).await?;

        let content = async_fs::read(&file.file_path).await.map_err(|e| {
            error!("Failed to read file content: {}", e);
            ApiError::internal_server_error("Failed to read file".to_string())
        })?;

        Ok((content, file.mime_type, file.original_name))
    }

    pub async fn get_thumbnail(&self, file_id: &str) -> Result<Vec<u8>, ApiError> {
        let file = self.get_file(file_id).await?;

        if !file.is_image() {
            return Err(ApiError::bad_request("File is not an image".to_string()));
        }

        let thumbnail_path = self.get_thumbnail_path(&file.filename);

        if !thumbnail_path.exists() {
            return Err(ApiError::not_found("Thumbnail not found".to_string()));
        }

        let content = async_fs::read(&thumbnail_path).await.map_err(|e| {
            error!("Failed to read thumbnail: {}", e);
            ApiError::internal_server_error("Failed to read thumbnail".to_string())
        })?;

        Ok(content)
    }

    async fn ensure_upload_dir_exists(&self) -> Result<(), ApiError> {
        let upload_path = Path::new(&self.upload_dir);
        if !upload_path.exists() {
            async_fs::create_dir_all(upload_path).await.map_err(|e| {
                error!("Failed to create upload directory: {}", e);
                ApiError::internal_server_error("Failed to create upload directory".to_string())
            })?;
        }

        let thumbnails_path = upload_path.join("thumbnails");
        if !thumbnails_path.exists() {
            async_fs::create_dir_all(thumbnails_path)
                .await
                .map_err(|e| {
                    error!("Failed to create thumbnails directory: {}", e);
                    ApiError::internal_server_error(
                        "Failed to create thumbnails directory".to_string(),
                    )
                })?;
        }

        Ok(())
    }

    fn validate_file_type(&self, mime_type: &str) -> Result<(), ApiError> {
        let allowed_types = [
            // 图片
            "image/jpeg",
            "image/jpg",
            "image/png",
            "image/gif",
            "image/webp",
            "image/svg+xml",
            // 文档
            "application/pdf",
            "application/msword",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.ms-excel",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.ms-powerpoint",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            // 文本
            "text/plain",
            "text/markdown",
            "text/csv",
            // 代码
            "application/json",
            "application/xml",
            "text/html",
            "text/css",
            "text/javascript",
            // 压缩文件
            "application/zip",
            "application/x-tar",
            "application/gzip",
        ];

        if !allowed_types.contains(&mime_type) {
            return Err(ApiError::bad_request(format!(
                "File type '{}' is not allowed",
                mime_type
            )));
        }

        Ok(())
    }

    fn determine_file_type(&self, mime_type: &str) -> String {
        match mime_type {
            t if t.starts_with("image/") => "image".to_string(),
            t if t.starts_with("video/") => "video".to_string(),
            t if t.starts_with("audio/") => "audio".to_string(),
            "application/pdf" => "pdf".to_string(),
            t if t.contains("word") || t.contains("document") => "document".to_string(),
            t if t.contains("excel") || t.contains("spreadsheet") => "spreadsheet".to_string(),
            t if t.contains("powerpoint") || t.contains("presentation") => {
                "presentation".to_string()
            }
            t if t.starts_with("text/") => "text".to_string(),
            t if t.contains("zip") || t.contains("tar") || t.contains("gzip") => {
                "archive".to_string()
            }
            _ => "other".to_string(),
        }
    }

    async fn generate_thumbnail(&self, file_path: &Path, filename: &str) -> Result<()> {
        let img = image::open(file_path)?;
        let thumbnail = img.thumbnail(300, 300);

        let thumbnail_path = self.get_thumbnail_path(filename);
        thumbnail.save_with_format(&thumbnail_path, ImageFormat::Jpeg)?;

        Ok(())
    }

    fn get_thumbnail_path(&self, filename: &str) -> std::path::PathBuf {
        Path::new(&self.upload_dir)
            .join("thumbnails")
            .join(format!("thumb_{}", filename))
    }
}
