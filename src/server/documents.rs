//! Document downloads: plain axum routes that stream from Garage (ADR 0001).

use axum::{
    Extension,
    body::Body,
    extract::Path,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio_util::io::ReaderStream;

use super::{AppState, storage::FileRole};

/// `GET /documents/{id}/pdf`: a Document's PDF, inline so browsers open it.
///
/// Served only when the Document is Published and untrashed and belongs to
/// at least one Published, untrashed Test. The PDF is the generated one if
/// there is one, otherwise the Original when it is itself a PDF. Anything
/// else, including an object missing from Garage, is a 404.
pub async fn pdf(Extension(state): Extension<AppState>, Path(id): Path<i64>) -> Response {
    let role: Option<String> = match sqlx::query_scalar(
        "SELECT f.role::text
         FROM document_files f
         JOIN documents d ON d.id = f.document_id
         WHERE d.id = $1
           AND d.published AND d.trashed_at IS NULL
           AND EXISTS (SELECT 1 FROM test_documents td JOIN tests t ON t.id = td.test_id
                       WHERE td.document_id = d.id AND t.published AND t.trashed_at IS NULL)
           AND (f.role = 'pdf' OR f.content_type = 'application/pdf')
         ORDER BY f.role = 'pdf' DESC
         LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(role) => role,
        Err(e) => return server_error(e),
    };
    let role = match role.as_deref() {
        Some("pdf") => FileRole::Pdf,
        Some(_) => FileRole::Original,
        None => return not_found(),
    };

    // Open the object before answering, so a missing one is a real 404.
    let key = state.storage.document_key(id, role);
    let object = match state.storage.get(&key).await {
        Ok(Some(object)) => object,
        Ok(None) => {
            dioxus::logger::tracing::warn!("Document {id} has no object at {key}");
            return not_found();
        }
        Err(e) => return server_error(e),
    };

    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "application/pdf")
        // Placeholder name; the download name scheme comes with ticket #19.
        .header(header::CONTENT_DISPOSITION, format!("inline; filename=\"D{id}.pdf\""))
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(len) = object.content_length {
        response = response.header(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    let body = Body::from_stream(ReaderStream::new(object.body.into_async_read()));
    response.body(body).unwrap_or_else(server_error)
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Document not found").into_response()
}

fn server_error(e: impl std::fmt::Display) -> Response {
    dioxus::logger::tracing::error!("{e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}
