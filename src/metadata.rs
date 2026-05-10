use axum::{Json, extract::{Path, Query, State}, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use crate::AppState;

const DEFAULT_PER_PAGE: i64 = 20;

#[derive(Clone)]
pub struct MetadataStore {
    pool: SqlitePool,
}

#[derive(Serialize, FromRow)]
pub struct FileMetadata {
    pub id: String,
    pub file_name: String,
    pub size_bytes: i64,
    pub mime_type: String,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct ListParams {
    pub page: Option<i64>,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub items: Vec<FileMetadata>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
    pub total_pages: i64,
}

impl MetadataStore {
    pub async fn new(db_url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePool::connect(db_url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn insert(
        &self,
        id: &str,
        file_name: &str,
        size_bytes: i64,
        mime_type: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO file_metadata (id, file_name, size_bytes, mime_type) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(file_name)
        .bind(size_bytes)
        .bind(mime_type)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<FileMetadata>, sqlx::Error> {
        sqlx::query_as::<_, FileMetadata>(
            "SELECT id, file_name, size_bytes, mime_type, created_at FROM file_metadata WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list(
        &self,
        page: i64,
        per_page: i64,
    ) -> Result<(Vec<FileMetadata>, i64), sqlx::Error> {
        let offset = (page - 1) * per_page;

        let items = sqlx::query_as::<_, FileMetadata>(
            "SELECT id, file_name, size_bytes, mime_type, created_at FROM file_metadata ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(per_page)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM file_metadata")
            .fetch_one(&self.pool)
            .await?;

        Ok((items, total.0))
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

pub async fn get_file_metadata(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.metadata.get_by_id(&id).await {
        Ok(Some(meta)) => (StatusCode::OK, Json(serde_json::json!(meta))).into_response(),
        Ok(None) => {
            let body = serde_json::json!({ "error": "not found" });
            (StatusCode::NOT_FOUND, Json(body)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to query file metadata");
            let body = serde_json::json!({ "error": "internal error" });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}

pub async fn list_file_metadata(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = DEFAULT_PER_PAGE;

    match state.metadata.list(page, per_page).await {
        Ok((items, total)) => {
            let total_pages = (total + per_page - 1) / per_page;
            let resp = ListResponse {
                items,
                page,
                per_page,
                total,
                total_pages,
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
