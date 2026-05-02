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
    pub file_path: String,
    pub size_bytes: u64,
    pub mime_type: String,
}

pub async fn upload_file(
    State(state): State<AppState>,
    body: Body,
) -> Result<impl IntoResponse, UploadError> {
    let file_id = Uuid::new_v4().to_string();
    let file_path = state.upload_dir.join(&file_id);

    let stream = body
        .into_data_stream()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));
    let mut reader = StreamReader::new(stream);

    let mut file = tokio::fs::File::create(&file_path).await?;
    let size_bytes = match tokio::io::copy(&mut reader, &mut file).await {
        Ok(size) => size,
        Err(e) => {
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(UploadError::Io(e));
        }
    };
    file.flush().await?;

    let mime_type = mime_type_magic(&file_path).await?;

    let response = UploadResponse {
        id: file_id,
        file_path: file_path.to_string_lossy().to_string(),
        size_bytes,
        mime_type,
    };

    Ok((StatusCode::CREATED, Json(response)))
}
