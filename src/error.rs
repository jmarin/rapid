use thiserror::Error;

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
