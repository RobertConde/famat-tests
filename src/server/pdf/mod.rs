//! PDF publishing: every Document's Original is kept, and a PDF is made from
//! it in the background (spec: "PDF publishing").
//!
//! - **Queue** ([`queue`]): a Postgres table, `pdf_jobs`, with one job per
//!   `preparing` Document. [`enqueue`] adds one inside the caller's
//!   transaction; call [`Queue::wake`] after committing. At startup every
//!   `preparing` Document without a job gets one ([`enqueue_preparing`]).
//! - **Workers:** `PDF_WORKERS` (2) jobs at once, each with a hard
//!   `PDF_TIMEOUT_SECS` (120 s) timeout that kills the converter's whole
//!   process group, and one retry.
//! - **Converters** ([`convert`]): LibreOffice, headless, with a throwaway
//!   profile per job and `MathTypeToMath` off, for Word, RTF, ODF and
//!   PowerPoint files; a lossless image wrapper ([`image`]); PDFs pass
//!   through; ZIPs are not convertible. A file with no extension has its
//!   type read from its contents ([`sniff`]).
//! - **Outcomes:** a success stores the PDF in Garage (`documents/{id}/pdf`,
//!   its sha256 in `document_files`) and sets `ready`. A failure after the
//!   retry sets `failed` and Needs review, with the error as the comment.
//! - **Admin actions** ([`admin`]): Upload PDF, Retry PDF and Retry all
//!   failed, each a CSRF-checked form post written to the change log.

pub mod admin;
mod convert;
mod image;
mod queue;
pub mod sniff;

use std::sync::Arc;

use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::Notify;

pub use queue::start;

/// A job is tried at most this often: once, plus one retry.
pub const MAX_ATTEMPTS: i16 = 2;

/// How a failed conversion's Needs review comment starts.
pub const FAILURE_PREFIX: &str = "PDF conversion failed: ";

/// Wakes idle workers when a job is queued. Shared through `AppState`.
#[derive(Clone, Default)]
pub struct Queue {
    wake: Arc<Notify>,
}

impl Queue {
    /// Tell idle workers there may be a job. Call it after the transaction
    /// that queued the job commits. Workers also poll, so a missed wake only
    /// delays a job by a moment (and jobs queued by other processes, such as
    /// `famat import`, are found the same way).
    pub fn wake(&self) {
        self.wake.notify_waiters();
        // A worker between polls takes this permit instead.
        self.wake.notify_one();
    }
}

/// Queue a conversion of a Document's Original, inside the caller's
/// transaction. The caller sets `pdf_status = 'preparing'`.
///
/// Queuing a Document that already has a job starts that job afresh: no
/// attempts used, and a conversion already running for it is discarded when
/// it finishes. So Replace original can simply queue again.
pub async fn enqueue(tx: &mut Transaction<'_, Postgres>, document_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO pdf_jobs (document_id) VALUES ($1)
         ON CONFLICT (document_id) DO UPDATE
         SET attempts = 0, run_after = now(), lease = NULL, leased_until = NULL, last_error = NULL",
    )
    .bind(document_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Give every `preparing` Document without a job one. Runs at startup, so a
/// backfill that only set `preparing` (or anything a crash left behind) is
/// converted. Returns how many jobs it queued.
pub async fn enqueue_preparing(db: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "INSERT INTO pdf_jobs (document_id)
         SELECT id FROM documents WHERE pdf_status = 'preparing'
         ORDER BY id
         ON CONFLICT (document_id) DO NOTHING",
    )
    .execute(db)
    .await?
    .rows_affected())
}

/// A Document's Needs review flag and comment, as conversion outcomes
/// change them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub needs_review: bool,
    pub comment: Option<String>,
}

impl Review {
    /// The comment's lines that are not about a failed conversion.
    fn other_lines(&self) -> Option<String> {
        let rest: Vec<&str> = self
            .comment
            .as_deref()
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with(FAILURE_PREFIX) && !line.trim().is_empty())
            .collect();
        (!rest.is_empty()).then(|| rest.join("\n"))
    }

    fn mentions_failure(&self) -> bool {
        self.comment
            .as_deref()
            .is_some_and(|c| c.lines().any(|line| line.starts_with(FAILURE_PREFIX)))
    }

    /// After a PDF is made or uploaded: a Needs review that was only about
    /// a failed conversion is cleared; anything else is kept.
    pub fn after_success(&self) -> Self {
        if !self.mentions_failure() {
            return self.clone();
        }
        let comment = self.other_lines();
        Self {
            needs_review: comment.is_some(),
            comment,
        }
    }

    /// After a conversion fails for good: Needs review, with the error as
    /// the comment (after any other reason already there).
    pub fn after_failure(&self, error: &str) -> Self {
        let line = format!("{FAILURE_PREFIX}{error}");
        Self {
            needs_review: true,
            comment: Some(match self.other_lines() {
                Some(rest) => format!("{rest}\n{line}"),
                None => line,
            }),
        }
    }
}
