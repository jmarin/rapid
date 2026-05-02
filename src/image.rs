use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use axum::{Json, body::Body, extract::State, http::{HeaderMap, StatusCode}, response::IntoResponse};
use futures_util::TryStreamExt;
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::io::StreamReader;
use uuid::Uuid;

use crate::{AppState, error::UploadError, magic::mime_type_magic, ws::UploadEvent};

const MIN_CHUNK_SIZE: u64 = 8 * 1024 * 1024; // 8MB
const MAX_PARTS: u64 = 10_000;

struct ProgressNotifier<'a> {
    tx: &'a mpsc::Sender<UploadEvent>,
    upload_id: &'a str,
}

impl<'a> ProgressNotifier<'a> {
    fn new(
        tx: Option<&'a mpsc::Sender<UploadEvent>>,
        upload_id: Option<&'a str>,
    ) -> Option<Self> {
        Some(Self { tx: tx?, upload_id: upload_id? })
    }

    async fn send(&self, event: UploadEvent) {
        let _ = self.tx.send(event).await;
    }
}

#[derive(Serialize)]
pub struct UploadResponse {
    pub id: String,
    pub key: String,
    pub size_bytes: u64,
    pub mime_type: String,
}

/// Calculate chunk size as a multiple of 8MB that keeps total parts <= 10,000.
fn calculate_chunk_size(file_size: u64) -> u64 {
    let mut chunk_size = MIN_CHUNK_SIZE;
    while file_size.div_ceil(chunk_size) > MAX_PARTS {
        chunk_size *= 2;
    }
    chunk_size
}

pub async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse, UploadError> {
    let file_id = Uuid::new_v4().to_string();
    let temp_path = state.upload_dir.join(&file_id);

    let upload_id = headers
        .get("x-upload-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

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
    drop(file);

    // Look up progress sender after body is fully received
    let progress_tx: Option<mpsc::Sender<UploadEvent>> = if let Some(ref uid) = upload_id {
        let map = state.upload_progress.read().await;
        map.get(uid).cloned()
    } else {
        None
    };

    // Detect MIME type from temp file
    let mime_type = mime_type_magic(&temp_path).await?;

    let notifier = ProgressNotifier::new(progress_tx.as_ref(), upload_id.as_deref());

    // Upload to S3: single PUT for small files, multipart for large files
    if size_bytes < MIN_CHUNK_SIZE {
        if let Some(n) = &notifier {
            n.send(UploadEvent::UploadStarted {
                upload_id: n.upload_id.to_string(),
                total_parts: 1,
            })
            .await;
        }

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

        if let Some(n) = &notifier {
            n.send(UploadEvent::UploadCompleted {
                upload_id: n.upload_id.to_string(),
            })
            .await;
        }
    } else {
        multipart_upload(&state, &temp_path, &file_id, size_bytes, &mime_type, &notifier).await?;
    }

    // Clean up progress entry from shared map
    if let Some(ref uid) = upload_id {
        let mut map = state.upload_progress.write().await;
        map.remove(uid);
    }

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

async fn multipart_upload(
    state: &AppState,
    temp_path: &std::path::Path,
    key: &str,
    file_size: u64,
    content_type: &str,
    notifier: &Option<ProgressNotifier<'_>>,
) -> Result<(), UploadError> {
    let chunk_size = calculate_chunk_size(file_size);
    let num_parts = file_size.div_ceil(chunk_size);

    // Initiate multipart upload
    let create_resp = state
        .s3_client
        .create_multipart_upload()
        .bucket(&state.s3_bucket)
        .key(key)
        .content_type(content_type)
        .send()
        .await
        .map_err(|e| UploadError::S3(e.to_string()))?;

    let upload_id = create_resp
        .upload_id()
        .ok_or_else(|| UploadError::S3("missing upload_id".to_string()))?
        .to_string();

    // Notify client that multipart upload has started
    if let Some(n) = notifier {
        n.send(UploadEvent::UploadStarted {
            upload_id: n.upload_id.to_string(),
            total_parts: num_parts,
        })
        .await;
    }

    // Upload parts in parallel, throttled by the global semaphore
    let mut join_set = JoinSet::new();

    for part_idx in 0..num_parts {
        let part_number = (part_idx + 1) as i32; // S3 parts are 1-indexed
        let offset = part_idx * chunk_size;
        // Most parts are exactly chunk_size (e.g. 8MB), but the last part is usually smaller
        let length = std::cmp::min(chunk_size, file_size - offset) as usize;

        let s3_client = state.s3_client.clone();
        let bucket = state.s3_bucket.clone();
        let key = key.to_string();
        let upload_id = upload_id.clone();
        let temp_path = temp_path.to_path_buf();
        let sem = state.upload_semaphore.clone();

        join_set.spawn(async move {
            // Acquire global permit — suspends if all permits are taken
            let _permit = sem.acquire().await.map_err(|e| e.to_string())?;

            // Read this part's chunk in a single spawn_blocking call.
            // Each task opens its own FD (safe for parallel reads at different offsets).
            // With the global semaphore limiting concurrency, FD usage stays bounded.
            let buf = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                let mut file = std::fs::File::open(&temp_path).map_err(|e| e.to_string())?;
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; length];
                file.read_exact(&mut buf).map_err(|e| e.to_string())?;
                Ok(buf)
            })
            .await
            .map_err(|e| e.to_string())??;

            let resp = s3_client
                .upload_part()
                .bucket(&bucket)
                .key(&key)
                .upload_id(&upload_id)
                .part_number(part_number)
                .body(ByteStream::from(buf))
                .send()
                .await
                .map_err(|e| e.to_string())?;

            let e_tag = resp
                .e_tag()
                .ok_or_else(|| "missing ETag in upload_part response".to_string())?
                .to_string();

            Ok::<(i32, String), String>((part_number, e_tag))
        });
    }

    // Collect results
    let mut completed_parts = Vec::with_capacity(num_parts as usize);
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok((part_number, e_tag))) => {
                completed_parts.push(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(e_tag)
                        .build(),
                );

                if let Some(n) = notifier {
                    n.send(UploadEvent::PartCompleted {
                        upload_id: n.upload_id.to_string(),
                        part_number,
                        total_parts: num_parts,
                    })
                    .await;
                }
            }
            Ok(Err(e)) => {
                if let Some(n) = notifier {
                    n.send(UploadEvent::UploadFailed {
                        upload_id: n.upload_id.to_string(),
                        error: e.clone(),
                    })
                    .await;
                }
                let _ = state
                    .s3_client
                    .abort_multipart_upload()
                    .bucket(&state.s3_bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .send()
                    .await;
                return Err(UploadError::S3(e));
            }
            Err(e) => {
                if let Some(n) = notifier {
                    n.send(UploadEvent::UploadFailed {
                        upload_id: n.upload_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                }
                let _ = state
                    .s3_client
                    .abort_multipart_upload()
                    .bucket(&state.s3_bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .send()
                    .await;
                return Err(UploadError::S3(e.to_string()));
            }
        }
    }

    // Parts must be sorted by part number for CompleteMultipartUpload
    completed_parts.sort_by_key(|p| p.part_number());

    let completed_upload = CompletedMultipartUpload::builder()
        .set_parts(Some(completed_parts))
        .build();

    state
        .s3_client
        .complete_multipart_upload()
        .bucket(&state.s3_bucket)
        .key(key)
        .upload_id(&upload_id)
        .multipart_upload(completed_upload)
        .send()
        .await
        .map_err(|e| UploadError::S3(e.to_string()))?;

    if let Some(n) = notifier {
        n.send(UploadEvent::UploadCompleted {
            upload_id: n.upload_id.to_string(),
        })
        .await;
    }

    Ok(())
}
