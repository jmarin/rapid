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

/// Calculate total pages using ceiling division.
pub fn total_pages(total: i64, per_page: i64) -> i64 {
    if per_page <= 0 {
        return 0;
    }
    (total + per_page - 1) / per_page
}

/// Clamp page number to at least 1.
pub fn clamp_page(page: Option<i64>) -> i64 {
    page.unwrap_or(1).max(1)
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
    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM file_metadata WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
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

pub async fn delete_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Look up metadata first to confirm it exists
    let meta = match state.metadata.get_by_id(&id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not found" }))).into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to query metadata for delete");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "internal error" }))).into_response();
        }
    };

    // Delete from S3
    if let Err(e) = state.s3_client
        .delete_object()
        .bucket(&state.s3_bucket)
        .key(&id)
        .send()
        .await
    {
        tracing::error!(error = %e, id = %id, "failed to delete S3 object");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "failed to delete file from storage" }))).into_response();
    }

    // Delete from DB
    match state.metadata.delete(&id).await {
        Ok(_) => {
            tracing::info!(id = %id, file_name = %meta.file_name, "deleted file");
            (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to delete metadata");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "file deleted from storage but metadata removal failed" }))).into_response()
        }
    }
}

pub async fn list_file_metadata(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let page = clamp_page(params.page);
    let per_page = DEFAULT_PER_PAGE;

    match state.metadata.list(page, per_page).await {
        Ok((items, total)) => {
            let tp = total_pages(total, per_page);
            let resp = ListResponse {
                items,
                page,
                per_page,
                total,
                total_pages: tp,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_pages_exact_division() {
        assert_eq!(total_pages(40, 20), 2);
    }

    #[test]
    fn total_pages_with_remainder() {
        assert_eq!(total_pages(41, 20), 3);
    }

    #[test]
    fn total_pages_zero_items() {
        assert_eq!(total_pages(0, 20), 0);
    }

    #[test]
    fn total_pages_one_item() {
        assert_eq!(total_pages(1, 20), 1);
    }

    #[test]
    fn total_pages_per_page_equals_total() {
        assert_eq!(total_pages(20, 20), 1);
    }

    #[test]
    fn total_pages_zero_per_page() {
        assert_eq!(total_pages(10, 0), 0);
    }

    #[test]
    fn clamp_page_none_defaults_to_1() {
        assert_eq!(clamp_page(None), 1);
    }

    #[test]
    fn clamp_page_zero_clamps_to_1() {
        assert_eq!(clamp_page(Some(0)), 1);
    }

    #[test]
    fn clamp_page_negative_clamps_to_1() {
        assert_eq!(clamp_page(Some(-5)), 1);
    }

    #[test]
    fn clamp_page_positive_passes_through() {
        assert_eq!(clamp_page(Some(3)), 3);
    }
}
