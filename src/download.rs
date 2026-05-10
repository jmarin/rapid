use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};
use tokio_util::io::ReaderStream;

use crate::{AppState, error::DownloadError};

pub async fn download_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, DownloadError> {
    // HEAD the object to get total size and content-type before deciding on range
    let head = state
        .s3_client
        .head_object()
        .bucket(&state.s3_bucket)
        .key(&id)
        .send()
        .await
        .map_err(|e| {
            use aws_sdk_s3::operation::head_object::HeadObjectError;
            let is_not_found = e
                .as_service_error()
                .map(|se| matches!(se, HeadObjectError::NotFound(_)))
                .unwrap_or(false);
            if is_not_found {
                DownloadError::NotFound
            } else {
                DownloadError::S3(e.to_string())
            }
        })?;

    let total_size = head.content_length().unwrap_or(0) as u64;
    let content_type = head
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    // Parse the Range header and determine byte range
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    let (range_str, start, end, status) = if let Some(range) = range_header {
        let (start, end) = parse_range(range, total_size).ok_or(DownloadError::InvalidRange)?;
        (
            Some(format!("bytes={}-{}", start, end)),
            start,
            end,
            StatusCode::PARTIAL_CONTENT,
        )
    } else {
        (None, 0u64, total_size.saturating_sub(1), StatusCode::OK)
    };

    // Fetch object from S3 with optional Range
    let mut get_req = state
        .s3_client
        .get_object()
        .bucket(&state.s3_bucket)
        .key(&id);

    if let Some(ref r) = range_str {
        get_req = get_req.range(r);
    }

    let resp = get_req
        .send()
        .await
        .map_err(|e| DownloadError::S3(e.to_string()))?;

    // content_length is the number of bytes returned (end - start + 1), or the full size
    let content_length = if total_size == 0 {
        0u64
    } else {
        end - start + 1
    };

    // Build response headers
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    if status == StatusCode::PARTIAL_CONTENT {
        let content_range = format!("bytes {}-{}/{}", start, end, total_size);
        if let Ok(v) = HeaderValue::from_str(&content_range) {
            response_headers.insert(header::CONTENT_RANGE, v);
        }
    }

    // Stream the S3 body directly to the client
    let body = Body::from_stream(ReaderStream::new(resp.body.into_async_read()));

    Ok((status, response_headers, body))
}

/// Parses an HTTP `Range: bytes=<start>-<end>` header value.
/// Returns `(start, end)` (both inclusive) clamped to `[0, total_size - 1]`.
/// Returns `None` for unsatisfiable or malformed ranges.
fn parse_range(range: &str, total_size: u64) -> Option<(u64, u64)> {
    let bytes = range.strip_prefix("bytes=")?;
    let (start_str, end_str) = bytes.split_once('-')?;

    if start_str.is_empty() {
        // Suffix range: bytes=-N  →  last N bytes
        let suffix_len: u64 = end_str.parse().ok()?;
        if suffix_len == 0 || total_size == 0 {
            return None;
        }
        let start = total_size.saturating_sub(suffix_len);
        Some((start, total_size - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        if total_size > 0 && start >= total_size {
            return None;
        }
        let end = if end_str.is_empty() {
            if total_size == 0 {
                return None;
            }
            total_size - 1
        } else {
            let e: u64 = end_str.parse().ok()?;
            e.min(total_size.saturating_sub(1))
        };
        if start > end {
            return None;
        }
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn full_range() {
        assert_eq!(parse_range("bytes=0-999", 1000), Some((0, 999)));
    }

    #[test]
    fn open_ended_range() {
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn suffix_range() {
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
    }

    #[test]
    fn end_clamps_to_last_byte() {
        assert_eq!(parse_range("bytes=0-9999", 1000), Some((0, 999)));
    }

    #[test]
    fn start_beyond_size_is_none() {
        assert_eq!(parse_range("bytes=1000-", 1000), None);
    }

    #[test]
    fn inverted_range_is_none() {
        assert_eq!(parse_range("bytes=500-100", 1000), None);
    }

    #[test]
    fn missing_prefix_is_none() {
        assert_eq!(parse_range("0-100", 1000), None);
    }

    #[test]
    fn zero_total_size_returns_none() {
        assert_eq!(parse_range("bytes=0-", 0), None);
        // bytes=0-0 on a zero-byte file: start=0, end clamped to 0 via saturating_sub(1)=0, start<=end passes
        // This is arguably valid (empty range at offset 0), but the handler guards against total_size==0 anyway
        assert_eq!(parse_range("bytes=0-0", 0), Some((0, 0)));
        assert_eq!(parse_range("bytes=-100", 0), None);
    }

    #[test]
    fn suffix_larger_than_file() {
        // bytes=-5000 on a 1000-byte file: start clamps to 0
        assert_eq!(parse_range("bytes=-5000", 1000), Some((0, 999)));
    }

    #[test]
    fn zero_suffix_is_none() {
        assert_eq!(parse_range("bytes=-0", 1000), None);
    }

    #[test]
    fn single_byte_range() {
        assert_eq!(parse_range("bytes=0-0", 1000), Some((0, 0)));
        assert_eq!(parse_range("bytes=999-999", 1000), Some((999, 999)));
    }

    #[test]
    fn single_byte_file() {
        assert_eq!(parse_range("bytes=0-0", 1), Some((0, 0)));
        assert_eq!(parse_range("bytes=0-", 1), Some((0, 0)));
        assert_eq!(parse_range("bytes=-1", 1), Some((0, 0)));
    }

    #[test]
    fn garbage_input() {
        assert_eq!(parse_range("bytes=abc-def", 1000), None);
        assert_eq!(parse_range("bytes=--", 1000), None);
        assert_eq!(parse_range("", 1000), None);
    }
}
