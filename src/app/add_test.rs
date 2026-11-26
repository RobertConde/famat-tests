//! **Add test** (`/admin/tests/new`): the six Facet dropdowns, each with
//! **+ New value**, a multi-file drop zone, and **Save hidden** / **Publish**.
//!
//! The form is one native multipart post, so it works without JavaScript:
//! the drop zone is the file input itself, and each file's Document kind is
//! guessed from its name on the server. Once the page has hydrated, choosing
//! files lists them with their guessed kinds (changeable) and a label field
//! each, posted as `kind_{i}` and `label_{i}` for the i-th file.
//!
//! When the Facet values match an existing Test, the files are held and the
//! page (`?pending=`) offers **Add these files to that Test instead**. After
//! saving, the page (`?added=`) shows the Test and its Documents.
//!
//! The logic is server-only, in `crate::server::add_test`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, MultipartFormData, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::admin::AdminMenu;
use super::upload_rules::{ACCEPT, ALLOWED_TYPES, Pairer, Placement, guess_kind, refusal};

/// Everything the Add test page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AddTestPage {
    pub csrf: String,
    /// The six dropdowns, in order: Year, Month, Competition, Division/Level,
    /// Test type, Host.
    pub facets: Vec<FacetOptions>,
    /// Document kind names, in display order.
    pub kinds: Vec<String>,
    /// The Test just saved (`?added=`).
    pub added: Option<TestSummary>,
    /// Files held because their Test matched an existing one (`?pending=`).
    pub pending: Option<PendingUpload>,
}

/// One Facet dropdown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacetOptions {
    /// The form field: `year`, `month`, `competition`, `division_level`,
    /// `test_type` or `host`. The new value's text field is `{name}_new`.
    pub name: String,
    pub label: String,
    /// `(value, text)` in display order: a year, or a Facet value's ID.
    pub options: Vec<(String, String)>,
    /// Month only: offer **Not applicable** (State, National, Mail-in).
    pub not_applicable: bool,
    /// The option chosen when the page loads: a value as in `options`,
    /// `unknown` or `not_applicable`; empty for "Choose…".
    pub selected: String,
}

/// A Test and its Documents, as an Admin checks them after saving.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestSummary {
    pub id: i64,
    /// e.g. "1993 February Invitational · Plant City · Calculus/Mu Individual"
    pub description: String,
    pub published: bool,
    pub documents: Vec<DocumentSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "server", derive(sqlx::FromRow))]
pub struct DocumentSummary {
    pub id: i64,
    pub kind: String,
    pub label: Option<String>,
    pub published: bool,
    /// `preparing`, `ready`, `failed` or `not_convertible`.
    pub pdf_status: String,
    /// `sheet`, `ai` or `admin`.
    pub label_source: String,
    /// The Original's extension, without the dot.
    pub original_extension: String,
    pub original_bytes: i64,
    /// The size of its PDF, when it has one apart from the Original.
    pub pdf_bytes: Option<i64>,
}

/// An upload waiting for the Admin's decision about a duplicate Test.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingUpload {
    pub id: i64,
    /// The Facet values chosen, described like [`TestSummary::description`].
    pub description: String,
    /// Whether **Publish** (rather than **Save hidden**) was pressed.
    pub publish: bool,
    pub documents: Vec<PendingDocument>,
    /// The untrashed Tests with the same six Facet values.
    pub matches: Vec<TestSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingDocument {
    pub original_name: String,
    /// The supplied PDF of a Word+PDF pair.
    pub pdf_name: Option<String>,
    pub kind: String,
    pub label: Option<String>,
}

/// The duplicate warning's choice. Posted natively; only the server reads it.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct FinishForm {
    pub csrf: String,
    pub pending: String,
    /// `add:{test id}`, `new` or `discard`.
    pub action: String,
}

/// The Add test page's data. An Admin endpoint: anonymous callers get 401.
/// `added` and `pending` are the page's query parameters; anything that is
/// not a known ID is ignored.
#[get("/api/admin/add-test?added&pending",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn add_test_page(added: String, pending: String) -> Result<AddTestPage, ServerFnError> {
    crate::server::add_test::page(&state, &headers, &added, &pending).await
}

/// `POST /api/admin/tests`: the Add test form, a native multipart post. The
/// `csrf` field must come first, as the form puts it. Answers 303 to the
/// saved Test's summary or to the duplicate warning, 401/403 without a
/// session or token, 415/413 for a file of the wrong type or over 100 MB.
#[post("/api/admin/tests",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
#[middleware(crate::server::upload::UploadLimit::max(crate::app::upload_rules::MAX_UPLOAD_BYTES))]
pub async fn add_test(form: MultipartFormData) -> Result<Response, ServerFnError> {
    crate::server::add_test::upload(&state, &headers, form).await
}

/// `POST /api/admin/uploads/finish`: the duplicate warning's buttons.
#[post("/api/admin/uploads/finish",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn finish_upload(form: Form<FinishForm>) -> Result<Response, ServerFnError> {
    crate::server::add_test::finish(&state, &headers, form.0).await
}

pub const CSS: &str = r#"
.add-test fieldset { border: 1px solid var(--rule); border-radius: 6px; padding: 12px 16px 14px; margin: 0 0 14px }
.add-test legend { font-weight: 700; padding-inline: 4px }
.add-test .facets { display: grid; grid-template-columns: max-content minmax(0, 20rem); gap: 8px 12px; align-items: start }
.add-test .facet { display: contents }
.add-test .facet > label { text-align: right; font-weight: 600; font-size: .9rem; padding-top: 6px }
.add-test .facet .control { display: grid; gap: 4px }
.add-test select, .add-test input[type=text] { width: 100%; padding: 5px 8px; border: 1px solid var(--rule);
  border-radius: 5px; background: var(--paper); font-size: .9rem }
.add-test .new-value { display: none }
.add-test .facet:has(option[value=new]:checked) .new-value { display: block }
.add-test .dropzone-input { display: block; width: 100%; padding: 32px 16px; border: 2px dashed var(--faint);
  border-radius: 8px; background: var(--head); cursor: pointer; font-size: .9rem }
.add-test .dropzone-input:hover, .add-test .dropzone-input:focus-visible { border-color: var(--accent) }
.add-test .hint { color: var(--muted); font-size: .85rem; margin: 6px 0 0 }
.add-test table { margin-top: 10px }
.add-test td.refused, .refusal { color: #b00020 }
.add-test .actions { display: flex; gap: 12px; justify-content: flex-end }
.add-test .actions button, .finish button { border: 1px solid var(--accent); border-radius: 5px; padding: 5px 16px;
  font-weight: 600; cursor: pointer; background: var(--paper); color: var(--accent) }
.add-test .actions button[value=publish], .finish button.primary { background: var(--accent); color: var(--paper) }
.summary, .warning { border: 1px solid var(--rule); border-radius: 6px; padding: 10px 16px; margin: 0 0 16px }
.warning { border-color: #d9a400; background: #fffbea }
.summary ul, .warning ul { margin: 6px 0; padding-left: 20px }
.finish { display: flex; flex-wrap: wrap; gap: 8px 12px; margin: 10px 0 4px }
.muted { color: var(--muted) }
@media (max-width: 700px) {
  .add-test .facets { grid-template-columns: minmax(0, 1fr) }
  .add-test .facet > label { text-align: left }
}
"#;

/// `/admin/tests/new`, with `?added=` after saving and `?pending=` when the
/// Facet values matched an existing Test. `notice=hidden` says a Publish was
/// saved hidden because the Test fails the publish rule.
#[component]
pub fn AddTest(added: ReadSignal<String>, pending: ReadSignal<String>, notice: ReadSignal<String>) -> Element {
    let page = match use_loader(move || add_test_page(added(), pending())) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Add test" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read();
    rsx! {
        document::Style { {CSS} }
        AdminMenu {}
        h2 { "Add test" }
        if let Some(test) = &page.added {
            AddedTest { test: test.clone(), publish_refused: notice() == "hidden" }
        }
        if let Some(upload) = &page.pending {
            DuplicateWarning { upload: upload.clone(), csrf: page.csrf.clone() }
        } else {
            AddTestForm { csrf: page.csrf.clone(), facets: page.facets.clone(), kinds: page.kinds.clone() }
        }
    }
}

#[component]
fn AddTestForm(csrf: String, facets: Vec<FacetOptions>, kinds: Vec<String>) -> Element {
    rsx! {
        form {
            id: "add-test",
            class: "add-test",
            method: "post",
            action: "/api/admin/tests",
            enctype: "multipart/form-data",
            // First, so the server checks it before reading any file.
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            fieldset { class: "facets",
                legend { "Test" }
                for facet in facets {
                    FacetField { key: "{facet.name}", facet }
                }
            }
            fieldset {
                legend { "Files" }
                FilePicker { kinds }
            }
            div { class: "actions",
                // The first button is the one Enter presses: the safe one.
                button { r#type: "submit", name: "save", value: "hidden", "Save hidden" }
                button { r#type: "submit", name: "save", value: "publish", "Publish" }
            }
        }
    }
}

/// One dropdown: the values, Unknown, (Not applicable,) and **+ New value**,
/// whose text field shows when it is picked. Shared with the Test edit page,
/// which preselects the Test's values; its parent needs the `add-test` class.
#[component]
pub(crate) fn FacetField(facet: FacetOptions) -> Element {
    let id = format!("f-{}", facet.name);
    let new_name = format!("{}_new", facet.name);
    let new_label = format!("New {}", facet.label);
    rsx! {
        div { class: "facet",
            label { r#for: "{id}", "{facet.label}:" }
            div { class: "control",
                select { id: "{id}", name: "{facet.name}", required: true,
                    option { value: "", disabled: true, selected: facet.selected.is_empty(), "Choose…" }
                    for (value, text) in facet.options.iter() {
                        option { key: "{value}", value: "{value}", selected: *value == facet.selected, "{text}" }
                    }
                    option { value: "unknown", selected: facet.selected == "unknown", "Unknown" }
                    if facet.not_applicable {
                        option { value: "not_applicable", selected: facet.selected == "not_applicable", "Not applicable" }
                    }
                    option { value: "new", "+ New value…" }
                }
                input {
                    class: "new-value",
                    r#type: "text",
                    name: "{new_name}",
                    aria_label: "{new_label}",
                    placeholder: "{new_label}",
                }
            }
        }
    }
}

/// A file chosen in the browser, once the page has hydrated.
#[derive(Clone, Debug, PartialEq)]
struct Picked {
    name: String,
    size: u64,
}

/// What the preview shows for one chosen file.
#[derive(Clone, Debug, PartialEq)]
enum Preview {
    /// It becomes a Document's Original, with this guessed kind.
    Original(&'static str),
    /// It is the supplied PDF of the named Word file.
    PdfOf(String),
    /// It cannot be added.
    Refused(String),
}

/// The same placement the server makes, for the files in their order.
fn preview(files: &[Picked]) -> Vec<Preview> {
    let mut pairer = Pairer::default();
    // Per Document so far, the index of the file that is its Original.
    let mut originals: Vec<usize> = Vec::new();
    let mut rows = Vec::with_capacity(files.len());
    for (i, file) in files.iter().enumerate() {
        if let Some(message) = refusal(&file.name, Some(file.size)) {
            rows.push(Preview::Refused(message));
            continue;
        }
        match pairer.place(&file.name) {
            Placement::New { .. } => {
                originals.push(i);
                rows.push(Preview::Original(guess_kind(&file.name)));
            }
            Placement::PdfFor { document } => {
                rows.push(Preview::PdfOf(files[originals[document]].name.clone()));
            }
            Placement::WordFor { document } => {
                rows[originals[document]] = Preview::PdfOf(file.name.clone());
                originals[document] = i;
                rows.push(Preview::Original(guess_kind(&file.name)));
            }
        }
    }
    rows
}

/// The drop zone: the native file input, styled as a large target (dropping
/// files onto a file input works without JavaScript). After hydration it
/// lists the chosen files with a kind and a label field for each. Shared
/// with the Test edit page's **+ Add files**.
#[component]
pub(crate) fn FilePicker(kinds: Vec<String>) -> Element {
    let mut files = use_signal(Vec::<Picked>::new);
    let picked = files.read().clone();
    let rows = preview(&picked);
    let refused = rows.iter().any(|row| matches!(row, Preview::Refused(_)));
    rsx! {
        input {
            id: "files",
            class: "dropzone-input",
            r#type: "file",
            name: "files",
            multiple: true,
            required: true,
            accept: ACCEPT,
            aria_label: "Files",
            onchange: move |event: FormEvent| {
                files.set(
                    event.files().iter().map(|f| Picked { name: f.name(), size: f.size() }).collect(),
                );
            },
        }
        p { class: "hint",
            "Drop the Test’s files here, or click to choose them: {ALLOWED_TYPES}, up to 100 MB each. "
            "Each file’s kind is guessed from its name (T_, S_, A_, C_, E_, “solutions”, “key”…). "
            "A Word file and a PDF with the same name become one Document."
        }
        if !picked.is_empty() {
            table { id: "picked",
                thead {
                    tr {
                        th { "File" }
                        th { "Kind" }
                        th { "Label (optional)" }
                    }
                }
                tbody {
                    for (i , (file , row)) in picked.iter().zip(rows).enumerate() {
                        tr { key: "{i}-{file.name}",
                            td { "{file.name}" }
                            match row {
                                Preview::Original(guess) => rsx! {
                                    td {
                                        select { name: "kind_{i}", aria_label: "Kind of {file.name}",
                                            for kind in kinds.iter() {
                                                option { key: "{kind}", value: "{kind}", selected: kind == guess, "{kind}" }
                                            }
                                        }
                                    }
                                    td {
                                        input {
                                            r#type: "text",
                                            name: "label_{i}",
                                            aria_label: "Label of {file.name}",
                                            placeholder: "e.g. 2 per page",
                                        }
                                    }
                                },
                                Preview::PdfOf(word) => rsx! {
                                    td { colspan: 2, class: "muted", "The PDF of {word}" }
                                },
                                Preview::Refused(message) => rsx! {
                                    td { colspan: 2, class: "refused", "{message}" }
                                },
                            }
                        }
                    }
                }
            }
            if refused {
                p { class: "refusal", "Choose the files again without the ones that can’t be added." }
            }
        }
    }
}

#[component]
fn AddedTest(test: TestSummary, publish_refused: bool) -> Element {
    rsx! {
        section { id: "added", class: "summary",
            if publish_refused {
                p { id: "notice", class: "refusal",
                    "Saved hidden: a Test needs its Year or Competition known to be published."
                }
            }
            h3 { "Saved T{test.id}" }
            TestDetails { test }
        }
    }
}

/// A Test's description, visibility and Documents.
#[component]
fn TestDetails(test: TestSummary) -> Element {
    rsx! {
        p { class: "test",
            span { class: "description", "{test.description}" }
            " · "
            span { class: "visibility", if test.published { "Published" } else { "Hidden" } }
        }
        ul { class: "documents",
            for document in test.documents.iter() {
                DocumentLine { key: "{document.id}", document: document.clone() }
            }
        }
    }
}

#[component]
fn DocumentLine(document: DocumentSummary) -> Element {
    let pdf = match (document.pdf_status.as_str(), document.pdf_bytes) {
        ("ready", Some(_)) => "PDF supplied",
        ("ready", None) => "PDF ready",
        ("preparing", _) => "PDF being prepared",
        ("failed", _) => "PDF failed",
        _ => "no PDF (not convertible)",
    };
    let source = match document.label_source.as_str() {
        "sheet" => "labels from the sheet",
        "ai" => "labels from AI",
        _ => "labels by an Admin",
    };
    rsx! {
        li { id: "D{document.id}", class: "document",
            span { class: "mono", "D{document.id}" }
            " "
            span { class: "kind", "{document.kind}" }
            if let Some(label) = &document.label {
                " ("
                span { class: "label", "{label}" }
                ")"
            }
            " · "
            span { class: "original", ".{document.original_extension}, {document.original_bytes} bytes" }
            " · "
            span { class: "pdf-status", "{pdf}" }
            " · "
            span { class: "visibility", if document.published { "Published" } else { "Hidden" } }
            " · "
            span { class: "label-source", "{source}" }
        }
    }
}

/// The duplicate-Test guard.
#[component]
fn DuplicateWarning(upload: PendingUpload, csrf: String) -> Element {
    let several = upload.matches.len() > 1;
    rsx! {
        section { id: "duplicate", class: "warning",
            if upload.matches.is_empty() {
                p { id: "duplicate-intro", "The Test these files were held for no longer matches another one." }
            } else if several {
                p { id: "duplicate-intro", "These Tests already have the Facet values {upload.description}:" }
            } else {
                p { id: "duplicate-intro", "A Test already has the Facet values {upload.description}:" }
            }
            for test in upload.matches.iter() {
                div { key: "{test.id}", class: "match", id: "match-T{test.id}",
                    h3 { "T{test.id}" }
                    TestDetails { test: test.clone() }
                }
            }
            p { "The files waiting to be saved ("
                if upload.publish { "to publish" } else { "to save hidden" }
                "):"
            }
            ul { class: "held",
                for document in upload.documents.iter() {
                    li {
                        "{document.original_name}"
                        if let Some(pdf) = &document.pdf_name { " + {pdf}" }
                        " → {document.kind}"
                        if let Some(label) = &document.label { " ({label})" }
                    }
                }
            }
            form { id: "finish", class: "finish", method: "post", action: "/api/admin/uploads/finish",
                input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                input { r#type: "hidden", name: "pending", value: "{upload.id}" }
                for test in upload.matches.iter() {
                    button {
                        key: "{test.id}",
                        class: "primary",
                        r#type: "submit",
                        name: "action",
                        value: "add:{test.id}",
                        if several {
                            "Add these files to T{test.id} instead"
                        } else {
                            "Add these files to that Test instead"
                        }
                    }
                }
                button { r#type: "submit", name: "action", value: "new", "Save as a new Test anyway" }
                button { r#type: "submit", name: "action", value: "discard", "Discard these files" }
            }
        }
    }
}
