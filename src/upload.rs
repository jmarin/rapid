use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use futures_util::TryStreamExt;
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::io::StreamReader;
use uuid::Uuid;

use crate::{AppState, error::UploadError, image::{self, STANDARD_SIZES, HIGH_RES}, magic::mime_type_magic, ws::UploadEvent};

const MIN_CHUNK_SIZE: u64 = 8 * 1024 * 1024; // 8MB
const MAX_PARTS: u64 = 10_000;
const SEMAPHORE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

struct ProgressNotifier<'a> {
    tx: &'a mpsc::Sender<UploadEvent>,
    upload_id: &'a str,
}

impl<'a> ProgressNotifier<'a> {
    fn new(tx: Option<&'a mpsc::Sender<UploadEvent>>, upload_id: Option<&'a str>) -> Option<Self> {
        Some(Self {
            tx: tx?,
            upload_id: upload_id?,
        })
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

/// Returns `true` if the MIME type is an allowed upload type (image or video).
pub fn is_allowed_mime_type(mime_type: &str) -> bool {
    mime_type.starts_with("image/") || mime_type.starts_with("video/")
}

/// Calculate chunk size as a multiple of 8MB that keeps total parts <= 10,000.
fn calculate_chunk_size(file_size: u64) -> u64 {
    let mut chunk_size = MIN_CHUNK_SIZE;
    while file_size.div_ceil(chunk_size) > MAX_PARTS {
        chunk_size *= 2;
    }
    chunk_size
}

use tempfile::NamedTempFile;

pub async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse, UploadError> {
    let file_id = Uuid::new_v4().to_string();

    let upload_id = headers
        .get("x-upload-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let file_name = headers
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    // Stream request body to a temp file (auto-deleted on drop).
    // Preserve the original file extension so the `image` crate can detect the format.
    let suffix = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let temp_file = tempfile::Builder::new()
        .suffix(&suffix)
        .tempfile_in(&state.upload_dir)?;
    let temp_path = temp_file.path().to_path_buf();

    let stream = body
        .into_data_stream()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));
    let mut reader = StreamReader::new(stream);

    let mut file = tokio::fs::File::create(&temp_path).await?;
    let size_bytes = match tokio::io::copy(&mut reader, &mut file).await {
        Ok(size) => size,
        Err(e) => {
            return Err(UploadError::Io(e));
        }
    };
    file.flush().await?;

    // Look up progress sender after body is fully received
    let progress_tx: Option<mpsc::Sender<UploadEvent>> = upload_id
        .as_ref()
        .and_then(|uid| state.upload_progress.get(uid).map(|r| r.value().clone()));

    // Detect MIME type from temp file
    let mime_type = mime_type_magic(&temp_path).await?;

    // Reject files that are not image or video: delete temp file, notify client, and return 422
    if !is_allowed_mime_type(&mime_type) {
        if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
            let _ = tx
                .send(UploadEvent::UploadFailed {
                    upload_id: uid.clone(),
                    error: format!("not an image or video: {mime_type}"),
                })
                .await;
        }
        if let Some(ref uid) = upload_id {
            state.upload_progress.remove(uid);
        }
        return Err(UploadError::NotAnImageOrVideo(mime_type));
    }

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
            n.send(UploadEvent::PartCompleted {
                upload_id: n.upload_id.to_string(),
                part_number: 1,
                total_parts: 1,
            })
            .await;
            n.send(UploadEvent::UploadCompleted {
                upload_id: n.upload_id.to_string(),
            })
            .await;
        }
    } else {
        multipart_upload(
            &state, &temp_path, &file_id, size_bytes, &mime_type, &notifier,
        )
        .await?;
    }

    // Read image dimensions if this is an image
    let (img_width, img_height) = if mime_type.starts_with("image/") {
        match image::get_dimensions(&temp_path) {
            Ok((w, h)) => (Some(w), Some(h)),
            Err(e) => {
                tracing::warn!(file_id = %file_id, error = %e, "failed to read image dimensions");
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    // Save file metadata to SQLite
    if let Err(e) = state
        .metadata
        .insert(&file_id, &file_name, size_bytes as i64, &mime_type, img_width, img_height)
        .await
    {
        tracing::error!(file_id = %file_id, error = %e, "failed to save file metadata");
    }

    // Spawn background image processing (keeps temp_file alive via ownership transfer)
    let spawn_state = state.clone();
    let cleanup_state = state.clone();
    let spawn_file_id = file_id.clone();
    let spawn_temp_path = temp_path.clone();
    let spawn_mime = mime_type.clone();
    let spawn_progress_tx = progress_tx.clone();
    let spawn_upload_id = upload_id.clone();
    tokio::spawn(async move {
        process_image_derivatives(
            spawn_state,
            spawn_file_id,
            spawn_temp_path,
            temp_file, // move ownership to keep file alive
            spawn_mime,
            spawn_progress_tx,
            spawn_upload_id.clone(),
        )
        .await;
        // Clean up progress entry after processing is done
        if let Some(ref uid) = spawn_upload_id {
            cleanup_state.upload_progress.remove(uid);
        }
    });

    let key = file_id.clone();
    let response = UploadResponse {
        id: file_id,
        key,
        size_bytes,
        mime_type,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Background image processing: generates derivatives from the local temp file,
/// uploads each to S3, inserts metadata rows, and sends WebSocket progress events.
/// The `_temp_file` parameter is held to prevent deletion until processing completes.
async fn process_image_derivatives(
    state: AppState,
    file_id: String,
    temp_path: std::path::PathBuf,
    _temp_file: NamedTempFile,
    mime_type: String,
    progress_tx: Option<mpsc::Sender<UploadEvent>>,
    upload_id: Option<String>,
) {
    // Only process image types (not video)
    if !mime_type.starts_with("image/") {
        return;
    }

    // Determine which sizes to generate
    let specs: Vec<image::ResizeSpec> = {
        let mut s: Vec<image::ResizeSpec> = STANDARD_SIZES.to_vec();
        match image::needs_high_res(&temp_path) {
            Ok(true) => s.push(HIGH_RES),
            Ok(false) => {}
            Err(e) => {
                tracing::error!(file_id = %file_id, error = %e, "failed to check image dimensions");
                return;
            }
        }
        s
    };

    let total = specs.len() as u32;

    // Notify processing started
    if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
        let _ = tx
            .send(UploadEvent::ProcessingStarted {
                upload_id: uid.clone(),
                total_derivatives: total,
            })
            .await;
    }

    // Create a temp directory for derivative output files
    let derivative_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(file_id = %file_id, error = %e, "failed to create temp dir for derivatives");
            if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
                let _ = tx
                    .send(UploadEvent::ProcessingFailed {
                        upload_id: uid.clone(),
                        error: e.to_string(),
                    })
                    .await;
            }
            return;
        }
    };

    // Pre-compute output paths for each spec.
    let spec_outputs: Vec<(image::ResizeSpec, std::path::PathBuf)> = specs
        .iter()
        .map(|spec| {
            let size_label = format!("{}x{}", spec.width, spec.height);
            let out = derivative_dir
                .path()
                .join(format!("{}_{}.png", file_id, size_label));
            (*spec, out)
        })
        .collect();

    // Resize all derivatives sequentially — libvips manages its own thread pool.
    let blocking_input = temp_path.clone();
    let spec_outputs_clone = spec_outputs.clone();
    let resize_start = std::time::Instant::now();
    let resize_results = tokio::task::spawn_blocking(move || {
        image::resize_all(&blocking_input, &spec_outputs_clone)
    })
    .await;
    let resize_elapsed = resize_start.elapsed();
    tracing::info!(file_id = %file_id, elapsed_ms = resize_elapsed.as_millis(), "all derivatives resized");

    let per_spec_results = match resize_results {
        Ok(Ok(batch)) => {
            // Send decode timing event
            tracing::info!(file_id = %file_id, decode_ms = batch.decode_elapsed.as_millis(), "image decoded");
            if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
                let _ = tx
                    .send(UploadEvent::DecodingCompleted {
                        upload_id: uid.clone(),
                        elapsed_ms: batch.decode_elapsed.as_millis(),
                    })
                    .await;
            }
            batch.items
        }
        Ok(Err(e)) => {
            tracing::error!(file_id = %file_id, error = %e, "failed to decode image for derivatives");
            if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
                let _ = tx
                    .send(UploadEvent::ProcessingFailed {
                        upload_id: uid.clone(),
                        error: e.to_string(),
                    })
                    .await;
            }
            return;
        }
        Err(e) => {
            tracing::error!(file_id = %file_id, error = %e, "spawn_blocking panicked during image processing");
            if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
                let _ = tx
                    .send(UploadEvent::ProcessingFailed {
                        upload_id: uid.clone(),
                        error: "image processing task panicked".to_string(),
                    })
                    .await;
            }
            return;
        }
    };

    // Now iterate through results: insert DB rows, upload to S3, send WS events.
    for (i, (_label, output_path, resize_result)) in per_spec_results.into_iter().enumerate() {
        let size_label = format!(
            "{}x{}",
            specs[i].width, specs[i].height
        );
        let s3_key = format!("{}_{}", file_id, size_label);
        let derivative_id = Uuid::new_v4().to_string();

        // Insert derivative row as "processing"
        if let Err(e) = state
            .metadata
            .insert_derivative(
                &derivative_id,
                &file_id,
                &size_label,
                &s3_key,
                specs[i].width as i64,
                specs[i].height as i64,
                "processing",
            )
            .await
        {
            tracing::error!(file_id = %file_id, size = %size_label, error = %e, "failed to insert derivative metadata");
            continue;
        }

        // Check if resize succeeded
        if let Err(e) = resize_result {
            tracing::error!(file_id = %file_id, size = %size_label, error = %e, "image resize failed");
            let _ = state
                .metadata
                .update_derivative_status(&derivative_id, "failed")
                .await;
            if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
                let _ = tx
                    .send(UploadEvent::ProcessingFailed {
                        upload_id: uid.clone(),
                        error: format!("resize failed for {}: {}", size_label, e),
                    })
                    .await;
            }
            continue;
        }

        // Update stored dimensions to actual output size (may differ from spec
        // when the original is smaller than the target and we skip upscaling).
        if let Ok((actual_w, actual_h)) = image::get_dimensions(&output_path) {
            if actual_w != specs[i].width || actual_h != specs[i].height {
                let _ = state
                    .metadata
                    .update_derivative_dimensions(&derivative_id, actual_w as i64, actual_h as i64)
                    .await;
            }
        }

        // Upload derivative to S3
        let body_stream = match ByteStream::from_path(&output_path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(file_id = %file_id, size = %size_label, error = %e, "failed to read derivative file");
                let _ = state
                    .metadata
                    .update_derivative_status(&derivative_id, "failed")
                    .await;
                continue;
            }
        };

        if let Err(e) = state
            .s3_client
            .put_object()
            .bucket(&state.s3_bucket)
            .key(&s3_key)
            .body(body_stream)
            .content_type("image/jpeg")
            .send()
            .await
        {
            tracing::error!(file_id = %file_id, size = %size_label, error = %e, "failed to upload derivative to S3");
            let _ = state
                .metadata
                .update_derivative_status(&derivative_id, "failed")
                .await;
            continue;
        }

        // Mark derivative as completed
        let _ = state
            .metadata
            .update_derivative_status(&derivative_id, "completed")
            .await;

        // Notify progress
        if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
            let _ = tx
                .send(UploadEvent::DerivativeCompleted {
                    upload_id: uid.clone(),
                    size_label: size_label.clone(),
                    derivative_number: (i + 1) as u32,
                    total_derivatives: total,
                })
                .await;
        }
    }

    // Notify processing completed
    if let (Some(tx), Some(uid)) = (&progress_tx, &upload_id) {
        let _ = tx
            .send(UploadEvent::ProcessingCompleted {
                upload_id: uid.clone(),
                elapsed_ms: resize_elapsed.as_millis(),
            })
            .await;
    }

    tracing::info!(file_id = %file_id, "image processing completed");
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

    // Upload parts in parallel, throttled by both global AND per-upload semaphores
    let per_upload_sem = Arc::new(Semaphore::new(state.max_parts_per_upload));
    let mut join_set = JoinSet::new();
    let bucket: Arc<str> = state.s3_bucket.as_str().into();

    for part_idx in 0..num_parts {
        let part_number = (part_idx + 1) as i32; // S3 parts are 1-indexed
        let offset = part_idx * chunk_size;
        // Most parts are exactly chunk_size (e.g. 8MB), but the last part is usually smaller
        let length = std::cmp::min(chunk_size, file_size - offset) as usize;

        let s3_client = state.s3_client.clone();
        let bucket = bucket.clone();
        let key = key.to_string();
        let upload_id = upload_id.clone();
        let temp_path = temp_path.to_path_buf();
        let global_sem = state.upload_semaphore.clone();
        let local_sem = per_upload_sem.clone();

        join_set.spawn(async move {
            // Acquire per-upload permit first (fast, local)
            let _local_permit = tokio::time::timeout(
                SEMAPHORE_TIMEOUT,
                local_sem.acquire(),
            )
            .await
            .map_err(|_| "per-upload semaphore acquisition timed out".to_string())?
            .map_err(|e| e.to_string())?;

            // Acquire global permit with timeout
            let _global_permit = tokio::time::timeout(
                SEMAPHORE_TIMEOUT,
                global_sem.acquire(),
            )
            .await
            .map_err(|_| "global semaphore acquisition timed out".to_string())?
            .map_err(|e| e.to_string())?;

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
                .bucket(&*bucket)
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── calculate_chunk_size ──

    #[test]
    fn chunk_size_small_file() {
        // Anything <= 8MB * 10_000 should use the minimum 8MB chunk
        assert_eq!(calculate_chunk_size(1), MIN_CHUNK_SIZE);
        assert_eq!(calculate_chunk_size(MIN_CHUNK_SIZE), MIN_CHUNK_SIZE);
    }

    #[test]
    fn chunk_size_at_max_parts_boundary() {
        // Exactly 10_000 parts at 8MB each = 80GB
        let boundary = MIN_CHUNK_SIZE * MAX_PARTS;
        assert_eq!(calculate_chunk_size(boundary), MIN_CHUNK_SIZE);
    }

    #[test]
    fn chunk_size_just_above_boundary() {
        // One byte over forces doubling
        let boundary = MIN_CHUNK_SIZE * MAX_PARTS + 1;
        assert_eq!(calculate_chunk_size(boundary), MIN_CHUNK_SIZE * 2);
    }

    #[test]
    fn chunk_size_very_large_file() {
        // 1 TB file
        let one_tb = 1024 * 1024 * 1024 * 1024u64;
        let chunk = calculate_chunk_size(one_tb);
        assert!(chunk >= MIN_CHUNK_SIZE);
        assert!(one_tb.div_ceil(chunk) <= MAX_PARTS);
    }

    #[test]
    fn chunk_size_zero() {
        // Edge case: zero-byte file
        assert_eq!(calculate_chunk_size(0), MIN_CHUNK_SIZE);
    }

    // ── is_allowed_mime_type ──

    #[test]
    fn allows_image_types() {
        assert!(is_allowed_mime_type("image/jpeg"));
        assert!(is_allowed_mime_type("image/png"));
        assert!(is_allowed_mime_type("image/webp"));
        assert!(is_allowed_mime_type("image/x-fujifilm-raf"));
    }

    #[test]
    fn allows_video_types() {
        assert!(is_allowed_mime_type("video/mp4"));
        assert!(is_allowed_mime_type("video/quicktime"));
    }

    #[test]
    fn rejects_non_media_types() {
        assert!(!is_allowed_mime_type("application/pdf"));
        assert!(!is_allowed_mime_type("text/plain"));
        assert!(!is_allowed_mime_type("application/octet-stream"));
        assert!(!is_allowed_mime_type(""));
    }
}
