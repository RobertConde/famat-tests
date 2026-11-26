//! Trash, server side (the page and the Remove buttons are
//! `crate::app::trash`).
//!
//! - **Remove** sets `trashed_at` and nothing else, so **Restore**, which
//!   clears it, brings a Test or Document back exactly as it was, Published
//!   state included. Every public query (search, downloads, zips, old URLs)
//!   and every Admin page already leaves out Trashed Tests and Documents.
//! - **Delete permanently** is only for something in the Trash. It deletes
//!   the rows and the Garage objects (Original and PDF) of every Document it
//!   takes, and logs each record with [`change_log::record_deleted`].
//!
//! Documents shared between Tests:
//! - Removing a Test leaves its Documents untrashed: they are out of sight
//!   through that Test only, and come back with it. One also on an
//!   untrashed Test stays visible there.
//! - Removing a Document takes it off every Test it is on.
//! - Deleting a Test permanently also deletes the Documents that are on no
//!   other Test (Trashed Tests included), Trashed or not. A Document also
//!   on another Test is only taken off this one (its `tests` change is
//!   logged), and lives on there.
//! - Deleting a Document permanently takes it off every Test, Trashed or
//!   not.
//! - Restoring a Document whose Tests are all in the Trash is refused (409):
//!   it would come back out of sight. Restore one of its Tests first.
//!
//! Locks are taken in the order the rest of the app uses, so two writers
//! never deadlock: PDF jobs, then Documents (as the PDF workers do), then
//! Tests (as Also on Tests… does). Deleting a Document locks its PDF job
//! first, so a conversion that is storing its PDF finishes before the
//! objects are deleted, and one still running finds its job gone and throws
//! its result away. The objects are deleted inside the transaction, before
//! it commits: if Garage fails, nothing is deleted from Postgres and the
//! item stays in the Trash to try again.
//!
//! `pending_uploads` holds no foreign keys: its files have Document IDs that
//! are not in `documents` yet. An upload waiting to be added to a Test that
//! is then deleted is refused when the Admin chooses that Test, as it is
//! for a Trashed one, and can still be saved as a new Test or discarded.
//! `pdf_jobs` rows go with their Document (`ON DELETE CASCADE`).

use std::collections::HashMap;

use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};
use dioxus::prelude::ServerFnError;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::{
    AppState,
    add_test::{self, Refusal},
    auth,
    change_log::{self, Change, Entity},
    internal_error,
    storage::FileRole,
};
use crate::app::{
    test_edit::edit_path,
    trash::{DocumentRef, RemoveForm, TestRef, TrashItemForm, TrashPage, TrashedDocument, TrashedItem, TrashedTest},
};

/// The Trash page, which Remove on a Test and every Trash action answer to.
const TRASH: &str = "/admin/trash";

/// The time zone the Admin pages show times in (FAMAT is in Florida).
pub(super) const TIME_ZONE: &str = "America/New_York";

/// A timestamp column as the Admin pages show it, in [`TIME_ZONE`].
pub(super) fn local_time_sql(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE '{TIME_ZONE}', 'YYYY-MM-DD HH24:MI:SS')")
}

/// An ID as a form posts it: `12`, `T12` or `D12`.
fn parse_id(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let digits = raw.strip_prefix(['T', 't', 'D', 'd']).unwrap_or(raw);
    digits.trim().parse().ok().filter(|id| *id > 0)
}

/// A Trash item, `T12` or `D34`.
fn parse_item(raw: &str) -> Option<(Entity, i64)> {
    let raw = raw.trim();
    let entity = match raw.chars().next()? {
        'T' | 't' => Entity::Test,
        'D' | 'd' => Entity::Document,
        _ => return None,
    };
    Some((entity, parse_id(raw)?))
}

fn refuse(status: StatusCode, message: impl Into<String>, back: String, text: &'static str) -> Refusal {
    Refusal::new(status, message).back_to(back, text)
}

fn refuse_to_trash(status: StatusCode, message: impl Into<String>) -> Refusal {
    refuse(status, message, TRASH.to_owned(), "Back to the Trash")
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

/// `Some(trashed)` for an existing row, locked `FOR UPDATE`.
async fn lock_trashed(
    tx: &mut Transaction<'_, Postgres>,
    entity: Entity,
    id: i64,
) -> Result<Option<bool>, sqlx::Error> {
    let sql = match entity {
        Entity::Test => "SELECT trashed_at IS NOT NULL FROM tests WHERE id = $1 FOR UPDATE",
        _ => "SELECT trashed_at IS NOT NULL FROM documents WHERE id = $1 FOR UPDATE",
    };
    sqlx::query_scalar(sql).bind(id).fetch_optional(&mut **tx).await
}

/// Set or clear `trashed_at`, and log it as `trashed`.
async fn set_trashed(
    tx: &mut Transaction<'_, Postgres>,
    entity: Entity,
    id: i64,
    trashed: bool,
) -> Result<(), sqlx::Error> {
    let sql = match (entity, trashed) {
        (Entity::Test, true) => "UPDATE tests SET trashed_at = now() WHERE id = $1",
        (Entity::Test, false) => "UPDATE tests SET trashed_at = NULL WHERE id = $1",
        (_, true) => "UPDATE documents SET trashed_at = now() WHERE id = $1",
        (_, false) => "UPDATE documents SET trashed_at = NULL WHERE id = $1",
    };
    sqlx::query(sql).bind(id).execute(&mut **tx).await?;
    change_log::record(tx, entity, id, [Change::new("trashed", !trashed, trashed)]).await
}

// ------------------------------------------------------------------ Remove

/// `POST /api/admin/tests/remove`.
pub async fn remove_test(state: &AppState, headers: &HeaderMap, form: RemoveForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let Some(id) = parse_id(&form.test) else {
        return Ok(refuse(
            StatusCode::NOT_FOUND,
            "That Test does not exist.",
            "/".to_owned(),
            "Back to search",
        )
        .response());
    };
    write(state, async |tx| {
        match lock_trashed(tx, Entity::Test, id).await? {
            None => {
                return Ok(Err(refuse(
                    StatusCode::NOT_FOUND,
                    format!("There is no Test T{id}."),
                    "/".to_owned(),
                    "Back to search",
                )));
            }
            // Already removed (a second click): nothing to do.
            Some(true) => {}
            Some(false) => set_trashed(tx, Entity::Test, id, true).await?,
        }
        Ok(Ok(format!("{TRASH}#T{id}")))
    })
    .await
}

/// `POST /api/admin/documents/remove`.
pub async fn remove_document(
    state: &AppState,
    headers: &HeaderMap,
    form: RemoveForm,
) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let back = parse_id(&form.test).map_or_else(
        || "/".to_owned(),
        |test| format!("{}#documents-heading", edit_path(test)),
    );
    let Some(id) = parse_id(&form.document) else {
        return Ok(refuse(
            StatusCode::NOT_FOUND,
            "That Document does not exist.",
            back,
            "Back to the Test",
        )
        .response());
    };
    write(state, async |tx| {
        match lock_trashed(tx, Entity::Document, id).await? {
            None => {
                return Ok(Err(refuse(
                    StatusCode::NOT_FOUND,
                    format!("There is no Document D{id}."),
                    back,
                    "Back to the Test",
                )));
            }
            Some(true) => {}
            Some(false) => set_trashed(tx, Entity::Document, id, true).await?,
        }
        Ok(Ok(back))
    })
    .await
}

// ----------------------------------------------------------------- Restore

/// `POST /api/admin/trash/restore`.
pub async fn restore(state: &AppState, headers: &HeaderMap, form: TrashItemForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let Some((entity, id)) = parse_item(&form.item) else {
        return Ok(refuse_to_trash(StatusCode::BAD_REQUEST, "Choose a Test or Document in the Trash.").response());
    };
    let shown = entity.display_id(id);
    write(state, async |tx| {
        match lock_trashed(tx, entity, id).await? {
            None => {
                return Ok(Err(refuse_to_trash(
                    StatusCode::NOT_FOUND,
                    format!("{shown} no longer exists."),
                )));
            }
            Some(false) => return Ok(Ok(TRASH.to_owned())),
            Some(true) => {}
        }
        if entity == Entity::Document {
            let tests: Vec<(i64, bool)> = sqlx::query_as(
                "SELECT t.id, t.trashed_at IS NOT NULL
                 FROM test_documents td JOIN tests t ON t.id = td.test_id
                 WHERE td.document_id = $1
                 ORDER BY t.id
                 FOR SHARE OF t",
            )
            .bind(id)
            .fetch_all(&mut **tx)
            .await?;
            if tests.iter().all(|(_, trashed)| *trashed) {
                let tests: Vec<String> = tests.iter().map(|(t, _)| format!("T{t}")).collect();
                return Ok(Err(refuse_to_trash(
                    StatusCode::CONFLICT,
                    format!(
                        "{shown} would come back out of sight: its Tests ({}) are all in the Trash. \
                         Restore one of them first.",
                        tests.join(", ")
                    ),
                )));
            }
        }
        set_trashed(tx, entity, id, false).await?;
        Ok(Ok(TRASH.to_owned()))
    })
    .await
}

// ------------------------------------------------------ Delete permanently

/// `POST /api/admin/trash/delete`.
pub async fn delete(state: &AppState, headers: &HeaderMap, form: TrashItemForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let Some((entity, id)) = parse_item(&form.item) else {
        return Ok(refuse_to_trash(StatusCode::BAD_REQUEST, "Choose a Test or Document in the Trash.").response());
    };
    let shown = entity.display_id(id);
    let not_trashed = || {
        refuse_to_trash(
            StatusCode::CONFLICT,
            format!("{shown} is not in the Trash. Remove it first; only what is in the Trash can be deleted."),
        )
    };
    let gone = || refuse_to_trash(StatusCode::NOT_FOUND, format!("{shown} no longer exists."));
    write(state, async |tx| {
        match entity {
            Entity::Test => {
                // Its Documents first (jobs, then Documents, then the Test):
                // nothing can attach a Document to a Test in the Trash, so
                // this list can only shrink before the Test is locked.
                let documents: Vec<i64> = sqlx::query_scalar(
                    "SELECT document_id FROM test_documents WHERE test_id = $1 ORDER BY document_id",
                )
                .bind(id)
                .fetch_all(&mut **tx)
                .await?;
                lock_documents(tx, &documents).await?;
                match lock_trashed(tx, Entity::Test, id).await? {
                    None => return Ok(Err(gone())),
                    Some(false) => return Ok(Err(not_trashed())),
                    Some(true) => {}
                }
                delete_test(state, tx, id).await?;
            }
            _ => {
                lock_documents(tx, &[id]).await?;
                match lock_trashed(tx, Entity::Document, id).await? {
                    None => return Ok(Err(gone())),
                    Some(false) => return Ok(Err(not_trashed())),
                    Some(true) => {}
                }
                delete_documents(state, tx, &[id]).await?;
            }
        }
        Ok(Ok(TRASH.to_owned()))
    })
    .await
}

/// Lock the Documents' PDF jobs, then the Documents, in the PDF workers'
/// order.
async fn lock_documents(tx: &mut Transaction<'_, Postgres>, ids: &[i64]) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT id FROM pdf_jobs WHERE document_id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    sqlx::query("SELECT id FROM documents WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The Test as the change log shows a deleted one.
#[derive(FromRow)]
struct DeletedTest {
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

/// Delete a locked, Trashed Test, and the Documents on no other Test.
async fn delete_test(state: &AppState, tx: &mut Transaction<'_, Postgres>, id: i64) -> anyhow::Result<()> {
    let test: DeletedTest = sqlx::query_as(
        "SELECT t.year, t.month_applicable, m.name AS month, c.name AS competition,
                dl.name AS division_level, tt.name AS test_type, h.name AS host,
                t.published, t.needs_review, t.needs_review_comment
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         WHERE t.id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    // Each Document with every Test it is on.
    let documents: Vec<(i64, Vec<i64>)> = sqlx::query_as(
        "SELECT td.document_id, array_agg(o.test_id ORDER BY o.test_id)
         FROM test_documents td
         JOIN test_documents o ON o.document_id = td.document_id
         WHERE td.test_id = $1
         GROUP BY td.document_id
         ORDER BY td.document_id",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let (only_here, shared): (Vec<_>, Vec<_>) = documents.into_iter().partition(|(_, tests)| tests == &[id]);
    let only_here: Vec<i64> = only_here.into_iter().map(|(document, _)| document).collect();
    delete_documents(state, tx, &only_here).await?;
    for (document, before) in shared {
        let after: Vec<i64> = before.iter().copied().filter(|t| *t != id).collect();
        change_log::record(
            tx,
            Entity::Document,
            document,
            [Change::new("tests", test_list(&before), test_list(&after))],
        )
        .await?;
    }
    sqlx::query("DELETE FROM tests WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    let month = if test.month_applicable {
        test.month
    } else {
        Some("Not applicable".to_owned())
    };
    change_log::record_deleted(
        tx,
        Entity::Test,
        id,
        [
            Change::cleared("year", test.year),
            Change::cleared("month", month),
            Change::cleared("competition", test.competition),
            Change::cleared("division_level", test.division_level),
            Change::cleared("test_type", test.test_type),
            Change::cleared("host", test.host),
            Change::cleared("published", test.published),
            Change::cleared("needs_review", test.needs_review),
            Change::cleared("needs_review_comment", test.needs_review_comment),
            Change::cleared("trashed", true),
        ],
    )
    .await?;
    Ok(())
}

fn test_list(tests: &[i64]) -> Option<String> {
    (!tests.is_empty()).then(|| {
        tests
            .iter()
            .map(|t| Entity::Test.display_id(*t))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// The Document as the change log shows a deleted one.
#[derive(FromRow)]
struct DeletedDocument {
    id: i64,
    kind: String,
    label: Option<String>,
    published: bool,
    needs_review: bool,
    needs_review_comment: Option<String>,
    label_source: String,
    pdf_status: String,
    trashed: bool,
    tests: Vec<i64>,
    original_extension: Option<String>,
    original_bytes: Option<i64>,
    original_sha256: Option<Vec<u8>>,
    pdf_bytes: Option<i64>,
    pdf_sha256: Option<Vec<u8>>,
}

fn describe_file(name: &str, bytes: Option<i64>, sha256: Option<&[u8]>) -> Option<String> {
    Some(format!("{name} ({} bytes, sha256 {})", bytes?, hex::encode(sha256?)))
}

/// Delete locked Documents: their rows (with their Test links, files and
/// PDF jobs), their objects, and a change log record for each.
async fn delete_documents(state: &AppState, tx: &mut Transaction<'_, Postgres>, ids: &[i64]) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let documents: Vec<DeletedDocument> = sqlx::query_as(
        "SELECT d.id, k.name AS kind, d.label, d.published, d.needs_review, d.needs_review_comment,
                d.label_source::text, d.pdf_status::text, d.trashed_at IS NOT NULL AS trashed,
                coalesce((SELECT array_agg(td.test_id ORDER BY td.test_id)
                          FROM test_documents td WHERE td.document_id = d.id), '{}') AS tests,
                o.extension AS original_extension, o.byte_size AS original_bytes, o.sha256 AS original_sha256,
                p.byte_size AS pdf_bytes, p.sha256 AS pdf_sha256
         FROM documents d
         JOIN document_kinds k ON k.id = d.kind_id
         LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
         LEFT JOIN document_files p ON p.document_id = d.id AND p.role = 'pdf'
         WHERE d.id = ANY($1)
         ORDER BY d.id",
    )
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM documents WHERE id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    for document in &documents {
        let original = document.original_extension.as_ref().and_then(|extension| {
            describe_file(
                &format!("original.{extension}"),
                document.original_bytes,
                document.original_sha256.as_deref(),
            )
        });
        let pdf = describe_file("PDF", document.pdf_bytes, document.pdf_sha256.as_deref());
        change_log::record_deleted(
            tx,
            Entity::Document,
            document.id,
            [
                Change::cleared("kind", &document.kind),
                Change::cleared("label", &document.label),
                Change::cleared("published", document.published),
                Change::cleared("needs_review", document.needs_review),
                Change::cleared("needs_review_comment", &document.needs_review_comment),
                Change::cleared("label_source", &document.label_source),
                Change::cleared("tests", test_list(&document.tests)),
                Change::cleared("original", original),
                Change::cleared("pdf", pdf),
                Change::cleared("pdf_status", &document.pdf_status),
                Change::cleared("trashed", document.trashed),
            ],
        )
        .await?;
    }
    // Last, so a Garage failure rolls everything back. Deleting an object
    // that is not there succeeds, so trying again finishes the job.
    for document in &documents {
        for role in [FileRole::Original, FileRole::Pdf] {
            state
                .storage
                .delete(&state.storage.document_key(document.id, role))
                .await?;
        }
    }
    Ok(())
}

// -------------------------------------------------------------------- page

pub async fn page(state: &AppState, headers: &HeaderMap) -> Result<TrashPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    let items = items(&state.db).await.map_err(internal_error)?;
    Ok(TrashPage { csrf, items })
}

#[derive(FromRow)]
struct TestRecord {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
    host: Option<String>,
    published: bool,
    trashed: bool,
    removed: Option<String>,
    /// In microseconds, for sorting.
    trashed_at: Option<i64>,
}

impl TestRecord {
    fn description(&self) -> String {
        add_test::describe(
            self.year,
            self.month_applicable.then_some(self.month.as_deref()),
            self.competition.as_deref(),
            self.host.as_deref(),
            self.division_level.as_deref(),
            self.test_type.as_deref(),
        )
    }
}

/// The Tests `ids` (or, with `None`, every Trashed one), with their names.
async fn tests(db: &PgPool, ids: Option<&[i64]>) -> Result<Vec<TestRecord>, sqlx::Error> {
    let sql = format!(
        "SELECT t.id, t.year, t.month_applicable, m.name AS month, c.name AS competition,
                dl.name AS division_level, tt.name AS test_type, h.name AS host,
                t.published, t.trashed_at IS NOT NULL AS trashed,
                {} AS removed,
                (extract(epoch FROM t.trashed_at) * 1000000)::bigint AS trashed_at
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         WHERE CASE WHEN $1::bigint[] IS NULL THEN t.trashed_at IS NOT NULL ELSE t.id = ANY($1) END
         ORDER BY t.id",
        local_time_sql("t.trashed_at")
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(ids).fetch_all(db).await
}

#[derive(FromRow)]
struct DocumentRecord {
    id: i64,
    kind: String,
    label: Option<String>,
    published: bool,
    trashed: bool,
    removed: Option<String>,
    trashed_at: Option<i64>,
    tests: Vec<i64>,
}

async fn items(db: &PgPool) -> Result<Vec<TrashedItem>, sqlx::Error> {
    let trashed_tests = tests(db, None).await?;
    let test_ids: Vec<i64> = trashed_tests.iter().map(|t| t.id).collect();
    // Trashed Documents, and every Document of a Trashed Test.
    let sql = format!(
        "SELECT d.id, k.name AS kind, d.label, d.published, d.trashed_at IS NOT NULL AS trashed,
                {} AS removed,
                (extract(epoch FROM d.trashed_at) * 1000000)::bigint AS trashed_at,
                coalesce((SELECT array_agg(td.test_id ORDER BY td.test_id)
                          FROM test_documents td WHERE td.document_id = d.id), '{{}}') AS tests
         FROM documents d
         JOIN document_kinds k ON k.id = d.kind_id
         WHERE d.trashed_at IS NOT NULL
            OR d.id IN (SELECT document_id FROM test_documents WHERE test_id = ANY($1))
         ORDER BY d.id",
        local_time_sql("d.trashed_at")
    );
    let documents: Vec<DocumentRecord> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(&test_ids)
        .fetch_all(db)
        .await?;
    // Every Test those Documents are on, for their descriptions.
    let mut other: Vec<i64> = documents.iter().flat_map(|d| d.tests.iter().copied()).collect();
    other.sort_unstable();
    other.dedup();
    let all_tests: HashMap<i64, TestRecord> = tests(db, Some(&other)).await?.into_iter().map(|t| (t.id, t)).collect();

    let mut items: Vec<(i64, TrashedItem)> = Vec::new();
    for test in &trashed_tests {
        let documents = documents
            .iter()
            .filter(|d| d.tests.contains(&test.id))
            .map(|d| DocumentRef {
                id: d.id,
                kind: d.kind.clone(),
                label: d.label.clone(),
                trashed: d.trashed,
                shared: d.tests.len() > 1,
            })
            .collect();
        items.push((
            test.trashed_at.unwrap_or_default(),
            TrashedItem::Test(TrashedTest {
                id: test.id,
                removed: test.removed.clone().unwrap_or_default(),
                description: test.description(),
                published: test.published,
                documents,
            }),
        ));
    }
    for document in documents.iter().filter(|d| d.trashed) {
        let tests = document
            .tests
            .iter()
            .filter_map(|id| all_tests.get(id))
            .map(|t| TestRef {
                id: t.id,
                description: t.description(),
                trashed: t.trashed,
            })
            .collect();
        items.push((
            document.trashed_at.unwrap_or_default(),
            TrashedItem::Document(TrashedDocument {
                id: document.id,
                removed: document.removed.clone().unwrap_or_default(),
                kind: document.kind.clone(),
                label: document.label.clone(),
                published: document.published,
                tests,
            }),
        ));
    }
    // Newest first; for one moment, the higher ID first.
    items.sort_by(|(a_at, a), (b_at, b)| b_at.cmp(a_at).then_with(|| item_id(b).cmp(&item_id(a))));
    Ok(items.into_iter().map(|(_, item)| item).collect())
}

fn item_id(item: &TrashedItem) -> i64 {
    match item {
        TrashedItem::Test(test) => test.id,
        TrashedItem::Document(document) => document.id,
    }
}
