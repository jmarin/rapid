pub mod error;
pub mod image;
pub mod magic;

#[cfg(test)]
pub mod test_utils;

use axum::{
    Router,
    http::{Method, StatusCode},
    routing::get,
    serve::Serve,
};
pub use error::*;
use std::error::Error;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, services::ServeDir};

pub struct Application {
    server: Serve<TcpListener, Router, Router>,
    pub address: String,
}

pub struct ErrorResponse {
    pub error: String,
}

#[derive(Clone)]
pub struct AppState {}

impl Application {
    pub async fn build(app_state: AppState, address: &str) -> Result<Self, Box<dyn Error>> {
        let allowed_origins = ["http:://localhost:3000".parse()?];

        let cors = CorsLayer::new()
            .allow_methods([Method::GET, Method::POST])
            .allow_credentials(true)
            .allow_origin(allowed_origins);

        let assets_dir = ServeDir::new("assets");

        let router = Router::new()
            .fallback_service(assets_dir)
            .route("/health", get(liveness))
            .route("/ready", get(readiness))
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
        self.server.await
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
