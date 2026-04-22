use std::cell::RefCell;
use std::path::{Path, PathBuf};

use magic::cookie::Load;
use magic::{Cookie, cookie::DatabasePaths};

use crate::MimeTypeError;

const CUSTOM_MAGIC_DATABASE_DIR: &str = env!("CUSTOM_MAGIC_DATABASE_DIR");

thread_local! {
    // Each blocking worker thread keeps its own default libmagic cookie.
    // RefCell allows lazy initialization and reuse within that thread.
    static DEFAULT_COOKIE: RefCell<Option<Cookie<Load>>> = const { RefCell::new(None) };

    // Separate per-thread cookie for the custom magic databases used as a fallback.
    static CUSTOM_COOKIE: RefCell<Option<Cookie<Load>>> = const { RefCell::new(None) };
}

// Open a libmagic cookie configured to return MIME types and load the given databases.
fn load_cookie(database: &DatabasePaths) -> Result<Cookie<Load>, MimeTypeError> {
    let flags = magic::cookie::Flags::MIME_TYPE;

    let cookie = Cookie::open(flags).map_err(|e| MimeTypeError::MagicError(e.to_string()))?;
    cookie
        .load(database)
        .map_err(|e| MimeTypeError::MagicError(e.to_string()))
}

// Lazily initialize and reuse the default cookie for the current thread.
fn with_default_cookie<F, T>(f: F) -> Result<T, MimeTypeError>
where
    F: FnOnce(&Cookie<Load>) -> Result<T, MimeTypeError>,
{
    DEFAULT_COOKIE.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(load_cookie(&DatabasePaths::default())?);
        }
        f(slot.as_ref().unwrap())
    })
}

// Lazily initialize and reuse the custom cookie for the current thread.
fn with_custom_cookie<F, T>(f: F) -> Result<T, MimeTypeError>
where
    F: FnOnce(&Cookie<Load>) -> Result<T, MimeTypeError>,
{
    CUSTOM_COOKIE.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(load_cookie(&custom_magic_databases()?)?);
        }
        f(slot.as_ref().unwrap())
    })
}

// Collect all custom compiled magic database files from the configured directory.
fn custom_magic_databases() -> Result<DatabasePaths, MimeTypeError> {
    let custom_database_dir = PathBuf::from(CUSTOM_MAGIC_DATABASE_DIR);

    let custom_databases = std::fs::read_dir(&custom_database_dir)
        .map_err(|e| MimeTypeError::MagicError(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| MimeTypeError::MagicError(e.to_string()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mgc"))
        .collect::<Vec<_>>();

    DatabasePaths::try_from(custom_databases).map_err(|e| MimeTypeError::MagicError(e.to_string()))
}

// Detect the MIME type with the default database first, then fall back to the custom one
// if libmagic only returns the generic application/octet-stream result.
fn mime_type_magic_blocking(path: &Path) -> Result<String, MimeTypeError> {
    let metadata =
        std::fs::metadata(path).map_err(|e| MimeTypeError::MetadataError(e.to_string()))?;
    if metadata.len() == 0 {
        return Err(MimeTypeError::ZeroByteFileError);
    }

    let mime_type = with_default_cookie(|cookie| {
        cookie
            .file(path)
            .map_err(|e| MimeTypeError::MagicError(e.to_string()))
    })?;

    if mime_type != "application/octet-stream" {
        return Ok(mime_type);
    }

    with_custom_cookie(|cookie| {
        cookie
            .file(path)
            .map_err(|e| MimeTypeError::MagicError(e.to_string()))
    })
}

/// Detects the MIME type of a file using libmagic with automatic fallback.
///
/// This function performs asynchronous MIME type detection by running the blocking
/// libmagic operations on Tokio's blocking thread pool. It first attempts detection
/// using the default libmagic database, then falls back to a custom database if the
/// default returns the generic `application/octet-stream` result.
///
/// # Arguments
///
/// * `path` - A reference to the file path to analyze
///
/// # Returns
///
/// * `Ok(String)` - The detected MIME type (e.g., `"image/jpeg"`)
/// * `Err(MimeTypeError::ZeroByteFileError)` - If the file is empty
/// * `Err(MimeTypeError::MetadataError(_))` - If file metadata cannot be read
/// * `Err(MimeTypeError::MagicError(_))` - If libmagic operations fail
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// use rapid::magic::mime_type_magic;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let mime_type = mime_type_magic(Path::new("data/files/NGC 6888.jpg")).await?;
///     println!("Detected MIME type: {}", mime_type);
///     Ok(())
/// }
/// ```
///
/// # Fallback Behavior
///
/// If the default database returns `application/octet-stream`, the function
/// automatically attempts detection using custom magic databases from the directory
/// specified by the `CUSTOM_MAGIC_DATABASE_DIR` environment variable.
///
pub async fn mime_type_magic(path: &Path) -> Result<String, MimeTypeError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || mime_type_magic_blocking(&path))
        .await
        .map_err(|e| MimeTypeError::MagicError(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::assert_all_files_with_extension_have_mime_type;

    use std::path::Path;
    use std::sync::Once;

    async fn detect_by_path(path: String) -> Result<String, MimeTypeError> {
        mime_type_magic(Path::new(&path)).await
    }

    #[tokio::test]
    async fn mime_type_jpg_files() {
        assert_all_files_with_extension_have_mime_type(
            "data/files",
            "jpg",
            "image/jpeg",
            &[],
            detect_by_path,
        )
        .await;
    }

    #[tokio::test]
    async fn mime_type_fujifilm_x_raw_files() {
        assert_all_files_with_extension_have_mime_type(
            "data/files/raw",
            "raf",
            "image/x-fuji-raf",
            &[],
            detect_by_path,
        )
        .await;
    }

    #[tokio::test]
    async fn mime_type_indd_files() {
        assert_all_files_with_extension_have_mime_type(
            "data/files",
            "indd",
            "application/x-indesign",
            &[],
            detect_by_path,
        )
        .await;
    }
}
