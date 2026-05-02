use aws_sdk_s3::primitives::ByteStream;
use axum::{Json, body::Body, extract::State, http::StatusCode, response::IntoResponse};
use futures_util::TryStreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::StreamReader;
use uuid::Uuid;

use crate::{AppState, error::UploadError, magic::mime_type_magic};

#[derive(Serialize)]
pub struct UploadResponse {
    pub id: String,
    pub key: String,
    pub size_bytes: u64,
    pub mime_type: String,
}

pub async fn upload_file(
    State(state): State<AppState>,
    body: Body,
) -> Result<impl IntoResponse, UploadError> {
    let file_id = Uuid::new_v4().to_string();
    let temp_path = state.upload_dir.join(&file_id);

    // Stream request body to a temp file
    let stream = body
        .into_data_stream()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));
    let mut reader = StreamReader::new(stream);

    let mut file = tokio::fs::File::create(&temp_path).await?;
    let size_bytes = match tokio::io::copy(&mut reader, &mut file).await {
        Ok(size) => size,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(UploadError::Io(e));
        }
    };
    file.flush().await?;
    drop(file); // This is not necessary on Linux, but can be an issue if we have the write handle still open on Windows and we try to access the file for reading right after. 

    // Detect MIME type from temp file
    let mime_type = mime_type_magic(&temp_path).await?;

    // Upload temp file to S3
    let body_stream = ByteStream::from_path(&temp_path)
        .await
        .map_err(|e| UploadError::S3(e.to_string()))?;

    state
        .s3_client
        .put_object()
        .bucket(&state.s3_bucket)
        .key(&file_id)
        .body(body_stream)
        .content_type(&mime_type)
        .send()
        .await
        .map_err(|e| UploadError::S3(e.to_string()))?;

    // Clean up temp file
    let _ = tokio::fs::remove_file(&temp_path).await;

    let response = UploadResponse {
        id: file_id.clone(),
        key: file_id,
        size_bytes,
        mime_type,
    };

    Ok((StatusCode::CREATED, Json(response)))
}
