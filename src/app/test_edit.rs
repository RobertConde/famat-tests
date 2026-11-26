//! The **Test edit page** (`/admin/tests/{id}/edit`), reached from the
//! **Edit** link an Admin sees on each result row.
//!
//! - **Test:** the six Facet dropdowns (with **+ New value**, as on Add
//!   test), Published/Hidden, and Needs review with its comment.
//! - **Each Document:** kind, label, Published/Hidden and Needs review, its
//!   PDF status and label source, **Retry PDF** / **Upload PDF**
//!   ([`PdfActions`]), **Replace original**, and **Also on Tests…** to attach
//!   it to another Test by ID or detach it.
//! - **+ Add files:** the Add test drop zone and rules, adding Documents to
//!   this Test.
//! - **Merge into…:** move every Document onto another Test, and put this
//!   one in the Trash (`crate::app::bulk_edit`).
//!
//! Every action is its own native form post (CSRF-checked, Admin-only) that
//! answers 303 back to this page, so the page works without JavaScript.
//! The logic is server-only, in `crate::server::test_edit`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, MultipartFormData, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::{
    add_test::{self, FacetField, FacetOptions, FilePicker},
    admin::AdminMenu,
    bulk_edit::MergeInto,
    pdf::{self, PdfActions},
    trash::{RemoveDocument, RemoveTest},
    upload_rules::ACCEPT,
};

/// Everything the Test edit page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestEditPage {
    pub csrf: String,
    /// `None` when there is no such (untrashed) Test.
    pub test: Option<EditedTest>,
    /// Every Document kind as `(id, name)`, in display order.
    pub kinds: Vec<(i64, String)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditedTest {
    pub id: i64,
    /// e.g. "1993 February Invitational · Plant City · Calculus/Mu Individual"
    pub description: String,
    /// The six dropdowns, with the Test's values chosen.
    pub facets: Vec<FacetOptions>,
    pub published: bool,
    pub needs_review: bool,
    pub needs_review_comment: String,
    /// Its untrashed Documents, by ID.
    pub documents: Vec<EditedDocument>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "server", derive(sqlx::FromRow))]
pub struct EditedDocument {
    pub id: i64,
    pub kind_id: i64,
    pub kind: String,
    pub label: Option<String>,
    pub published: bool,
    pub needs_review: bool,
    pub needs_review_comment: Option<String>,
    /// `sheet`, `ai` or `admin`.
    pub label_source: String,
    /// `preparing`, `ready`, `failed` or `not_convertible`.
    pub pdf_status: String,
    /// The Original's extension, without the dot.
    pub original_extension: String,
    pub original_bytes: i64,
    /// The size of its PDF, when it has one apart from the Original.
    pub pdf_bytes: Option<i64>,
    /// Every untrashed Test it belongs to, by ID.
    pub tests: Vec<i64>,
}

// Posted natively by the browser; only the server reads them. Every field
// defaults to empty, so a missing one is refused with a message rather
// than a bare 422.

/// The Test form: the six dropdowns as Add test posts them, plus
/// `visibility` and Needs review.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveTestForm {
    pub csrf: String,
    pub test: String,
    pub year: String,
    pub year_new: String,
    pub month: String,
    pub month_new: String,
    pub competition: String,
    pub competition_new: String,
    pub division_level: String,
    pub division_level_new: String,
    pub test_type: String,
    pub test_type_new: String,
    pub host: String,
    pub host_new: String,
    /// `published` or `hidden`.
    pub visibility: String,
    /// Present (`on`) when ticked.
    pub needs_review: String,
    pub needs_review_comment: String,
}

/// One Document's form.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveDocumentForm {
    pub csrf: String,
    /// The Test whose edit page to come back to.
    pub test: String,
    pub document: String,
    /// A Document kind's ID.
    pub kind: String,
    pub label: String,
    /// `published` or `hidden`.
    pub visibility: String,
    /// Present (`on`) when ticked.
    pub needs_review: String,
    pub needs_review_comment: String,
}

/// **Also on Tests…**: `action` is `attach` (with the Test ID typed in
/// `attach`) or `detach:{test id}`.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentTestsForm {
    pub csrf: String,
    pub test: String,
    pub document: String,
    pub action: String,
    pub attach: String,
}

/// The page's data. An Admin endpoint: anonymous callers get 401. `id` is
/// the Test's ID, the page's path segment.
#[get("/api/admin/test-edit?id",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn test_edit_page(id: i64) -> Result<TestEditPage, ServerFnError> {
    crate::server::test_edit::page(&state, &headers, id).await
}

/// `POST /api/admin/tests/edit`: save the Test's Facet values,
/// Published/Hidden and Needs review. Every write here answers 303 back to
/// the edit page, or a page saying why nothing was saved; 401/403 without a
/// session or token.
#[post("/api/admin/tests/edit",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn save_test(form: Form<SaveTestForm>) -> Result<Response, ServerFnError> {
    crate::server::test_edit::save_test(&state, &headers, form.0).await
}

/// `POST /api/admin/documents/edit`: save a Document's kind, label,
/// Published/Hidden and Needs review; its label source becomes Admin.
#[post("/api/admin/documents/edit",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn save_document(form: Form<SaveDocumentForm>) -> Result<Response, ServerFnError> {
    crate::server::test_edit::save_document(&state, &headers, form.0).await
}

/// `POST /api/admin/documents/tests`: **Also on Tests…** attach or detach.
/// Detaching a Document from its only Test is refused (409).
#[post("/api/admin/documents/tests",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn document_tests(form: Form<DocumentTestsForm>) -> Result<Response, ServerFnError> {
    crate::server::test_edit::document_tests(&state, &headers, form.0).await
}

/// `POST /api/admin/documents/replace-original`: **Replace original**, a
/// native multipart post of `csrf` (first), `test`, `document` and the
/// `original` file. The file replaces the Original and its PDF is made
/// afresh by the queue. 415/413 for a file of the wrong type or over 100 MB.
#[post("/api/admin/documents/replace-original",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
#[middleware(crate::server::upload::UploadLimit::max(crate::server::pdf::admin::MAX_BODY_BYTES))]
pub async fn replace_original(form: MultipartFormData) -> Result<Response, ServerFnError> {
    crate::server::test_edit::replace_original(&state, &headers, form).await
}

/// `POST /api/admin/tests/add-files`: **+ Add files**, a native multipart
/// post like Add test's: `csrf` (first), `test`, the `files`, per-file
/// `kind_{i}` / `label_{i}`, and `save` (`hidden` or `publish`).
#[post("/api/admin/tests/add-files",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
#[middleware(crate::server::upload::UploadLimit::max(crate::app::upload_rules::MAX_UPLOAD_BYTES))]
pub async fn add_files(form: MultipartFormData) -> Result<Response, ServerFnError> {
    crate::server::test_edit::add_files(&state, &headers, form).await
}

const CSS: &str = r#"
.test-edit .description { color: var(--muted); margin: 0 0 12px }
.test-edit textarea { width: 100%; min-height: 3.2em; padding: 5px 8px; border: 1px solid var(--rule); border-radius: 5px;
  font: inherit; font-size: .9rem }
.test-edit .check { display: flex; gap: 6px; align-items: center; font-size: .9rem }
.test-edit section.document { border: 1px solid var(--rule); border-radius: 6px; padding: 10px 16px 12px; margin: 0 0 14px }
.test-edit section.document h4 { margin: 0 0 4px; font-size: 1rem }
.test-edit .facts { margin: 0 0 8px; font-size: .85rem; color: var(--muted) }
.test-edit .doc-form { display: grid; grid-template-columns: max-content minmax(0, 20rem); gap: 6px 12px; align-items: start;
  margin: 0 0 10px }
.test-edit .doc-form > label { text-align: right; font-weight: 600; font-size: .9rem; padding-top: 5px }
.test-edit .doc-form .actions, .test-edit .row-actions { grid-column: 1 / -1 }
.test-edit .row-actions { display: flex; flex-wrap: wrap; gap: 8px 16px; align-items: center; margin: 6px 0 }
.test-edit .row-actions form { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; margin: 0 }
.test-edit .row-actions input[type=file] { font-size: .8rem; max-width: 14rem }
.test-edit .row-actions input[type=text] { width: 7rem }
.test-edit .row-actions button { border: 1px solid var(--accent); border-radius: 5px; padding: 2px 10px;
  background: var(--paper); color: var(--accent); font-weight: 600; cursor: pointer; font-size: .85rem }
.test-edit .row-actions button:disabled { color: var(--faint); border-color: var(--rule); cursor: default }
.test-edit .on-test { font-family: var(--f-mono) }
@media (max-width: 700px) {
  .test-edit .doc-form { grid-template-columns: minmax(0, 1fr) }
  .test-edit .doc-form > label { text-align: left }
}
"#;

/// The page that every write comes back to.
pub fn edit_path(test: i64) -> String {
    format!("/admin/tests/{test}/edit")
}

/// `/admin/tests/{id}/edit`.
#[component]
pub fn EditTest(id: ReadSignal<i64>) -> Element {
    let page = match use_loader(move || test_edit_page(id())) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Edit test" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read();
    let Some(test) = page.test.clone() else {
        return rsx! {
            AdminMenu {}
            h2 { "Edit test" }
            p { id: "no-test", "There is no Test T{id}, or it is in the Trash." }
        };
    };
    let csrf = page.csrf.clone();
    let kind_names: Vec<String> = page.kinds.iter().map(|(_, name)| name.clone()).collect();
    rsx! {
        document::Style { {add_test::CSS} }
        document::Style { {pdf::CSS} }
        document::Style { {CSS} }
        AdminMenu {}
        div { class: "test-edit",
            h2 { "Edit T{test.id}" }
            p { id: "description", class: "description", "{test.description}" }
            TestForm { test: test.clone(), csrf: csrf.clone() }
            RemoveTest { test: test.id, csrf: csrf.clone() }
            h3 { id: "documents-heading", "Documents ({test.documents.len()})" }
            for document in test.documents.iter() {
                DocumentEditor {
                    key: "{document.id}",
                    document: document.clone(),
                    test: test.id,
                    csrf: csrf.clone(),
                    kinds: page.kinds.clone(),
                }
            }
            AddFiles { test: test.id, csrf: csrf.clone(), kinds: kind_names }
            MergeInto { from: test.id, csrf: csrf.clone() }
        }
    }
}

#[component]
fn TestForm(test: EditedTest, csrf: String) -> Element {
    rsx! {
        form {
            id: "edit-test",
            class: "add-test",
            method: "post",
            action: "/api/admin/tests/edit",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "test", value: "{test.id}" }
            fieldset { class: "facets",
                legend { "Test" }
                for facet in test.facets.iter() {
                    FacetField { key: "{facet.name}", facet: facet.clone() }
                }
                div { class: "facet",
                    label { r#for: "t-visibility", "Visibility:" }
                    div { class: "control",
                        Visibility { id: "t-visibility", published: test.published }
                    }
                }
                div { class: "facet",
                    label { r#for: "t-needs-review", "Needs review:" }
                    div { class: "control",
                        NeedsReview {
                            id: "t-needs-review",
                            ticked: test.needs_review,
                            comment: test.needs_review_comment.clone(),
                        }
                    }
                }
            }
            div { class: "actions",
                button { r#type: "submit", "Save test" }
            }
        }
    }
}

/// Published / Hidden, posted as `visibility`.
#[component]
fn Visibility(id: String, published: bool) -> Element {
    rsx! {
        select { id: "{id}", name: "visibility",
            option { value: "published", selected: published, "Published" }
            option { value: "hidden", selected: !published, "Hidden" }
        }
    }
}

/// The Needs review checkbox and its comment. Unticking it clears the
/// comment too.
#[component]
fn NeedsReview(id: String, ticked: bool, comment: String) -> Element {
    rsx! {
        label { class: "check",
            input {
                r#type: "checkbox",
                id: "{id}",
                name: "needs_review",
                value: "on",
                checked: ticked,
            }
            "Needs review"
        }
        // Not a text child: SSR would put hydration comments around it, and
        // a browser reads everything inside a textarea as its text.
        textarea {
            name: "needs_review_comment",
            aria_label: "Needs review comment",
            placeholder: "Why it needs review (cleared when unticked)",
            dangerous_inner_html: escape_text(&comment),
        }
    }
}

/// Text made safe to place inside an element as HTML.
fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn pdf_status_text(document: &EditedDocument) -> &'static str {
    match (document.pdf_status.as_str(), document.pdf_bytes) {
        ("ready", Some(_)) => "PDF ready",
        ("ready", None) => "PDF ready (the Original is a PDF)",
        ("preparing", _) => "PDF being prepared",
        ("failed", _) => "PDF failed",
        _ => "no PDF (not convertible)",
    }
}

fn label_source_text(source: &str) -> &'static str {
    match source {
        "sheet" => "labels from the sheet",
        "ai" => "labels from AI",
        _ => "labels by an Admin",
    }
}

#[component]
fn DocumentEditor(document: EditedDocument, test: i64, csrf: String, kinds: Vec<(i64, String)>) -> Element {
    let id = document.id;
    let return_to = format!("{}#D{id}", edit_path(test));
    let only_test = document.tests.len() <= 1;
    let extension = if document.original_extension.is_empty() {
        "(no extension)".to_owned()
    } else {
        format!(".{}", document.original_extension)
    };
    rsx! {
        section { id: "D{id}", class: "document",
            h4 {
                span { class: "mono", "D{id}" }
                " "
                span { class: "kind", "{document.kind}" }
                if let Some(label) = &document.label {
                    " ("
                    span { class: "label", "{label}" }
                    ")"
                }
            }
            p { class: "facts",
                span { class: "original", "Original {extension}, {document.original_bytes} bytes" }
                " · "
                span { class: "pdf-status", {pdf_status_text(&document)} }
                " · "
                span { class: "visibility", if document.published { "Published" } else { "Hidden" } }
                " · "
                span { class: "label-source", {label_source_text(&document.label_source)} }
            }
            if document.pdf_status == "failed" {
                if let Some(comment) = &document.needs_review_comment {
                    p { class: "refusal pdf-error", "{comment}" }
                }
            }
            form { class: "doc-form save-document", method: "post", action: "/api/admin/documents/edit",
                input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                input { r#type: "hidden", name: "test", value: "{test}" }
                input { r#type: "hidden", name: "document", value: "{id}" }
                label { r#for: "d{id}-kind", "Kind:" }
                select { id: "d{id}-kind", name: "kind",
                    for (kind_id , name) in kinds.iter() {
                        option {
                            key: "{kind_id}",
                            value: "{kind_id}",
                            selected: *kind_id == document.kind_id,
                            "{name}"
                        }
                    }
                }
                label { r#for: "d{id}-label", "Label:" }
                input {
                    id: "d{id}-label",
                    r#type: "text",
                    name: "label",
                    value: document.label.clone().unwrap_or_default(),
                    placeholder: "optional, e.g. 2 per page",
                }
                label { r#for: "d{id}-visibility", "Visibility:" }
                Visibility { id: "d{id}-visibility", published: document.published }
                label { r#for: "d{id}-needs-review", "Needs review:" }
                div {
                    NeedsReview {
                        id: "d{id}-needs-review",
                        ticked: document.needs_review,
                        comment: document.needs_review_comment.clone().unwrap_or_default(),
                    }
                }
                div { class: "row-actions actions",
                    button { r#type: "submit", "Save document" }
                }
            }
            div { class: "row-actions",
                PdfActions { document: id, csrf: csrf.clone(), return_to: return_to.clone() }
            }
            div { class: "row-actions",
                form {
                    class: "replace-original",
                    method: "post",
                    action: "/api/admin/documents/replace-original",
                    enctype: "multipart/form-data",
                    // First, so the server checks it before reading the file.
                    input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                    input { r#type: "hidden", name: "test", value: "{test}" }
                    input { r#type: "hidden", name: "document", value: "{id}" }
                    label {
                        span { class: "sr", "New Original for D{id}" }
                        input { r#type: "file", name: "original", accept: ACCEPT, required: true }
                    }
                    button { r#type: "submit", "Replace original" }
                }
            }
            div { class: "row-actions",
                form { class: "also-on", method: "post", action: "/api/admin/documents/tests",
                    input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                    input { r#type: "hidden", name: "test", value: "{test}" }
                    input { r#type: "hidden", name: "document", value: "{id}" }
                    span { "Also on Tests…" }
                    for on in document.tests.iter() {
                        span { key: "{on}", class: "on-test", id: "D{id}-on-T{on}",
                            if *on == test {
                                "T{on}"
                            } else {
                                a { href: edit_path(*on), "T{on}" }
                            }
                            " "
                            button {
                                r#type: "submit",
                                name: "action",
                                value: "detach:{on}",
                                disabled: only_test,
                                title: if only_test { "A Document must stay on at least one Test" } else { "" },
                                "Detach"
                            }
                        }
                    }
                    input {
                        r#type: "text",
                        name: "attach",
                        aria_label: "Test ID to attach D{id} to",
                        placeholder: "T123",
                    }
                    button { r#type: "submit", name: "action", value: "attach", "Attach" }
                }
            }
            div { class: "row-actions",
                RemoveDocument { document: id, test, csrf: csrf.clone(), shared: !only_test }
            }
        }
    }
}

/// **+ Add files**: Add test's drop zone, adding Documents to this Test.
#[component]
fn AddFiles(test: i64, csrf: String, kinds: Vec<String>) -> Element {
    rsx! {
        form {
            id: "add-files",
            class: "add-test",
            method: "post",
            action: "/api/admin/tests/add-files",
            enctype: "multipart/form-data",
            // First, so the server checks it before reading any file.
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "test", value: "{test}" }
            fieldset {
                legend { "+ Add files" }
                FilePicker { kinds }
            }
            div { class: "actions",
                button { r#type: "submit", name: "save", value: "hidden", "Add hidden" }
                button { r#type: "submit", name: "save", value: "publish", "Add published" }
            }
        }
    }
}
