//! The Admin's PDF actions (the page is `crate::app::pdf`):
//!
//! - **Upload PDF** ([`upload`]): replaces a Document's PDF by hand, for a
//!   failed or bad conversion. Its status becomes ready.
//! - **Retry PDF** ([`retry`]): queues the conversion afresh.
//! - **Retry all failed** ([`retry_failed`]).
//!
//! Each is a CSRF-checked native form post, answered with a 303 to the
//! posted `return_to`, and logged to the change log. A PDF made or uploaded
//! clears a Needs review that was only about a failed conversion.

use axum::{
    body::Bytes,
    extract::multipart::Field,
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use dioxus::{fullstack::MultipartFormData, prelude::ServerFnError};
use futures::StreamExt;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::{Review, enqueue};
use crate::{
    app::{
        pdf::{PDF_PROBLEMS, PdfDocument, PdfProblemsPage, RetryFailedForm, RetryPdfForm},
        upload_rules::{MAX_FILE_BYTES, size_refusal},
    },
    server::{
        AppState, auth,
        change_log::{self, Change, Entity},
        documents::escape_html,
        internal_error,
        storage::{FileRole, PutError},
    },
};

/// The Upload PDF post: one file of at most 100 MB, plus its fields.
pub const MAX_BODY_BYTES: usize = MAX_FILE_BYTES as usize + 1024 * 1024;

/// How many of the Documents being prepared the page lists.
const PREPARING_LISTED: i64 = 50;

// -------------------------------------------------------------------- page

#[derive(FromRow)]
struct Row {
    id: i64,
    tests: Vec<i64>,
    kind: String,
    label: Option<String>,
    original_extension: String,
    pdf_status: String,
    comment: Option<String>,
    attempts: Option<i16>,
    converting: Option<bool>,
}

impl Row {
    fn into_document(self) -> PdfDocument {
        let job = match (self.attempts, self.converting) {
            (None, _) => None,
            (Some(n), Some(true)) => Some(format!("Converting (attempt {n} of {})", super::MAX_ATTEMPTS)),
            (Some(0), _) => Some("Queued".to_owned()),
            (Some(_), _) => Some("Queued for its retry".to_owned()),
        };
        PdfDocument {
            id: self.id,
            tests: self.tests,
            kind: self.kind,
            label: self.label,
            original_extension: self.original_extension,
            pdf_status: self.pdf_status,
            comment: self.comment,
            job,
        }
    }
}

pub async fn page(state: &AppState, headers: &HeaderMap) -> Result<PdfProblemsPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    let failed = documents(&state.db, "failed", None).await.map_err(internal_error)?;
    let preparing = documents(&state.db, "preparing", Some(PREPARING_LISTED))
        .await
        .map_err(internal_error)?;
    let preparing_total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM documents WHERE pdf_status = 'preparing' AND trashed_at IS NULL")
            .fetch_one(&state.db)
            .await
            .map_err(internal_error)?;
    Ok(PdfProblemsPage {
        csrf,
        failed,
        preparing,
        preparing_total,
    })
}

async fn documents(db: &PgPool, status: &str, limit: Option<i64>) -> Result<Vec<PdfDocument>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT d.id, k.name AS kind, d.label, d.pdf_status::text, d.needs_review_comment AS comment,
                coalesce(o.extension, '') AS original_extension,
                coalesce((SELECT array_agg(td.test_id ORDER BY td.test_id)
                          FROM test_documents td WHERE td.document_id = d.id), '{}') AS tests,
                j.attempts, j.leased_until > now() AS converting
         FROM documents d
         JOIN document_kinds k ON k.id = d.kind_id
         LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
         LEFT JOIN pdf_jobs j ON j.document_id = d.id
         WHERE d.pdf_status = $1::pdf_status AND d.trashed_at IS NULL
         ORDER BY d.id
         LIMIT $2",
    )
    .bind(status)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Row::into_document).collect())
}

// ------------------------------------------------------------- Retry PDF

/// `POST /api/admin/documents/retry-pdf`.
pub async fn retry(state: &AppState, headers: &HeaderMap, form: RetryPdfForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let back = return_to(&form.return_to);
    let Ok(id) = form.document.trim().parse::<i64>() else {
        return Ok(refusal(StatusCode::BAD_REQUEST, "That Document does not exist.", &back));
    };
    let mut tx = state.db.begin().await.map_err(internal_error)?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT pdf_status::text FROM documents WHERE id = $1 AND trashed_at IS NULL FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_error)?;
    match status.as_deref() {
        None => return Ok(refusal(StatusCode::NOT_FOUND, &format!("D{id} does not exist."), &back)),
        Some("not_convertible") => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                &format!("D{id} is not convertible: a PDF can't be made from a ZIP. Upload one instead."),
                &back,
            ));
        }
        Some(before) => requeue(&mut tx, id, before).await.map_err(internal_error)?,
    }
    tx.commit().await.map_err(internal_error)?;
    state.pdf.wake();
    Ok(auth::see_other(&back, None))
}

/// Set a Document preparing, queue it afresh, and log it.
async fn requeue(tx: &mut Transaction<'_, Postgres>, id: i64, before: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE documents SET pdf_status = 'preparing', updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    enqueue(tx, id).await?;
    change_log::record(
        tx,
        Entity::Document,
        id,
        [Change::new("pdf_status", before, "preparing")],
    )
    .await
}

// ------------------------------------------------------ Retry all failed

/// `POST /api/admin/pdfs/retry-failed`.
pub async fn retry_failed(
    state: &AppState,
    headers: &HeaderMap,
    form: RetryFailedForm,
) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let back = return_to(&form.return_to);
    let mut tx = state.db.begin().await.map_err(internal_error)?;
    let failed: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM documents WHERE pdf_status = 'failed' AND trashed_at IS NULL ORDER BY id FOR UPDATE",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(internal_error)?;
    for id in failed {
        requeue(&mut tx, id, "failed").await.map_err(internal_error)?;
    }
    tx.commit().await.map_err(internal_error)?;
    state.pdf.wake();
    Ok(auth::see_other(&back, None))
}

// ------------------------------------------------------------- Upload PDF

/// `POST /api/admin/documents/upload-pdf`. The `csrf` field must come first,
/// and `document` before the file, as the form puts them.
pub async fn upload(
    state: &AppState,
    headers: &HeaderMap,
    mut form: MultipartFormData,
) -> Result<Response, ServerFnError> {
    // Anonymous callers are refused before any of the body is read.
    auth::require_admin(state, headers).await?;
    let csrf = match form.next_field().await {
        Ok(Some(field)) if field.name() == Some("csrf") => field.text().await.unwrap_or_default(),
        _ => String::new(),
    };
    auth::require_admin_form(state, headers, &csrf).await?;

    let (mut document, mut back) = (String::new(), PDF_PROBLEMS.to_owned());
    let response = loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break refusal(StatusCode::BAD_REQUEST, "Choose a PDF to upload.", &back),
            Err(_) => break refusal(StatusCode::BAD_REQUEST, "The upload was cut off. Try again.", &back),
        };
        match (field.name().unwrap_or_default(), field.file_name()) {
            ("pdf", Some(name)) if !name.is_empty() => {
                let name = name.to_owned();
                break receive(state, &document, &back, name, field).await?;
            }
            ("pdf", _) => break refusal(StatusCode::BAD_REQUEST, "Choose a PDF to upload.", &back),
            ("document", None) => document = field.text().await.unwrap_or_default(),
            ("return_to", None) => back = return_to(&field.text().await.unwrap_or_default()),
            _ => {}
        }
    };
    // Read the rest, so the browser is still listening when a refusal arrives.
    while let Ok(Some(mut field)) = form.next_field().await {
        while let Ok(Some(_)) = field.chunk().await {}
    }
    Ok(response)
}

#[derive(FromRow)]
struct Current {
    pdf_status: String,
    needs_review: bool,
    needs_review_comment: Option<String>,
}

async fn receive(
    state: &AppState,
    document: &str,
    back: &str,
    name: String,
    mut field: Field<'_>,
) -> Result<Response, ServerFnError> {
    let Ok(id) = document.trim().parse::<i64>() else {
        return Ok(refusal(StatusCode::BAD_REQUEST, "That Document does not exist.", back));
    };
    let exists: Option<bool> = sqlx::query_scalar("SELECT true FROM documents WHERE id = $1 AND trashed_at IS NULL")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal_error)?;
    if exists.is_none() {
        return Ok(refusal(StatusCode::NOT_FOUND, &format!("D{id} does not exist."), back));
    }

    // It must be a PDF: check its first bytes before storing any of it.
    let mut head = Vec::new();
    while head.len() < 5 {
        match field.chunk().await {
            Ok(Some(chunk)) => head.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(_) => {
                return Ok(refusal(
                    StatusCode::BAD_REQUEST,
                    "The upload was cut off. Try again.",
                    back,
                ));
            }
        }
    }
    if !head.starts_with(b"%PDF-") {
        return Ok(refusal(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            &format!("“{name}” is not a PDF."),
            back,
        ));
    }

    // Any conversion still running is discarded first, so it can't
    // overwrite this PDF (a worker storing one holds its job locked).
    sqlx::query("DELETE FROM pdf_jobs WHERE document_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(internal_error)?;

    let body =
        futures::stream::iter([Ok::<_, axum::extract::multipart::MultipartError>(Bytes::from(head))]).chain(field);
    let key = state.storage.document_key(id, FileRole::Pdf);
    let stored = match state
        .storage
        .put_stream(&key, "application/pdf", body, MAX_FILE_BYTES)
        .await
    {
        Ok(stored) => stored,
        Err(e) => {
            // Nothing was stored; a Document still preparing gets its job back.
            requeue_if_preparing(state, id).await;
            return Ok(match e {
                PutError::TooLarge => refusal(StatusCode::PAYLOAD_TOO_LARGE, &size_refusal(&name), back),
                PutError::Body(_) => refusal(StatusCode::BAD_REQUEST, "The upload was cut off. Try again.", back),
                PutError::Storage(e) => return Err(internal_error(e)),
            });
        }
    };

    let saved = async {
        let mut tx = state.db.begin().await?;
        let current: Current = sqlx::query_as(
            "SELECT pdf_status::text, needs_review, needs_review_comment FROM documents WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let before_pdf: Option<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT byte_size, sha256 FROM document_files WHERE document_id = $1 AND role = 'pdf'")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        sqlx::query(
            "INSERT INTO document_files (document_id, role, content_type, extension, byte_size, sha256)
             VALUES ($1, 'pdf', 'application/pdf', 'pdf', $2, $3)
             ON CONFLICT (document_id, role) DO UPDATE
             SET content_type = excluded.content_type, extension = excluded.extension,
                 byte_size = excluded.byte_size, sha256 = excluded.sha256, created_at = now()",
        )
        .bind(id)
        .bind(stored.byte_size)
        .bind(stored.sha256.to_vec())
        .execute(&mut *tx)
        .await?;
        let review = Review {
            needs_review: current.needs_review,
            comment: current.needs_review_comment.clone(),
        };
        let after = review.after_success();
        sqlx::query(
            "UPDATE documents
             SET pdf_status = 'ready', needs_review = $2, needs_review_comment = $3, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(after.needs_review)
        .bind(&after.comment)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM pdf_jobs WHERE document_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        change_log::record(
            &mut tx,
            Entity::Document,
            id,
            [
                Change::new(
                    "pdf",
                    before_pdf.map(|(size, sha256)| format!("PDF ({size} bytes, sha256 {})", hex::encode(sha256))),
                    format!(
                        "{name} ({} bytes, sha256 {})",
                        stored.byte_size,
                        hex::encode(stored.sha256)
                    ),
                ),
                Change::new("pdf_status", current.pdf_status, "ready"),
                Change::new("needs_review", review.needs_review, after.needs_review),
                Change::new("needs_review_comment", review.comment, after.comment),
            ],
        )
        .await?;
        tx.commit().await?;
        anyhow::Ok(())
    }
    .await;
    saved.map_err(internal_error)?;
    Ok(auth::see_other(back, None))
}

async fn requeue_if_preparing(state: &AppState, id: i64) {
    let requeued = async {
        let mut tx = state.db.begin().await?;
        let preparing: Option<bool> =
            sqlx::query_scalar("SELECT true FROM documents WHERE id = $1 AND pdf_status = 'preparing' FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if preparing.is_some() {
            enqueue(&mut tx, id).await?;
        }
        tx.commit().await
    }
    .await;
    match requeued {
        Ok(()) => state.pdf.wake(),
        Err(e) => dioxus::logger::tracing::error!("re-queuing D{id}: {e}"),
    }
}

// ----------------------------------------------------------------- helpers

/// Where to send the Admin afterwards: the posted `return_to` if it is an
/// Admin page of this site, otherwise the PDF problems page.
fn return_to(posted: &str) -> String {
    let posted = posted.trim();
    let safe = posted.starts_with("/admin/")
        && !posted.contains("//")
        && !posted.contains('\\')
        && !posted.chars().any(char::is_control);
    if safe {
        posted.to_owned()
    } else {
        PDF_PROBLEMS.to_owned()
    }
}

/// A refusal shown to the Admin as a page, after which nothing changed.
fn refusal(status: StatusCode, message: &str, back: &str) -> Response {
    let message = escape_html(message);
    let back = escape_html(back);
    let page = format!(
        "<!DOCTYPE html>\
<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Not done · FAMAT Test Bank</title>\
<style>body{{font:16px/1.4 system-ui,sans-serif;margin:0;color:#1a1a1a;background:#fff}}\
main{{max-width:64rem;margin:0 auto;padding:1rem}}#refusal{{color:#b00020}}</style></head>\
<body><main><h1>Nothing was changed</h1>\
<p id=\"refusal\">{message}</p>\
<p><a href=\"{back}\">Go back</a></p>\
</main></body></html>"
    );
    (status, [(header::CACHE_CONTROL, "no-store")], Html(page)).into_response()
}
