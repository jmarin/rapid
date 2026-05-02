use axum::{http::StatusCode, response::IntoResponse};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("object not found")]
    NotFound,
    #[error("S3 error: {0}")]
    S3(String),
    #[error("invalid Range header")]
    InvalidRange,
}

impl IntoResponse for DownloadError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            DownloadError::NotFound => StatusCode::NOT_FOUND,
            DownloadError::S3(_) => StatusCode::BAD_GATEWAY,
            DownloadError::InvalidRange => StatusCode::RANGE_NOT_SATISFIABLE,
        };
        let body = serde_json::json!({ "error": self.to_string() });
        (status, axum::Json(body)).into_response()
    }
}

#[derive(Debug, Error)]
pub enum MimeTypeError {
    #[error("file size is 0")]
    ZeroByteFileError,
    #[error("metadata error: {0}")]
    MetadataError(String),
    #[error("detected MIME type is application/octet-stream (score: {0})")]
    GenericMimeDetection(f32),
    #[error("magic error: {0}")]
    MagicError(String),
}

#[derive(Debug, Error)]
pub enum UploadError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("stream error: {0}")]
    Stream(String),
    #[error("MIME type detection failed: {0}")]
    MimeDetection(#[from] MimeTypeError),
    #[error("S3 error: {0}")]
    S3(String),
    #[error("not an image or video: {0}")]
    NotAnImageOrVideo(String),
}

impl IntoResponse for UploadError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            UploadError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            UploadError::Stream(_) => StatusCode::BAD_REQUEST,
            UploadError::MimeDetection(_) => StatusCode::UNPROCESSABLE_ENTITY,
            UploadError::S3(_) => StatusCode::BAD_GATEWAY,
            UploadError::NotAnImageOrVideo(_) => StatusCode::UNPROCESSABLE_ENTITY,
        };
        let body = serde_json::json!({ "error": self.to_string() });
        (status, axum::Json(body)).into_response()
    }
}
