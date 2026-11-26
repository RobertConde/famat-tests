//! PDF problems (`/admin/needs-review`, linked from the search page's
//! Needs review filter): Documents whose PDF failed
//! or is still being prepared, with **Upload PDF**, **Retry PDF** and
//! **Retry all failed**.
//!
//! The actions are native form posts, so they work without JavaScript, and
//! come back to `return_to`. [`PdfActions`] and [`RetryAllFailed`] are the
//! forms on their own, for the Test edit page to embed.
//!
//! The logic is server-only, in `crate::server::pdf::admin`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, MultipartFormData, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::admin::AdminMenu;

/// Where the actions return to when no (usable) `return_to` is posted.
pub const PDF_PROBLEMS: &str = "/admin/needs-review";

/// Everything the PDF problems page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PdfProblemsPage {
    pub csrf: String,
    /// Untrashed Documents whose PDF failed, by ID.
    pub failed: Vec<PdfDocument>,
    /// The first [`PdfProblemsPage::preparing_total`] of the Documents whose
    /// PDF is being prepared, by ID.
    pub preparing: Vec<PdfDocument>,
    pub preparing_total: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PdfDocument {
    pub id: i64,
    /// The Tests it belongs to.
    pub tests: Vec<i64>,
    pub kind: String,
    pub label: Option<String>,
    /// The Original's extension, without the dot; empty when it has none.
    pub original_extension: String,
    /// `failed` or `preparing`.
    pub pdf_status: String,
    /// Its Needs review comment, which says why a conversion failed.
    pub comment: Option<String>,
    /// Where its job is: "Queued", "Converting (attempt 1 of 2)"…
    pub job: Option<String>,
}

/// **Retry PDF**. Posted natively; only the server reads it.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct RetryPdfForm {
    pub csrf: String,
    pub document: String,
    #[serde(default)]
    pub return_to: String,
}

/// **Retry all failed**. Posted natively; only the server reads it.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct RetryFailedForm {
    pub csrf: String,
    #[serde(default)]
    pub return_to: String,
}

/// The PDF problems page's data. An Admin endpoint: anonymous callers get 401.
#[get("/api/admin/pdf-problems",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn pdf_problems_page() -> Result<PdfProblemsPage, ServerFnError> {
    crate::server::pdf::admin::page(&state, &headers).await
}

/// `POST /api/admin/documents/upload-pdf`: **Upload PDF**, a native
/// multipart post of `csrf` (first), `document`, `return_to` and the `pdf`
/// file. The file replaces the Document's PDF and its status becomes ready.
/// Answers 303 to `return_to`, 401/403 without a session or token, 404 for
/// an unknown Document, 415 for a file that isn't a PDF, 413 over 100 MB.
#[post("/api/admin/documents/upload-pdf",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
#[middleware(axum::extract::DefaultBodyLimit::max(crate::server::pdf::admin::MAX_BODY_BYTES))]
pub async fn upload_pdf(form: MultipartFormData) -> Result<Response, ServerFnError> {
    crate::server::pdf::admin::upload(&state, &headers, form).await
}

/// `POST /api/admin/documents/retry-pdf`: **Retry PDF**, queuing the
/// Document's conversion afresh. Answers 303 to `return_to`; 404 for an
/// unknown Document, 409 for one that isn't convertible (a ZIP).
#[post("/api/admin/documents/retry-pdf",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn retry_pdf(form: Form<RetryPdfForm>) -> Result<Response, ServerFnError> {
    crate::server::pdf::admin::retry(&state, &headers, form.0).await
}

/// `POST /api/admin/pdfs/retry-failed`: **Retry all failed**. Answers 303
/// to `return_to`.
#[post("/api/admin/pdfs/retry-failed",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn retry_failed_pdfs(form: Form<RetryFailedForm>) -> Result<Response, ServerFnError> {
    crate::server::pdf::admin::retry_failed(&state, &headers, form.0).await
}

pub const CSS: &str = r#"
.pdf-problems h3 { font-size: 1rem; margin: 18px 0 6px }
.pdf-problems td.comment { white-space: pre-line; color: #b00020; font-size: .85rem }
.pdf-actions { display: flex; flex-wrap: wrap; gap: 6px 12px; align-items: center }
.pdf-actions form { display: flex; gap: 6px; align-items: center; margin: 0 }
.pdf-actions input[type=file] { font-size: .8rem; max-width: 14rem }
.pdf-actions button, form.retry-failed button { border: 1px solid var(--accent); border-radius: 5px; padding: 2px 10px;
  background: var(--paper); color: var(--accent); font-weight: 600; cursor: pointer; font-size: .85rem }
form.retry-failed { margin: 6px 0 10px }
"#;

/// `/admin/needs-review`: the PDF problems.
#[component]
pub fn PdfProblems() -> Element {
    let page = match use_loader(pdf_problems_page) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "PDF problems" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read();
    let csrf = page.csrf.clone();
    rsx! {
        document::Style { {CSS} }
        AdminMenu {}
        div { class: "pdf-problems",
            h2 { "PDF problems" }
            h3 { id: "failed-heading", "Failed ({page.failed.len()})" }
            if page.failed.is_empty() {
                p { class: "muted", id: "no-failed", "No PDF has failed." }
            } else {
                RetryAllFailed { csrf: csrf.clone(), return_to: PDF_PROBLEMS.to_owned() }
                DocumentTable { id: "failed", documents: page.failed.clone(), csrf: csrf.clone() }
            }
            h3 { id: "preparing-heading", "Being prepared ({page.preparing_total})" }
            if page.preparing.is_empty() {
                p { class: "muted", id: "none-preparing", "No PDF is being prepared." }
            } else {
                DocumentTable { id: "preparing", documents: page.preparing.clone(), csrf: csrf.clone() }
                if page.preparing_total > page.preparing.len() as i64 {
                    p { class: "muted", "And {page.preparing_total - page.preparing.len() as i64} more." }
                }
            }
        }
    }
}

#[component]
fn DocumentTable(id: &'static str, documents: Vec<PdfDocument>, csrf: String) -> Element {
    rsx! {
        div { class: "tbl",
            table { id,
                thead {
                    tr {
                        th { "Document" }
                        th { "Tests" }
                        th { "Original" }
                        th { "Status" }
                        th { "Actions" }
                    }
                }
                tbody {
                    for document in documents {
                        tr { key: "{document.id}", id: "D{document.id}",
                            td { class: "document",
                                span { class: "mono", "D{document.id} " }
                                "{document.kind}"
                                if let Some(label) = &document.label { " ({label})" }
                            }
                            td { class: "tests mono",
                                {document.tests.iter().map(|t| format!("T{t}")).collect::<Vec<_>>().join(", ")}
                            }
                            td { class: "original mono",
                                if document.original_extension.is_empty() {
                                    "(no extension)"
                                } else {
                                    ".{document.original_extension}"
                                }
                            }
                            if document.pdf_status == "failed" {
                                td { class: "comment", {document.comment.clone().unwrap_or_default()} }
                            } else {
                                td { class: "job", {document.job.clone().unwrap_or_default()} }
                            }
                            td {
                                PdfActions {
                                    document: document.id,
                                    csrf: csrf.clone(),
                                    return_to: PDF_PROBLEMS.to_owned(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// **Retry PDF** and **Upload PDF** for one Document, coming back to
/// `return_to` (an `/admin/…` path).
#[component]
pub fn PdfActions(document: i64, csrf: String, return_to: String) -> Element {
    rsx! {
        div { class: "pdf-actions",
            form { class: "retry-pdf", method: "post", action: "/api/admin/documents/retry-pdf",
                input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                input { r#type: "hidden", name: "document", value: "{document}" }
                input { r#type: "hidden", name: "return_to", value: "{return_to}" }
                button { r#type: "submit", "Retry PDF" }
            }
            form {
                class: "upload-pdf",
                method: "post",
                action: "/api/admin/documents/upload-pdf",
                enctype: "multipart/form-data",
                input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                input { r#type: "hidden", name: "document", value: "{document}" }
                input { r#type: "hidden", name: "return_to", value: "{return_to}" }
                label {
                    span { class: "sr", "PDF for D{document}" }
                    input { r#type: "file", name: "pdf", accept: ".pdf,application/pdf", required: true }
                }
                button { r#type: "submit", "Upload PDF" }
            }
        }
    }
}

/// **Retry all failed**, coming back to `return_to` (an `/admin/…` path).
#[component]
pub fn RetryAllFailed(csrf: String, return_to: String) -> Element {
    rsx! {
        form { class: "retry-failed", method: "post", action: "/api/admin/pdfs/retry-failed",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "return_to", value: "{return_to}" }
            button { r#type: "submit", "Retry all failed" }
        }
    }
}
