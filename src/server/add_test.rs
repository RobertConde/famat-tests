//! Add test, server side (the page is `crate::app::add_test`).
//!
//! An upload goes like this:
//! 1. The session is checked before anything is read, then the `csrf` field,
//!    which the form puts first, before any file is.
//! 2. Each file streams into Garage as it arrives, under a Document ID drawn
//!    from the `documents` sequence: `documents/{id}/original`, or `/pdf` for
//!    the PDF of a Word+PDF pair ([`Pairer`]). A file of the wrong type or
//!    over 100 MB refuses the whole upload, and everything stored so far is
//!    deleted.
//! 3. With every field read, the Facet choices, kinds and labels make a
//!    [`Plan`]. If its six Facet values match an untrashed Test, the plan is
//!    held in `pending_uploads` for the duplicate warning; otherwise it is
//!    saved.
//! 4. Saving ([`commit`]) writes the Test, any new Facet values, the
//!    Documents and their files' rows, and their change log entries, in one
//!    transaction.
//!
//! Documents get `label_source = admin`. A PDF Original, or a Word Original
//! with its PDF supplied, is `ready`; a ZIP is `not_convertible`; any other
//! Original is `preparing`, with a job queued for its conversion
//! (`server::pdf`) in the same transaction.

use std::collections::HashMap;

use axum::{
    extract::multipart::{Field, MultipartError},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use dioxus::{fullstack::MultipartFormData, prelude::ServerFnError};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction, types::Json};

use super::{
    AppState, auth,
    change_log::{self, Change, Entity},
    documents::escape_html,
    facet_values::{self, FacetValueList, LISTED_FACETS},
    internal_error,
    storage::{FileRole, PutError},
    upload,
};
use crate::app::{
    add_test::{AddTestPage, DocumentSummary, FacetOptions, FinishForm, PendingDocument, PendingUpload, TestSummary},
    upload_rules::{self, MAX_FILE_BYTES, Pairer, Placement, guess_kind, size_refusal, split_name},
};

/// Abandoned duplicate warnings are swept, with their files, after this.
const PENDING_DAYS: i32 = 7;

// -------------------------------------------------------------------- plan

/// What one dropdown chose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Choice {
    Unknown,
    /// Month only.
    NotApplicable,
    /// An existing Facet value, by ID.
    Value(i64),
    /// A value to create, typed under **+ New value**.
    New(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Facets {
    pub(super) year: Option<i16>,
    pub(super) month: Choice,
    competition: Choice,
    division_level: Choice,
    test_type: Choice,
    host: Choice,
}

impl Facets {
    pub(super) fn listed(&self) -> [(&'static str, &Choice); 5] {
        [
            ("month", &self.month),
            ("competition", &self.competition),
            ("division_level", &self.division_level),
            ("test_type", &self.test_type),
            ("host", &self.host),
        ]
    }
}

/// A received upload, ready to save: the stored row `pending_uploads.plan`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    facets: Facets,
    /// **Publish** rather than **Save hidden**.
    publish: bool,
    documents: Vec<PlannedDocument>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PlannedDocument {
    /// Drawn from the `documents` sequence; its files are already in Garage.
    id: i64,
    kind_id: i64,
    kind: String,
    label: Option<String>,
    original: StoredFile,
    /// The supplied PDF of a Word+PDF pair.
    pdf: Option<StoredFile>,
}

impl PlannedDocument {
    fn pdf_status(&self) -> &'static str {
        if self.pdf.is_some() || self.original.extension == "pdf" {
            "ready"
        } else if self.original.extension == "zip" {
            "not_convertible"
        } else {
            "preparing"
        }
    }
}

/// A file in Garage, as `document_files` records it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StoredFile {
    /// The name it was uploaded with; kept only in the change log.
    name: String,
    extension: String,
    content_type: String,
    byte_size: i64,
    sha256: String,
}

impl StoredFile {
    /// How the change log shows it.
    pub(super) fn describe(&self) -> String {
        format!("{} ({} bytes, sha256 {})", self.name, self.byte_size, self.sha256)
    }
}

/// The content type an Original of each allowed extension is stored with;
/// what the browser claimed is ignored.
pub(super) fn content_type(extension: &str) -> &'static str {
    match extension {
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rtf" => "application/rtf",
        "odt" => "application/vnd.oasis.opendocument.text",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

// ------------------------------------------------------------------ upload

/// A refusal shown to the Admin as a page, after which nothing was saved.
/// Shared with the Test edit page (`server::test_edit`), which points its
/// link back there.
pub(super) struct Refusal {
    status: StatusCode,
    message: String,
    /// The page to go back to, and the link's text.
    back: (String, &'static str),
}

impl Refusal {
    pub(super) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            back: ("/admin/tests/new".to_owned(), "Back to Add test"),
        }
    }

    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// Link back to `href` instead of Add test.
    pub(super) fn back_to(mut self, href: String, text: &'static str) -> Self {
        self.back = (href, text);
        self
    }

    pub(super) fn internal(e: impl std::fmt::Display) -> Self {
        dioxus::logger::tracing::error!("add test: {e:#}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong on the server. Try again.",
        )
    }

    pub(super) fn multipart(e: MultipartError) -> Self {
        if upload::over_limit(&e) {
            Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "That is too much to upload at once: add at most 1 GB of files at a time.",
            )
        } else {
            Self::bad_request("The upload was cut off. Try again.")
        }
    }

    pub(super) fn response(&self) -> Response {
        let message = escape_html(&self.message);
        let (back, back_text) = (escape_html(&self.back.0), self.back.1);
        let page = format!(
            "<!DOCTYPE html>\
<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Not saved · FAMAT Test Bank</title>\
<style>body{{font:16px/1.4 system-ui,sans-serif;margin:0;color:#1a1a1a;background:#fff}}\
main{{max-width:64rem;margin:0 auto;padding:1rem}}#refusal{{color:#b00020}}</style></head>\
<body><main><h1>Nothing was saved</h1>\
<p id=\"refusal\">{message}</p>\
<p><a href=\"{back}\">{back_text}</a></p>\
</main></body></html>"
        );
        (self.status, [(header::CACHE_CONTROL, "no-store")], Html(page)).into_response()
    }
}

/// A Document whose files are arriving.
struct Received {
    id: i64,
    /// The index of the file that is its Original, whose `kind_{i}` and
    /// `label_{i}` apply.
    original_index: usize,
    original: Option<StoredFile>,
    pdf: Option<StoredFile>,
}

#[derive(Default)]
pub(super) struct Upload {
    /// Text fields; the first of each name wins.
    fields: HashMap<String, String>,
    /// Files seen so far, including refused ones.
    files: usize,
    pairer: Pairer,
    documents: Vec<Received>,
}

/// The text fields of a posted form, trimmed; empty when absent.
pub(super) trait Fields {
    fn field(&self, name: &str) -> &str;
}

impl Fields for HashMap<String, String> {
    fn field(&self, name: &str) -> &str {
        self.get(name).map_or("", |v| v.trim())
    }
}

impl Fields for Upload {
    fn field(&self, name: &str) -> &str {
        self.fields.field(name)
    }
}

impl Upload {
    async fn receive_file(&mut self, state: &AppState, file_name: String, field: Field<'_>) -> Result<(), Refusal> {
        let index = self.files;
        self.files += 1;
        if let Some(message) = upload_rules::refusal(&file_name, None) {
            return Err(Refusal::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, message));
        }
        let (_, extension) = split_name(&file_name);
        let content_type = content_type(&extension);
        let (document, role) = match self.pairer.place(&file_name) {
            Placement::New { document } => {
                let id = next_document_id(&state.db).await.map_err(Refusal::internal)?;
                self.documents.push(Received {
                    id,
                    original_index: index,
                    original: None,
                    pdf: None,
                });
                (document, FileRole::Original)
            }
            Placement::PdfFor { document } => (document, FileRole::Pdf),
            Placement::WordFor { document } => {
                // The PDF that came first becomes this Document's PDF.
                let received = &mut self.documents[document];
                let storage = &state.storage;
                storage
                    .copy(
                        &storage.document_key(received.id, FileRole::Original),
                        &storage.document_key(received.id, FileRole::Pdf),
                    )
                    .await
                    .map_err(Refusal::internal)?;
                received.pdf = received.original.take();
                received.original_index = index;
                (document, FileRole::Original)
            }
        };

        let id = self.documents[document].id;
        let key = state.storage.document_key(id, role);
        let stored = state
            .storage
            .put_stream(&key, content_type, field, MAX_FILE_BYTES)
            .await
            .map_err(|e| match e {
                PutError::TooLarge => Refusal::new(StatusCode::PAYLOAD_TOO_LARGE, size_refusal(&file_name)),
                PutError::Body(e) => Refusal::multipart(e),
                PutError::Storage(e) => Refusal::internal(e),
            })?;
        let file = StoredFile {
            name: file_name,
            extension,
            content_type: content_type.to_owned(),
            byte_size: stored.byte_size,
            sha256: hex::encode(stored.sha256),
        };
        match role {
            FileRole::Original => self.documents[document].original = Some(file),
            FileRole::Pdf => self.documents[document].pdf = Some(file),
        }
        Ok(())
    }

    /// Delete every file stored so far.
    pub(super) async fn discard(&self, state: &AppState) {
        for document in &self.documents {
            state.storage.delete_document_files(document.id).await;
        }
    }
}

/// `POST /api/admin/tests`.
pub async fn upload(
    state: &AppState,
    headers: &HeaderMap,
    mut form: MultipartFormData,
) -> Result<Response, ServerFnError> {
    // Anonymous callers are refused before any of the body is used.
    auth::require_admin(state, headers).await?;
    let csrf = match form.next_field().await {
        Ok(Some(field)) if field.name() == Some("csrf") => field.text().await.unwrap_or_default(),
        _ => String::new(),
    };
    auth::require_admin_form(state, headers, &csrf).await?;
    sweep_abandoned(state).await;

    let mut upload = Upload::default();
    let planned = match receive(state, &mut form, &mut upload).await {
        Ok(()) => plan(state, &upload).await,
        Err(refusal) => Err(refusal),
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => {
            // The rest of the body is read before the refusal goes out
            // (`server::upload`).
            upload.discard(state).await;
            return Ok(refusal.response());
        }
    };

    match save_or_hold(state, &plan).await {
        Ok(location) => Ok(auth::see_other(&location, None)),
        Err(e) => {
            for document in &plan.documents {
                state.storage.delete_document_files(document.id).await;
            }
            Err(internal_error(e))
        }
    }
}

/// Read every field after the CSRF token, storing files as they come.
pub(super) async fn receive(
    state: &AppState,
    form: &mut MultipartFormData,
    upload: &mut Upload,
) -> Result<(), Refusal> {
    loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => return Ok(()),
            Err(e) => return Err(Refusal::multipart(e)),
        };
        let name = field.name().unwrap_or_default().to_owned();
        match field.file_name().map(str::to_owned) {
            // A file input with nothing chosen sends one empty, nameless file.
            Some(file_name) if name == "files" && file_name.is_empty() => {}
            Some(file_name) if name == "files" => upload.receive_file(state, file_name, field).await?,
            Some(_) => {}
            None => {
                let value = field.text().await.map_err(Refusal::multipart)?;
                upload.fields.entry(name).or_insert(value);
            }
        }
    }
}

/// The Facet choices, kinds and labels of a fully received upload.
async fn plan(state: &AppState, upload: &Upload) -> Result<Plan, Refusal> {
    let documents = planned_documents(state, upload).await?;
    let values = FacetValueList::load(&state.db).await.map_err(Refusal::internal)?;
    Ok(Plan {
        facets: facet_choices(&values, upload)?,
        publish: upload.field("save") == "publish",
        documents,
    })
}

/// The six dropdowns' choices in a posted form.
pub(super) fn facet_choices(values: &FacetValueList, fields: &impl Fields) -> Result<Facets, Refusal> {
    Ok(Facets {
        year: year_choice(fields)?,
        month: choice(values, fields, "month", "Month")?,
        competition: choice(values, fields, "competition", "Competition")?,
        division_level: choice(values, fields, "division_level", "Division/Level")?,
        test_type: choice(values, fields, "test_type", "Test type")?,
        host: choice(values, fields, "host", "Host")?,
    })
}

/// The Documents of a fully received upload, with their kinds and labels.
pub(super) async fn planned_documents(state: &AppState, upload: &Upload) -> Result<Vec<PlannedDocument>, Refusal> {
    if upload.documents.is_empty() {
        return Err(Refusal::bad_request("Choose at least one file to add."));
    }
    let kinds = document_kinds(&state.db).await.map_err(Refusal::internal)?;
    let mut documents = Vec::with_capacity(upload.documents.len());
    for received in &upload.documents {
        let (Some(original), pdf) = (received.original.clone(), received.pdf.clone()) else {
            return Err(Refusal::internal(format!("Document {} has no Original", received.id)));
        };
        let i = received.original_index;
        let chosen = upload.field(&format!("kind_{i}"));
        let (kind_id, kind) = if chosen.is_empty() {
            let guess = guess_kind(&original.name);
            kinds
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(guess))
                .or(kinds.first())
                .cloned()
                .ok_or_else(|| Refusal::internal("there are no Document kinds"))?
        } else {
            kinds
                .iter()
                .find(|(_, name)| name == chosen)
                .cloned()
                .ok_or_else(|| Refusal::bad_request(format!("There is no Document kind “{chosen}”.")))?
        };
        let label = Some(upload.field(&format!("label_{i}")))
            .filter(|label| !label.is_empty())
            .map(str::to_owned);
        documents.push(PlannedDocument {
            id: received.id,
            kind_id,
            kind,
            label,
            original,
            pdf,
        });
    }
    Ok(documents)
}

pub(super) fn year_choice(fields: &impl Fields) -> Result<Option<i16>, Refusal> {
    let typed = fields.field("year_new");
    let raw = if typed.is_empty() { fields.field("year") } else { typed };
    match raw {
        "" => Err(Refusal::bad_request("Choose the Year, or Unknown.")),
        "unknown" => Ok(None),
        "new" => Err(Refusal::bad_request("Type the new Year.")),
        raw => match raw.parse::<i16>() {
            Ok(year) if (1900..=2100).contains(&year) => Ok(Some(year)),
            _ => Err(Refusal::bad_request("The Year must be a number from 1900 to 2100.")),
        },
    }
}

/// The dropdown `facet`'s choice. Text under **+ New value** wins over the
/// dropdown, and names an existing value (or alias, ignoring case) when
/// there is one.
pub(super) fn choice(
    values: &FacetValueList,
    fields: &impl Fields,
    facet: &str,
    label: &str,
) -> Result<Choice, Refusal> {
    let typed = fields.field(&format!("{facet}_new"));
    if !typed.is_empty() {
        return Ok(values
            .find(facet, typed)
            .map_or_else(|| Choice::New(typed.to_owned()), Choice::Value));
    }
    match fields.field(facet) {
        "" => Err(Refusal::bad_request(format!("Choose the {label}, or Unknown."))),
        "unknown" => Ok(Choice::Unknown),
        "not_applicable" if facet == "month" => Ok(Choice::NotApplicable),
        "new" => Err(Refusal::bad_request(format!("Type the new {label}."))),
        raw => raw
            .parse::<i64>()
            .ok()
            .filter(|id| values.name(facet, *id).is_some())
            .map(Choice::Value)
            .ok_or_else(|| {
                Refusal::bad_request(format!("That {label} no longer exists. Reload the page and try again."))
            }),
    }
}

/// `(id, name)` of every Document kind, in display order.
pub(super) async fn document_kinds(db: &PgPool) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as("SELECT id, name FROM document_kinds ORDER BY display_order, name")
        .fetch_all(db)
        .await
}

async fn next_document_id(db: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT nextval(pg_get_serial_sequence('documents', 'id'))")
        .fetch_one(db)
        .await
}

/// Save the plan as a new Test, or hold it if its Test has a duplicate.
/// Returns where to send the Admin.
async fn save_or_hold(state: &AppState, plan: &Plan) -> anyhow::Result<String> {
    if !duplicates(&state.db, &plan.facets).await?.is_empty() {
        let id: i64 = sqlx::query_scalar("INSERT INTO pending_uploads (plan) VALUES ($1) RETURNING id")
            .bind(Json(plan))
            .fetch_one(&state.db)
            .await?;
        return Ok(format!("/admin/tests/new?pending={id}"));
    }
    let mut tx = state.db.begin().await?;
    let saved = commit(&mut tx, plan, Target::NewTest).await?;
    tx.commit().await?;
    state.pdf.wake();
    Ok(saved.location())
}

/// The untrashed Tests with exactly these six Facet values. A value still
/// to be created matches none.
async fn duplicates(db: &PgPool, facets: &Facets) -> Result<Vec<i64>, sqlx::Error> {
    let mut ids = Vec::new();
    for (_, choice) in facets.listed() {
        match choice {
            Choice::New(_) => return Ok(Vec::new()),
            Choice::Value(id) => ids.push(Some(*id)),
            Choice::Unknown | Choice::NotApplicable => ids.push(None),
        }
    }
    sqlx::query_scalar(
        "SELECT id FROM tests
         WHERE trashed_at IS NULL
           AND year IS NOT DISTINCT FROM $1
           AND month_applicable = $2
           AND month_id IS NOT DISTINCT FROM $3
           AND competition_id IS NOT DISTINCT FROM $4
           AND division_level_id IS NOT DISTINCT FROM $5
           AND test_type_id IS NOT DISTINCT FROM $6
           AND host_id IS NOT DISTINCT FROM $7
         ORDER BY id",
    )
    .bind(facets.year)
    .bind(facets.month != Choice::NotApplicable)
    .bind(ids[0])
    .bind(ids[1])
    .bind(ids[2])
    .bind(ids[3])
    .bind(ids[4])
    .fetch_all(db)
    .await
}

// ------------------------------------------------------------------ saving

enum Target {
    NewTest,
    Existing(i64),
}

struct Saved {
    test_id: i64,
    /// **Publish** was pressed, but the Test fails the publish rule, so it
    /// was saved hidden.
    publish_refused: bool,
}

impl Saved {
    fn location(&self) -> String {
        let notice = if self.publish_refused { "&notice=hidden" } else { "" };
        format!("/admin/tests/new?added={}{notice}", self.test_id)
    }
}

/// A Facet value as saved: its ID and, for the change log, its name.
pub(super) struct Resolved {
    pub(super) id: Option<i64>,
    pub(super) name: Option<String>,
}

/// Write the plan into the transaction, with its change log entries.
///
/// - A new Test is Published only if **Publish** was pressed and it passes
///   the publish rule (Year or Competition known; its Documents all have a
///   kind). Its Documents are Published, so publishing the Test later shows
///   them.
/// - Files added to an existing Test leave the Test as it is; the Documents
///   are Published or Hidden as the Admin chose.
async fn commit(tx: &mut Transaction<'_, Postgres>, plan: &Plan, target: Target) -> anyhow::Result<Saved> {
    let facets = &plan.facets;
    let mut resolved = Vec::with_capacity(5);
    for (facet, choice) in facets.listed() {
        resolved.push(resolve(tx, facet, choice).await?);
    }
    let [month, competition, division_level, test_type, host] =
        <[Resolved; 5]>::try_from(resolved).map_err(|_| anyhow::anyhow!("five listed Facets"))?;

    let (test_id, publish_refused, documents_published) = match target {
        Target::NewTest => {
            let publishable = facets.year.is_some() || competition.id.is_some();
            let published = plan.publish && publishable;
            let month_applicable = facets.month != Choice::NotApplicable;
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO tests (year, month_id, month_applicable, competition_id,
                                    division_level_id, test_type_id, host_id, published)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 RETURNING id",
            )
            .bind(facets.year)
            .bind(month.id)
            .bind(month_applicable)
            .bind(competition.id)
            .bind(division_level.id)
            .bind(test_type.id)
            .bind(host.id)
            .bind(published)
            .fetch_one(&mut **tx)
            .await?;
            let month_name = if month_applicable {
                month.name
            } else {
                Some("Not applicable".to_owned())
            };
            change_log::record_created(
                tx,
                Entity::Test,
                id,
                [
                    Change::set("year", facets.year),
                    Change::set("month", month_name),
                    Change::set("competition", competition.name),
                    Change::set("division_level", division_level.name),
                    Change::set("test_type", test_type.name),
                    Change::set("host", host.name),
                    Change::set("published", published),
                ],
            )
            .await?;
            (id, plan.publish && !publishable, true)
        }
        Target::Existing(id) => (id, false, plan.publish),
    };
    add_documents(tx, test_id, &plan.documents, documents_published).await?;
    Ok(Saved {
        test_id,
        publish_refused,
    })
}

/// Write Documents whose files are already in Garage, attached to one
/// Test, with their change log entries, queuing the conversions they need.
/// The caller wakes the PDF queue after committing.
pub(super) async fn add_documents(
    tx: &mut Transaction<'_, Postgres>,
    test_id: i64,
    documents: &[PlannedDocument],
    documents_published: bool,
) -> anyhow::Result<()> {
    for document in documents {
        let pdf_status = document.pdf_status();
        sqlx::query(
            "INSERT INTO documents (id, kind_id, label, published, label_source, pdf_status)
             VALUES ($1, $2, $3, $4, 'admin', $5::pdf_status)",
        )
        .bind(document.id)
        .bind(document.kind_id)
        .bind(&document.label)
        .bind(documents_published)
        .bind(pdf_status)
        .execute(&mut **tx)
        .await?;
        if pdf_status == "preparing" {
            super::pdf::enqueue(tx, document.id).await?;
        }
        sqlx::query("INSERT INTO test_documents (test_id, document_id) VALUES ($1, $2)")
            .bind(test_id)
            .bind(document.id)
            .execute(&mut **tx)
            .await?;
        let files = std::iter::once((FileRole::Original, &document.original))
            .chain(document.pdf.iter().map(|pdf| (FileRole::Pdf, pdf)));
        for (role, file) in files {
            sqlx::query(
                "INSERT INTO document_files (document_id, role, content_type, extension, byte_size, sha256)
                 VALUES ($1, $2::file_role, $3, $4, $5, $6)",
            )
            .bind(document.id)
            .bind(role.as_str())
            .bind(&file.content_type)
            .bind(&file.extension)
            .bind(file.byte_size)
            .bind(hex::decode(&file.sha256)?)
            .execute(&mut **tx)
            .await?;
        }
        change_log::record_created(
            tx,
            Entity::Document,
            document.id,
            [
                Change::set("tests", Entity::Test.display_id(test_id)),
                Change::set("kind", &document.kind),
                Change::set("label", &document.label),
                Change::set("published", documents_published),
                Change::set("label_source", "admin"),
                Change::set("pdf_status", pdf_status),
                Change::set("original", document.original.describe()),
                Change::set("pdf", document.pdf.as_ref().map(StoredFile::describe)),
            ],
        )
        .await?;
    }
    Ok(())
}

/// The ID and name of a Facet choice, creating (and logging) a new value at
/// the end of its Facet's display order.
pub(super) async fn resolve(
    tx: &mut Transaction<'_, Postgres>,
    facet: &str,
    choice: &Choice,
) -> anyhow::Result<Resolved> {
    let id = match choice {
        Choice::Unknown | Choice::NotApplicable => return Ok(Resolved { id: None, name: None }),
        Choice::Value(id) => *id,
        Choice::New(name) => {
            // It may have been created since the form was read.
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM facet_values WHERE facet = $1::facet AND lower(name) = lower($2)
                 UNION ALL
                 SELECT facet_value_id FROM facet_value_aliases WHERE facet = $1::facet AND lower(alias) = lower($2)
                 LIMIT 1",
            )
            .bind(facet)
            .bind(name)
            .fetch_optional(&mut **tx)
            .await?;
            match existing {
                Some(id) => id,
                None => facet_values::create(tx, facet, name).await?,
            }
        }
    };
    let name: String = sqlx::query_scalar("SELECT name FROM facet_values WHERE facet = $1::facet AND id = $2")
        .bind(facet)
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Resolved {
        id: Some(id),
        name: Some(name),
    })
}

/// Point every held upload that chose Facet value `from` at `to` instead,
/// for a merge. A held plan's choice is `{"Value": id}` (see [`Choice`]).
pub async fn replace_held_value(
    tx: &mut Transaction<'_, Postgres>,
    facet: &str,
    from: i64,
    to: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE pending_uploads
         SET plan = jsonb_set(plan, ARRAY['facets', $1], jsonb_build_object('Value', $3::bigint))
         WHERE plan -> 'facets' -> $1 = jsonb_build_object('Value', $2::bigint)",
    )
    .bind(facet)
    .bind(from)
    .bind(to)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Whether a held upload chose Facet value `id`, so it can't be deleted.
pub async fn held_uses(tx: &mut Transaction<'_, Postgres>, facet: &str, id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pending_uploads
                        WHERE plan -> 'facets' -> $1 = jsonb_build_object('Value', $2::bigint))",
    )
    .bind(facet)
    .bind(id)
    .fetch_one(&mut **tx)
    .await
}

// --------------------------------------------------------- the duplicate guard

/// `POST /api/admin/uploads/finish`: **Add these files to that Test
/// instead** (`add:{id}`), **Save as a new Test anyway** (`new`) or
/// **Discard these files** (`discard`).
pub async fn finish(state: &AppState, headers: &HeaderMap, form: FinishForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let Ok(pending) = form.pending.parse::<i64>() else {
        return Ok(Refusal::bad_request("That upload does not exist.").response());
    };
    let target = match form.action.as_str() {
        "new" => Target::NewTest,
        "discard" => return discard(state, pending).await,
        action => match action.strip_prefix("add:").and_then(|id| id.parse().ok()) {
            Some(id) => Target::Existing(id),
            None => return Ok(Refusal::bad_request("Choose one of the buttons.").response()),
        },
    };
    finish_into(state, pending, target).await.map_err(internal_error)
}

async fn finish_into(state: &AppState, pending: i64, target: Target) -> anyhow::Result<Response> {
    let mut tx = state.db.begin().await?;
    // Taking the row makes a second click (or a second tab) a no-op.
    let plan: Option<Json<Plan>> = sqlx::query_scalar("DELETE FROM pending_uploads WHERE id = $1 RETURNING plan")
        .bind(pending)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(Json(plan)) = plan else {
        return Ok(already_finished());
    };
    if let Target::Existing(id) = target {
        let usable: Option<bool> =
            sqlx::query_scalar("SELECT true FROM tests WHERE id = $1 AND trashed_at IS NULL FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if usable.is_none() {
            // Rolling back keeps the upload waiting.
            return Ok(Refusal::new(
                StatusCode::CONFLICT,
                format!("T{id} is in the Trash or gone. Go back and choose again."),
            )
            .response());
        }
    }
    let saved = commit(&mut tx, &plan, target).await?;
    tx.commit().await?;
    state.pdf.wake();
    Ok(auth::see_other(&saved.location(), None))
}

async fn discard(state: &AppState, pending: i64) -> Result<Response, ServerFnError> {
    let plan: Option<Json<Plan>> = sqlx::query_scalar("DELETE FROM pending_uploads WHERE id = $1 RETURNING plan")
        .bind(pending)
        .fetch_optional(&state.db)
        .await
        .map_err(internal_error)?;
    let Some(Json(plan)) = plan else {
        return Ok(already_finished());
    };
    for document in &plan.documents {
        state.storage.delete_document_files(document.id).await;
    }
    Ok(auth::see_other("/admin/tests/new", None))
}

fn already_finished() -> Response {
    Refusal::new(StatusCode::CONFLICT, "These files were already saved or discarded.").response()
}

/// Delete duplicate warnings nobody answered within [`PENDING_DAYS`], and
/// their files. Failures are only logged; the next upload tries again.
async fn sweep_abandoned(state: &AppState) {
    let plans: Result<Vec<Json<Plan>>, _> = sqlx::query_scalar(
        "DELETE FROM pending_uploads WHERE created_at < now() - make_interval(days => $1) RETURNING plan",
    )
    .bind(PENDING_DAYS)
    .fetch_all(&state.db)
    .await;
    match plans {
        Ok(plans) => {
            for Json(plan) in plans {
                for document in &plan.documents {
                    state.storage.delete_document_files(document.id).await;
                }
            }
        }
        Err(e) => dioxus::logger::tracing::warn!("sweeping abandoned uploads: {e}"),
    }
}

// -------------------------------------------------------------------- page

/// The Add test page's data. `added` and `pending` are the page's query
/// parameters; anything that is not a known ID shows nothing.
pub async fn page(
    state: &AppState,
    headers: &HeaderMap,
    added: &str,
    pending: &str,
) -> Result<AddTestPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    page_data(&state.db, csrf, added, pending).await.map_err(internal_error)
}

/// The six Facet dropdowns, in order, with nothing chosen: every Year in use
/// and each listed Facet's values.
pub(super) async fn facet_options(db: &PgPool, values: &FacetValueList) -> Result<Vec<FacetOptions>, sqlx::Error> {
    let years: Vec<i16> = sqlx::query_scalar(
        "SELECT DISTINCT year FROM tests WHERE trashed_at IS NULL AND year IS NOT NULL ORDER BY year DESC",
    )
    .fetch_all(db)
    .await?;
    let mut facets = vec![FacetOptions {
        name: "year".to_owned(),
        label: "Year".to_owned(),
        options: years.iter().map(|y| (y.to_string(), y.to_string())).collect(),
        not_applicable: false,
        selected: String::new(),
    }];
    for (facet, label) in LISTED_FACETS {
        facets.push(FacetOptions {
            name: facet.to_owned(),
            label: label.to_owned(),
            options: values
                .values
                .iter()
                .filter(|(f, _, _)| f == facet)
                .map(|(_, id, name)| (id.to_string(), name.clone()))
                .collect(),
            not_applicable: facet == "month",
            selected: String::new(),
        });
    }
    Ok(facets)
}

async fn page_data(db: &PgPool, csrf: String, added: &str, pending: &str) -> anyhow::Result<AddTestPage> {
    let values = FacetValueList::load(db).await?;
    let facets = facet_options(db, &values).await?;
    let kinds = document_kinds(db).await?.into_iter().map(|(_, name)| name).collect();

    let added = match added.parse::<i64>() {
        Ok(id) => summary(db, id).await?,
        Err(_) => None,
    };
    let pending = match pending.parse::<i64>() {
        Ok(id) => pending_upload(db, &values, id).await?,
        Err(_) => None,
    };
    Ok(AddTestPage {
        csrf,
        facets,
        kinds,
        added,
        pending,
    })
}

#[derive(FromRow)]
struct TestRecord {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    host: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
    published: bool,
}

/// A Test and its untrashed Documents, or `None` if there is no such Test.
async fn summary(db: &PgPool, id: i64) -> Result<Option<TestSummary>, sqlx::Error> {
    let test: Option<TestRecord> = sqlx::query_as(
        "SELECT t.id, t.year, t.month_applicable, t.published,
                m.name AS month, c.name AS competition, h.name AS host,
                dl.name AS division_level, tt.name AS test_type
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id
         WHERE t.id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(test) = test else {
        return Ok(None);
    };
    let documents: Vec<DocumentSummary> = sqlx::query_as(
        "SELECT d.id, k.name AS kind, d.label, d.published, d.pdf_status::text, d.label_source::text,
                coalesce(o.extension, '') AS original_extension,
                coalesce(o.byte_size, 0) AS original_bytes,
                p.byte_size AS pdf_bytes
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
    Ok(Some(TestSummary {
        id: test.id,
        description: describe(
            test.year,
            if test.month_applicable {
                Some(test.month.as_deref())
            } else {
                None
            },
            test.competition.as_deref(),
            test.host.as_deref(),
            test.division_level.as_deref(),
            test.test_type.as_deref(),
        ),
        published: test.published,
        documents,
    }))
}

/// "1993 February Invitational · Plant City · Calculus/Mu Individual", with
/// a dash for each unknown value. `month` is `None` when not applicable.
pub(super) fn describe(
    year: Option<i16>,
    month: Option<Option<&str>>,
    competition: Option<&str>,
    host: Option<&str>,
    division_level: Option<&str>,
    test_type: Option<&str>,
) -> String {
    let dash = |value: Option<&str>| value.unwrap_or("—").to_owned();
    let mut event = vec![year.map_or_else(|| "—".to_owned(), |y| y.to_string())];
    if let Some(month) = month {
        event.push(dash(month));
    }
    event.push(dash(competition));
    format!(
        "{} · {} · {} {}",
        event.join(" "),
        dash(host),
        dash(division_level),
        dash(test_type)
    )
}

async fn pending_upload(db: &PgPool, values: &FacetValueList, id: i64) -> anyhow::Result<Option<PendingUpload>> {
    let plan: Option<Json<Plan>> = sqlx::query_scalar("SELECT plan FROM pending_uploads WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    let Some(Json(plan)) = plan else {
        return Ok(None);
    };
    let name = |choice: &Choice| match choice {
        Choice::Value(id) => values
            .values
            .iter()
            .find(|(_, v, _)| v == id)
            .map(|(_, _, name)| name.clone()),
        Choice::New(name) => Some(name.clone()),
        Choice::Unknown | Choice::NotApplicable => None,
    };
    let facets = &plan.facets;
    let month = name(&facets.month);
    let description = describe(
        facets.year,
        (facets.month != Choice::NotApplicable).then_some(month.as_deref()),
        name(&facets.competition).as_deref(),
        name(&facets.host).as_deref(),
        name(&facets.division_level).as_deref(),
        name(&facets.test_type).as_deref(),
    );
    let mut matches = Vec::new();
    for test in duplicates(db, facets).await? {
        matches.extend(summary(db, test).await?);
    }
    Ok(Some(PendingUpload {
        id,
        description,
        publish: plan.publish,
        documents: plan
            .documents
            .iter()
            .map(|d| PendingDocument {
                original_name: d.original.name.clone(),
                pdf_name: d.pdf.as_ref().map(|pdf| pdf.name.clone()),
                kind: d.kind.clone(),
                label: d.label.clone(),
            })
            .collect(),
        matches,
    }))
}
