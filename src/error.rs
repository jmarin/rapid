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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status_of(resp: axum::response::Response) -> StatusCode {
        resp.status()
    }

    // ── DownloadError ──

    #[test]
    fn download_not_found_returns_404() {
        assert_eq!(status_of(DownloadError::NotFound.into_response()), StatusCode::NOT_FOUND);
    }

    #[test]
    fn download_s3_returns_502() {
        assert_eq!(
            status_of(DownloadError::S3("boom".into()).into_response()),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn download_invalid_range_returns_416() {
        assert_eq!(
            status_of(DownloadError::InvalidRange.into_response()),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
    }

    // ── UploadError ──

    #[test]
    fn upload_io_returns_500() {
        let err = UploadError::Io(std::io::Error::new(std::io::ErrorKind::Other, "disk full"));
        assert_eq!(status_of(err.into_response()), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn upload_stream_returns_400() {
        assert_eq!(
            status_of(UploadError::Stream("bad".into()).into_response()),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn upload_mime_detection_returns_422() {
        let err = UploadError::MimeDetection(MimeTypeError::ZeroByteFileError);
        assert_eq!(status_of(err.into_response()), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn upload_s3_returns_502() {
        assert_eq!(
            status_of(UploadError::S3("timeout".into()).into_response()),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn upload_not_image_returns_422() {
        assert_eq!(
            status_of(UploadError::NotAnImageOrVideo("text/plain".into()).into_response()),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
