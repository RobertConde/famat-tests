//! Document downloads: plain axum routes that stream from Garage (ADR 0001).
//!
//! Downloads are scoped to a Test, because a Document can belong to several
//! and its download name carries the ID of the Test it came from:
//!
//! - `GET /tests/{test_id}/documents/{id}/pdf`: the PDF, inline.
//! - `GET /tests/{test_id}/documents/{id}/original`: the Original, as a download.
//! - `GET /documents/{id}/pdf` and `/original`: redirect to the same file
//!   under the Document's first Published Test (lowest ID).
//!
//! Only Published, untrashed Documents of Published, untrashed Tests are
//! served; everything else, including an object missing from Garage, is a 404.

use axum::{
    Extension,
    body::Body,
    extract::Path,
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use sqlx::{FromRow, PgPool};
use tokio_util::io::ReaderStream;

use super::{
    AppState,
    download_name::{NameParts, TestParts, content_disposition, file_name},
    storage::FileRole,
};

/// `GET /tests/{test_id}/documents/{id}/pdf`: the Document's PDF, inline so
/// browsers open it.
///
/// The PDF is the generated one once it is ready, otherwise the Original when
/// it is itself a PDF. When there is no PDF (yet) but there is an Original,
/// a page says so and links to the Original.
pub async fn pdf(Extension(state): Extension<AppState>, Path((test_id, id)): Path<(i64, i64)>) -> Response {
    let document = match Download::find(&state.db, test_id, id).await {
        Ok(Some(document)) => document,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e),
    };
    let role = if document.pdf_status == "ready" && document.pdf.is_some() {
        FileRole::Pdf
    } else if document.original.as_ref().is_some_and(File::is_pdf) {
        FileRole::Original
    } else if let Some(original) = &document.original {
        return no_pdf_page(&document, original);
    } else {
        return not_found();
    };
    let disposition = content_disposition("inline", &document.file_name("pdf"));
    stream(&state, id, role, "application/pdf", &disposition).await
}

/// `GET /tests/{test_id}/documents/{id}/original`: the Original exactly as
/// uploaded, as a download named with its own extension.
pub async fn original(Extension(state): Extension<AppState>, Path((test_id, id)): Path<(i64, i64)>) -> Response {
    let document = match Download::find(&state.db, test_id, id).await {
        Ok(Some(document)) => document,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e),
    };
    let Some(original) = &document.original else {
        return not_found();
    };
    let disposition = content_disposition("attachment", &document.file_name(&original.extension));
    stream(&state, id, FileRole::Original, &original.content_type, &disposition).await
}

/// `GET /documents/{id}/pdf`: redirect to the PDF under the Document's first
/// Published Test.
pub async fn pdf_of_first_test(state: Extension<AppState>, Path(id): Path<i64>) -> Response {
    redirect_to_first_test(state, id, "pdf").await
}

/// `GET /documents/{id}/original`: redirect to the Original under the
/// Document's first Published Test.
pub async fn original_of_first_test(state: Extension<AppState>, Path(id): Path<i64>) -> Response {
    redirect_to_first_test(state, id, "original").await
}

async fn redirect_to_first_test(Extension(state): Extension<AppState>, id: i64, file: &str) -> Response {
    let test_id: Option<i64> = match sqlx::query_scalar(
        "SELECT t.id
         FROM test_documents td
         JOIN tests t     ON t.id = td.test_id
         JOIN documents d ON d.id = td.document_id
         WHERE d.id = $1
           AND d.published AND d.trashed_at IS NULL
           AND t.published AND t.trashed_at IS NULL
         ORDER BY t.id
         LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(test_id) => test_id,
        Err(e) => return server_error(e),
    };
    match test_id {
        // Temporary: the first Test can change as Tests are hidden or merged.
        Some(test_id) => Redirect::temporary(&format!("/tests/{test_id}/documents/{id}/{file}")).into_response(),
        None => not_found(),
    }
}

/// A Published Document as downloaded from one of its Published Tests:
/// what its name is made of, and its files.
#[derive(FromRow)]
struct Download {
    #[sqlx(flatten)]
    test: TestFacets,
    id: i64,
    kind: String,
    label: Option<String>,
    pdf_status: String,
    #[sqlx(skip)]
    original: Option<File>,
    #[sqlx(skip)]
    pdf: Option<File>,
}

/// The Facets of the Test a download comes from, as named in its download
/// names. Selected as `t.id AS test_id, t.year, t.month_applicable, m.name
/// AS month, c.name AS competition, h.name AS host, dl.name AS
/// division_level, tt.name AS test_type` (see [`TEST_FACETS_FROM`]).
#[derive(FromRow)]
pub(super) struct TestFacets {
    pub test_id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    host: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
}

/// The select list and joins [`TestFacets`] reads, over `tests t`.
pub(super) const TEST_FACETS_FROM: &str = "t.id AS test_id, t.year, t.month_applicable,
        m.name AS month, c.name AS competition, h.name AS host,
        dl.name AS division_level, tt.name AS test_type
    FROM tests t
    LEFT JOIN facet_values m  ON m.id  = t.month_id
    LEFT JOIN facet_values c  ON c.id  = t.competition_id
    LEFT JOIN facet_values h  ON h.id  = t.host_id
    LEFT JOIN facet_values dl ON dl.id = t.division_level_id
    LEFT JOIN facet_values tt ON tt.id = t.test_type_id";

impl TestFacets {
    pub fn name_parts(&self) -> TestParts<'_> {
        TestParts {
            test_id: self.test_id,
            year: self.year,
            month: self.month.as_deref().filter(|_| self.month_applicable),
            competition: self.competition.as_deref(),
            host: self.host.as_deref(),
            division_level: self.division_level.as_deref(),
            test_type: self.test_type.as_deref(),
        }
    }
}

/// One of a Document's files, as `document_files` describes it.
#[derive(FromRow)]
pub(super) struct File {
    pub role: String,
    pub content_type: String,
    /// Lower case, without the dot; empty when the file had none.
    pub extension: String,
}

impl File {
    pub fn is_pdf(&self) -> bool {
        self.content_type == "application/pdf" || self.extension == "pdf"
    }
}

impl Download {
    /// `None` unless the Document belongs to the Test and both are Published
    /// and untrashed.
    async fn find(db: &PgPool, test_id: i64, id: i64) -> Result<Option<Self>, sqlx::Error> {
        let download: Option<Self> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT d.id, k.name AS kind, d.label, d.pdf_status::text, {TEST_FACETS_FROM}
             JOIN test_documents td ON td.test_id = t.id
             JOIN documents d       ON d.id = td.document_id
             JOIN document_kinds k  ON k.id = d.kind_id
             WHERE t.id = $1 AND d.id = $2
               AND t.published AND t.trashed_at IS NULL
               AND d.published AND d.trashed_at IS NULL"
        )))
        .bind(test_id)
        .bind(id)
        .fetch_optional(db)
        .await?;
        let Some(mut download) = download else {
            return Ok(None);
        };
        let files: Vec<File> =
            sqlx::query_as("SELECT role::text, content_type, extension FROM document_files WHERE document_id = $1")
                .bind(id)
                .fetch_all(db)
                .await?;
        for file in files {
            match file.role.as_str() {
                "original" => download.original = Some(file),
                _ => download.pdf = Some(file),
            }
        }
        Ok(Some(download))
    }

    fn file_name(&self, extension: &str) -> String {
        file_name(
            &NameParts {
                test: self.test.name_parts(),
                document_id: self.id,
                kind: &self.kind,
                label: self.label.as_deref(),
            },
            extension,
        )
    }

    fn original_href(&self) -> String {
        format!("/tests/{}/documents/{}/original", self.test.test_id, self.id)
    }
}

/// Stream one of a Document's objects. The object is opened before
/// answering, so a missing one is a real 404.
async fn stream(state: &AppState, id: i64, role: FileRole, content_type: &str, disposition: &str) -> Response {
    let key = state.storage.document_key(id, role);
    let object = match state.storage.get(&key).await {
        Ok(Some(object)) => object,
        Ok(None) => {
            dioxus::logger::tracing::warn!("Document {id} has no object at {key}");
            return not_found();
        }
        Err(e) => return server_error(e),
    };

    let content_type =
        HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream"));
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(len) = object.content_length {
        response = response.header(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    let body = Body::from_stream(ReaderStream::new(object.body.into_async_read()));
    response.body(body).unwrap_or_else(server_error)
}

/// The page shown instead of a PDF that is still being prepared (or could
/// not be made), offering the Original.
fn no_pdf_page(document: &Download, original: &File) -> Response {
    let (title, message) = if document.pdf_status == "preparing" {
        (
            "PDF being prepared",
            "The PDF of this Document is being prepared. Try again in a few minutes, or download the Original:",
        )
    } else {
        ("No PDF", "This Document has no PDF. Download the Original:")
    };
    let name = escape_html(&document.file_name(&original.extension));
    let href = document.original_href();
    let page = format!(
        "<!DOCTYPE html>\
<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title} · FAMAT Test Bank</title>\
<style>body{{font:16px/1.4 system-ui,sans-serif;margin:0;color:#1a1a1a;background:#fff}}\
main{{max-width:64rem;margin:0 auto;padding:1rem}}</style></head>\
<body><main><h1>{title}</h1>\
<p id=\"pdf-status\">{message}</p>\
<p><a id=\"original\" href=\"{href}\" download>{name}</a></p>\
<p><a href=\"/\">Back to the search</a></p>\
</main></body></html>"
    );
    // Never cached: the PDF replaces this page once it is ready.
    ([(header::CACHE_CONTROL, "no-store")], Html(page)).into_response()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Document not found").into_response()
}

pub(super) fn server_error(e: impl std::fmt::Display) -> Response {
    dioxus::logger::tracing::error!("{e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}
