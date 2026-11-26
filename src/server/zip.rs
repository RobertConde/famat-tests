//! Zips of Published, untrashed Documents, as plain axum routes (ADR 0001):
//!
//! - **Per Test:** `GET /tests/{test_id}/zip[?group=…][&format=…]`, named
//!   `T{id} {Year} {Month} {Competition} - {Host} - {Division/Level} {Test
//!   type}[ - {Kind group}].zip`.
//! - **Bulk:** `GET /tests/zip?sel=…[&group=…][&format=…]` (see
//!   [`bulk_zip`]), named `FAMAT Test Bank - {N} tests[ - {Kind group}].zip`,
//!   with one folder per Test named like that Test's own zip.
//!
//! - `group`: `all` (default), `tests`, `answers`, `solutions` or `other`
//!   (see [`KindGroup`]).
//! - `format`: `pdf` (default; the Original when there is no ready PDF),
//!   `original` or `both` (see [`Format`]).
//!
//! Entries are named by the Document filename scheme. They are Stored (PDFs
//! are already compressed) and dated by when each file was stored.
//!
//! Objects stream from Garage through a `tokio::io::duplex` pipe into the
//! response, so memory stays flat whatever their size. The first object is
//! opened before answering, so a missing one is a real 404; a later failure
//! aborts the response, so the client sees a failed download rather than a
//! short zip.

use std::{
    collections::{HashMap, HashSet},
    io,
};

use anyhow::Context;
use async_zip::{Compression, ZipDateTime, ZipDateTimeBuilder, ZipEntryBuilder, tokio::write::ZipFileWriter};
use axum::{
    Extension,
    body::Body,
    extract::{Path, Query, RawQuery},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::{AsyncWriteExt, StreamExt, stream};
use serde::Deserialize;
use sqlx::{FromRow, PgPool};
use tokio::io::DuplexStream;
use tokio_util::{compat::TokioAsyncReadCompatExt, io::ReaderStream};

use super::{
    AppState,
    documents::{File, TEST_FACETS_FROM, TestFacets, server_error},
    download_name::{NameParts, bulk_zip_name, content_disposition, file_name, folder_name, zip_name},
    search::RESULTS_ORDER,
    storage::{FileRole, Object, Storage},
};
use crate::download::{Contents, Format, KindGroup, MAX_BULK_TESTS, parse_selection};

/// Zip entry dates are local times without a zone (async_zip cannot write
/// the extended timestamp field). FAMAT is in Florida, so they are given in
/// Eastern time.
const ZIP_TIME_ZONE: &str = "America/New_York";

/// How much of the zip is buffered between the writer and the response.
const PIPE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct ZipQuery {
    #[serde(default)]
    group: KindGroup,
    #[serde(default)]
    format: Format,
}

pub async fn test_zip(
    Extension(state): Extension<AppState>,
    Path(test_id): Path<i64>,
    Query(query): Query<ZipQuery>,
) -> Response {
    let test = match published_test(&state.db, test_id).await {
        Ok(Some(test)) => test,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e),
    };
    let entries = match entries(&state.db, std::slice::from_ref(&test), query.group, query.format, false).await {
        Ok(entries) => entries,
        Err(e) => return server_error(e),
    };
    let group = (query.group != KindGroup::All).then(|| query.group.label());
    let name = zip_name(&test.name_parts(), group);
    respond(&state, entries, name, format!("Test {test_id}")).await
}

/// `GET /tests/zip?sel=…[&group=…][&format=…]`: the bulk zip of the selected
/// Tests, one folder per Test, named `FAMAT Test Bank - {N} tests[ - {Kind
/// group}].zip`.
///
/// - `sel`: Test IDs, comma-separated and/or repeated (the search page's
///   selection form sends both). At most [`MAX_BULK_TESTS`] different ones,
///   or the answer is 413.
/// - `group` and `format` as for one Test.
/// - Anything else is ignored: the search page's selection form also sends
///   its search.
///
/// Only Published, untrashed Tests and Documents are zipped, whatever is
/// asked for. `N` counts the Tests in the zip: Tests with nothing in the
/// group get no folder.
pub async fn bulk_zip(Extension(state): Extension<AppState>, RawQuery(raw): RawQuery) -> Response {
    let request = match BulkRequest::parse(raw.as_deref().unwrap_or("")) {
        Ok(request) => request,
        Err(refusal) => return refusal.into_response(),
    };
    let tests = match published_tests(&state.db, &request.ids).await {
        Ok(tests) => tests,
        Err(e) => return server_error(e),
    };
    let entries = match entries(&state.db, &tests, request.group, request.format, true).await {
        Ok(entries) => entries,
        Err(e) => return server_error(e),
    };
    let mut zipped: Vec<i64> = entries.iter().map(|entry| entry.test_id).collect();
    zipped.dedup();
    let group = (request.group != KindGroup::All).then(|| request.group.label());
    let name = bulk_zip_name(zipped.len(), group);
    respond(&state, entries, name, format!("{} Tests", zipped.len())).await
}

/// What a bulk zip request asks for.
struct BulkRequest {
    /// Distinct, in the order asked.
    ids: Vec<i64>,
    group: KindGroup,
    format: Format,
}

impl BulkRequest {
    /// The request in a query string, or the 400 or 413 refusing it.
    fn parse(raw: &str) -> Result<Self, (StatusCode, String)> {
        let bad_request = |message: &str| (StatusCode::BAD_REQUEST, message.to_owned());
        let params: Vec<(String, String)> =
            serde_urlencoded::from_str(raw).map_err(|_| bad_request("Malformed query string"))?;
        let first = |key: &str| params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let group = match first("group") {
            None => KindGroup::All,
            Some(slug) => KindGroup::MENU
                .into_iter()
                .find(|group| group.slug() == slug)
                .ok_or_else(|| bad_request("Unknown group"))?,
        };
        let format = match first("format") {
            None => Format::Pdf,
            Some(slug) => Format::from_slug(slug).ok_or_else(|| bad_request("Unknown format"))?,
        };
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        for (_, value) in params.iter().filter(|(k, _)| k == "sel") {
            let parsed = parse_selection(value).ok_or_else(|| bad_request("Not a test ID in sel"))?;
            ids.extend(parsed.into_iter().filter(|id| seen.insert(*id)));
            if ids.len() > MAX_BULK_TESTS {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("A bulk download holds at most {MAX_BULK_TESTS} tests. Select fewer."),
                ));
            }
        }
        if ids.is_empty() {
            return Err(bad_request("No tests selected"));
        }
        Ok(Self { ids, group, format })
    }
}

/// Stream `entries` as a zip named `name`. `what` names it in the log.
async fn respond(state: &AppState, entries: Vec<Entry>, name: String, what: String) -> Response {
    let Some(first) = entries.first() else {
        return not_found();
    };
    let key = state.storage.document_key(first.document_id, first.role);
    let first = match state.storage.get(&key).await {
        Ok(Some(object)) => object,
        Ok(None) => {
            dioxus::logger::tracing::warn!("Document {} has no object at {key}", first.document_id);
            return not_found();
        }
        Err(e) => return server_error(e),
    };

    let (writer, reader) = tokio::io::duplex(PIPE_BYTES);
    let writing = tokio::spawn(write_zip(state.storage.clone(), writer, entries, first));
    // After the last byte, fail the body if the zip could not be finished,
    // so the connection is cut instead of ending a truncated zip cleanly.
    let outcome = stream::once(async move {
        let failure = match writing.await {
            Ok(Ok(())) => return None,
            Ok(Err(e)) => format!("{e:#}"),
            Err(e) => e.to_string(),
        };
        dioxus::logger::tracing::error!("zip of {what} failed: {failure}");
        Some(Err(io::Error::other("the zip could not be finished")))
    })
    .filter_map(|outcome| async move { outcome });
    let body = Body::from_stream(ReaderStream::new(reader).chain(outcome));

    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, content_disposition("attachment", &name))
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .unwrap_or_else(server_error)
}

/// The Test, if it is Published and untrashed.
async fn published_test(db: &PgPool, test_id: i64) -> Result<Option<TestFacets>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TEST_FACETS_FROM}
         WHERE t.id = $1 AND t.published AND t.trashed_at IS NULL"
    )))
    .bind(test_id)
    .fetch_optional(db)
    .await
}

/// Those of the Tests that are Published and untrashed, in results order.
async fn published_tests(db: &PgPool, ids: &[i64]) -> Result<Vec<TestFacets>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TEST_FACETS_FROM}
         WHERE t.id = ANY($1) AND t.published AND t.trashed_at IS NULL
         ORDER BY {RESULTS_ORDER}"
    )))
    .bind(ids)
    .fetch_all(db)
    .await
}

/// One file to put in the zip.
struct Entry {
    /// The Test it is zipped from.
    test_id: i64,
    document_id: i64,
    role: FileRole,
    name: String,
    stored_at: StoredAt,
}

#[derive(FromRow)]
struct DocumentRecord {
    test_id: i64,
    id: i64,
    kind: String,
    label: Option<String>,
    pdf_status: String,
    contains_questions: bool,
    contains_answers: bool,
    contains_solutions: bool,
}

#[derive(FromRow)]
struct FileRecord {
    document_id: i64,
    #[sqlx(flatten)]
    file: File,
    #[sqlx(flatten)]
    stored_at: StoredAt,
}

/// When a file was stored, as the zip's local date parts.
#[derive(Clone, Copy, FromRow)]
struct StoredAt {
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
}

impl StoredAt {
    fn dos(&self) -> ZipDateTime {
        let part = |n: i32| u32::try_from(n).unwrap_or_default();
        ZipDateTimeBuilder::new()
            .year(self.year)
            .month(part(self.month))
            .day(part(self.day))
            .hour(part(self.hour))
            .minute(part(self.minute))
            .second(part(self.second))
            .build()
    }
}

/// The files to zip from `tests`, Test by Test in the order given, each
/// Test's in results order (kind, label, ID), with their names. With
/// `folders`, each Test's files go in a folder named like its own zip.
async fn entries(
    db: &PgPool,
    tests: &[TestFacets],
    group: KindGroup,
    format: Format,
    folders: bool,
) -> Result<Vec<Entry>, sqlx::Error> {
    let test_ids: Vec<i64> = tests.iter().map(|t| t.test_id).collect();
    let documents: Vec<DocumentRecord> = sqlx::query_as(
        "SELECT td.test_id, d.id, k.name AS kind, d.label, d.pdf_status::text,
                k.contains_questions, k.contains_answers, k.contains_solutions
         FROM test_documents td
         JOIN documents d      ON d.id = td.document_id
         JOIN document_kinds k ON k.id = d.kind_id
         WHERE td.test_id = ANY($1) AND d.published AND d.trashed_at IS NULL
         ORDER BY k.display_order, k.name, d.label NULLS FIRST, d.id",
    )
    .bind(&test_ids)
    .fetch_all(db)
    .await?;
    let documents: Vec<DocumentRecord> = documents
        .into_iter()
        .filter(|d| {
            group.includes(Contents {
                questions: d.contains_questions,
                answers: d.contains_answers,
                solutions: d.contains_solutions,
            })
        })
        .collect();

    let mut ids: Vec<i64> = documents.iter().map(|d| d.id).collect();
    ids.sort_unstable();
    ids.dedup();
    let files: Vec<FileRecord> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.document_id, f.role::text, f.content_type, f.extension,
                extract(year FROM l.at)::int AS year, extract(month FROM l.at)::int AS month,
                extract(day FROM l.at)::int AS day, extract(hour FROM l.at)::int AS hour,
                extract(minute FROM l.at)::int AS minute, floor(extract(second FROM l.at))::int AS second
         FROM document_files f
         CROSS JOIN LATERAL (SELECT f.created_at AT TIME ZONE '{ZIP_TIME_ZONE}' AS at) l
         WHERE f.document_id = ANY($1)"
    )))
    .bind(&ids)
    .fetch_all(db)
    .await?;

    let files: HashMap<(i64, &str), &FileRecord> = files
        .iter()
        .map(|f| ((f.document_id, f.file.role.as_str()), f))
        .collect();
    let mut by_test: HashMap<i64, Vec<&DocumentRecord>> = HashMap::new();
    for document in &documents {
        by_test.entry(document.test_id).or_default().push(document);
    }

    let mut entries = Vec::new();
    for test in tests {
        let folder = folders.then(|| folder_name(&test.name_parts()));
        for document in by_test.remove(&test.test_id).unwrap_or_default() {
            push_entries(&mut entries, test, folder.as_deref(), document, &files, format);
        }
    }
    Ok(entries)
}

/// Add the files of one of a Test's Documents that `format` asks for.
fn push_entries(
    entries: &mut Vec<Entry>,
    test: &TestFacets,
    folder: Option<&str>,
    document: &DocumentRecord,
    files: &HashMap<(i64, &str), &FileRecord>,
    format: Format,
) {
    let file = |role: &str| files.get(&(document.id, role)).copied();
    let original = file("original");
    let pdf = file("pdf").filter(|_| document.pdf_status == "ready");
    let original_is_pdf = original.is_some_and(|o| o.file.is_pdf());
    let chosen: Vec<(FileRole, &FileRecord)> = match format {
        // The Original stands in for a missing PDF, e.g. a ZIP Document.
        Format::Pdf => match (pdf, original) {
            (Some(pdf), _) => vec![(FileRole::Pdf, pdf)],
            (None, Some(original)) => vec![(FileRole::Original, original)],
            (None, None) => vec![],
        },
        Format::Original => original.map(|o| (FileRole::Original, o)).into_iter().collect(),
        Format::Both => pdf
            .filter(|_| !original_is_pdf)
            .map(|p| (FileRole::Pdf, p))
            .into_iter()
            .chain(original.map(|o| (FileRole::Original, o)))
            .collect(),
    };
    for (role, record) in chosen {
        let parts = NameParts {
            test: test.name_parts(),
            document_id: document.id,
            kind: &document.kind,
            label: document.label.as_deref(),
        };
        let name = file_name(&parts, &record.file.extension);
        entries.push(Entry {
            test_id: test.test_id,
            document_id: document.id,
            role,
            name: match folder {
                Some(folder) => format!("{folder}/{name}"),
                None => name,
            },
            stored_at: record.stored_at,
        });
    }
}

/// Write the zip into the pipe, one object at a time, starting with the
/// already opened `first`.
async fn write_zip(storage: Storage, pipe: DuplexStream, entries: Vec<Entry>, first: Object) -> anyhow::Result<()> {
    let mut zip = ZipFileWriter::with_tokio(pipe);
    let mut first = Some(first);
    for entry in entries {
        let key = storage.document_key(entry.document_id, entry.role);
        let object = match first.take() {
            Some(object) => object,
            None => storage
                .get(&key)
                .await?
                .with_context(|| format!("Document {} has no object at {key}", entry.document_id))?,
        };
        let header = ZipEntryBuilder::new(entry.name.into(), Compression::Stored)
            .last_modification_date(entry.stored_at.dos())
            .unix_permissions(0o100_644);
        let mut writer = zip.write_entry_stream(header).await?;
        let mut body = object.body.into_async_read().compat();
        futures::io::copy(&mut body, &mut writer)
            .await
            .with_context(|| format!("copying {key} into the zip"))?;
        writer.close().await?;
    }
    zip.close().await?.close().await?;
    Ok(())
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}
