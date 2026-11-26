//! Writing a [`Plan`]: the files into Garage, then the archive tables in one
//! transaction, then removing Garage objects no longer in the archive.
//!
//! The import may run again and again, each time replacing the whole
//! archive, until an Admin changes something: from the first change log
//! entry on it refuses ([`Frozen`]). That is checked before anything is
//! touched, and again inside the transaction, with the change log locked so
//! no Admin write can slip in between.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, bail};
use futures::{StreamExt, TryStreamExt, stream};
use sqlx::{PgPool, Postgres, Transaction};
use tokio_util::io::ReaderStream;

use super::plan::{LAST_LEGACY_DOCUMENT_ID, LAST_LEGACY_TEST_ID, Plan, PlannedDocument};
use crate::server::storage::{FileRole, Storage, Stored};

/// Files streamed into Garage at once.
const CONCURRENT_UPLOADS: usize = 8;

/// The tables the import owns: wiped and rebuilt on every run. The
/// `document_kinds` list is not among them; the import only reads it.
const ARCHIVE_TABLES: &str = "test_documents, document_files, documents, tests, \
     facet_value_aliases, facet_values, pending_uploads";

/// Why the import refuses to run: an Admin has started editing.
#[derive(Debug)]
pub struct Frozen {
    pub entries: i64,
    pub since: String,
}

impl std::fmt::Display for Frozen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "refusing to import: Admins have started editing the archive (the change log has {} {}, \
             the first from {}). Importing again would wipe their changes.",
            self.entries,
            if self.entries == 1 { "entry" } else { "entries" },
            self.since,
        )
    }
}

impl std::error::Error for Frozen {}

/// Fails with [`Frozen`] once the change log has any entry.
pub async fn ensure_not_frozen<'e>(db: impl sqlx::PgExecutor<'e>) -> anyhow::Result<()> {
    let (entries, since): (i64, Option<String>) = sqlx::query_as(
        "SELECT count(*), to_char(min(changed_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI \"UTC\"')
         FROM change_log",
    )
    .fetch_one(db)
    .await
    .context("reading the change log")?;
    if entries > 0 {
        return Err(Frozen {
            entries,
            since: since.unwrap_or_default(),
        }
        .into());
    }
    Ok(())
}

/// Stream every Document's Original into Garage, then read each one back and
/// check its size and SHA-256 against what was read from the source file.
/// Returns what was stored, by Document ID.
pub async fn store_files(storage: &Storage, plan: &Plan) -> anyhow::Result<BTreeMap<i64, Stored>> {
    let total = plan.documents.len();
    let mut done = 0;
    let mut stored = BTreeMap::new();
    let mut uploads = stream::iter(&plan.documents)
        .map(|document| async move { store_file(storage, document).await.map(|s| (document.id, s)) })
        .buffer_unordered(CONCURRENT_UPLOADS);
    while let Some((id, file)) = uploads.try_next().await? {
        stored.insert(id, file);
        done += 1;
        if done % 500 == 0 || done == total {
            eprintln!("famat import: stored {done} of {total} files");
        }
    }
    Ok(stored)
}

async fn store_file(storage: &Storage, document: &PlannedDocument) -> anyhow::Result<Stored> {
    let path = &document.file.path;
    let key = storage.document_key(document.id, FileRole::Original);
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let body = ReaderStream::with_capacity(file, 1024 * 1024);
    let stored = storage
        .put_stream(&key, document.content_type, body, u64::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("storing {}: {e}", path.display()))?;
    if stored.byte_size as u64 != document.file.byte_size {
        bail!(
            "{} changed while it was being imported: it was {} bytes, {} were read",
            path.display(),
            document.file.byte_size,
            stored.byte_size
        );
    }
    match storage.stored(&key).await? {
        Some(in_garage) if in_garage == stored => Ok(stored),
        Some(in_garage) => bail!(
            "Garage's copy of {} does not match it: {} bytes, sha256 {}, but the file has {} bytes, sha256 {}",
            path.display(),
            in_garage.byte_size,
            hex::encode(in_garage.sha256),
            stored.byte_size,
            hex::encode(stored.sha256),
        ),
        None => bail!("{key} is missing from Garage just after storing it"),
    }
}

/// Replace the archive tables with the plan, in one transaction.
pub async fn write(db: &PgPool, plan: &Plan, stored: &BTreeMap<i64, Stored>) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    // SHARE blocks every write to the change log, so no Admin change can
    // commit between this check and the end of the import.
    sqlx::query("LOCK TABLE change_log IN SHARE MODE")
        .execute(&mut *tx)
        .await?;
    ensure_not_frozen(&mut *tx).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "TRUNCATE {ARCHIVE_TABLES} RESTART IDENTITY CASCADE"
    )))
    .execute(&mut *tx)
    .await
    .context("wiping the archive tables")?;

    let value_ids = write_facet_values(&mut tx, plan)
        .await
        .context("writing Facet values")?;
    write_tests(&mut tx, plan, &value_ids).await.context("writing Tests")?;
    write_documents(&mut tx, plan, stored)
        .await
        .context("writing Documents")?;

    // New records are numbered after the imported ones.
    for (table, floor) in [
        ("tests", LAST_LEGACY_TEST_ID),
        ("documents", LAST_LEGACY_DOCUMENT_ID),
        ("facet_values", 1),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT setval(pg_get_serial_sequence('{table}', 'id'), GREATEST((SELECT max(id) FROM {table}), $1))"
        )))
        .bind(floor)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("advancing the {table} IDs"))?;
    }
    tx.commit().await?;
    Ok(())
}

/// Facet values get IDs 1, 2, … in Facet then display order, so a re-run
/// gives every value the same ID. Returns them by (facet, name).
async fn write_facet_values(
    tx: &mut Transaction<'_, Postgres>,
    plan: &Plan,
) -> anyhow::Result<BTreeMap<(&'static str, String), i64>> {
    let (mut ids, mut facets, mut names, mut orders) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut by_name = BTreeMap::new();
    for (facet, values) in &plan.facet_values {
        for (order, name) in values.iter().enumerate() {
            let id = ids.len() as i64 + 1;
            ids.push(id);
            facets.push(facet.as_str());
            names.push(name.as_str());
            orders.push(order as i32 + 1);
            by_name.insert((facet.as_str(), name.clone()), id);
        }
    }
    sqlx::query(
        "INSERT INTO facet_values (id, facet, name, display_order)
         SELECT id, facet::facet, name, display_order
         FROM unnest($1::bigint[], $2::text[], $3::text[], $4::integer[]) AS v (id, facet, name, display_order)",
    )
    .bind(&ids)
    .bind(&facets)
    .bind(&names)
    .bind(&orders)
    .execute(&mut **tx)
    .await?;
    Ok(by_name)
}

async fn write_tests(
    tx: &mut Transaction<'_, Postgres>,
    plan: &Plan,
    value_ids: &BTreeMap<(&'static str, String), i64>,
) -> anyhow::Result<()> {
    use super::plan::Facet;
    let value = |facet: Facet, name: Option<&str>| -> anyhow::Result<Option<i64>> {
        name.map(|name| {
            value_ids
                .get(&(facet.as_str(), name.to_owned()))
                .copied()
                .with_context(|| format!("no {} value {name:?}", facet.label()))
        })
        .transpose()
    };
    let mut columns = TestColumns::default();
    for test in &plan.tests {
        let labels = &test.labels;
        columns.id.push(test.id);
        columns.year.push(labels.year);
        columns.month.push(value(Facet::Month, Facet::Month.of(labels))?);
        columns
            .month_applicable
            .push(labels.month != super::tidy::Month::NotApplicable);
        columns
            .competition
            .push(value(Facet::Competition, Facet::Competition.of(labels))?);
        columns
            .division_level
            .push(value(Facet::DivisionLevel, Facet::DivisionLevel.of(labels))?);
        columns
            .test_type
            .push(value(Facet::TestType, Facet::TestType.of(labels))?);
        columns.host.push(value(Facet::Host, Facet::Host.of(labels))?);
        columns.published.push(test.published);
    }
    sqlx::query(
        "INSERT INTO tests (id, year, month_id, month_applicable, competition_id,
                            division_level_id, test_type_id, host_id, published)
         SELECT * FROM unnest($1::bigint[], $2::smallint[], $3::bigint[], $4::boolean[], $5::bigint[],
                              $6::bigint[], $7::bigint[], $8::bigint[], $9::boolean[])",
    )
    .bind(&columns.id)
    .bind(&columns.year)
    .bind(&columns.month)
    .bind(&columns.month_applicable)
    .bind(&columns.competition)
    .bind(&columns.division_level)
    .bind(&columns.test_type)
    .bind(&columns.host)
    .bind(&columns.published)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Default)]
struct TestColumns {
    id: Vec<i64>,
    year: Vec<Option<i16>>,
    month: Vec<Option<i64>>,
    month_applicable: Vec<bool>,
    competition: Vec<Option<i64>>,
    division_level: Vec<Option<i64>>,
    test_type: Vec<Option<i64>>,
    host: Vec<Option<i64>>,
    published: Vec<bool>,
}

async fn write_documents(
    tx: &mut Transaction<'_, Postgres>,
    plan: &Plan,
    stored: &BTreeMap<i64, Stored>,
) -> anyhow::Result<()> {
    let kinds: Vec<(String, i64)> = sqlx::query_as("SELECT name, id FROM document_kinds")
        .fetch_all(&mut **tx)
        .await?;
    let kinds: BTreeMap<String, i64> = kinds.into_iter().collect();

    let (mut ids, mut kind_ids, mut published, mut needs_review, mut comments) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut sources, mut pdf_statuses) = (Vec::new(), Vec::new());
    let (mut link_tests, mut link_documents) = (Vec::new(), Vec::new());
    let (mut content_types, mut extensions, mut sizes, mut hashes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for document in &plan.documents {
        let kind_id = kinds
            .get(document.kind)
            .with_context(|| format!("there is no Document kind {:?}", document.kind))?;
        let file = stored
            .get(&document.id)
            .with_context(|| format!("D{}'s file was not stored", document.id))?;
        ids.push(document.id);
        kind_ids.push(*kind_id);
        published.push(document.published);
        needs_review.push(document.needs_review.is_some());
        comments.push(document.needs_review.as_deref());
        sources.push(document.label_source.as_str());
        pdf_statuses.push(document.pdf_status);
        for test in &document.tests {
            link_tests.push(*test);
            link_documents.push(document.id);
        }
        content_types.push(document.content_type);
        extensions.push(document.extension.as_str());
        sizes.push(file.byte_size);
        hashes.push(hex::encode(file.sha256));
    }

    sqlx::query(
        "INSERT INTO documents (id, kind_id, published, needs_review, needs_review_comment, label_source, pdf_status)
         SELECT id, kind_id, published, needs_review, comment, source::label_source, pdf_status::pdf_status
         FROM unnest($1::bigint[], $2::bigint[], $3::boolean[], $4::boolean[], $5::text[], $6::text[], $7::text[])
              AS d (id, kind_id, published, needs_review, comment, source, pdf_status)",
    )
    .bind(&ids)
    .bind(&kind_ids)
    .bind(&published)
    .bind(&needs_review)
    .bind(&comments)
    .bind(&sources)
    .bind(&pdf_statuses)
    .execute(&mut **tx)
    .await?;

    sqlx::query("INSERT INTO test_documents (test_id, document_id) SELECT * FROM unnest($1::bigint[], $2::bigint[])")
        .bind(&link_tests)
        .bind(&link_documents)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT INTO document_files (document_id, role, content_type, extension, byte_size, sha256)
         SELECT id, 'original', content_type, extension, byte_size, decode(sha256, 'hex')
         FROM unnest($1::bigint[], $2::text[], $3::text[], $4::bigint[], $5::text[])
              AS f (id, content_type, extension, byte_size, sha256)",
    )
    .bind(&ids)
    .bind(&content_types)
    .bind(&extensions)
    .bind(&sizes)
    .bind(&hashes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete every Document object in Garage that the import did not just
/// write: what a previous run, or an upload since, left behind. Returns how
/// many were deleted.
pub async fn sweep(storage: &Storage, plan: &Plan) -> anyhow::Result<usize> {
    let kept: HashSet<String> = plan
        .documents
        .iter()
        .map(|d| storage.document_key(d.id, FileRole::Original))
        .collect();
    let mut deleted = 0;
    for key in storage.list(&storage.documents_prefix()).await? {
        if !kept.contains(&key) {
            storage.delete(&key).await?;
            deleted += 1;
        }
    }
    Ok(deleted)
}
