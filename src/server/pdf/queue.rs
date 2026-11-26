//! The workers that run the `pdf_jobs` queue.
//!
//! Each of the `PDF_WORKERS` workers takes one job at a time, so no more
//! than that many conversions ever run at once:
//! 1. **Claim** the oldest due job with `FOR UPDATE SKIP LOCKED`, counting
//!    the attempt and taking a lease (a random token, valid for the timeout
//!    plus a margin). A job whose lease ran out (its worker died) is due
//!    again.
//! 2. **Convert** outside any transaction.
//! 3. **Record** the outcome in one transaction that first locks the job and
//!    checks the lease is still this worker's. If the job was queued afresh
//!    (Replace original) or deleted (Upload PDF) meanwhile, the result is
//!    thrown away. The PDF goes into Garage while the job is locked, so an
//!    Admin's Upload PDF, which deletes the job first, always lands last.
//!
//! A failed first attempt is released for its retry; a failed second one
//! marks the Document failed and Needs review.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use dioxus::logger::tracing::{error, info, warn};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use tokio_util::io::ReaderStream;

use super::{
    MAX_ATTEMPTS, Queue, Review,
    convert::{self, Original, Outcome},
};
use crate::server::{
    AppState,
    config::PdfConfig,
    storage::{FileRole, Storage},
};

/// How often an idle worker looks for jobs it was not woken for.
const POLL: Duration = Duration::from_secs(2);
/// How long past the timeout a lease lasts, for fetching the Original and
/// storing the PDF.
const LEASE_MARGIN: Duration = Duration::from_secs(120);

/// Under `dx serve` the router is rebuilt on every hot-patch; the workers
/// must start only once per process.
static STARTED: AtomicBool = AtomicBool::new(false);

struct Worker {
    db: PgPool,
    storage: Storage,
    queue: Queue,
    config: PdfConfig,
}

/// Queue every `preparing` Document that has no job, then start the
/// workers. Call once the database is migrated.
pub fn start(state: &AppState, config: PdfConfig) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let worker = Arc::new(Worker {
        db: state.db.clone(),
        storage: state.storage.clone(),
        queue: state.pdf.clone(),
        config,
    });
    tokio::spawn(async move {
        match super::enqueue_preparing(&worker.db).await {
            Ok(0) => {}
            Ok(n) => info!("queued {n} PDF conversions at startup"),
            Err(e) => error!("queuing PDF conversions at startup: {e}"),
        }
        if worker.config.workers == 0 {
            info!("PDF conversion is off (PDF_WORKERS=0)");
            return;
        }
        for _ in 0..worker.config.workers {
            tokio::spawn(work(worker.clone()));
        }
    });
}

async fn work(worker: Arc<Worker>) {
    loop {
        match claim(&worker).await {
            Ok(Some(job)) => worker.run(job).await,
            Ok(None) => {
                tokio::select! {
                    () = worker.queue.wake.notified() => {}
                    () = tokio::time::sleep(POLL) => {}
                }
            }
            Err(e) => {
                error!("claiming a PDF job: {e}");
                tokio::time::sleep(POLL).await;
            }
        }
    }
}

#[derive(FromRow)]
struct Job {
    id: i64,
    document_id: i64,
    /// Including this one.
    attempts: i16,
    /// The lease token, as text.
    lease: String,
    last_error: Option<String>,
}

async fn claim(worker: &Worker) -> Result<Option<Job>, sqlx::Error> {
    let lease_secs = (worker.config.timeout + LEASE_MARGIN).as_secs_f64();
    sqlx::query_as(
        "UPDATE pdf_jobs
         SET attempts = attempts + 1, lease = gen_random_uuid(),
             leased_until = now() + make_interval(secs => $1)
         WHERE id = (
             SELECT id FROM pdf_jobs
             WHERE run_after <= now() AND (leased_until IS NULL OR leased_until < now())
             ORDER BY run_after, id
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         RETURNING id, document_id, attempts, lease::text, last_error",
    )
    .bind(lease_secs)
    .fetch_optional(&worker.db)
    .await
}

#[derive(FromRow)]
struct DocumentState {
    pdf_status: String,
    extension: Option<String>,
    content_type: Option<String>,
}

impl Worker {
    async fn run(&self, job: Job) {
        if let Err(e) = self.try_run(&job).await {
            // The lease runs out and the job is claimed again.
            error!("PDF job for D{}: {e:#}", job.document_id);
        }
    }

    async fn try_run(&self, job: &Job) -> anyhow::Result<()> {
        if job.attempts > MAX_ATTEMPTS {
            // Its earlier attempts never finished (the server stopped).
            let error = job
                .last_error
                .clone()
                .unwrap_or_else(|| "the conversion was interrupted twice".to_owned());
            return self.fail(job, &error).await;
        }
        let document: Option<DocumentState> = sqlx::query_as(
            "SELECT d.pdf_status::text, o.extension, o.content_type
             FROM documents d
             LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
             WHERE d.id = $1",
        )
        .bind(job.document_id)
        .fetch_optional(&self.db)
        .await?;
        let Some(document) = document.filter(|d| d.pdf_status == "preparing") else {
            // Nothing to do: it is no longer waiting for a PDF.
            return match self.lock(job).await? {
                Some(tx) => done(tx, job).await,
                None => Ok(()),
            };
        };
        let (Some(extension), Some(content_type)) = (document.extension, document.content_type) else {
            return self.failed_attempt(job, "the Document has no Original").await;
        };
        let original = Original {
            extension,
            content_type,
        };

        info!(
            "converting D{} (attempt {} of {MAX_ATTEMPTS})",
            job.document_id, job.attempts
        );
        match convert::convert(&self.config, &self.storage, job.document_id, &original).await {
            Ok(Outcome::Made { pdf, _dir }) => {
                let stored = self.store(job, &pdf).await;
                if let Err(e) = stored {
                    warn!("storing the PDF of D{}: {e:#}", job.document_id);
                    return self.failed_attempt(job, &format!("storing the PDF: {e:#}")).await;
                }
                Ok(())
            }
            Ok(Outcome::OriginalIsPdf) => self.set_status(job, "ready").await,
            Ok(Outcome::NotConvertible) => self.set_status(job, "not_convertible").await,
            Err(error) => {
                warn!("converting D{} failed: {error}", job.document_id);
                self.failed_attempt(job, &error).await
            }
        }
    }

    /// Store the PDF and mark the Document ready.
    async fn store(&self, job: &Job, pdf: &std::path::Path) -> anyhow::Result<()> {
        let file = tokio::fs::File::open(pdf).await?;
        let Some(mut tx) = self.lock(job).await? else {
            return Ok(());
        };
        let key = self.storage.document_key(job.document_id, FileRole::Pdf);
        let stored = self
            .storage
            .put_stream(&key, "application/pdf", ReaderStream::new(file), u64::MAX)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sqlx::query(
            "INSERT INTO document_files (document_id, role, content_type, extension, byte_size, sha256)
             VALUES ($1, 'pdf', 'application/pdf', 'pdf', $2, $3)
             ON CONFLICT (document_id, role) DO UPDATE
             SET content_type = excluded.content_type, extension = excluded.extension,
                 byte_size = excluded.byte_size, sha256 = excluded.sha256, created_at = now()",
        )
        .bind(job.document_id)
        .bind(stored.byte_size)
        .bind(stored.sha256.to_vec())
        .execute(&mut *tx)
        .await?;
        set_outcome(&mut tx, job.document_id, "ready", Review::after_success).await?;
        done(tx, job).await
    }

    async fn set_status(&self, job: &Job, status: &str) -> anyhow::Result<()> {
        let Some(mut tx) = self.lock(job).await? else {
            return Ok(());
        };
        set_outcome(&mut tx, job.document_id, status, Review::after_success).await?;
        done(tx, job).await
    }

    /// A failed attempt: released for its retry, or the Document fails.
    async fn failed_attempt(&self, job: &Job, error: &str) -> anyhow::Result<()> {
        if job.attempts < MAX_ATTEMPTS {
            let released = sqlx::query(
                "UPDATE pdf_jobs SET lease = NULL, leased_until = NULL, last_error = $3, run_after = now()
                 WHERE id = $1 AND lease = $2::uuid",
            )
            .bind(job.id)
            .bind(&job.lease)
            .bind(error)
            .execute(&self.db)
            .await?;
            if released.rows_affected() > 0 {
                self.queue.wake();
            }
            return Ok(());
        }
        self.fail(job, error).await
    }

    async fn fail(&self, job: &Job, error: &str) -> anyhow::Result<()> {
        let Some(mut tx) = self.lock(job).await? else {
            return Ok(());
        };
        set_outcome(&mut tx, job.document_id, "failed", |review| review.after_failure(error)).await?;
        done(tx, job).await
    }

    /// Lock the job in a new transaction, if it is still this worker's. If
    /// it was queued afresh or deleted meanwhile, the result is discarded.
    async fn lock(&self, job: &Job) -> anyhow::Result<Option<Transaction<'static, Postgres>>> {
        let mut tx = self.db.begin().await?;
        let ours: Option<i64> =
            sqlx::query_scalar("SELECT id FROM pdf_jobs WHERE id = $1 AND lease = $2::uuid FOR UPDATE")
                .bind(job.id)
                .bind(&job.lease)
                .fetch_optional(&mut *tx)
                .await?;
        if ours.is_none() {
            info!(
                "D{}: its job changed during the conversion; result discarded",
                job.document_id
            );
            return Ok(None);
        }
        Ok(Some(tx))
    }
}

/// Delete the finished job and commit its outcome.
async fn done(mut tx: Transaction<'static, Postgres>, job: &Job) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM pdf_jobs WHERE id = $1")
        .bind(job.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Set a Document's PDF status, and its Needs review as the outcome has it.
async fn set_outcome(
    tx: &mut Transaction<'_, Postgres>,
    document_id: i64,
    status: &str,
    review: impl FnOnce(&Review) -> Review,
) -> anyhow::Result<()> {
    let (needs_review, comment): (bool, Option<String>) =
        sqlx::query_as("SELECT needs_review, needs_review_comment FROM documents WHERE id = $1 FOR UPDATE")
            .bind(document_id)
            .fetch_one(&mut **tx)
            .await?;
    let after = review(&Review { needs_review, comment });
    sqlx::query(
        "UPDATE documents
         SET pdf_status = $2::pdf_status, needs_review = $3, needs_review_comment = $4, updated_at = now()
         WHERE id = $1",
    )
    .bind(document_id)
    .bind(status)
    .bind(after.needs_review)
    .bind(after.comment)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
