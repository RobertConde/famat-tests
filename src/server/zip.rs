//! Per-Test zips: `GET /tests/{test_id}/zip[?group=…][&format=…]`, a plain
//! axum route (ADR 0001) streaming a zip of a Test's Published, untrashed
//! Documents.
//!
//! - `group`: `all` (default), `tests`, `answers`, `solutions` or `other`
//!   (see [`KindGroup`]).
//! - `format`: `pdf` (default; the Original when there is no ready PDF),
//!   `original` or `both` (see [`Format`]).
//!
//! The zip is named `T{id} {Year} {Month} {Competition} - {Host} -
//! {Division/Level} {Test type}[ - {Kind group}].zip`, and its entries by the
//! Document filename scheme. Entries are Stored (PDFs are already
//! compressed) and dated by when each file was stored.
//!
//! Objects stream from Garage through a `tokio::io::duplex` pipe into the
//! response, so memory stays flat whatever their size. The first object is
//! opened before answering, so a missing one is a real 404; a later failure
//! aborts the response, so the client sees a failed download rather than a
//! short zip.

use std::io;

use anyhow::Context;
use async_zip::{Compression, ZipDateTime, ZipDateTimeBuilder, ZipEntryBuilder, tokio::write::ZipFileWriter};
use axum::{
    Extension,
    body::Body,
    extract::{Path, Query},
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
    download_name::{NameParts, content_disposition, file_name, zip_name},
    storage::{FileRole, Object, Storage},
};
use crate::download::{Contents, Format, KindGroup};

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
    let entries = match entries(&state.db, &test, query.group, query.format).await {
        Ok(entries) => entries,
        Err(e) => return server_error(e),
    };
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

    let group = (query.group != KindGroup::All).then(|| query.group.label());
    let name = zip_name(&test.name_parts(), group);
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
        dioxus::logger::tracing::error!("zip of Test {test_id} failed: {failure}");
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

/// One file to put in the zip.
struct Entry {
    document_id: i64,
    role: FileRole,
    name: String,
    stored_at: StoredAt,
}

#[derive(FromRow)]
struct DocumentRecord {
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

/// The files to zip, in results order (kind, label, ID), with their names.
async fn entries(db: &PgPool, test: &TestFacets, group: KindGroup, format: Format) -> Result<Vec<Entry>, sqlx::Error> {
    let documents: Vec<DocumentRecord> = sqlx::query_as(
        "SELECT d.id, k.name AS kind, d.label, d.pdf_status::text,
                k.contains_questions, k.contains_answers, k.contains_solutions
         FROM test_documents td
         JOIN documents d      ON d.id = td.document_id
         JOIN document_kinds k ON k.id = d.kind_id
         WHERE td.test_id = $1 AND d.published AND d.trashed_at IS NULL
         ORDER BY k.display_order, k.name, d.label NULLS FIRST, d.id",
    )
    .bind(test.test_id)
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

    let ids: Vec<i64> = documents.iter().map(|d| d.id).collect();
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

    let mut entries = Vec::new();
    for document in &documents {
        let file = |role: &str| {
            files
                .iter()
                .find(|f| f.document_id == document.id && f.file.role == role)
        };
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
            entries.push(Entry {
                document_id: document.id,
                role,
                name: file_name(&parts, &record.file.extension),
                stored_at: record.stored_at,
            });
        }
    }
    Ok(entries)
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
