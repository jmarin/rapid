use dotenvy::dotenv;
use rapid::{AppState, Application, metadata::MetadataStore};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod utils;

use crate::utils::constants::prod::{
    APP_ADDRESS, DEFAULT_DB_PATH, DEFAULT_LOG_LEVEL, DEFAULT_MAX_INFLIGHT_PARTS,
    DEFAULT_S3_BUCKET, DEFAULT_S3_ENDPOINT, DEFAULT_S3_REGION, DEFAULT_UPLOAD_DIR,
};

// Temporary main. This will run the Axum web service
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let log_level = env::var("RAPID_LOG_LEVEL").unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string());
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(log_level))
        .init();

    let upload_dir = match env::var("RAPID_UPLOAD_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => env::temp_dir().join(DEFAULT_UPLOAD_DIR),
    };
    tokio::fs::create_dir_all(&upload_dir).await?;

    // S3 configuration
    let s3_endpoint =
        env::var("RAPID_S3_ENDPOINT").unwrap_or_else(|_| DEFAULT_S3_ENDPOINT.to_string());
    let s3_region = env::var("RAPID_S3_REGION").unwrap_or_else(|_| DEFAULT_S3_REGION.to_string());
    let s3_access_key = env::var("RAPID_S3_ACCESS_KEY").expect("RAPID_S3_ACCESS_KEY must be set");
    let s3_secret_key = env::var("RAPID_S3_SECRET_KEY").expect("RAPID_S3_SECRET_KEY must be set");
    let s3_bucket = env::var("RAPID_S3_BUCKET").unwrap_or_else(|_| DEFAULT_S3_BUCKET.to_string());

    let s3_creds =
        aws_sdk_s3::config::Credentials::new(s3_access_key, s3_secret_key, None, None, "rapid-env");

    let s3_config = aws_sdk_s3::Config::builder()
        .region(aws_sdk_s3::config::Region::new(s3_region))
        .endpoint_url(&s3_endpoint)
        .credentials_provider(s3_creds)
        .behavior_version_latest()
        .force_path_style(true)
        .build();

    let s3_client = aws_sdk_s3::Client::from_conf(s3_config);

    // Ensure the bucket exists. If it doesn't, create it
    match s3_client.head_bucket().bucket(&s3_bucket).send().await {
        Ok(_) => info!("S3 bucket '{}' exists", s3_bucket),
        Err(_) => {
            info!("Creating S3 bucket '{}'", s3_bucket);
            s3_client
                .create_bucket()
                .bucket(&s3_bucket)
                .send()
                .await
                .expect("Failed to create S3 bucket");
        }
    }

    let db_url = env::var("RAPID_DB_PATH").unwrap_or_else(|_| DEFAULT_DB_PATH.to_string());
    let metadata = MetadataStore::new(&db_url)
        .await
        .expect("Failed to initialize metadata database");

    let max_inflight_parts: usize = env::var("RAPID_MAX_INFLIGHT_PARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_INFLIGHT_PARTS);

    // Per-upload cap: at most 1/4 of global permits, but no fewer than 4
    let max_parts_per_upload = (max_inflight_parts / 4).max(4);

    let app_state = AppState {
        upload_dir,
        s3_client,
        s3_bucket,
        upload_semaphore: Arc::new(Semaphore::new(max_inflight_parts)),
        max_parts_per_upload,
        upload_progress: Arc::new(RwLock::new(HashMap::new())),
        metadata,
    };

    let app = Application::build(app_state.clone(), APP_ADDRESS)
        .await
        .expect("Failed to start application");

    app.run().await.expect("Failed to run application");

    info!("closing metadata database");
    app_state.metadata.close().await;

    Ok(())
}
