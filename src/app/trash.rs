//! **Trash** (`/admin/trash`) and the **Remove** buttons that fill it.
//!
//! - **Remove** ([`RemoveTest`], [`RemoveDocument`] on the Test edit page)
//!   moves a Test or a Document to Trash. Only `trashed_at` changes, so it
//!   vanishes from the public site and Admin pages alike, and **Restore**
//!   brings it back exactly as it was, Published or Hidden.
//! - **The Trash page** lists what was removed, newest first, each with
//!   **Restore** and **Delete permanently**. Deleting permanently is the
//!   only real delete: it removes the rows and the Garage objects.
//!
//! Every action is a native form post (CSRF-checked, Admin-only) answered
//! with a 303, so the page works without JavaScript. The logic, and the
//! rules for Documents shared between Tests, are server-only, in
//! `crate::server::trash`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::admin::AdminMenu;

/// Everything the Trash page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrashPage {
    pub csrf: String,
    /// Newest first.
    pub items: Vec<TrashedItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrashedItem {
    Test(TrashedTest),
    Document(TrashedDocument),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrashedTest {
    pub id: i64,
    /// When it was removed, as an Admin reads it.
    pub removed: String,
    /// e.g. "1993 February Invitational · Plant City · Calculus/Mu Individual"
    pub description: String,
    pub published: bool,
    /// Its Documents, by ID.
    pub documents: Vec<DocumentRef>,
}

/// A Document of a Trashed Test.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentRef {
    pub id: i64,
    pub kind: String,
    pub label: Option<String>,
    /// In the Trash itself too.
    pub trashed: bool,
    /// Also on another Test, so deleting this one keeps it.
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrashedDocument {
    pub id: i64,
    pub removed: String,
    pub kind: String,
    pub label: Option<String>,
    pub published: bool,
    /// Every Test it is on, by ID.
    pub tests: Vec<TestRef>,
}

impl TrashedDocument {
    /// Restoring it is refused while every Test it is on is in the Trash:
    /// it would come back out of sight.
    pub fn restorable(&self) -> bool {
        self.tests.iter().any(|t| !t.trashed)
    }
}

/// A Test a Trashed Document is on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestRef {
    pub id: i64,
    pub description: String,
    pub trashed: bool,
}

// Posted natively by the browser; only the server reads them. Fields
// default to empty, so a missing one is refused with a message.

/// **Remove** on the Test edit page: the Test, or one of its Documents
/// when `document` is given (`test` is then the page to come back to).
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoveForm {
    pub csrf: String,
    pub test: String,
    pub document: String,
}

/// **Restore** or **Delete permanently** on the Trash page. `item` is a
/// display ID: `T12` or `D34`.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrashItemForm {
    pub csrf: String,
    pub item: String,
}

/// The page's data. An Admin endpoint: anonymous callers get 401.
#[get("/api/admin/trash",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn trash_page() -> Result<TrashPage, ServerFnError> {
    crate::server::trash::page(&state, &headers).await
}

/// `POST /api/admin/tests/remove`: move a Test to Trash, then show the
/// Trash. Its Documents go out of sight with it, except on other Tests.
#[post("/api/admin/tests/remove",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn remove_test(form: Form<RemoveForm>) -> Result<Response, ServerFnError> {
    crate::server::trash::remove_test(&state, &headers, form.0).await
}

/// `POST /api/admin/documents/remove`: move a Document to Trash (off every
/// Test it is on), then go back to the edit page.
#[post("/api/admin/documents/remove",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn remove_document(form: Form<RemoveForm>) -> Result<Response, ServerFnError> {
    crate::server::trash::remove_document(&state, &headers, form.0).await
}

/// `POST /api/admin/trash/restore`: bring a Test or Document back exactly
/// as it was.
#[post("/api/admin/trash/restore",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn restore(form: Form<TrashItemForm>) -> Result<Response, ServerFnError> {
    crate::server::trash::restore(&state, &headers, form.0).await
}

/// `POST /api/admin/trash/delete`: delete a Trashed Test or Document for
/// good, rows and Garage objects. Refused (409) unless it is in the Trash.
#[post("/api/admin/trash/delete",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn delete_permanently(form: Form<TrashItemForm>) -> Result<Response, ServerFnError> {
    crate::server::trash::delete(&state, &headers, form.0).await
}

pub(super) const CSS: &str = r#"
.trash-intro { color: var(--muted); font-size: .9rem; max-width: 46rem }
#trash td { vertical-align: top }
#trash td.when { white-space: nowrap; color: var(--muted); font-size: .85rem }
#trash .what { font-weight: 600 }
#trash .details { font-size: .85rem; color: var(--muted) }
#trash .details .mono { color: var(--ink) }
#trash .details .document + .document::before { content: ", " }
#trash .details .test + .test::before { content: "; " }
#trash td.actions { white-space: nowrap }
#trash td.actions form, form.remove { display: inline-flex; margin: 0 6px 0 0 }
#trash button, form.remove button { border: 1px solid var(--rule); border-radius: 5px; padding: 3px 10px;
  background: var(--paper); color: var(--ink); cursor: pointer; font-size: .85rem }
#trash button.restore { border-color: var(--accent); color: var(--accent); font-weight: 600 }
#trash button.delete, form.remove button { color: #b00020 }
#trash button:disabled { color: var(--faint); cursor: default }
#trash .blocked { font-size: .8rem; color: var(--muted); margin: 4px 0 0 }
"#;

/// `/admin/trash`.
#[component]
pub fn Trash() -> Element {
    let page = match use_loader(trash_page) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Trash" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read();
    let csrf = page.csrf.clone();
    rsx! {
        document::Style { {CSS} }
        AdminMenu {}
        h2 { "Trash" }
        p { class: "trash-intro",
            "Removed Tests and Documents are out of sight everywhere until restored. Restore brings one back "
            "exactly as it was. Delete permanently removes it and its files for good."
        }
        if page.items.is_empty() {
            p { id: "trash-empty", class: "empty", "The Trash is empty." }
        } else {
            div { class: "tbl",
                table { id: "trash",
                    thead {
                        tr {
                            th { "Removed" }
                            th { "Item" }
                            th { "Details" }
                            th { span { class: "sr", "Actions" } }
                        }
                    }
                    tbody {
                        for item in page.items.iter() {
                            ItemRow { key: "{item.display_id()}", item: item.clone(), csrf: csrf.clone() }
                        }
                    }
                }
            }
        }
    }
}

impl TrashedItem {
    /// `T12` or `D34`, as the forms post it.
    pub fn display_id(&self) -> String {
        match self {
            Self::Test(test) => format!("T{}", test.id),
            Self::Document(document) => format!("D{}", document.id),
        }
    }
}

#[component]
fn ItemRow(item: TrashedItem, csrf: String) -> Element {
    match item {
        TrashedItem::Test(test) => rsx! { TestRow { test, csrf } },
        TrashedItem::Document(document) => rsx! { DocumentRow { document, csrf } },
    }
}

fn visibility(published: bool) -> &'static str {
    if published { "Published" } else { "Hidden" }
}

fn kind_and_label(kind: &str, label: &Option<String>) -> String {
    match label {
        Some(label) => format!("{kind} ({label})"),
        None => kind.to_owned(),
    }
}

#[component]
fn TestRow(test: TrashedTest, csrf: String) -> Element {
    let item = format!("T{}", test.id);
    let deleted_with: Vec<String> = test
        .documents
        .iter()
        .filter(|d| !d.shared)
        .map(|d| format!("D{}", d.id))
        .collect();
    let warning = if deleted_with.is_empty() {
        format!("Delete {item} for good")
    } else {
        format!("Delete {item} for good, with {}", deleted_with.join(", "))
    };
    rsx! {
        tr { id: "{item}", class: "test",
            td { class: "when", "{test.removed}" }
            td {
                span { class: "what mono", "{item}" }
                " Test · {visibility(test.published)}"
                div { class: "description", "{test.description}" }
            }
            td { class: "details",
                if test.documents.is_empty() {
                    "No Documents"
                } else {
                    "Documents: "
                    for document in test.documents.iter() {
                        span { key: "{document.id}", class: "document", id: "{item}-D{document.id}",
                            span { class: "mono", "D{document.id}" }
                            " {kind_and_label(&document.kind, &document.label)}"
                            if document.trashed {
                                " (in the Trash)"
                            }
                            if document.shared {
                                " (also on another Test)"
                            }
                        }
                    }
                }
            }
            td { class: "actions",
                TrashActions { item: item.clone(), csrf, restorable: true, warning }
            }
        }
    }
}

#[component]
fn DocumentRow(document: TrashedDocument, csrf: String) -> Element {
    let item = format!("D{}", document.id);
    let restorable = document.restorable();
    let tests: Vec<String> = document.tests.iter().map(|t| format!("T{}", t.id)).collect();
    rsx! {
        tr { id: "{item}", class: "document",
            td { class: "when", "{document.removed}" }
            td {
                span { class: "what mono", "{item}" }
                " {kind_and_label(&document.kind, &document.label)} · {visibility(document.published)}"
            }
            td { class: "details",
                "On "
                for test in document.tests.iter() {
                    span { key: "{test.id}", class: "test", id: "{item}-T{test.id}",
                        if test.trashed {
                            span { class: "mono", "T{test.id}" }
                            " (in the Trash)"
                        } else {
                            a { class: "mono", href: super::test_edit::edit_path(test.id), "T{test.id}" }
                        }
                        " {test.description}"
                    }
                }
            }
            td { class: "actions",
                TrashActions {
                    item: item.clone(),
                    csrf,
                    restorable,
                    warning: format!("Delete {item} and its files for good"),
                }
                if !restorable {
                    p { class: "blocked",
                        "Its Tests are all in the Trash: restore {tests.join(\" or \")} first."
                    }
                }
            }
        }
    }
}

#[component]
fn TrashActions(item: String, csrf: String, restorable: bool, warning: String) -> Element {
    rsx! {
        form { class: "restore", method: "post", action: "/api/admin/trash/restore",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "item", value: "{item}" }
            button {
                class: "restore",
                r#type: "submit",
                disabled: !restorable,
                aria_label: "Restore {item}",
                "Restore"
            }
        }
        form { class: "delete", method: "post", action: "/api/admin/trash/delete",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "item", value: "{item}" }
            button {
                class: "delete",
                r#type: "submit",
                title: "{warning}",
                aria_label: "Delete {item} permanently",
                "Delete permanently"
            }
        }
    }
}

/// **Remove** for the Test edit page's Test.
#[component]
pub fn RemoveTest(test: i64, csrf: String) -> Element {
    rsx! {
        document::Style { {CSS} }
        form { id: "remove-test", class: "remove", method: "post", action: "/api/admin/tests/remove",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "test", value: "{test}" }
            button {
                r#type: "submit",
                title: "Move T{test} to the Trash, where it can be restored",
                "Remove test"
            }
        }
    }
}

/// **Remove** for one Document on the Test edit page. It leaves every
/// Test the Document is on.
#[component]
pub fn RemoveDocument(document: i64, test: i64, csrf: String, shared: bool) -> Element {
    let title = if shared {
        format!("Move D{document} to the Trash: it leaves every Test it is on")
    } else {
        format!("Move D{document} to the Trash, where it can be restored")
    };
    rsx! {
        form { class: "remove remove-document", method: "post", action: "/api/admin/documents/remove",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "test", value: "{test}" }
            input { r#type: "hidden", name: "document", value: "{document}" }
            button { r#type: "submit", title: "{title}", "Remove document" }
        }
    }
}
