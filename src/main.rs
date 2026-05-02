use dotenvy::dotenv;
use rapid::{AppState, Application, magic::mime_type_magic};
use std::path::PathBuf;
use std::{env, path::Path};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod utils;

use utils::*;

use crate::utils::constants::prod::{APP_ADDRESS, DEFAULT_LOG_LEVEL, DEFAULT_UPLOAD_DIR};

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

    let app_state = AppState { upload_dir };

    let app = Application::build(app_state, APP_ADDRESS)
        .await
        .expect("Failed to start application");

    app.run().await.expect("Failed to run application");

    Ok(())
}
