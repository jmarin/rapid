use dotenvy::dotenv;
use rapid::{AppState, Application, magic::mime_type_magic};
use std::{env, path::Path};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod utils;

use utils::*;

use crate::utils::constants::prod::APP_ADDRESS;

// Temporary main. This will run the Axum web service
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let log_level = env::var("RAPID_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(log_level))
        .init();

    //info!("Rapid starting up on port {}", http_port);

    let mime_type = mime_type_magic(Path::new("data/files/NGC 6888.jpg")).await?;
    info!("Detected MIME type: {}", mime_type);

    let app_state = AppState {};

    let app = Application::build(app_state, APP_ADDRESS)
        .await
        .expect("Failed to start application");

    app.run().await.expect("Failed to run application");

    Ok(())
}
