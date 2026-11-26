//! Staged files (issue #48): with JavaScript, Add test, **+ Add files** and
//! **Replace original** upload each file in its own request, so no request
//! goes over Cloudflare's 100 MB, and then post the form with references
//! to the staged files instead of their bytes (`crate::app::staging`).
//!
//! - [`stage`] (`POST /api/admin/uploads/stage`, Admin-only, CSRF-checked)
//!   applies the per-file rules (type, 90 MB) and streams the file into
//!   Garage at `staged/{id}`, recording it in `staged_files`. It answers
//!   JSON: `{"staged": id, …}`, or `{"error": message}` with a 4xx.
//! - A form that posts `staged` IDs [`claim`]s each, copies its object to
//!   where a direct upload would have streamed it, and saves exactly as a
//!   direct upload would (`server::add_test`, `server::test_edit`). Once
//!   saved it [`forget`]s them; a refused form [`release`]s them, so the
//!   same form can be posted again.
//! - Staged files never saved are swept after [`STAGED_HOURS`] ([`sweep`]),
//!   on every staging and every Add test post.
//!
//! The row is written before the object and deleted after it, so no staged
//! object is ever left without a row for the sweep to find.

use axum::{
    Json,
    extract::multipart::Field,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use dioxus::{fullstack::MultipartFormData, prelude::ServerFnError};
use serde_json::json;
use sqlx::{FromRow, PgPool};

use super::{
    AppState, add_test, auth,
    storage::{FileRole, PutError},
    upload,
};
use crate::app::upload_rules::{self, MAX_FILE_BYTES, POST_SIZE_REFUSAL, size_refusal, split_name};

/// A staged file not saved within this long is deleted.
pub const STAGED_HOURS: i32 = 24;

/// A claim held longer than this (by a request that died) no longer stops
/// the sweep.
const CLAIM_MINUTES: i32 = 60;

/// A staged file, claimed by the form saving it.
#[derive(Clone, Debug, FromRow)]
pub struct StagedFile {
    pub id: String,
    pub name: String,
    pub extension: String,
    pub content_type: String,
    pub byte_size: i64,
    pub sha256: Vec<u8>,
}

impl StagedFile {
    /// Copy the staged object to `role` of Document `document`.
    pub async fn copy_to(&self, state: &AppState, document: i64, role: FileRole) -> anyhow::Result<()> {
        let storage = &state.storage;
        storage
            .copy(&storage.staged_key(&self.id), &storage.document_key(document, role))
            .await
    }
}

/// A form's message for a staged reference that can't be used.
pub const GONE: &str = "A file uploaded for this form was already saved, or waited more than a day. \
                        Choose the files again.";

// -------------------------------------------------------------- staging

/// The JSON answer to a staging request that was refused.
fn refused(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// `POST /api/admin/uploads/stage`: `csrf` (first), then one `file`.
pub async fn stage(
    state: &AppState,
    headers: &HeaderMap,
    mut form: MultipartFormData,
) -> Result<Response, ServerFnError> {
    // Anonymous callers are refused before any of the body is used.
    auth::require_admin(state, headers).await?;
    let csrf = match form.next_field().await {
        Ok(Some(field)) if field.name() == Some("csrf") => field.text().await.unwrap_or_default(),
        _ => String::new(),
    };
    auth::require_admin_form(state, headers, &csrf).await?;
    sweep(state).await;

    // The rest of the body is read before a refusal goes out (`server::upload`).
    loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => return Ok(refused(StatusCode::BAD_REQUEST, "Choose a file to upload.")),
            Err(e) if upload::over_limit(&e) => return Ok(refused(StatusCode::PAYLOAD_TOO_LARGE, POST_SIZE_REFUSAL)),
            Err(_) => return Ok(refused(StatusCode::BAD_REQUEST, "The upload was cut off. Try again.")),
        };
        match (field.name(), field.file_name()) {
            (Some("file"), Some(name)) if !name.is_empty() => {
                let name = name.to_owned();
                return receive(state, name, field).await;
            }
            (Some("file"), _) => return Ok(refused(StatusCode::BAD_REQUEST, "Choose a file to upload.")),
            _ => {}
        }
    }
}

async fn receive(state: &AppState, name: String, field: Field<'_>) -> Result<Response, ServerFnError> {
    if let Some(message) = upload_rules::refusal(&name, None) {
        return Ok(refused(StatusCode::UNSUPPORTED_MEDIA_TYPE, &message));
    }
    let (_, extension) = split_name(&name);
    let content_type = add_test::content_type(&extension);
    let id = new_id();
    sqlx::query("INSERT INTO staged_files (id, name, extension, content_type) VALUES ($1, $2, $3, $4)")
        .bind(&id)
        .bind(&name)
        .bind(&extension)
        .bind(content_type)
        .execute(&state.db)
        .await
        .map_err(super::internal_error)?;

    let key = state.storage.staged_key(&id);
    let stored = match state
        .storage
        .put_stream(&key, content_type, field, MAX_FILE_BYTES)
        .await
    {
        Ok(stored) => stored,
        Err(e) => {
            // A refused or failed put stores nothing.
            delete_row(&state.db, &id).await;
            return match e {
                PutError::TooLarge => Ok(refused(StatusCode::PAYLOAD_TOO_LARGE, &size_refusal(&name))),
                PutError::Body(e) if upload::over_limit(&e) => {
                    Ok(refused(StatusCode::PAYLOAD_TOO_LARGE, &size_refusal(&name)))
                }
                PutError::Body(_) => Ok(refused(StatusCode::BAD_REQUEST, "The upload was cut off. Try again.")),
                PutError::Storage(e) => Err(super::internal_error(e)),
            };
        }
    };
    sqlx::query("UPDATE staged_files SET byte_size = $2, sha256 = $3 WHERE id = $1")
        .bind(&id)
        .bind(stored.byte_size)
        .bind(stored.sha256.to_vec())
        .execute(&state.db)
        .await
        .map_err(super::internal_error)?;
    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "staged": id, "name": name, "byte_size": stored.byte_size })),
    )
        .into_response())
}

/// 32 random hex digits.
fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    hex::encode(bytes)
}

// ------------------------------------------------------------- claiming

/// Take a staged file for the form being saved: `None` if there is no such
/// file, or it is still arriving, already taken, or too old.
pub async fn claim(db: &PgPool, id: &str) -> Result<Option<StagedFile>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE staged_files SET claimed_at = now()
         WHERE id = $1 AND claimed_at IS NULL AND byte_size IS NOT NULL
           AND created_at > now() - make_interval(hours => $2)
         RETURNING id, name, extension, content_type, byte_size, sha256",
    )
    .bind(id.trim())
    .bind(STAGED_HOURS)
    .fetch_optional(db)
    .await
}

/// Give back files claimed by a form that was refused, so it can be posted
/// again. Failures are only logged; the sweep gets them in the end.
pub async fn release(state: &AppState, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    if let Err(e) = sqlx::query("UPDATE staged_files SET claimed_at = NULL WHERE id = ANY($1)")
        .bind(ids)
        .execute(&state.db)
        .await
    {
        dioxus::logger::tracing::warn!("releasing staged files: {e}");
    }
}

/// Delete staged files whose form was saved: each object, then its row.
pub async fn forget(state: &AppState, ids: &[String]) {
    for id in ids {
        match state.storage.delete(&state.storage.staged_key(id)).await {
            Ok(()) => delete_row(&state.db, id).await,
            Err(e) => dioxus::logger::tracing::warn!("{e:#}"),
        }
    }
}

async fn delete_row(db: &PgPool, id: &str) {
    if let Err(e) = sqlx::query("DELETE FROM staged_files WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
    {
        dioxus::logger::tracing::warn!("deleting staged file {id}: {e}");
    }
}

/// Delete the staged files nobody saved within [`STAGED_HOURS`], unless a
/// form is saving one right now. Failures are only logged; the next sweep
/// tries again.
pub async fn sweep(state: &AppState) {
    let old: Result<Vec<String>, _> = sqlx::query_scalar(
        "SELECT id FROM staged_files
         WHERE created_at < now() - make_interval(hours => $1)
           AND (claimed_at IS NULL OR claimed_at < now() - make_interval(mins => $2))",
    )
    .bind(STAGED_HOURS)
    .bind(CLAIM_MINUTES)
    .fetch_all(&state.db)
    .await;
    match old {
        Ok(old) => forget(state, &old).await,
        Err(e) => dioxus::logger::tracing::warn!("sweeping staged files: {e}"),
    }
}
