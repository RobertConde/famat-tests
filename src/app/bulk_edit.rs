//! **Edit selected** and **Merge into…** (`/admin/tests/bulk`), opened from
//! the search page's bulk bar with the selection as it is ticked.
//!
//! - **Set one Facet** on every selected Test (with **+ New value**), one
//!   form per Facet.
//! - **Tick or clear Needs review** on every selected Test.
//! - **Merge** one of exactly two selected Tests into the other. The Test
//!   edit page has the same action, merging into a Test typed by ID
//!   ([`MergeInto`]).
//!
//! Every action is its own native form post (Admin-only, CSRF-checked). An
//! edit answers 303 back to this page with a summary: how many Tests
//! changed, how many were already so, and which were refused and why. A
//! merge answers 303 to the surviving Test's edit page.
//!
//! The logic is server-only, in `crate::server::bulk_edit`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::{
    add_test::{self, FacetField, FacetOptions},
    admin::AdminMenu,
    test_edit::edit_path,
};

/// The bulk edit page. The bulk bar's buttons GET it with the selection in
/// `sel`, the search's fields and `page`.
pub const BULK_EDIT_PATH: &str = "/admin/tests/bulk";

/// Where **Merge** posts.
pub const MERGE_PATH: &str = "/api/admin/tests/merge";

/// Everything the bulk edit page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BulkEditPage {
    pub csrf: String,
    /// The selected Tests that exist and aren't in the Trash, by ID.
    pub tests: Vec<BulkTest>,
    /// The selection as the forms post it (`sel`): those Tests' IDs.
    pub selection: String,
    /// The six dropdowns, with nothing chosen.
    pub facets: Vec<FacetOptions>,
    /// The search page the selection came from, with the selection kept.
    pub back: String,
    /// What the last edit did, when the page is its answer.
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BulkTest {
    pub id: i64,
    /// e.g. "1993 February Invitational · Plant City · Calculus/Mu Individual"
    pub description: String,
    pub published: bool,
    pub needs_review: bool,
    /// How many untrashed Documents it has.
    pub documents: i64,
}

/// The summary of one bulk edit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// What was done, e.g. "Host set to Lawton Chiles".
    pub done: String,
    pub changed: Vec<i64>,
    /// Already as asked.
    pub unchanged: Vec<i64>,
    /// `(Test ID, why)`.
    pub refused: Vec<(i64, String)>,
}

/// **Edit selected**: either one Facet (`facet` names it, with its dropdown
/// fields as Add test posts them) or Needs review (`review` is `tick` or
/// `clear`). Posted natively; only the server reads it. Every field
/// defaults to empty, so a missing one is refused with a message.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BulkEditForm {
    pub csrf: String,
    /// The selected Test IDs, comma-separated.
    pub sel: String,
    /// The search page to link back to.
    pub back: String,
    /// `year`, `month`, `competition`, `division_level`, `test_type` or
    /// `host`; empty for a Needs review edit.
    pub facet: String,
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
    /// `tick` or `clear`.
    pub review: String,
    /// When ticking: a reason added to each Test's comment.
    pub needs_review_comment: String,
}

/// **Merge into…**: Test `from` into Test `into` (`T123` or `123`).
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MergeForm {
    pub csrf: String,
    pub from: String,
    pub into: String,
}

/// The page's data, from the page's own query string. An Admin endpoint:
/// anonymous callers get 401.
#[get("/api/admin/tests/bulk",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>,
    raw_query: dioxus::fullstack::extract::RawQuery)]
pub async fn bulk_edit_page() -> Result<BulkEditPage, ServerFnError> {
    crate::server::bulk_edit::page(&state, &headers, raw_query.0.as_deref()).await
}

/// `POST /api/admin/tests/bulk-edit`: set one Facet, or tick or clear Needs
/// review, on every selected Test, logging each one changed. A Published
/// Test that would be left with its Year and Competition both unknown is
/// refused (and left as it was). Answers 303 back to the bulk edit page
/// with the summary; 401/403 without a session or token.
#[post("/api/admin/tests/bulk-edit",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn bulk_edit(form: Form<BulkEditForm>) -> Result<Response, ServerFnError> {
    crate::server::bulk_edit::edit(&state, &headers, form.0).await
}

/// `POST /api/admin/tests/merge`: move every Document of `from` onto
/// `into`, and put `from` in the Trash. Answers 303 to `into`'s edit page;
/// 401/403 without a session or token.
#[post("/api/admin/tests/merge",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn merge_tests(form: Form<MergeForm>) -> Result<Response, ServerFnError> {
    crate::server::bulk_edit::merge(&state, &headers, form.0).await
}

const CSS: &str = r#"
.bulk-edit h3 { font-size: 1rem; margin: 20px 0 8px }
.bulk-edit .outcome { border: 1px solid var(--rule); border-radius: 6px; padding: 10px 16px; margin: 0 0 14px; background: var(--head) }
.bulk-edit .outcome p { margin: 0 }
.bulk-edit .outcome ul { margin: 6px 0 0; padding-left: 20px; color: #b00020 }
.bulk-edit .set-facet { display: grid; grid-template-columns: max-content minmax(0, 20rem) max-content; gap: 8px 12px;
  align-items: start; margin: 0 0 8px }
.bulk-edit .set-facet .facet { display: contents }
.bulk-edit form.review { display: grid; gap: 8px; max-width: 34rem }
.bulk-edit textarea { width: 100%; min-height: 3.2em; padding: 5px 8px; border: 1px solid var(--rule); border-radius: 5px;
  font: inherit; font-size: .9rem }
.bulk-edit .buttons, .merge-into .buttons { display: flex; flex-wrap: wrap; gap: 8px 12px; align-items: center }
.bulk-edit button, .merge-into button { border: 1px solid var(--accent); border-radius: 5px; padding: 4px 14px;
  font-weight: 600; cursor: pointer; background: var(--paper); color: var(--accent) }
.merge-into fieldset { border: 1px solid var(--rule); border-radius: 6px; padding: 12px 16px 14px; margin: 0 0 14px }
.merge-into legend { font-weight: 700; padding-inline: 4px }
.merge-into p { margin: 0 0 8px; font-size: .9rem; color: var(--muted) }
.merge-into input[type=text] { width: 8rem; padding: 4px 8px; border: 1px solid var(--rule); border-radius: 5px }
@media (max-width: 700px) {
  .bulk-edit .set-facet { grid-template-columns: minmax(0, 1fr) }
}
"#;

/// "1 test" / "3 tests".
fn tests_text(n: usize) -> String {
    if n == 1 {
        "1 test".to_owned()
    } else {
        format!("{n} tests")
    }
}

/// `/admin/tests/bulk`.
#[component]
pub fn BulkEdit() -> Element {
    let page = match use_loader(bulk_edit_page) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Edit selected" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read();
    let n = page.tests.len();
    let count = tests_text(n);
    rsx! {
        document::Style { {add_test::CSS} }
        document::Style { {CSS} }
        AdminMenu {}
        div { class: "bulk-edit",
            h2 { "Edit selected" }
            if let Some(outcome) = &page.outcome {
                OutcomeSummary { outcome: outcome.clone() }
            }
            p {
                a { id: "back", href: "{page.back}", "Back to the search" }
            }
            if page.tests.is_empty() {
                p { id: "nothing-selected", "No tests are selected. Tick some on the search page first." }
            } else {
                h3 { id: "selected-heading", "Selected ({count})" }
                div { class: "tbl",
                    table { id: "selected-tests",
                        thead {
                            tr {
                                th { "Test" }
                                th { "Labels" }
                                th { "Visibility" }
                                th { "Needs review" }
                                th { "Documents" }
                            }
                        }
                        tbody {
                            for test in page.tests.iter() {
                                tr { key: "{test.id}", id: "T{test.id}",
                                    td { class: "mono", a { href: edit_path(test.id), "T{test.id}" } }
                                    td { class: "description", "{test.description}" }
                                    td { class: "visibility", if test.published { "Published" } else { "Hidden" } }
                                    td { class: "needs-review", if test.needs_review { "Yes" } else { "—" } }
                                    td { class: "documents", "{test.documents}" }
                                }
                            }
                        }
                    }
                }
                h3 { "Set one Facet on all {count}" }
                for facet in page.facets.iter() {
                    form {
                        key: "{facet.name}",
                        id: "set-{facet.name}",
                        class: "add-test set-facet",
                        method: "post",
                        action: "/api/admin/tests/bulk-edit",
                        Hidden { csrf: page.csrf.clone(), selection: page.selection.clone(), back: page.back.clone() }
                        input { r#type: "hidden", name: "facet", value: "{facet.name}" }
                        FacetField { facet: facet.clone() }
                        div { class: "buttons",
                            button { r#type: "submit", "Set {facet.label}" }
                        }
                    }
                }
                h3 { "Needs review" }
                form {
                    id: "set-review",
                    class: "review",
                    method: "post",
                    action: "/api/admin/tests/bulk-edit",
                    Hidden { csrf: page.csrf.clone(), selection: page.selection.clone(), back: page.back.clone() }
                    textarea {
                        name: "needs_review_comment",
                        aria_label: "Needs review comment",
                        placeholder: "Why they need review (added to each Test's comment when ticking)",
                    }
                    div { class: "buttons",
                        button { r#type: "submit", name: "review", value: "tick", "Tick Needs review" }
                        button { r#type: "submit", name: "review", value: "clear", "Clear Needs review" }
                    }
                }
                h3 { id: "merge", "Merge" }
                if let [a, b] = page.tests.as_slice() {
                    p { class: "muted",
                        "Every Document of the first moves onto the second, and the first, left empty, goes to the Trash."
                    }
                    div { class: "buttons",
                        MergeButton { from: a.id, into: b.id, csrf: page.csrf.clone() }
                        MergeButton { from: b.id, into: a.id, csrf: page.csrf.clone() }
                    }
                } else {
                    p { id: "merge-needs-two", class: "muted", "Select exactly two tests to merge one into the other." }
                }
            }
        }
    }
}

/// The fields every bulk edit form carries.
#[component]
fn Hidden(csrf: String, selection: String, back: String) -> Element {
    rsx! {
        input { r#type: "hidden", name: "csrf", value: "{csrf}" }
        input { r#type: "hidden", name: "sel", value: "{selection}" }
        input { r#type: "hidden", name: "back", value: "{back}" }
    }
}

#[component]
fn OutcomeSummary(outcome: Outcome) -> Element {
    let changed = outcome.changed.len();
    let unchanged = outcome.unchanged.len();
    let refused = outcome.refused.len();
    rsx! {
        div { id: "outcome", class: "outcome",
            p {
                b { "{outcome.done}: " }
                span { id: "changed", "{changed} changed" }
                ", "
                span { id: "unchanged", "{unchanged} unchanged" }
                if refused > 0 {
                    ", "
                    span { id: "refused", "{refused} refused" }
                }
                "."
            }
            if refused > 0 {
                ul { id: "refusals",
                    for (id , why) in outcome.refused.iter() {
                        li { key: "{id}", id: "refused-T{id}", "T{id}: {why}" }
                    }
                }
            }
        }
    }
}

#[component]
fn MergeButton(from: i64, into: i64, csrf: String) -> Element {
    rsx! {
        form { class: "merge", id: "merge-T{from}-into-T{into}", method: "post", action: MERGE_PATH,
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "from", value: "{from}" }
            input { r#type: "hidden", name: "into", value: "{into}" }
            button { r#type: "submit", "Merge T{from} into T{into}" }
        }
    }
}

/// **Merge into…** on the Test edit page: this Test's Documents move onto
/// the Test typed by ID, and this one goes to the Trash.
#[component]
pub fn MergeInto(from: i64, csrf: String) -> Element {
    rsx! {
        document::Style { {CSS} }
        form { id: "merge-test", class: "merge-into", method: "post", action: MERGE_PATH,
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "from", value: "{from}" }
            fieldset {
                legend { "Merge into…" }
                p { "Move every Document of T{from} onto another Test, and put T{from}, left empty, in the Trash." }
                div { class: "buttons",
                    input {
                        r#type: "text",
                        name: "into",
                        required: true,
                        aria_label: "Test ID to merge T{from} into",
                        placeholder: "T123",
                    }
                    button { r#type: "submit", "Merge" }
                }
            }
        }
    }
}
