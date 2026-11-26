//! **Document kinds** (`/admin/document-kinds`): every kind in display
//! order with how many Documents use it. An Admin renames a kind and ticks
//! what it contains (questions / answers / solutions), adds one, reorders
//! them, and deletes one no Document uses.
//!
//! Every action is a native form post, so the page works without
//! JavaScript: each row's name and ticks belong to its own **Save** form
//! (through the `form` attribute, since a form cannot span table cells), and
//! ↑ / ↓ reorder. Once the page has hydrated, rows can also be dragged by
//! their handle; dropping one posts the whole new order.
//!
//! The logic is server-only, in `crate::server::document_kinds`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::admin::AdminMenu;
use crate::download::Contents;

/// The longest name a kind may have, in characters. Kind names go into
/// download names, which are capped at about 150.
pub const MAX_NAME_CHARS: usize = 60;

/// Everything the page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentKindsPage {
    pub csrf: String,
    /// In display order.
    pub kinds: Vec<KindRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KindRow {
    pub id: i64,
    pub name: String,
    pub contains: Contents,
    /// Documents of this kind, Hidden ones and those in Trash included.
    pub documents: i64,
    /// Files of this kind waiting on Add test's duplicate warning.
    pub waiting: i64,
}

impl KindRow {
    fn in_use(&self) -> bool {
        self.documents > 0 || self.waiting > 0
    }
}

// Posted natively by the browser; only the server reads them. A checkbox is
// posted as `true` when ticked and left out when not.

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct AddKindForm {
    pub csrf: String,
    pub name: String,
    #[serde(default)]
    pub questions: bool,
    #[serde(default)]
    pub answers: bool,
    #[serde(default)]
    pub solutions: bool,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct SaveKindForm {
    pub csrf: String,
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub questions: bool,
    #[serde(default)]
    pub answers: bool,
    #[serde(default)]
    pub solutions: bool,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct MoveKindForm {
    pub csrf: String,
    pub id: i64,
    /// `up` or `down`.
    pub direction: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct ReorderKindsForm {
    pub csrf: String,
    /// Every kind's ID in the new order, comma-separated.
    pub order: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct DeleteKindForm {
    pub csrf: String,
    pub id: i64,
}

/// The page's data. An Admin endpoint: anonymous callers get 401.
#[get("/api/admin/document-kinds",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn document_kinds_page() -> Result<DocumentKindsPage, ServerFnError> {
    crate::server::document_kinds::page(&state, &headers).await
}

/// `POST /api/admin/document-kinds/add`: **+ Add**.
#[post("/api/admin/document-kinds/add",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn add_kind(form: Form<AddKindForm>) -> Result<Response, ServerFnError> {
    crate::server::document_kinds::add(&state, &headers, form.0).await
}

/// `POST /api/admin/document-kinds/save`: a row's **Save** (name and ticks).
#[post("/api/admin/document-kinds/save",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn save_kind(form: Form<SaveKindForm>) -> Result<Response, ServerFnError> {
    crate::server::document_kinds::save(&state, &headers, form.0).await
}

/// `POST /api/admin/document-kinds/move`: a row's ↑ or ↓.
#[post("/api/admin/document-kinds/move",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn move_kind(form: Form<MoveKindForm>) -> Result<Response, ServerFnError> {
    crate::server::document_kinds::move_kind(&state, &headers, form.0).await
}

/// `POST /api/admin/document-kinds/order`: the order after a drag.
#[post("/api/admin/document-kinds/order",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn reorder_kinds(form: Form<ReorderKindsForm>) -> Result<Response, ServerFnError> {
    crate::server::document_kinds::reorder(&state, &headers, form.0).await
}

/// `POST /api/admin/document-kinds/delete`: **Delete**, refused (409) for a
/// kind in use.
#[post("/api/admin/document-kinds/delete",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn delete_kind(form: Form<DeleteKindForm>) -> Result<Response, ServerFnError> {
    crate::server::document_kinds::delete(&state, &headers, form.0).await
}

const CSS: &str = r#"
.kinds-intro { color: var(--muted); font-size: .9rem; max-width: 46rem }
#kinds td { vertical-align: middle }
#kinds td.tick, #kinds th.tick, #kinds td.count { text-align: center }
#kinds input[type=text], #add-kind input[type=text] { width: 100%; min-width: 9rem; padding: 4px 8px;
  border: 1px solid var(--rule); border-radius: 5px; background: var(--paper); font-size: .9rem }
#kinds td.order { white-space: nowrap }
#kinds form { display: inline-flex; gap: 4px; margin: 0 }
#kinds td.actions { white-space: nowrap }
#kinds button, #add-kind button { border: 1px solid var(--rule); border-radius: 5px; padding: 3px 10px;
  background: var(--paper); color: var(--ink); cursor: pointer; font-size: .85rem }
#kinds button:disabled { color: var(--faint); cursor: default }
#kinds button.save, #add-kind button { border-color: var(--accent); color: var(--accent); font-weight: 600 }
#kinds button.delete { color: #b00020 }
#kinds .handle { cursor: grab; color: var(--muted); padding: 0 6px; user-select: none }
#kinds tr.dragging td { background: var(--accent-soft) }
#kinds .in-use { color: var(--muted); font-size: .85rem }
#add-kind { display: flex; flex-wrap: wrap; gap: 8px 14px; align-items: center; margin: 16px 0 0;
  border: 1px solid var(--rule); border-radius: 6px; padding: 10px 16px }
#add-kind h3 { margin: 0; font-size: 1rem }
#add-kind label { display: flex; gap: 5px; align-items: center; font-size: .9rem }
#add-kind .name { flex: 1 1 14rem }
@media (max-width: 700px) {
  #kinds tr { grid-template-columns: 1fr 1fr 1fr }
  #kinds td.name, #kinds td.order, #kinds td.actions { grid-column: 1 / -1 }
  #kinds td.tick, #kinds td.count { text-align: left }
}
"#;

/// `/admin/document-kinds`.
#[component]
pub fn DocumentKinds() -> Element {
    let page = match use_loader(document_kinds_page) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Document kinds" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read();
    rsx! {
        document::Style { {CSS} }
        AdminMenu {}
        h2 { "Document kinds" }
        p { class: "kinds-intro",
            "Each kind says what its Documents contain. The search’s Includes filters and the download "
            "menus’ groups follow these ticks, and Add test’s kind dropdowns follow this order. "
            "A kind can be deleted once no Document uses it."
        }
        KindTable { csrf: page.csrf.clone(), kinds: page.kinds.clone() }
        AddKind { csrf: page.csrf.clone() }
    }
}

/// The kinds, one row each. Rows render in `order`, which only a drag
/// changes before the page reloads.
#[component]
fn KindTable(csrf: String, kinds: Vec<KindRow>) -> Element {
    let saved: Vec<i64> = kinds.iter().map(|k| k.id).collect();
    let mut order = use_signal(|| saved.clone());
    let mut dragging = use_signal(|| None::<i64>);
    let mut dropped = use_signal(|| false);
    // Dragging needs JavaScript, so the handles only appear once hydrated.
    let mut hydrated = use_signal(|| false);
    use_effect(move || hydrated.set(true));

    let rows: Vec<(usize, KindRow)> = order
        .read()
        .iter()
        .filter_map(|id| kinds.iter().find(|k| k.id == *id).cloned())
        .enumerate()
        .collect();
    let last = rows.len().saturating_sub(1);
    let saved_order = saved.clone();
    let saved_ids = join_ids(&saved);

    rsx! {
        div { class: "tbl",
            table { id: "kinds",
                thead {
                    tr {
                        th { "Order" }
                        th { "Name" }
                        th { class: "tick", "Questions" }
                        th { class: "tick", "Answers" }
                        th { class: "tick", "Solutions" }
                        th { class: "tick", "Documents" }
                        th { span { class: "sr", "Actions" } }
                    }
                }
                tbody {
                    for (i , kind) in rows {
                        tr {
                            key: "{kind.id}",
                            id: "K{kind.id}",
                            class: if dragging() == Some(kind.id) { "kind dragging" } else { "kind" },
                            ondragover: move |event| {
                                event.prevent_default();
                                let Some(dragged) = dragging() else { return };
                                if dragged == kind.id {
                                    return;
                                }
                                let mut ids = order();
                                let (Some(from), Some(to)) = (
                                    ids.iter().position(|&k| k == dragged),
                                    ids.iter().position(|&k| k == kind.id),
                                ) else {
                                    return;
                                };
                                let moved = ids.remove(from);
                                ids.insert(to, moved);
                                order.set(ids);
                            },
                            ondrop: move |event| {
                                event.prevent_default();
                                if dragging().is_none() {
                                    return;
                                }
                                dropped.set(true);
                                // Digits and commas only, so safe to splice in.
                                let ids = join_ids(&order());
                                let _ = document::eval(&format!(
                                    "const f = document.getElementById('reorder'); \
                                     f.elements.order.value = '{ids}'; f.submit();"
                                ));
                            },
                            td { class: "order",
                                if hydrated() {
                                    span {
                                        class: "handle",
                                        draggable: "true",
                                        title: "Drag to reorder",
                                        aria_hidden: "true",
                                        ondragstart: move |event| {
                                            let transfer = event.data_transfer();
                                            // Firefox only starts a drag that carries data.
                                            let _ = transfer.set_data("text/plain", &kind.id.to_string());
                                            transfer.set_effect_allowed("move");
                                            dropped.set(false);
                                            dragging.set(Some(kind.id));
                                        },
                                        ondragend: {
                                            let saved_order = saved_order.clone();
                                            move |_| {
                                                if !dropped() {
                                                    // Cancelled: put the rows back.
                                                    order.set(saved_order.clone());
                                                }
                                                dragging.set(None);
                                            }
                                        },
                                        "⠿"
                                    }
                                }
                                form { class: "move", method: "post", action: "/api/admin/document-kinds/move",
                                    input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                                    input { r#type: "hidden", name: "id", value: "{kind.id}" }
                                    button {
                                        r#type: "submit",
                                        name: "direction",
                                        value: "up",
                                        disabled: i == 0,
                                        aria_label: "Move {kind.name} up",
                                        title: "Move up",
                                        "↑"
                                    }
                                    button {
                                        r#type: "submit",
                                        name: "direction",
                                        value: "down",
                                        disabled: i == last,
                                        aria_label: "Move {kind.name} down",
                                        title: "Move down",
                                        "↓"
                                    }
                                }
                            }
                            td { class: "name",
                                input {
                                    r#type: "text",
                                    name: "name",
                                    form: "kind-{kind.id}",
                                    value: "{kind.name}",
                                    required: true,
                                    maxlength: MAX_NAME_CHARS as i64,
                                    aria_label: "Name of {kind.name}",
                                }
                            }
                            Tick {
                                kind: kind.id,
                                field: "questions",
                                heading: "Questions",
                                name: kind.name.clone(),
                                checked: kind.contains.questions,
                            }
                            Tick {
                                kind: kind.id,
                                field: "answers",
                                heading: "Answers",
                                name: kind.name.clone(),
                                checked: kind.contains.answers,
                            }
                            Tick {
                                kind: kind.id,
                                field: "solutions",
                                heading: "Solutions",
                                name: kind.name.clone(),
                                checked: kind.contains.solutions,
                            }
                            td { class: "count mono", "data-l": "Documents",
                                "{kind.documents}"
                                if kind.waiting > 0 {
                                    span { class: "in-use", title: "Files waiting on Add test’s duplicate warning",
                                        " + {kind.waiting} waiting"
                                    }
                                }
                            }
                            td { class: "actions",
                                form {
                                    id: "kind-{kind.id}",
                                    class: "save",
                                    method: "post",
                                    action: "/api/admin/document-kinds/save",
                                    input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                                    input { r#type: "hidden", name: "id", value: "{kind.id}" }
                                    button { class: "save", r#type: "submit", aria_label: "Save {kind.name}", "Save" }
                                }
                                " "
                                if kind.in_use() {
                                    span { class: "in-use", title: "A kind can be deleted once no Document uses it",
                                        "In use"
                                    }
                                } else {
                                    form { class: "delete", method: "post", action: "/api/admin/document-kinds/delete",
                                        input { r#type: "hidden", name: "csrf", value: "{csrf}" }
                                        input { r#type: "hidden", name: "id", value: "{kind.id}" }
                                        button {
                                            class: "delete",
                                            r#type: "submit",
                                            aria_label: "Delete {kind.name}",
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // What a drop posts; the drop fills in `order`.
        form { id: "reorder", method: "post", action: "/api/admin/document-kinds/order", hidden: true,
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "order", value: "{saved_ids}" }
        }
    }
}

/// `3,1,2`: what the reorder form posts.
fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// One contains-flag checkbox of a row's Save form.
#[component]
fn Tick(kind: i64, field: &'static str, heading: &'static str, name: String, checked: bool) -> Element {
    rsx! {
        td { class: "tick", "data-l": "{heading}",
            input {
                r#type: "checkbox",
                name: field,
                value: "true",
                form: "kind-{kind}",
                checked,
                aria_label: "{name} contains {field}",
            }
        }
    }
}

/// **+ Add**: a new kind goes to the end of the order.
#[component]
fn AddKind(csrf: String) -> Element {
    rsx! {
        form { id: "add-kind", method: "post", action: "/api/admin/document-kinds/add",
            h3 { "New kind" }
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            input {
                class: "name",
                r#type: "text",
                name: "name",
                required: true,
                maxlength: MAX_NAME_CHARS as i64,
                aria_label: "Name of the new kind",
                placeholder: "e.g. Answer sheet",
            }
            label { input { r#type: "checkbox", name: "questions", value: "true" } "Questions" }
            label { input { r#type: "checkbox", name: "answers", value: "true" } "Answers" }
            label { input { r#type: "checkbox", name: "solutions", value: "true" } "Solutions" }
            button { r#type: "submit", "+ Add" }
        }
    }
}
