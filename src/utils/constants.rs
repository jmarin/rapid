pub mod prod {
    pub const APP_ADDRESS: &str = "0.0.0.0:8080";
    pub const DEFAULT_LOG_LEVEL: &str = "info";
    pub const DEFAULT_UPLOAD_DIR: &str = "uploads";
    pub const DEFAULT_S3_ENDPOINT: &str = "http://localhost:9000";
    pub const DEFAULT_S3_BUCKET: &str = "rapid-uploads";
    pub const DEFAULT_S3_REGION: &str = "us-east-1";
}
