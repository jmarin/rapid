use axum::{Json, extract::{Path, Query, State}, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, sqlite::SqlitePoolOptions};

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
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Serialize, FromRow)]
pub struct Derivative {
    pub id: String,
    pub parent_id: String,
    pub size_label: String,
    pub s3_key: String,
    pub width: i64,
    pub height: i64,
    pub status: String,
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
    pub has_next: bool,
}

/// Clamp page number to at least 1.
pub fn clamp_page(page: Option<i64>) -> i64 {
    page.unwrap_or(1).max(1)
}

impl MetadataStore {
    pub async fn new(db_url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("PRAGMA journal_mode=WAL")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("PRAGMA busy_timeout=5000")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("PRAGMA synchronous=NORMAL")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(db_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn insert(
        &self,
        id: &str,
        file_name: &str,
        size_bytes: i64,
        mime_type: &str,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO file_metadata (id, file_name, size_bytes, mime_type, width, height) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(file_name)
        .bind(size_bytes)
        .bind(mime_type)
        .bind(width.map(|v| v as i64))
        .bind(height.map(|v| v as i64))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<FileMetadata>, sqlx::Error> {
        sqlx::query_as::<_, FileMetadata>(
            "SELECT id, file_name, size_bytes, mime_type, created_at, width, height FROM file_metadata WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list(
        &self,
        page: i64,
        per_page: i64,
    ) -> Result<(Vec<FileMetadata>, bool), sqlx::Error> {
        let offset = (page - 1) * per_page;
        let fetch_limit = per_page + 1; // Fetch one extra to detect next page

        let mut items = sqlx::query_as::<_, FileMetadata>(
            "SELECT id, file_name, size_bytes, mime_type, created_at, width, height FROM file_metadata ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(fetch_limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let has_next = items.len() as i64 > per_page;
        if has_next {
            items.pop(); // Remove the extra probe row
        }

        Ok((items, has_next))
    }
    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM file_metadata WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn insert_derivative(
        &self,
        id: &str,
        parent_id: &str,
        size_label: &str,
        s3_key: &str,
        width: i64,
        height: i64,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO derivatives (id, parent_id, size_label, s3_key, width, height, status) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(parent_id)
        .bind(size_label)
        .bind(s3_key)
        .bind(width)
        .bind(height)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_derivative_status(
        &self,
        id: &str,
        status: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("UPDATE derivatives SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_derivative_dimensions(
        &self,
        id: &str,
        width: i64,
        height: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("UPDATE derivatives SET width = ?, height = ? WHERE id = ?")
            .bind(width)
            .bind(height)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_derivatives_by_parent(
        &self,
        parent_id: &str,
    ) -> Result<Vec<Derivative>, sqlx::Error> {
        sqlx::query_as::<_, Derivative>(
            "SELECT id, parent_id, size_label, s3_key, width, height, status, created_at FROM derivatives WHERE parent_id = ? ORDER BY width ASC",
        )
        .bind(parent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_derivative_by_parent_and_size(
        &self,
        parent_id: &str,
        size_label: &str,
    ) -> Result<Option<Derivative>, sqlx::Error> {
        sqlx::query_as::<_, Derivative>(
            "SELECT id, parent_id, size_label, s3_key, width, height, status, created_at FROM derivatives WHERE parent_id = ? AND size_label = ?",
        )
        .bind(parent_id)
        .bind(size_label)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete_derivatives_by_parent(
        &self,
        parent_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        let keys: Vec<(String,)> = sqlx::query_as(
            "SELECT s3_key FROM derivatives WHERE parent_id = ?",
        )
        .bind(parent_id)
        .fetch_all(&self.pool)
        .await?;

        sqlx::query("DELETE FROM derivatives WHERE parent_id = ?")
            .bind(parent_id)
            .execute(&self.pool)
            .await?;

        Ok(keys.into_iter().map(|(k,)| k).collect())
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

    // Delete original from S3
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

    // Delete derivative S3 objects
    match state.metadata.delete_derivatives_by_parent(&id).await {
        Ok(s3_keys) => {
            for key in &s3_keys {
                if let Err(e) = state.s3_client
                    .delete_object()
                    .bucket(&state.s3_bucket)
                    .key(key)
                    .send()
                    .await
                {
                    tracing::warn!(error = %e, key = %key, "failed to delete derivative from S3");
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, id = %id, "failed to delete derivative metadata");
        }
    }

    // Delete parent from DB (CASCADE will also remove derivative rows)
    match state.metadata.delete(&id).await {
        Ok(_) => {
            tracing::info!(id = %id, file_name = %meta.file_name, "deleted file and derivatives");
            (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to delete metadata");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "file deleted from storage but metadata removal failed" }))).into_response()
        }
    }
}

pub async fn list_derivatives(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.metadata.get_derivatives_by_parent(&id).await {
        Ok(derivatives) => {
            (StatusCode::OK, Json(serde_json::json!({ "parent_id": id, "derivatives": derivatives }))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to list derivatives");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "internal error" }))).into_response()
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
        Ok((items, has_next)) => {
            let mut enriched_items = Vec::with_capacity(items.len());
            for item in &items {
                let (derivatives_status, derivatives_count) = if item.mime_type.starts_with("image/") {
                    match state.metadata.get_derivatives_by_parent(&item.id).await {
                        Ok(derivs) if derivs.is_empty() => ("processing".to_string(), 0usize),
                        Ok(derivs) => {
                            let completed = derivs.iter().filter(|d| d.status == "completed").count();
                            if completed == derivs.len() {
                                ("ready".to_string(), completed)
                            } else {
                                ("processing".to_string(), completed)
                            }
                        }
                        Err(_) => ("unknown".to_string(), 0usize),
                    }
                } else {
                    ("none".to_string(), 0usize)
                };

                let mut val = serde_json::to_value(item).unwrap();
                val.as_object_mut().unwrap().insert("derivatives_status".to_string(), serde_json::json!(derivatives_status));
                val.as_object_mut().unwrap().insert("derivatives_count".to_string(), serde_json::json!(derivatives_count));
                enriched_items.push(val);
            }

            let resp = serde_json::json!({
                "items": enriched_items,
                "page": page,
                "per_page": per_page,
                "has_next": has_next,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to list file metadata");
            let body = serde_json::json!({ "error": "internal error" });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}

pub async fn get_file_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let meta = match state.metadata.get_by_id(&id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not found" }))).into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to get file detail");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "internal error" }))).into_response();
        }
    };

    let derivatives = match state.metadata.get_derivatives_by_parent(&id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "failed to get derivatives for detail");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "internal error" }))).into_response();
        }
    };

    let body = serde_json::json!({
        "file": meta,
        "derivatives": derivatives,
    });
    (StatusCode::OK, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

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
