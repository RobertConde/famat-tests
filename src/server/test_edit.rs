//! The Test edit page, server side (the page is `crate::app::test_edit`).
//!
//! Every write is a CSRF-checked native form post from an Admin, done in one
//! transaction with its change log entries, and answered with a 303 back to
//! the edit page (or a page saying why nothing was saved):
//!
//! - **Save test** ([`save_test`]): the six Facet values (a typed **+ New
//!   value** is created, as on Add test), Published/Hidden and Needs review.
//!   Publishing a Test that fails the publish rule (Year or Competition
//!   known) is refused.
//! - **Save document** ([`save_document`]): kind, label, Published/Hidden and
//!   Needs review. Its label source becomes `admin`.
//! - **Also on Tests…** ([`document_tests`]): attach a Document to another
//!   untrashed Test, or detach it; detaching it from its last untrashed
//!   Test is refused (a Test in the Trash doesn't count).
//! - **Replace original** ([`replace_original`]): the new file streams over
//!   `documents/{id}/original`, its row changes, any PDF (made or supplied)
//!   is dropped, and the Document is queued for conversion afresh.
//! - **+ Add files** ([`add_files`]): Add test's upload, rules and saving
//!   (`server::add_test`), into this Test.
//!
//! Unticking Needs review clears its comment too, so a stale reason never
//! outlives the flag.
//!
//! Multipart refusals after the CSRF check read the rest of the body first
//! ([`add_test::drain`]), so the browser is still listening when the page
//! arrives. #47 is making that one shared helper for every early rejection;
//! switch the `drain` calls here to it.

use std::collections::HashMap;

use axum::{
    extract::multipart::Field,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use dioxus::{fullstack::MultipartFormData, prelude::ServerFnError};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::{
    AppState,
    add_test::{self, Choice, Fields, Refusal, Resolved, Upload},
    auth,
    change_log::{self, Change, Entity},
    facet_values::FacetValueList,
    internal_error, pdf,
    storage::{FileRole, PutError},
};
use crate::app::{
    test_edit::{
        DocumentTestsForm, EditedDocument, EditedTest, SaveDocumentForm, SaveTestForm, TestEditPage, edit_path,
    },
    upload_rules::{self, MAX_FILE_BYTES, size_refusal, split_name},
};

/// The longest label a Document may have; download names are capped near 150.
const MAX_LABEL_CHARS: usize = 60;

/// An ID as an Admin types or a form posts it: `12`, `T12` or `D12`.
pub(super) fn parse_id(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let digits = raw.strip_prefix(['T', 't', 'D', 'd']).unwrap_or(raw);
    digits.trim().parse().ok().filter(|id| *id > 0)
}

/// A refusal that links back to the edit page of `test` (if known).
fn refuse(status: StatusCode, message: impl Into<String>, test: Option<i64>) -> Refusal {
    let back = test.map_or_else(|| "/".to_owned(), edit_path);
    Refusal::new(status, message).back_to(back, "Back to the Test")
}

/// The edit page of the posted Test, at the Document's section.
fn back_to_document(test: Option<i64>, document: i64) -> String {
    match test {
        Some(test) => format!("{}#D{document}", edit_path(test)),
        None => "/".to_owned(),
    }
}

/// A comment as saved: trimmed, `None` when empty or when the flag is off.
fn review_comment(needs_review: bool, comment: &str) -> Option<String> {
    let comment = comment.trim();
    (needs_review && !comment.is_empty()).then(|| comment.to_owned())
}

/// `published` or `hidden`.
fn visibility(posted: &str, test: Option<i64>) -> Result<bool, Refusal> {
    match posted {
        "published" => Ok(true),
        "hidden" => Ok(false),
        _ => Err(refuse(StatusCode::BAD_REQUEST, "Choose Published or Hidden.", test)),
    }
}

// -------------------------------------------------------------------- page

pub async fn page(state: &AppState, headers: &HeaderMap, id: i64) -> Result<TestEditPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    let kinds = add_test::document_kinds(&state.db).await.map_err(internal_error)?;
    let test = edited_test(&state.db, id).await.map_err(internal_error)?;
    Ok(TestEditPage { csrf, test, kinds })
}

#[derive(FromRow)]
struct TestRow {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month_id: Option<i64>,
    competition_id: Option<i64>,
    division_level_id: Option<i64>,
    test_type_id: Option<i64>,
    host_id: Option<i64>,
    published: bool,
    needs_review: bool,
    needs_review_comment: Option<String>,
}

async fn edited_test(db: &PgPool, id: i64) -> anyhow::Result<Option<EditedTest>> {
    let test: Option<TestRow> = sqlx::query_as(
        "SELECT id, year, month_applicable, month_id, competition_id, division_level_id, test_type_id, host_id,
                published, needs_review, needs_review_comment
         FROM tests WHERE id = $1 AND trashed_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(test) = test else {
        return Ok(None);
    };
    let values = FacetValueList::load(db).await?;
    let mut facets = add_test::facet_options(db, &values).await?;
    let chosen = |id: Option<i64>| id.map_or_else(|| "unknown".to_owned(), |id| id.to_string());
    for facet in &mut facets {
        facet.selected = match facet.name.as_str() {
            "year" => test.year.map_or_else(|| "unknown".to_owned(), |y| y.to_string()),
            "month" if !test.month_applicable => "not_applicable".to_owned(),
            "month" => chosen(test.month_id),
            "competition" => chosen(test.competition_id),
            "division_level" => chosen(test.division_level_id),
            "test_type" => chosen(test.test_type_id),
            "host" => chosen(test.host_id),
            _ => String::new(),
        };
        if facet.name == "year"
            && let Some(year) = test.year
            && !facet.options.iter().any(|(value, _)| *value == year.to_string())
        {
            facet.options.push((year.to_string(), year.to_string()));
        }
    }
    let name = |facet: &str, id: Option<i64>| id.and_then(|id| values.name(facet, id));
    let description = add_test::describe(
        test.year,
        test.month_applicable.then(|| name("month", test.month_id)),
        name("competition", test.competition_id),
        name("host", test.host_id),
        name("division_level", test.division_level_id),
        name("test_type", test.test_type_id),
    );
    let documents: Vec<EditedDocument> = sqlx::query_as(
        "SELECT d.id, d.kind_id, k.name AS kind, d.label, d.published, d.needs_review, d.needs_review_comment,
                d.label_source::text, d.pdf_status::text,
                coalesce(o.extension, '') AS original_extension,
                coalesce(o.byte_size, 0) AS original_bytes,
                p.byte_size AS pdf_bytes,
                coalesce((SELECT array_agg(o2.test_id ORDER BY o2.test_id)
                          FROM test_documents o2 JOIN tests t2 ON t2.id = o2.test_id
                          WHERE o2.document_id = d.id AND t2.trashed_at IS NULL), '{}') AS tests
         FROM test_documents td
         JOIN documents d      ON d.id = td.document_id
         JOIN document_kinds k ON k.id = d.kind_id
         LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
         LEFT JOIN document_files p ON p.document_id = d.id AND p.role = 'pdf'
         WHERE td.test_id = $1 AND d.trashed_at IS NULL
         ORDER BY d.id",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok(Some(EditedTest {
        id: test.id,
        description,
        facets,
        published: test.published,
        needs_review: test.needs_review,
        needs_review_comment: test.needs_review_comment.unwrap_or_default(),
        documents,
    }))
}

/// Run one write in a transaction that commits only if it succeeds, and
/// answer 303 to where it says, or with its refusal.
async fn write<F>(state: &AppState, write: F) -> Result<Response, ServerFnError>
where
    F: AsyncFnOnce(&mut Transaction<'static, Postgres>) -> anyhow::Result<Result<String, Refusal>>,
{
    let result: anyhow::Result<Result<String, Refusal>> = async {
        let mut tx = state.db.begin().await?;
        let outcome = write(&mut tx).await?;
        if outcome.is_ok() {
            tx.commit().await?;
        }
        Ok(outcome)
    }
    .await;
    match result.map_err(internal_error)? {
        Ok(location) => Ok(auth::see_other(&location, None)),
        Err(refusal) => Ok(refusal.response()),
    }
}

// --------------------------------------------------------------- save test

/// The Test as it was, with names for the change log.
#[derive(FromRow)]
struct TestBefore {
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
    host: Option<String>,
    published: bool,
    needs_review: bool,
    needs_review_comment: Option<String>,
}

/// How the change log shows a Month.
fn month_name(applicable: bool, name: Option<String>) -> Option<String> {
    if applicable {
        name
    } else {
        Some("Not applicable".to_owned())
    }
}

impl SaveTestForm {
    /// The dropdowns' fields, as Add test's parser reads them.
    fn facet_fields(&self) -> HashMap<String, String> {
        [
            ("year", &self.year),
            ("year_new", &self.year_new),
            ("month", &self.month),
            ("month_new", &self.month_new),
            ("competition", &self.competition),
            ("competition_new", &self.competition_new),
            ("division_level", &self.division_level),
            ("division_level_new", &self.division_level_new),
            ("test_type", &self.test_type),
            ("test_type_new", &self.test_type_new),
            ("host", &self.host),
            ("host_new", &self.host_new),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.clone()))
        .collect()
    }
}

/// `POST /api/admin/tests/edit`.
pub async fn save_test(state: &AppState, headers: &HeaderMap, form: SaveTestForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let Some(id) = parse_id(&form.test) else {
        return Ok(refuse(StatusCode::NOT_FOUND, "That Test does not exist.", None).response());
    };
    let values = FacetValueList::load(&state.db).await.map_err(internal_error)?;
    let facets = match add_test::facet_choices(&values, &form.facet_fields()) {
        Ok(facets) => facets,
        Err(refusal) => return Ok(refusal.back_to(edit_path(id), "Back to the Test").response()),
    };
    let published = match visibility(&form.visibility, Some(id)) {
        Ok(published) => published,
        Err(refusal) => return Ok(refusal.response()),
    };
    let needs_review = !form.needs_review.is_empty();
    let comment = review_comment(needs_review, &form.needs_review_comment);

    write(state, async |tx| {
        let before: Option<TestBefore> = sqlx::query_as(
            "SELECT t.year, t.month_applicable, m.name AS month, c.name AS competition,
                    dl.name AS division_level, tt.name AS test_type, h.name AS host,
                    t.published, t.needs_review, t.needs_review_comment
             FROM tests t
             LEFT JOIN facet_values m  ON m.id  = t.month_id
             LEFT JOIN facet_values c  ON c.id  = t.competition_id
             LEFT JOIN facet_values dl ON dl.id = t.division_level_id
             LEFT JOIN facet_values tt ON tt.id = t.test_type_id
             LEFT JOIN facet_values h  ON h.id  = t.host_id
             WHERE t.id = $1 AND t.trashed_at IS NULL
             FOR UPDATE OF t",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(before) = before else {
            return Ok(Err(refuse(
                StatusCode::NOT_FOUND,
                format!("T{id} does not exist or is in the Trash."),
                None,
            )));
        };
        let mut resolved = Vec::with_capacity(5);
        for (facet, choice) in facets.listed() {
            resolved.push(add_test::resolve(tx, facet, choice).await?);
        }
        let [month, competition, division_level, test_type, host] =
            <[Resolved; 5]>::try_from(resolved).map_err(|_| anyhow::anyhow!("five listed Facets"))?;
        if published && facets.year.is_none() && competition.id.is_none() {
            return Ok(Err(refuse(
                StatusCode::CONFLICT,
                "A Test needs its Year or Competition known to be published. \
                 Choose one of them, or save it Hidden.",
                Some(id),
            )));
        }
        let month_applicable = facets.month != Choice::NotApplicable;
        sqlx::query(
            "UPDATE tests
             SET year = $2, month_id = $3, month_applicable = $4, competition_id = $5,
                 division_level_id = $6, test_type_id = $7, host_id = $8,
                 published = $9, needs_review = $10, needs_review_comment = $11, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(facets.year)
        .bind(month.id)
        .bind(month_applicable)
        .bind(competition.id)
        .bind(division_level.id)
        .bind(test_type.id)
        .bind(host.id)
        .bind(published)
        .bind(needs_review)
        .bind(&comment)
        .execute(&mut **tx)
        .await?;
        change_log::record(
            tx,
            Entity::Test,
            id,
            [
                Change::new("year", before.year, facets.year),
                Change::new(
                    "month",
                    month_name(before.month_applicable, before.month),
                    month_name(month_applicable, month.name),
                ),
                Change::new("competition", before.competition, competition.name),
                Change::new("division_level", before.division_level, division_level.name),
                Change::new("test_type", before.test_type, test_type.name),
                Change::new("host", before.host, host.name),
                Change::new("published", before.published, published),
                Change::new("needs_review", before.needs_review, needs_review),
                Change::new("needs_review_comment", before.needs_review_comment, &comment),
            ],
        )
        .await?;
        Ok(Ok(edit_path(id)))
    })
    .await
}

// ----------------------------------------------------------- save document

#[derive(FromRow)]
struct DocumentBefore {
    kind: String,
    label: Option<String>,
    published: bool,
    needs_review: bool,
    needs_review_comment: Option<String>,
    label_source: String,
}

/// `POST /api/admin/documents/edit`.
pub async fn save_document(
    state: &AppState,
    headers: &HeaderMap,
    form: SaveDocumentForm,
) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let test = parse_id(&form.test);
    let Some(id) = parse_id(&form.document) else {
        return Ok(refuse(StatusCode::NOT_FOUND, "That Document does not exist.", test).response());
    };
    let published = match visibility(&form.visibility, test) {
        Ok(published) => published,
        Err(refusal) => return Ok(refusal.response()),
    };
    let label = form.label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.chars().count() > MAX_LABEL_CHARS {
        let message = format!("A label can be at most {MAX_LABEL_CHARS} characters.");
        return Ok(refuse(StatusCode::BAD_REQUEST, message, test).response());
    }
    let label = (!label.is_empty()).then_some(label);
    let needs_review = !form.needs_review.is_empty();
    let comment = review_comment(needs_review, &form.needs_review_comment);

    write(state, async |tx| {
        let before: Option<DocumentBefore> = sqlx::query_as(
            "SELECT k.name AS kind, d.label, d.published, d.needs_review, d.needs_review_comment,
                    d.label_source::text
             FROM documents d JOIN document_kinds k ON k.id = d.kind_id
             WHERE d.id = $1 AND d.trashed_at IS NULL
             FOR UPDATE OF d",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(before) = before else {
            return Ok(Err(refuse(
                StatusCode::NOT_FOUND,
                format!("D{id} does not exist or is in the Trash."),
                test,
            )));
        };
        // Held until commit, so the kind can't be deleted meanwhile.
        let kind: Option<(i64, String)> = match form.kind.trim().parse::<i64>() {
            Ok(kind) => {
                sqlx::query_as("SELECT id, name FROM document_kinds WHERE id = $1 FOR SHARE")
                    .bind(kind)
                    .fetch_optional(&mut **tx)
                    .await?
            }
            Err(_) => None,
        };
        let Some((kind_id, kind)) = kind else {
            return Ok(Err(refuse(
                StatusCode::BAD_REQUEST,
                "That Document kind no longer exists. Reload the page and try again.",
                test,
            )));
        };
        sqlx::query(
            "UPDATE documents
             SET kind_id = $2, label = $3, published = $4, needs_review = $5, needs_review_comment = $6,
                 label_source = 'admin', updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(kind_id)
        .bind(&label)
        .bind(published)
        .bind(needs_review)
        .bind(&comment)
        .execute(&mut **tx)
        .await?;
        change_log::record(
            tx,
            Entity::Document,
            id,
            [
                Change::new("kind", before.kind, kind),
                Change::new("label", before.label, &label),
                Change::new("published", before.published, published),
                Change::new("needs_review", before.needs_review, needs_review),
                Change::new("needs_review_comment", before.needs_review_comment, &comment),
                Change::new("label_source", before.label_source, "admin"),
            ],
        )
        .await?;
        Ok(Ok(back_to_document(test, id)))
    })
    .await
}

// ---------------------------------------------------------- Also on Tests…

fn test_list(tests: &[i64]) -> Option<String> {
    (!tests.is_empty()).then(|| {
        tests
            .iter()
            .map(|t| Entity::Test.display_id(*t))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// `POST /api/admin/documents/tests`.
pub async fn document_tests(
    state: &AppState,
    headers: &HeaderMap,
    form: DocumentTestsForm,
) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let test = parse_id(&form.test);
    let Some(id) = parse_id(&form.document) else {
        return Ok(refuse(StatusCode::NOT_FOUND, "That Document does not exist.", test).response());
    };
    write(state, async |tx| {
        // Locking the Document serializes attaches and detaches, so two
        // detaches can't together leave it on no Test.
        let found: Option<bool> =
            sqlx::query_scalar("SELECT true FROM documents WHERE id = $1 AND trashed_at IS NULL FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        if found.is_none() {
            return Ok(Err(refuse(
                StatusCode::NOT_FOUND,
                format!("D{id} does not exist or is in the Trash."),
                test,
            )));
        }
        let before: Vec<i64> =
            sqlx::query_scalar("SELECT test_id FROM test_documents WHERE document_id = $1 ORDER BY test_id")
                .bind(id)
                .fetch_all(&mut **tx)
                .await?;
        let back = back_to_document(test, id);
        let mut after = before.clone();
        if form.action == "attach" {
            let Some(other) = parse_id(&form.attach) else {
                return Ok(Err(refuse(
                    StatusCode::BAD_REQUEST,
                    "Type the ID of the Test to attach it to, like T123.",
                    test,
                )));
            };
            if before.contains(&other) {
                return Ok(Ok(back));
            }
            let usable: Option<bool> =
                sqlx::query_scalar("SELECT true FROM tests WHERE id = $1 AND trashed_at IS NULL FOR SHARE")
                    .bind(other)
                    .fetch_optional(&mut **tx)
                    .await?;
            if usable.is_none() {
                return Ok(Err(refuse(
                    StatusCode::NOT_FOUND,
                    format!("There is no Test T{other}, or it is in the Trash."),
                    test,
                )));
            }
            sqlx::query("INSERT INTO test_documents (test_id, document_id) VALUES ($1, $2)")
                .bind(other)
                .bind(id)
                .execute(&mut **tx)
                .await?;
            after.push(other);
            after.sort_unstable();
        } else if let Some(other) = form.action.strip_prefix("detach:").and_then(parse_id) {
            if !before.contains(&other) {
                return Ok(Ok(back));
            }
            // A Test in the Trash doesn't count: the Document would be
            // out of sight until that Test came back.
            let others_untrashed: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM test_documents td JOIN tests t ON t.id = td.test_id
                                WHERE td.document_id = $1 AND td.test_id <> $2 AND t.trashed_at IS NULL)",
            )
            .bind(id)
            .bind(other)
            .fetch_one(&mut **tx)
            .await?;
            if !others_untrashed {
                return Ok(Err(refuse(
                    StatusCode::CONFLICT,
                    format!(
                        "D{id} is on no other Test outside the Trash, and a Document must stay on at least \
                         one. Attach it to another Test first, or Remove it."
                    ),
                    test,
                )));
            }
            sqlx::query("DELETE FROM test_documents WHERE test_id = $1 AND document_id = $2")
                .bind(other)
                .bind(id)
                .execute(&mut **tx)
                .await?;
            after.retain(|t| *t != other);
        } else {
            return Ok(Err(refuse(StatusCode::BAD_REQUEST, "Choose Attach or Detach.", test)));
        }
        change_log::record(
            tx,
            Entity::Document,
            id,
            [Change::new("tests", test_list(&before), test_list(&after))],
        )
        .await?;
        Ok(Ok(back))
    })
    .await
}

// -------------------------------------------------------- Replace original

/// `POST /api/admin/documents/replace-original`. The `csrf` field must come
/// first, and `test` and `document` before the file, as the form puts them.
pub async fn replace_original(
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

    let (mut test, mut document) = (None, String::new());
    let outcome = loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break Err(refuse(StatusCode::BAD_REQUEST, "Choose a file to upload.", test)),
            Err(e) => break Err(Refusal::multipart(e).back_to(back_or_home(test), "Back to the Test")),
        };
        match (field.name().unwrap_or_default(), field.file_name()) {
            ("original", Some(name)) if !name.is_empty() => {
                let name = name.to_owned();
                break receive_original(state, test, &document, name, field).await;
            }
            ("original", _) => break Err(refuse(StatusCode::BAD_REQUEST, "Choose a file to upload.", test)),
            ("test", None) => test = parse_id(&field.text().await.unwrap_or_default()),
            ("document", None) => document = field.text().await.unwrap_or_default(),
            _ => {}
        }
    };
    match outcome {
        Ok(location) => Ok(auth::see_other(&location, None)),
        Err(refusal) => {
            add_test::drain(&mut form).await;
            Ok(refusal.response())
        }
    }
}

fn back_or_home(test: Option<i64>) -> String {
    test.map_or_else(|| "/".to_owned(), edit_path)
}

/// The file's row, before or after, as the change log shows it.
fn describe_file(name: &str, byte_size: i64, sha256: &[u8]) -> String {
    format!("{name} ({byte_size} bytes, sha256 {})", hex::encode(sha256))
}

async fn receive_original(
    state: &AppState,
    test: Option<i64>,
    document: &str,
    name: String,
    field: Field<'_>,
) -> Result<String, Refusal> {
    let Some(id) = parse_id(document) else {
        return Err(refuse(StatusCode::NOT_FOUND, "That Document does not exist.", test));
    };
    let exists: Option<bool> = sqlx::query_scalar("SELECT true FROM documents WHERE id = $1 AND trashed_at IS NULL")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(Refusal::internal)?;
    if exists.is_none() {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            format!("D{id} does not exist or is in the Trash."),
            test,
        ));
    }
    if let Some(message) = upload_rules::refusal(&name, None) {
        return Err(refuse(StatusCode::UNSUPPORTED_MEDIA_TYPE, message, test));
    }
    let (_, extension) = split_name(&name);
    let content_type = add_test::content_type(&extension);

    // A refused or failed put stores nothing, so the old Original is kept
    // until the whole new one is in.
    let key = state.storage.document_key(id, FileRole::Original);
    let stored = state
        .storage
        .put_stream(&key, content_type, field, MAX_FILE_BYTES)
        .await
        .map_err(|e| match e {
            PutError::TooLarge => refuse(StatusCode::PAYLOAD_TOO_LARGE, size_refusal(&name), test),
            PutError::Body(e) => Refusal::multipart(e).back_to(back_or_home(test), "Back to the Test"),
            PutError::Storage(e) => Refusal::internal(e),
        })?;

    let saved = async {
        let mut tx = state.db.begin().await?;
        // The job first, then the Document, in the order workers lock them.
        // Queuing afresh discards any conversion of the old Original.
        pdf::enqueue(&mut tx, id).await?;
        let status: String = sqlx::query_scalar("SELECT pdf_status::text FROM documents WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        let before_original: Option<(String, i64, Vec<u8>)> = sqlx::query_as(
            "SELECT extension, byte_size, sha256 FROM document_files WHERE document_id = $1 AND role = 'original'",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let before_pdf: Option<(i64, Vec<u8>)> = sqlx::query_as(
            "DELETE FROM document_files WHERE document_id = $1 AND role = 'pdf' RETURNING byte_size, sha256",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO document_files (document_id, role, content_type, extension, byte_size, sha256)
             VALUES ($1, 'original', $2, $3, $4, $5)
             ON CONFLICT (document_id, role) DO UPDATE
             SET content_type = excluded.content_type, extension = excluded.extension,
                 byte_size = excluded.byte_size, sha256 = excluded.sha256, created_at = now()",
        )
        .bind(id)
        .bind(content_type)
        .bind(&extension)
        .bind(stored.byte_size)
        .bind(stored.sha256.to_vec())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE documents SET pdf_status = 'preparing', updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        change_log::record(
            &mut tx,
            Entity::Document,
            id,
            [
                Change::new(
                    "original",
                    before_original.map(|(extension, size, sha256)| {
                        describe_file(&format!("original.{extension}"), size, &sha256)
                    }),
                    describe_file(&name, stored.byte_size, &stored.sha256),
                ),
                Change::new(
                    "pdf",
                    before_pdf
                        .as_ref()
                        .map(|(size, sha256)| describe_file("PDF", *size, sha256)),
                    None::<String>,
                ),
                Change::new("pdf_status", status, "preparing"),
            ],
        )
        .await?;
        // While the job is locked, so no worker is storing a new PDF.
        if before_pdf.is_some() {
            state
                .storage
                .delete(&state.storage.document_key(id, FileRole::Pdf))
                .await?;
        }
        tx.commit().await?;
        anyhow::Ok(())
    }
    .await;
    saved.map_err(Refusal::internal)?;
    state.pdf.wake();
    Ok(back_to_document(test, id))
}

// ------------------------------------------------------------ + Add files

/// `POST /api/admin/tests/add-files`. The `csrf` field must come first, as
/// the form puts it.
pub async fn add_files(
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

    let mut upload = Upload::default();
    let received = add_test::receive(state, &mut form, &mut upload).await;
    let test = parse_id(upload.field("test"));
    let planned = match received {
        Ok(()) => add_test::planned_documents(state, &upload).await,
        Err(refusal) => Err(refusal),
    };
    let documents = match planned {
        Ok(documents) => documents,
        Err(refusal) => {
            upload.discard(state).await;
            add_test::drain(&mut form).await;
            return Ok(refusal.back_to(back_or_home(test), "Back to the Test").response());
        }
    };
    let publish = upload.field("save") == "publish";

    let saved = async {
        let mut tx = state.db.begin().await?;
        let usable: Option<i64> = match test {
            Some(test) => {
                sqlx::query_scalar("SELECT id FROM tests WHERE id = $1 AND trashed_at IS NULL FOR UPDATE")
                    .bind(test)
                    .fetch_optional(&mut *tx)
                    .await?
            }
            None => None,
        };
        let Some(test) = usable else {
            return anyhow::Ok(None);
        };
        add_test::add_documents(&mut tx, test, &documents, publish).await?;
        tx.commit().await?;
        Ok(Some(test))
    }
    .await;
    match saved {
        Ok(Some(test)) => {
            state.pdf.wake();
            Ok(auth::see_other(&edit_path(test), None))
        }
        Ok(None) => {
            upload.discard(state).await;
            Ok(refuse(
                StatusCode::NOT_FOUND,
                "That Test does not exist or is in the Trash; the files were not added.",
                None,
            )
            .response())
        }
        Err(e) => {
            upload.discard(state).await;
            Err(internal_error(e))
        }
    }
}
