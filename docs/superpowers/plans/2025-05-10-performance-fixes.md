# Performance Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 15 performance and correctness issues identified in code review, ordered by severity and dependency.

**Architecture:** Phased approach — quick correctness wins first (CORS, temp file guard), then critical performance (SQLite WAL, semaphore timeouts, memory), then high-priority (DashMap, S3 timeouts, download optimization), then medium (backpressure, WebSocket hardening).

**Tech Stack:** Rust, Axum 0.8, aws-sdk-s3, sqlx (SQLite), tokio, tower-http

---

## Phase 1 — Quick Correctness Wins

These are trivial fixes with zero risk. Get them out of the way first.

### Task 1: Fix CORS origin typo

**Files:**
- Modify: `src/lib.rs:51`

- [x] **Step 1: Fix the typo**

In `src/lib.rs`, line 51, change the double-colon URL to a valid one:

```rust
// Before:
let allowed_origins = ["http:://localhost:3000".parse()?];

// After:
let allowed_origins = ["http://localhost:3000".parse()?];
```

- [x] **Step 2: Build to verify**

Run: `cargo build`
Expected: compiles successfully

- [x] **Step 3: Run tests**

Run: `cargo test`
Expected: all 49 tests pass

- [x] **Step 4: Commit**

```bash
git add src/lib.rs
git commit -m "fix: correct CORS origin URL (double colon typo)"
```

---

### Task 2: Temp file cleanup guard in upload_file

The temp file at `temp_path` is leaked if `multipart_upload()` or the single-PUT S3 call returns an error — the `?` propagates before reaching `tokio::fs::remove_file` at line 191.

**Files:**
- Modify: `src/upload.rs:64-201`

- [x] **Step 1: Add a drop guard struct**

Add this struct above `upload_file` in `src/upload.rs`:

```rust
/// RAII guard that deletes a temp file on drop unless disarmed.
struct TempFileGuard {
    path: Option<std::path::PathBuf>,
}

impl TempFileGuard {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Disarm the guard so it does NOT delete the file on drop.
    /// Not used currently since we always want cleanup, but available
    /// if we ever need to preserve the file.
    #[allow(dead_code)]
    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            // Best-effort sync delete. We're in a drop handler so we can't await.
            let _ = std::fs::remove_file(&path);
        }
    }
}
```

- [x] **Step 2: Use the guard in upload_file**

Replace the manual temp file creation and cleanup in `upload_file`. After creating the temp file (line 89), create the guard. Remove the manual `tokio::fs::remove_file` calls (lines 93, 113, 191):

```rust
// After line 89: let mut file = tokio::fs::File::create(&temp_path).await?;
// Add:
let _temp_guard = TempFileGuard::new(temp_path.clone());
```

Remove these lines since the guard handles cleanup:
- Line 93: `let _ = tokio::fs::remove_file(&temp_path).await;` (inside the copy error branch)
- Line 113: `let _ = tokio::fs::remove_file(&temp_path).await;` (inside the MIME rejection branch)
- Lines 190-191: the explicit temp file cleanup at the end

The early-return error paths (lines 94, 126) and the `?` on line 172 will now all trigger the guard's `Drop`, deleting the temp file.

- [x] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [x] **Step 4: Commit**

```bash
git add src/upload.rs
git commit -m "fix: ensure temp files are cleaned up on all error paths via drop guard"
```

---

## Phase 2 — Critical Performance (SQLite & Semaphore)

### Task 3: Enable SQLite WAL mode and busy timeout

Without WAL mode, writers block all readers. This is the single biggest bottleneck under concurrent load.

**Files:**
- Modify: `src/metadata.rs:51-54`
- Modify: `src/utils/constants.rs:9`

- [x] **Step 1: Update the default DB path constant**

In `src/utils/constants.rs`, add WAL mode and busy timeout pragmas to the default connection URL:

```rust
// Before:
pub const DEFAULT_DB_PATH: &str = "sqlite:data/rapid.db?mode=rwc";

// After:
pub const DEFAULT_DB_PATH: &str = "sqlite:data/rapid.db?mode=rwc";
```

Note: sqlx does not support pragmas in the URL for SQLite. We'll configure them on the pool instead.

- [x] **Step 2: Configure the pool with pragmas and explicit pool size**

In `src/metadata.rs`, replace the `new` method:

```rust
use sqlx::sqlite::SqlitePoolOptions;

impl MetadataStore {
    pub async fn new(db_url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("PRAGMA journal_mode=WAL")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("PRAGMA busy_timeout=5000")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("PRAGMA synchronous=NORMAL")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(db_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }
```

The `after_connect` callback runs on every new connection in the pool. `journal_mode=WAL` allows concurrent readers + one writer. `busy_timeout=5000` retries for 5 seconds instead of immediately returning SQLITE_BUSY. `synchronous=NORMAL` is safe with WAL and reduces fsync overhead.

- [x] **Step 3: Add the import**

Add `use sqlx::sqlite::SqlitePoolOptions;` at the top of `src/metadata.rs` (alongside the existing `use sqlx::{FromRow, SqlitePool};`):

```rust
use sqlx::{FromRow, SqlitePool, sqlite::SqlitePoolOptions};
```

- [x] **Step 4: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [x] **Step 5: Commit**

```bash
git add src/metadata.rs
git commit -m "perf: enable SQLite WAL mode, busy timeout, and explicit pool sizing"
```

---

### Task 4: Per-upload concurrency cap + semaphore acquisition timeout

Currently a single large upload (10GB = 1,250 parts) can monopolize all 24 semaphore permits. And if S3 hangs, a permit is held forever.

**Files:**
- Modify: `src/upload.rs:203-291` (multipart_upload function)
- Modify: `src/upload.rs:21-22` (add constants)

- [x] **Step 1: Add constants for per-upload cap and timeout**

Add after line 22 in `src/upload.rs`:

```rust
const MAX_PARTS_PER_UPLOAD: usize = 6;
const SEMAPHORE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
```

- [x] **Step 2: Add a per-upload semaphore and timeout to multipart_upload**

In the `multipart_upload` function, create a per-upload semaphore before the loop and acquire both in the spawned task:

```rust
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

    // Per-upload semaphore limits how many parts THIS upload can have in-flight,
    // so one large upload can't monopolize all global permits.
    let per_upload_sem = Arc::new(Semaphore::new(MAX_PARTS_PER_UPLOAD));

    // Upload parts in parallel, throttled by both global AND per-upload semaphores
    let mut join_set = JoinSet::new();

    for part_idx in 0..num_parts {
        let part_number = (part_idx + 1) as i32;
        let offset = part_idx * chunk_size;
        let length = std::cmp::min(chunk_size, file_size - offset) as usize;

        let s3_client = state.s3_client.clone();
        let bucket = state.s3_bucket.clone();
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

    // ... rest of the function (collecting results) stays the same
```

- [x] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass (unit tests don't exercise the multipart path)

- [x] **Step 4: Commit**

```bash
git add src/upload.rs
git commit -m "perf: add per-upload concurrency cap and semaphore acquisition timeout"
```

---

## Phase 3 — High Priority Performance

### Task 5: Replace RwLock<HashMap> with DashMap for upload_progress

**Files:**
- Modify: `Cargo.toml` (add dashmap dependency)
- Modify: `src/lib.rs:20,25,45` (change type)
- Modify: `src/upload.rs:101-106,122-125,185-188` (change access pattern)
- Modify: `src/ws.rs:75,84-86,107-111` (change access pattern)

- [x] **Step 1: Add dashmap dependency**

Add to `Cargo.toml` under `[dependencies]`:

```toml
dashmap = "6"
```

- [x] **Step 2: Update AppState in lib.rs**

Replace the `upload_progress` field type. Remove `RwLock` and `HashMap` imports if unused elsewhere:

```rust
// Before:
use std::collections::HashMap;
use tokio::sync::{RwLock, Semaphore, mpsc};

// After:
use dashmap::DashMap;
use tokio::sync::{Semaphore, mpsc};
```

Update the `AppState` struct:

```rust
#[derive(Clone)]
pub struct AppState {
    pub upload_dir: PathBuf,
    pub s3_client: aws_sdk_s3::Client,
    pub s3_bucket: String,
    pub upload_semaphore: Arc<Semaphore>,
    pub upload_progress: Arc<DashMap<String, mpsc::Sender<ws::UploadEvent>>>,
    pub metadata: MetadataStore,
}
```

- [x] **Step 3: Update main.rs initialization**

```rust
// Before:
use tokio::sync::{RwLock, Semaphore};

// After:
use dashmap::DashMap;
use tokio::sync::Semaphore;
```

```rust
// Before:
upload_progress: Arc::new(RwLock::new(HashMap::new())),

// After:
upload_progress: Arc::new(DashMap::new()),
```

Remove `use std::collections::HashMap;` from main.rs.

- [x] **Step 4: Update upload.rs — read access**

Replace lines 101-106 in `upload_file`:

```rust
// Before:
let progress_tx: Option<mpsc::Sender<UploadEvent>> = if let Some(ref uid) = upload_id {
    let map = state.upload_progress.read().await;
    map.get(uid).cloned()
} else {
    None
};

// After:
let progress_tx: Option<mpsc::Sender<UploadEvent>> = upload_id
    .as_ref()
    .and_then(|uid| state.upload_progress.get(uid).map(|r| r.value().clone()));
```

- [x] **Step 5: Update upload.rs — write access (MIME rejection)**

Replace lines 122-125:

```rust
// Before:
if let Some(ref uid) = upload_id {
    let mut map = state.upload_progress.write().await;
    map.remove(uid);
}

// After:
if let Some(ref uid) = upload_id {
    state.upload_progress.remove(uid);
}
```

- [x] **Step 6: Update upload.rs — write access (completion)**

Replace lines 185-188:

```rust
// Before:
if let Some(ref uid) = upload_id {
    let mut map = state.upload_progress.write().await;
    map.remove(uid);
}

// After:
if let Some(ref uid) = upload_id {
    state.upload_progress.remove(uid);
}
```

- [x] **Step 7: Update ws.rs — subscribe insert**

Replace lines 83-86:

```rust
// Before:
{
    let mut map = progress_map.write().await;
    map.insert(upload_id.clone(), event_tx.clone());
}

// After:
progress_map.insert(upload_id.clone(), event_tx.clone());
```

- [x] **Step 8: Update ws.rs — cleanup on disconnect**

Replace lines 106-112:

```rust
// Before:
let ids = subscribed_ids.lock().unwrap().clone();
if !ids.is_empty() {
    let mut map = state.upload_progress.write().await;
    for id in &ids {
        map.remove(id);
    }
}

// After:
let ids = subscribed_ids.lock().unwrap().clone();
for id in &ids {
    state.upload_progress.remove(id);
}
```

- [x] **Step 9: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all 49 tests pass

- [x] **Step 10: Commit**

```bash
git add Cargo.toml src/lib.rs src/main.rs src/upload.rs src/ws.rs
git commit -m "perf: replace RwLock<HashMap> with DashMap for lock-free upload progress"
```

---

### Task 6: Add S3 client timeouts

**Files:**
- Modify: `src/main.rs:46-52` (S3 config builder)

- [ ] **Step 1: Add timeout configuration to the S3 client**

In `src/main.rs`, add timeout config to the S3 builder:

```rust
// Before:
let s3_config = aws_sdk_s3::Config::builder()
    .region(aws_sdk_s3::config::Region::new(s3_region))
    .endpoint_url(&s3_endpoint)
    .credentials_provider(s3_creds)
    .behavior_version_latest()
    .force_path_style(true)
    .build();

// After:
use aws_sdk_s3::config::timeout::TimeoutConfig;
use std::time::Duration;

let timeout_config = TimeoutConfig::builder()
    .operation_timeout(Duration::from_secs(300))      // 5 min per S3 operation
    .operation_attempt_timeout(Duration::from_secs(60)) // 60s per attempt
    .connect_timeout(Duration::from_secs(10))
    .build();

let s3_config = aws_sdk_s3::Config::builder()
    .region(aws_sdk_s3::config::Region::new(s3_region))
    .endpoint_url(&s3_endpoint)
    .credentials_provider(s3_creds)
    .behavior_version_latest()
    .force_path_style(true)
    .timeout_config(timeout_config)
    .build();
```

Note: the exact import path may vary by aws-sdk-s3 version. If `aws_sdk_s3::config::timeout::TimeoutConfig` doesn't resolve, use `aws_config::timeout::TimeoutConfig` or check the docs for your pinned version (1.132.0). The builder may also be at `aws_smithy_types::timeout::TimeoutConfig`.

- [ ] **Step 2: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass. If the import path is wrong, check `cargo doc --open` for the correct location.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "perf: add S3 client timeouts (connect 10s, attempt 60s, operation 5min)"
```

---

### Task 7: Eliminate double round-trip in download (HEAD + GET)

For non-range requests, the HEAD call is unnecessary since GET returns Content-Length. For range requests, we can use the metadata store's `size_bytes` instead.

**Files:**
- Modify: `src/download.rs:11-105`

- [ ] **Step 1: Rewrite download_file to avoid HEAD for non-range requests**

```rust
pub async fn download_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, DownloadError> {
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    // For range requests, we need the total size to parse the Range header.
    // Look it up from the metadata store (free, local) instead of a HEAD call to S3.
    let total_size = if range_header.is_some() {
        match state.metadata.get_by_id(&id).await {
            Ok(Some(meta)) => meta.size_bytes as u64,
            Ok(None) => return Err(DownloadError::NotFound),
            Err(e) => return Err(DownloadError::S3(e.to_string())),
        }
    } else {
        0 // Not needed for non-range requests; we'll get it from the GET response
    };

    // Build the S3 GET request
    let mut get_req = state
        .s3_client
        .get_object()
        .bucket(&state.s3_bucket)
        .key(&id);

    let (status, start, end, actual_total_size) = if let Some(range) = range_header {
        let (s, e) = parse_range(range, total_size).ok_or(DownloadError::InvalidRange)?;
        get_req = get_req.range(format!("bytes={}-{}", s, e));
        (StatusCode::PARTIAL_CONTENT, s, e, total_size)
    } else {
        (StatusCode::OK, 0u64, 0u64, 0u64) // end/total filled in after GET
    };

    let resp = get_req.send().await.map_err(|e| {
        use aws_sdk_s3::operation::get_object::GetObjectError;
        let is_not_found = e
            .as_service_error()
            .map(|se| matches!(se, GetObjectError::NoSuchKey(_)))
            .unwrap_or(false);
        if is_not_found {
            DownloadError::NotFound
        } else {
            DownloadError::S3(e.to_string())
        }
    })?;

    let content_type = resp
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    // For non-range requests, get actual size from the response
    let (final_start, final_end, final_total) = if status == StatusCode::OK {
        let size = resp.content_length().unwrap_or(0) as u64;
        (0u64, size.saturating_sub(1), size)
    } else {
        (start, end, actual_total_size)
    };

    let content_length = if final_total == 0 {
        0u64
    } else {
        final_end - final_start + 1
    };

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string())
            .unwrap_or(HeaderValue::from_static("0")),
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    if status == StatusCode::PARTIAL_CONTENT {
        let content_range = format!("bytes {}-{}/{}", final_start, final_end, final_total);
        if let Ok(v) = HeaderValue::from_str(&content_range) {
            response_headers.insert(header::CONTENT_RANGE, v);
        }
    }

    let body = Body::from_stream(ReaderStream::new(resp.body.into_async_read()));

    Ok((status, response_headers, body))
}
```

- [ ] **Step 2: Add metadata import to download.rs**

The function now uses `state.metadata`, which is already on `AppState`. No new imports needed beyond `crate::AppState` which is already imported.

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass (parse_range tests are unaffected)

- [ ] **Step 4: Commit**

```bash
git add src/download.rs
git commit -m "perf: eliminate HEAD+GET double round-trip in downloads, use metadata store for range size"
```

---

### Task 8: Replace std::sync::Mutex with tokio::sync::Mutex in ws.rs

**Files:**
- Modify: `src/ws.rs:61,87,106`

- [ ] **Step 1: Change the Mutex type**

In `src/ws.rs`, replace the `std::sync::Mutex` with `tokio::sync::Mutex`:

```rust
// Before (line 61):
let subscribed_ids = std::sync::Mutex::new(Vec::<String>::new());

// After:
let subscribed_ids = tokio::sync::Mutex::new(Vec::<String>::new());
```

- [ ] **Step 2: Update lock calls to .await**

Line 87:
```rust
// Before:
subscribed_ids.lock().unwrap().push(upload_id.clone());

// After:
subscribed_ids.lock().await.push(upload_id.clone());
```

Line 106:
```rust
// Before:
let ids = subscribed_ids.lock().unwrap().clone();

// After:
let ids = subscribed_ids.lock().await.clone();
```

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [ ] **Step 4: Commit**

```bash
git add src/ws.rs
git commit -m "fix: replace std::sync::Mutex with tokio::sync::Mutex in async WebSocket handler"
```

---

### Task 9: Avoid COUNT(*) full table scan on every list request

**Files:**
- Modify: `src/metadata.rs:85-104` (list method)
- Create: `migrations/20250510_add_file_metadata_count_index.sql`

- [ ] **Step 1: Add a created_at index migration**

Create `migrations/20250511_add_created_at_index.sql`:

```sql
CREATE INDEX IF NOT EXISTS idx_file_metadata_created_at ON file_metadata(created_at DESC);
```

This speeds up the `ORDER BY created_at DESC LIMIT ? OFFSET ?` query. It won't help COUNT(*) directly, but it's needed anyway.

- [ ] **Step 2: Switch to a count-free pagination approach**

Replace the `list` method to use "has next page" detection instead of a full COUNT(*). Fetch one extra row to detect if there's a next page:

```rust
pub async fn list(
    &self,
    page: i64,
    per_page: i64,
) -> Result<(Vec<FileMetadata>, bool), sqlx::Error> {
    let offset = (page - 1) * per_page;
    let fetch_limit = per_page + 1; // Fetch one extra to detect next page

    let mut items = sqlx::query_as::<_, FileMetadata>(
        "SELECT id, file_name, size_bytes, mime_type, created_at FROM file_metadata ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(fetch_limit)
    .bind(offset)
    .fetch_all(&self.pool)
    .await?;

    let has_next = items.len() as i64 > per_page;
    if has_next {
        items.pop(); // Remove the extra probe row
    }

    Ok((items, has_next))
}
```

- [ ] **Step 3: Update ListResponse and the handler**

Update `ListResponse` to remove `total` and `total_pages`, replace with `has_next`:

```rust
#[derive(Serialize)]
pub struct ListResponse {
    pub items: Vec<FileMetadata>,
    pub page: i64,
    pub per_page: i64,
    pub has_next: bool,
}
```

Update `list_file_metadata`:

```rust
pub async fn list_file_metadata(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let page = clamp_page(params.page);
    let per_page = DEFAULT_PER_PAGE;

    match state.metadata.list(page, per_page).await {
        Ok((items, has_next)) => {
            let resp = ListResponse {
                items,
                page,
                per_page,
                has_next,
            };
            (StatusCode::OK, Json(serde_json::json!(resp))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to list file metadata");
            let body = serde_json::json!({ "error": "internal error" });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}
```

- [ ] **Step 4: Remove now-unused total_pages function**

The `total_pages` function and its tests can be removed since we no longer compute total pages. Remove the function (lines 37-43) and all tests that reference it (`total_pages_exact_division`, `total_pages_with_remainder`, `total_pages_zero_items`, `total_pages_one_item`, `total_pages_per_page_equals_total`, `total_pages_zero_per_page`).

- [ ] **Step 5: Update the metadata.html UI to use has_next**

In `assets/metadata.html`, update the JavaScript pagination logic:

```javascript
// In loadPage(), replace:
//   totalPages = data.total_pages;
//   pageInfo.textContent = `${data.page} / ${totalPages}`;
//   prevBtn.disabled = data.page <= 1;
//   nextBtn.disabled = data.page >= totalPages;

// With:
pageInfo.textContent = `Page ${data.page}`;
prevBtn.disabled = data.page <= 1;
nextBtn.disabled = !data.has_next;
```

Remove the `totalPages` variable and all references to `data.total_pages` and `data.total`.

Also update the empty-state check:

```javascript
// Before:
if (data.total === 0) {

// After:
if (data.items.length === 0 && data.page === 1) {
```

And update the next button handler:

```javascript
// Before:
nextBtn.addEventListener("click", () => { if (currentPage < totalPages) loadPage(currentPage + 1); });

// After:
nextBtn.addEventListener("click", () => loadPage(currentPage + 1));
```

- [ ] **Step 6: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, tests pass (some pagination tests will need updating since `total_pages` is removed)

- [ ] **Step 7: Commit**

```bash
git add src/metadata.rs migrations/ assets/metadata.html
git commit -m "perf: replace COUNT(*) with has_next pagination to avoid full table scan"
```

---

## Phase 4 — Medium Priority (Backpressure & WebSocket)

### Task 10: Add request timeout layer

**Files:**
- Modify: `Cargo.toml` (add tower feature)
- Modify: `src/lib.rs:60-76` (add timeout layer)

- [ ] **Step 1: Add tower dependency**

Add to `Cargo.toml`:

```toml
tower = { version = "0.5", features = ["timeout"] }
```

- [ ] **Step 2: Add a timeout layer to the upload route**

In `src/lib.rs`, wrap the upload route with a 30-minute timeout (generous for 10GB files):

```rust
use tower::timeout::TimeoutLayer;
use std::time::Duration;

let upload_route = Router::new()
    .route("/upload", post(upload::upload_file))
    .layer(DefaultBodyLimit::disable())
    .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024 * 1024))
    .layer(TimeoutLayer::new(Duration::from_secs(1800))); // 30 min
```

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml src/lib.rs
git commit -m "perf: add 30-minute request timeout to upload route"
```

---

### Task 11: Add WebSocket idle timeout and ping/pong

**Files:**
- Modify: `src/ws.rs:53-103`

- [ ] **Step 1: Add idle timeout and periodic ping**

Rewrite `handle_socket` to add a 5-minute idle timeout. If no message is received in 5 minutes, the connection is closed:

```rust
use std::time::Duration;

const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let (event_tx, mut event_rx) = mpsc::channel::<UploadEvent>(256);

    let subscribed_ids = tokio::sync::Mutex::new(Vec::<String>::new());

    // Forward events from mpsc channel -> WebSocket, with periodic pings
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Read client messages with idle timeout
    let progress_map = state.upload_progress.clone();
    let recv_task = async {
        loop {
            match tokio::time::timeout(WS_IDLE_TIMEOUT, ws_rx.next()).await {
                Ok(Some(Ok(msg))) => match msg {
                    Message::Text(text) => {
                        if let Ok(cmd) = serde_json::from_str::<WsCommand>(&text) {
                            match cmd {
                                WsCommand::Subscribe { upload_id } => {
                                    progress_map.insert(upload_id.clone(), event_tx.clone());
                                    subscribed_ids.lock().await.push(upload_id.clone());
                                    let _ = event_tx
                                        .send(UploadEvent::Subscribed { upload_id })
                                        .await;
                                }
                            }
                        }
                    }
                    Message::Pong(_) => {} // keep-alive response, reset idle timer
                    Message::Close(_) => break,
                    _ => {}
                },
                Ok(Some(Err(_))) => break,   // WebSocket error
                Ok(None) => break,            // Stream ended
                Err(_) => {
                    tracing::debug!("WebSocket idle timeout, closing connection");
                    break;
                }
            }
        }
    };

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    // Cleanup
    let ids = subscribed_ids.lock().await.clone();
    for id in &ids {
        state.upload_progress.remove(id);
    }
}
```

Note: this step assumes Task 5 (DashMap) and Task 8 (tokio::sync::Mutex) have already been applied. The `progress_map.insert()` and `state.upload_progress.remove()` calls use DashMap's API. The `subscribed_ids.lock().await` uses tokio::sync::Mutex.

- [ ] **Step 2: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/ws.rs
git commit -m "perf: add WebSocket idle timeout (5min) and periodic ping (30s)"
```

---

### Task 12: Reduce per-part String cloning in multipart_upload

**Files:**
- Modify: `src/lib.rs:43` (change s3_bucket type)
- Modify: `src/upload.rs:249` (remove clone)
- Modify: `src/main.rs` (wrap bucket in Arc)
- Modify: `src/metadata.rs:154-158` (update delete_file s3_bucket usage)
- Modify: `src/download.rs` (update s3_bucket usage)

- [ ] **Step 1: Change s3_bucket from String to Arc\<str\>**

In `src/lib.rs`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub upload_dir: PathBuf,
    pub s3_client: aws_sdk_s3::Client,
    pub s3_bucket: Arc<str>,
    pub upload_semaphore: Arc<Semaphore>,
    pub upload_progress: Arc<DashMap<String, mpsc::Sender<ws::UploadEvent>>>,
    pub metadata: MetadataStore,
}
```

- [ ] **Step 2: Update main.rs to wrap the bucket**

```rust
// Before:
let s3_bucket = env::var("RAPID_S3_BUCKET").unwrap_or_else(|_| DEFAULT_S3_BUCKET.to_string());

// After:
let s3_bucket: Arc<str> = env::var("RAPID_S3_BUCKET")
    .unwrap_or_else(|_| DEFAULT_S3_BUCKET.to_string())
    .into();
```

Add `use std::sync::Arc;` if not already imported (it is).

- [ ] **Step 3: Update upload.rs — multipart_upload loop**

The `state.s3_bucket.clone()` on line 249 now clones an `Arc<str>` (cheap pointer bump) instead of allocating a new `String`. No code change needed — `Arc<str>` implements `Clone` and `AsRef<str>`, and the AWS SDK accepts `impl Into<String>` which `Arc<str>` doesn't implement. So we need:

```rust
// Before:
let bucket = state.s3_bucket.clone();

// After:
let bucket = state.s3_bucket.to_string();
```

Wait — that still allocates. The real fix: clone the `Arc<str>` once before the loop:

```rust
// Before the for loop, add:
let bucket: Arc<str> = state.s3_bucket.clone();

// Inside the loop, remove the per-iteration clone:
// Before:
let bucket = state.s3_bucket.clone();

// After:
let bucket = bucket.clone(); // Arc clone = cheap refcount bump
```

Inside the spawned task, where `bucket` is passed to `.bucket(&bucket)`, the AWS SDK's `.bucket()` method accepts `impl Into<String>`. `&Arc<str>` derefs to `&str`, but the SDK needs ownership. We need to convert:

```rust
// In the spawned task:
.bucket(bucket.as_ref())
```

Actually, the S3 builder's `.bucket()` accepts `impl Into<String>`, and calling `.bucket(&*bucket)` gives `&str` which implements `Into<String>` via `.to_string()`. The simplest approach: keep `let bucket = state.s3_bucket.to_string();` once before the loop, then `let bucket = bucket.clone();` in the loop. This is one allocation total instead of N.

```rust
// Before the loop (replace line 249 area):
let bucket_str = state.s3_bucket.to_string();

// Inside the loop:
let bucket = bucket_str.clone();
```

This is still N clones of a String, but they share... no, `.clone()` on a `String` allocates. So actually the cleanest fix: keep `s3_bucket` as `String` in AppState, but clone it once before the loop and use `Arc<str>` locally:

```rust
// Before the for loop:
let bucket: Arc<str> = state.s3_bucket.as_str().into();

// Inside the loop:
let bucket = bucket.clone(); // Arc clone, no allocation

// Inside the spawned task, change .bucket(&bucket) to:
.bucket(&*bucket)
```

- [ ] **Step 4: Update all other .bucket() call sites**

In `src/download.rs`, `src/metadata.rs` (delete_file), and the single-PUT path in `upload.rs`, the `&state.s3_bucket` references work fine with `Arc<str>` since it derefs to `&str`. No changes needed for those sites — they don't clone.

Actually, the S3 SDK's `.bucket()` method takes `impl Into<String>`. `&str` converts to `String`. So `&*state.s3_bucket` works. Or just `state.s3_bucket.as_ref()`. Check if the compiler accepts it.

Given the complexity of the AWS SDK's type expectations, the simplest correct approach is:

Keep `s3_bucket` as `String` in `AppState`. Just move the clone out of the loop:

```rust
// Before the for loop, add:
let bucket = state.s3_bucket.clone();

// Inside the for loop, replace:
//   let bucket = state.s3_bucket.clone();
// with:
//   let bucket = bucket.clone();

// This is still String clones, but we can fix with Arc:
// Before the for loop:
let bucket: Arc<String> = Arc::new(state.s3_bucket.clone());

// Inside the for loop:
let bucket = bucket.clone(); // Arc bump, no alloc

// Inside the spawned task:
.bucket(bucket.as_str())
```

- [ ] **Step 5: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/main.rs src/upload.rs
git commit -m "perf: eliminate per-part String allocation for s3_bucket in multipart upload"
```

---

## Phase 5 — Low Priority Cleanup

### Task 13: Clean up file_id clone in upload response

**Files:**
- Modify: `src/upload.rs:193-198`

- [ ] **Step 1: Restructure to avoid clone**

```rust
// Before:
let response = UploadResponse {
    id: file_id.clone(),
    key: file_id,
    size_bytes,
    mime_type,
};

// After:
let key = file_id.clone();
let response = UploadResponse {
    id: file_id,
    key,
    size_bytes,
    mime_type,
};
```

Alternatively, if `id` and `key` are always the same, consider removing one field from `UploadResponse`. But that's an API change, so just swap the clone target for now.

- [ ] **Step 2: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/upload.rs
git commit -m "refactor: avoid unnecessary file_id clone in upload response"
```

---

## Execution Order Summary

| Phase | Task | Issue # | Severity | Risk |
|-------|------|---------|----------|------|
| 1 | 1. CORS typo | 13 | Medium | None |
| 1 | 2. Temp file guard | 4 | Critical | Low |
| 2 | 3. SQLite WAL + pool | 3 | Critical | Low |
| 2 | 4. Semaphore caps + timeout | 2 | Critical | Low |
| 3 | 5. DashMap | 5 | High | Low |
| 3 | 6. S3 timeouts | 7 | High | Low |
| 3 | 7. Download HEAD elimination | 9 | High | Medium |
| 3 | 8. tokio::sync::Mutex in ws | 8 | High | None |
| 3 | 9. COUNT(*) elimination | 6 | High | Medium (API change) |
| 4 | 10. Request timeout layer | 11 | Medium | Low |
| 4 | 11. WebSocket idle + ping | 12 | Medium | Low |
| 4 | 12. Per-part String cloning | 10 | Medium | Low |
| 5 | 13. file_id clone cleanup | 14 | Low | None |

**Not included (deferred):**
- Issue #1 (streaming S3 parts from disk instead of Vec buffering) — requires significant rearchitecting of the multipart upload to use file-backed ByteStream with offset support. The per-upload semaphore cap (Task 4) mitigates the worst of the memory pressure. Revisit when the AWS SDK improves `ByteStream::from_path` with offset support, or when memory profiling shows it's still a problem.
- Issue #15 (created_at as String) — low priority, would require a migration to change the column type.

**Dependencies between tasks:**
- Task 11 (WS idle timeout) depends on Task 5 (DashMap) and Task 8 (tokio::sync::Mutex)
- All other tasks are independent
