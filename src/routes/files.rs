use axum::{
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tracing::{error, info};

use crate::{
    error::ApiError,
    models::file::{FileQuery, UploadFileRequest},
    services::{auth::AuthService, file_upload::FileUploadService},
    utils::auth::extract_user_from_header,
};

pub fn router() -> Router {
    Router::new()
        .route("/", get(list_files).post(upload_file))
        .route("/:file_id", get(get_file_info).delete(delete_file))
        .route("/:file_id/download", get(download_file))
        .route("/:file_id/thumbnail", get(get_thumbnail))
}

async fn upload_file(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let service = &app_state.file_upload_service;
    let auth_service = &app_state.auth_service;
    let user_id = extract_user_from_header(&headers, &auth_service).await?;

    // 从 multipart 中提取请求参数
    let mut space_id = None;
    let mut document_id = None;
    let mut description = None;
    let mut file_name = None;
    let mut content_type = None;
    let mut file_data = None;

    // 读取完整 multipart，避免因字段顺序不同导致 space_id/document_id 丢失。
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("Failed to read multipart field: {}", e)))?
    {
        let field_name = field.name().unwrap_or("");

        match field_name {
            "space_id" => {
                space_id = Some(field.text().await.map_err(|e| {
                    ApiError::bad_request(format!("Failed to read space_id: {}", e))
                })?);
            }
            "document_id" => {
                document_id = Some(field.text().await.map_err(|e| {
                    ApiError::bad_request(format!("Failed to read document_id: {}", e))
                })?);
            }
            "description" => {
                description = Some(field.text().await.map_err(|e| {
                    ApiError::bad_request(format!("Failed to read description: {}", e))
                })?);
            }
            "file" => {
                let filename = field.file_name().map(|s| s.to_string());
                let field_content_type = field.content_type().map(|s| s.to_string());
                let data = field.bytes().await.map_err(|e| {
                    ApiError::bad_request(format!("Failed to read file data: {}", e))
                })?;

                // 检查文件大小
                if data.len() > 10 * 1024 * 1024 {
                    // 10MB limit
                    return Err(ApiError::bad_request(
                        "File size exceeds maximum allowed size of 10MB".to_string(),
                    ));
                }

                file_name = filename;
                content_type = field_content_type;
                file_data = Some(data);
            }
            _ => {
                // 忽略其他字段
            }
        }
    }

    let request = UploadFileRequest {
        space_id,
        document_id,
        description,
    };
    let file_response = service
        .upload_file_from_bytes(
            &user_id,
            file_data
                .ok_or_else(|| ApiError::bad_request("No file found in request".to_string()))?,
            file_name.ok_or_else(|| ApiError::bad_request("No filename provided".to_string()))?,
            content_type,
            request,
        )
        .await?;

    info!("File uploaded by user {}", user_id);
    Ok((StatusCode::CREATED, Json(file_response)))
}

async fn list_files(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<FileQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let service = &app_state.file_upload_service;
    let auth_service = &app_state.auth_service;
    let user_id = extract_user_from_header(&headers, &auth_service).await?;

    let files = service.list_files(&user_id, query).await?;
    Ok(Json(files))
}

async fn get_file_info(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    Path(file_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let service = &app_state.file_upload_service;
    let auth_service = &app_state.auth_service;
    let _user_id = extract_user_from_header(&headers, &auth_service).await?;

    let file = service.get_file(&file_id).await?;
    let file_response: crate::models::file::FileResponse = file.into();
    Ok(Json(file_response))
}

async fn download_file(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    Path(file_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let service = &app_state.file_upload_service;
    let auth_service = &app_state.auth_service;
    let _user_id = extract_user_from_header(&headers, &auth_service).await?;

    let (content, mime_type, original_name) = service.get_file_content(&file_id).await?;

    let headers = [
        (header::CONTENT_TYPE, mime_type),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", original_name),
        ),
    ];

    Ok((headers, content))
}

async fn get_thumbnail(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    Path(file_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let service = &app_state.file_upload_service;
    let auth_service = &app_state.auth_service;
    let _user_id = extract_user_from_header(&headers, &auth_service).await?;

    let thumbnail_content = service.get_thumbnail(&file_id).await?;

    let headers = [
        (header::CONTENT_TYPE, "image/jpeg".to_string()),
        (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
    ];

    Ok((headers, thumbnail_content))
}

async fn delete_file(
    Extension(app_state): Extension<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    Path(file_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let service = &app_state.file_upload_service;
    let auth_service = &app_state.auth_service;
    let user_id = extract_user_from_header(&headers, &auth_service).await?;

    service.delete_file(&user_id, &file_id).await?;

    info!("File {} deleted by user {}", file_id, user_id);
    Ok((
        StatusCode::OK,
        Json(json!({ "message": "File deleted successfully" })),
    ))
}
