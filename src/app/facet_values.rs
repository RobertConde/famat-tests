//! **Facet values** (`/admin/facets?facet=`): one view per listed Facet
//! (Month, Competition, Division/Level, Test type, Host), its values in
//! display order with how many Tests use each.
//!
//! Every action is a native form post, so the page works without
//! JavaScript: **↑ / ↓** move a value, and **Edit** opens **Rename**,
//! **Merge into…** and (for an unused value) **Delete**. Once hydrated,
//! values can also be dragged; dropping one posts the Facet's new order.
//!
//! The logic is server-only, in `crate::server::facet_values`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::admin::AdminMenu;

/// Everything the Facet values page shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacetValuesPage {
    pub csrf: String,
    /// The Facet shown: `month`, `competition`, `division_level`,
    /// `test_type` or `host`.
    pub facet: String,
    pub label: String,
    /// Every listed Facet, for the tabs.
    pub tabs: Vec<FacetTab>,
    /// The Facet's values, in display order.
    pub values: Vec<FacetValueRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacetTab {
    pub facet: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacetValueRow {
    pub id: i64,
    pub name: String,
    /// Tests using it, Hidden and Trashed ones too.
    pub tests: i64,
    /// Of those, the ones in the Trash.
    pub trashed_tests: i64,
    /// Former names, which old links and searches still find it by.
    pub aliases: Vec<String>,
}

// Posted natively by the browser; only the server reads them.

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct AddValueForm {
    pub csrf: String,
    pub facet: String,
    pub name: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct RenameValueForm {
    pub csrf: String,
    pub id: String,
    pub name: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct MergeValueForm {
    pub csrf: String,
    pub id: String,
    /// The surviving value's name (or a former name of it).
    pub into: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct DeleteValueForm {
    pub csrf: String,
    pub id: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct MoveValueForm {
    pub csrf: String,
    pub id: String,
    /// `up` or `down`.
    pub direction: String,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct ReorderForm {
    pub csrf: String,
    pub facet: String,
    /// The Facet's value IDs in their new order, comma-separated.
    pub order: String,
}

/// The page's data. An Admin endpoint: anonymous callers get 401.
#[get("/api/admin/facet-values?facet",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn facet_values_page(facet: String) -> Result<FacetValuesPage, ServerFnError> {
    crate::server::facet_values::page(&state, &headers, &facet).await
}

/// **+ Add value**. Every write answers 303 back to the page, or a page
/// saying why nothing was saved; 401/403 without a session or token.
#[post("/api/admin/facet-values",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn add_facet_value(form: Form<AddValueForm>) -> Result<Response, ServerFnError> {
    crate::server::facet_values::add(&state, &headers, form.0).await
}

#[post("/api/admin/facet-values/rename",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn rename_facet_value(form: Form<RenameValueForm>) -> Result<Response, ServerFnError> {
    crate::server::facet_values::rename(&state, &headers, form.0).await
}

#[post("/api/admin/facet-values/merge",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn merge_facet_value(form: Form<MergeValueForm>) -> Result<Response, ServerFnError> {
    crate::server::facet_values::merge(&state, &headers, form.0).await
}

#[post("/api/admin/facet-values/delete",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn delete_facet_value(form: Form<DeleteValueForm>) -> Result<Response, ServerFnError> {
    crate::server::facet_values::delete(&state, &headers, form.0).await
}

#[post("/api/admin/facet-values/move",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn move_facet_value(form: Form<MoveValueForm>) -> Result<Response, ServerFnError> {
    crate::server::facet_values::move_value(&state, &headers, form.0).await
}

#[post("/api/admin/facet-values/reorder",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn reorder_facet_values(form: Form<ReorderForm>) -> Result<Response, ServerFnError> {
    crate::server::facet_values::reorder(&state, &headers, form.0).await
}

const CSS: &str = r#"
.facet-tabs { display: flex; flex-wrap: wrap; gap: 4px; border-bottom: 1px solid var(--rule); margin: 0 0 12px; padding: 0 }
.facet-tabs a { display: block; padding: 6px 12px; border: 1px solid transparent; border-bottom: 0;
  border-radius: 6px 6px 0 0; color: var(--ink); font-weight: 500 }
.facet-tabs a[aria-current=page] { border-color: var(--rule); background: var(--paper); margin-bottom: -1px; color: var(--accent) }
.facet-tabs a:hover { text-decoration: none; background: var(--head) }
.values { list-style: none; margin: 0; padding: 0; border: 1px solid var(--rule); border-radius: 6px }
.values > li { display: grid; grid-template-columns: auto auto minmax(0, 1fr) auto; gap: 4px 10px; align-items: center;
  padding: 7px 10px; border-bottom: 1px solid var(--rule); background: var(--paper) }
.values > li:last-child { border-bottom: 0; border-radius: 0 0 6px 6px }
.values > li:first-child { border-radius: 6px 6px 0 0 }
.values > li.over { box-shadow: inset 0 2px 0 var(--accent) }
.values > li.dragging { opacity: .5 }
.values .handle { cursor: grab; color: var(--faint); user-select: none }
.values .move { display: flex; gap: 2px }
.values .move button, .values .edit button, .add-value button { border: 1px solid var(--rule); background: var(--paper);
  border-radius: 5px; padding: 2px 8px; cursor: pointer; font-size: .85rem }
.values .move button:disabled { color: var(--faint); cursor: default }
.values .name { font-weight: 600 }
.values .alias + .alias::before { content: ", " }
.values .muted { color: var(--muted); font-size: .85rem; margin: 6px 0 0 }
.values .uses, .values .aliases { color: var(--muted); font-size: .85rem }
.values .edit summary { cursor: pointer; color: var(--accent); font-size: .85rem }
.values .edit[open] { grid-column: 1 / -1 }
.values .edit form { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; margin: 6px 0 0 }
.values .edit input, .add-value input { padding: 3px 8px; border: 1px solid var(--rule); border-radius: 5px;
  background: var(--paper); font-size: .9rem; min-width: 0 }
.values .edit .delete button { color: #b00020 }
.add-value { display: flex; flex-wrap: wrap; gap: 6px; margin: 12px 0 0 }
.empty-facet { padding: 16px; color: var(--muted) }
"#;

/// `/admin/facets`, showing the Facet `?facet=` names (Month by default).
#[component]
pub fn FacetValues(facet: ReadSignal<String>) -> Element {
    let page = match use_loader(move || facet_values_page(facet())) {
        Ok(page) => page,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Facet values" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let page = page.read().clone();
    rsx! {
        document::Style { {CSS} }
        AdminMenu {}
        h2 { "Facet values" }
        nav { class: "facet-tabs", aria_label: "Facets",
            for tab in page.tabs.iter() {
                a {
                    key: "{tab.facet}",
                    href: "/admin/facets?facet={tab.facet}",
                    aria_current: if tab.facet == page.facet { "page" } else { "false" },
                    "{tab.label}"
                }
            }
        }
        ValueList { page: page.clone() }
        form {
            id: "add-value",
            class: "add-value",
            method: "post",
            action: "/api/admin/facet-values",
            input { r#type: "hidden", name: "csrf", value: "{page.csrf}" }
            input { r#type: "hidden", name: "facet", value: "{page.facet}" }
            input {
                r#type: "text",
                name: "name",
                required: true,
                aria_label: "New {page.label}",
                placeholder: "New {page.label}",
            }
            button { r#type: "submit", "+ Add value" }
        }
    }
}

/// The values, draggable by their handle once hydrated. A drop fills in the
/// hidden reorder form and submits it, so the order is saved like every
/// other change.
///
/// A row is `draggable` only while its handle is held: a draggable parent
/// stops the text fields inside it taking clicks in some browsers.
#[component]
fn ValueList(page: FacetValuesPage) -> Element {
    let mut grabbed = use_signal(|| None::<usize>);
    let mut dragging = use_signal(|| None::<usize>);
    let mut over = use_signal(|| None::<usize>);
    let ids: Vec<i64> = page.values.iter().map(|v| v.id).collect();
    let count = page.values.len();
    let targets_id = format!("merge-into-{}", page.facet);

    if page.values.is_empty() {
        return rsx! {
            p { class: "empty-facet", "No {page.label} values yet." }
        };
    }
    rsx! {
        form {
            id: "reorder",
            method: "post",
            action: "/api/admin/facet-values/reorder",
            input { r#type: "hidden", name: "csrf", value: "{page.csrf}" }
            input { r#type: "hidden", name: "facet", value: "{page.facet}" }
            input { r#type: "hidden", name: "order", value: "" }
        }
        datalist { id: "{targets_id}",
            for value in page.values.iter() {
                option { key: "{value.id}", value: "{value.name}" }
            }
        }
        ol { id: "values", class: "values",
            for (i , value) in page.values.iter().enumerate() {
                li {
                    key: "{value.id}",
                    id: "v{value.id}",
                    class: if over() == Some(i) && dragging() != Some(i) { "value over" } else if dragging() == Some(i) { "value dragging" } else { "value" },
                    draggable: if grabbed() == Some(i) { "true" } else { "false" },
                    ondragstart: move |event: DragEvent| {
                        // Firefox starts a drag only with some data set.
                        let _ = event.data_transfer().set_data("text/plain", &i.to_string());
                        event.data_transfer().set_effect_allowed("move");
                        dragging.set(Some(i));
                    },
                    ondragover: move |event: DragEvent| {
                        if dragging().is_some() {
                            event.prevent_default();
                            if over() != Some(i) {
                                over.set(Some(i));
                            }
                        }
                    },
                    ondragend: move |_| {
                        grabbed.set(None);
                        dragging.set(None);
                        over.set(None);
                    },
                    ondrop: {
                        let ids = ids.clone();
                        move |event: DragEvent| {
                            event.prevent_default();
                            let Some(from) = dragging() else { return };
                            dragging.set(None);
                            over.set(None);
                            if from == i {
                                return;
                            }
                            let mut order = ids.clone();
                            let moved = order.remove(from);
                            order.insert(i, moved);
                            let order: Vec<String> = order.iter().map(i64::to_string).collect();
                            // IDs only, so the script holds nothing typed by anyone.
                            let script = format!(
                                "const f = document.getElementById('reorder'); \
                                 f.elements.order.value = '{}'; f.requestSubmit();",
                                order.join(","),
                            );
                            document::eval(&script);
                        }
                    },
                    span {
                        class: "handle",
                        aria_hidden: "true",
                        title: "Drag to reorder",
                        onmousedown: move |_| grabbed.set(Some(i)),
                        onmouseup: move |_| grabbed.set(None),
                        "⠿"
                    }
                    form {
                        class: "move",
                        method: "post",
                        action: "/api/admin/facet-values/move",
                        input { r#type: "hidden", name: "csrf", value: "{page.csrf}" }
                        input { r#type: "hidden", name: "id", value: "{value.id}" }
                        button {
                            r#type: "submit",
                            name: "direction",
                            value: "up",
                            disabled: i == 0,
                            aria_label: "Move {value.name} up",
                            title: "Move up",
                            "↑"
                        }
                        button {
                            r#type: "submit",
                            name: "direction",
                            value: "down",
                            disabled: i + 1 == count,
                            aria_label: "Move {value.name} down",
                            title: "Move down",
                            "↓"
                        }
                    }
                    span {
                        span { class: "name", "{value.name}" }
                        " "
                        span { class: "uses",
                            "· "
                            span { class: "count", "{value.tests}" }
                            if value.tests == 1 { " test" } else { " tests" }
                            if value.trashed_tests > 0 { " ({value.trashed_tests} in Trash)" }
                        }
                        if !value.aliases.is_empty() {
                            " "
                            span { class: "aliases",
                                "· formerly "
                                for alias in value.aliases.iter() {
                                    span { key: "{alias}", class: "alias", "{alias}" }
                                }
                            }
                        }
                    }
                    details { class: "edit",
                        summary { "Edit" }
                        form {
                            class: "rename",
                            method: "post",
                            action: "/api/admin/facet-values/rename",
                            input { r#type: "hidden", name: "csrf", value: "{page.csrf}" }
                            input { r#type: "hidden", name: "id", value: "{value.id}" }
                            input {
                                r#type: "text",
                                name: "name",
                                value: "{value.name}",
                                required: true,
                                aria_label: "New name for {value.name}",
                            }
                            button { r#type: "submit", "Rename" }
                            span { class: "muted", "The old name keeps working in links and searches." }
                        }
                        form {
                            class: "merge",
                            method: "post",
                            action: "/api/admin/facet-values/merge",
                            input { r#type: "hidden", name: "csrf", value: "{page.csrf}" }
                            input { r#type: "hidden", name: "id", value: "{value.id}" }
                            input {
                                r#type: "text",
                                name: "into",
                                list: "{targets_id}",
                                required: true,
                                aria_label: "Merge {value.name} into",
                                placeholder: "Another {page.label}",
                            }
                            button { r#type: "submit", "Merge into…" }
                            span { class: "muted", "Its Tests move there, and this name becomes a former name of it." }
                        }
                        if value.tests == 0 {
                            form {
                                class: "delete",
                                method: "post",
                                action: "/api/admin/facet-values/delete",
                                input { r#type: "hidden", name: "csrf", value: "{page.csrf}" }
                                input { r#type: "hidden", name: "id", value: "{value.id}" }
                                button { r#type: "submit", "Delete" }
                            }
                        } else {
                            p { class: "muted", "In use, so it can’t be deleted; merge it instead." }
                        }
                    }
                }
            }
        }
    }
}
