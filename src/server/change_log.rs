//! The change log: every Admin write records what changed, before and after,
//! and when (spec: "Change log entry"). There is no "who", since everyone
//! shares one passcode.
//!
//! Entries are written inside the transaction that makes the change, so the
//! log and the data can never disagree: every function here takes the open
//! [`Transaction`], never a pool.
//!
//! ```ignore
//! let mut tx = state.db.begin().await?;
//! sqlx::query("UPDATE tests SET host_id = $2 WHERE id = $1")…;
//! change_log::record(&mut tx, Entity::Test, id, [Change::new("host", "Chiles", "Lawton Chiles")]).await?;
//! tx.commit().await?;
//! ```
//!
//! Conventions, so the Change log page can read every ticket's entries alike:
//! - One entry per field. `field` is a column-like name (`year`, `host`,
//!   `kind`, `label`, `published`, `pdf_status`, `original`, `tests`…).
//! - Values are what an Admin reads: Facet value and Document kind *names*,
//!   `true`/`false`, `T12, T40` for a Document's Tests. `None` is unknown or
//!   absent. Unchanged fields (before == after) are skipped.
//! - A created record has an `id` entry with no before ([`record_created`]);
//!   a permanently deleted one has an `id` entry with no after
//!   ([`record_deleted`]).

use sqlx::{Postgres, Transaction};

/// What kind of record changed.
#[allow(dead_code, reason = "DocumentKind is for the Document kinds page (#29)")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    Test,
    Document,
    FacetValue,
    DocumentKind,
}

impl Entity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Document => "document",
            Self::FacetValue => "facet_value",
            Self::DocumentKind => "document_kind",
        }
    }

    /// How an Admin reads the record's ID: `T12`, `D345`, or the bare number.
    pub fn display_id(self, id: i64) -> String {
        match self {
            Self::Test => format!("T{id}"),
            Self::Document => format!("D{id}"),
            Self::FacetValue | Self::DocumentKind => id.to_string(),
        }
    }
}

/// One field's change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub field: &'static str,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Change {
    pub fn new(field: &'static str, before: impl LogValue, after: impl LogValue) -> Self {
        Self {
            field,
            before: before.log_value(),
            after: after.log_value(),
        }
    }

    /// A field of a record being created.
    pub fn set(field: &'static str, after: impl LogValue) -> Self {
        Self::new(field, None::<String>, after)
    }

    /// A field of a record being deleted.
    pub fn cleared(field: &'static str, before: impl LogValue) -> Self {
        Self::new(field, before, None::<String>)
    }
}

/// A value as the change log shows it. `None` is unknown or absent.
pub trait LogValue {
    fn log_value(&self) -> Option<String>;
}

impl LogValue for str {
    fn log_value(&self) -> Option<String> {
        Some(self.to_owned())
    }
}

impl LogValue for String {
    fn log_value(&self) -> Option<String> {
        Some(self.clone())
    }
}

impl LogValue for bool {
    fn log_value(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl LogValue for i16 {
    fn log_value(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl LogValue for i32 {
    fn log_value(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl LogValue for i64 {
    fn log_value(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl<T: LogValue> LogValue for Option<T> {
    fn log_value(&self) -> Option<String> {
        self.as_ref().and_then(LogValue::log_value)
    }
}

impl<T: LogValue + ?Sized> LogValue for &T {
    fn log_value(&self) -> Option<String> {
        (**self).log_value()
    }
}

/// Record changes to one record, skipping fields whose value did not change.
pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    entity: Entity,
    entity_id: i64,
    changes: impl IntoIterator<Item = Change>,
) -> Result<(), sqlx::Error> {
    let (mut fields, mut befores, mut afters) = (Vec::new(), Vec::new(), Vec::new());
    for change in changes.into_iter().filter(|c| c.before != c.after) {
        fields.push(change.field);
        befores.push(change.before);
        afters.push(change.after);
    }
    if fields.is_empty() {
        return Ok(());
    }
    // WITH ORDINALITY keeps the entries' ids in the order given.
    sqlx::query(
        "INSERT INTO change_log (entity, entity_id, field, before, after)
         SELECT $1, $2, field, before, after
         FROM unnest($3::text[], $4::text[], $5::text[]) WITH ORDINALITY AS c (field, before, after, n)
         ORDER BY n",
    )
    .bind(entity.as_str())
    .bind(entity_id)
    .bind(&fields)
    .bind(&befores)
    .bind(&afters)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Record a new record: an `id` entry, then each field with no before.
/// Give fields with [`Change::set`].
pub async fn record_created(
    tx: &mut Transaction<'_, Postgres>,
    entity: Entity,
    entity_id: i64,
    fields: impl IntoIterator<Item = Change>,
) -> Result<(), sqlx::Error> {
    let id = Change::set("id", entity.display_id(entity_id));
    record(tx, entity, entity_id, std::iter::once(id).chain(fields)).await
}

/// Record a permanently deleted record: an `id` entry, then each field with
/// no after. Give fields with [`Change::cleared`].
pub async fn record_deleted(
    tx: &mut Transaction<'_, Postgres>,
    entity: Entity,
    entity_id: i64,
    fields: impl IntoIterator<Item = Change>,
) -> Result<(), sqlx::Error> {
    let id = Change::cleared("id", entity.display_id(entity_id));
    record(tx, entity, entity_id, std::iter::once(id).chain(fields)).await
}
