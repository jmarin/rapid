pub mod download;
pub mod error;
pub mod image;
pub mod magic;
pub mod metadata;
pub mod upload;
pub mod ws;

#[cfg(test)]
pub mod test_utils;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{Method, StatusCode},
    routing::{delete, get, post},
    serve::Serve,
};
pub use error::*;
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, Semaphore, mpsc};
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, services::ServeDir};

pub struct Application {
    server: Serve<TcpListener, Router, Router>,
    pub address: String,
}

pub struct ErrorResponse {
    pub error: String,
}

use crate::metadata::MetadataStore;

#[derive(Clone)]
pub struct AppState {
    pub upload_dir: PathBuf,
    pub s3_client: aws_sdk_s3::Client,
    pub s3_bucket: String,
    pub upload_semaphore: Arc<Semaphore>,
    pub max_parts_per_upload: usize,
    pub upload_progress: Arc<RwLock<HashMap<String, mpsc::Sender<ws::UploadEvent>>>>,
    pub metadata: MetadataStore,
}

impl Application {
    pub async fn build(app_state: AppState, address: &str) -> Result<Self, Box<dyn Error>> {
        let allowed_origins = ["http://localhost:3000".parse()?];

        let cors = CorsLayer::new()
            .allow_methods([Method::GET, Method::POST])
            .allow_credentials(true)
            .allow_origin(allowed_origins);

        let assets_dir = ServeDir::new("assets");

        let upload_route = Router::new()
            .route("/upload", post(upload::upload_file))
            .layer(DefaultBodyLimit::disable())
            .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024 * 1024)); // Setting a limit of 10GB file size for uploads

        let router = Router::new()
            .fallback_service(assets_dir)
            .route("/health", get(liveness))
            .route("/ready", get(readiness))
            .route("/files/{id}", get(download::download_file))
            .route("/files/{id}/metadata", get(metadata::get_file_metadata))
            .route("/api/metadata", get(metadata::list_file_metadata))
            .route("/api/metadata/{id}", delete(metadata::delete_file))
            .route("/ws/upload-progress", get(ws::ws_upload_progress))
            .merge(upload_route)
            .layer(cors)
            .with_state(app_state);

        let listener = tokio::net::TcpListener::bind(address).await?;
        let address = listener.local_addr()?.to_string();
        let server = axum::serve(listener, router);

        // Create a new Application instance and return it
        Ok(Self { server, address })
    }

    pub async fn run(self) -> Result<(), std::io::Error> {
        tracing::info!("listening on {}", &self.address);
        self.server
            .with_graceful_shutdown(async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("failed to listen for Ctrl-C");
                tracing::info!("shutdown signal received");
            })
            .await
    }
}

/// Liveness probe handler.
/// Returns 200 OK if the process is alive.
async fn liveness() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe handler.
/// Returns 200 OK when the controller is ready to handle requests.
/// For now, we're always ready once the server starts.
/// This could be extended to check controller state if needed.
async fn readiness() -> StatusCode {
    StatusCode::OK
}
