//! The **Change log** page (`/admin/change-log`): every Admin change, newest
//! first, one block per Admin action (the entries that share a
//! `changed_at`), each entry showing the record, the field and before →
//! after. A record's ID links to where it can be seen, while it exists.
//!
//! The filter is a plain GET form (`entity` and `id`), and the pager plain
//! links, so the page works without JavaScript. The data comes from
//! `crate::server::change_log_page`; the entries are written by
//! `crate::server::change_log`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{HeaderMap, Loading};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::admin::AdminMenu;

/// The entity types the filter offers, as `(value, label)`.
pub const ENTITIES: [(&str, &str); 4] = [
    ("test", "Tests"),
    ("document", "Documents"),
    ("facet_value", "Facet values"),
    ("document_kind", "Document kinds"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeLogPage {
    /// The filter as understood: an entity type (or empty for any) and the
    /// ID as typed.
    pub entity: String,
    pub id: String,
    /// Set when the typed ID was not understood, so it was ignored.
    pub bad_id: bool,
    /// 1-based, within `1..=pages`.
    pub page: i64,
    pub pages: i64,
    /// Admin actions matching the filter, on every page.
    pub total: i64,
    /// Newest first.
    pub actions: Vec<LoggedAction>,
}

/// One Admin action: the entries written in one transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoggedAction {
    /// When, as an Admin reads it.
    pub when: String,
    /// In the order they were written.
    pub entries: Vec<LoggedEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoggedEntry {
    pub id: i64,
    /// `test`, `document`, `facet_value` or `document_kind`.
    pub entity: String,
    pub entity_id: i64,
    /// e.g. `T12`, `D34`, `Facet value 7`.
    pub record: String,
    /// Where the record can be seen, while it still exists.
    pub link: Option<String>,
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The page's data for a filter and page, as the query string has them. An
/// Admin endpoint: anonymous callers get 401.
#[get("/api/admin/change-log?entity&id&page",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn change_log_page(entity: String, id: String, page: String) -> Result<ChangeLogPage, ServerFnError> {
    crate::server::change_log_page::page(&state, &headers, &entity, &id, &page).await
}

const CSS: &str = r#"
#change-log-filter { display: flex; flex-wrap: wrap; gap: 8px 12px; align-items: center; margin: 0 0 12px;
  border: 1px solid var(--rule); border-radius: 6px; padding: 10px 16px; font-size: .9rem }
#change-log-filter select, #change-log-filter input { padding: 4px 8px; border: 1px solid var(--rule); border-radius: 5px;
  background: var(--paper); font-size: .9rem }
#change-log-filter input { width: 8rem }
#change-log-filter button { border: 1px solid var(--accent); background: var(--accent); color: var(--paper);
  border-radius: 5px; padding: 4px 14px; font-weight: 600; cursor: pointer }
.change-log .action { margin: 0 0 14px }
.change-log .action h3 { font-size: .9rem; font-weight: 600; color: var(--muted); margin: 0 0 4px }
.change-log td.record { white-space: nowrap; width: 1% }
.change-log td.field { white-space: nowrap; width: 1%; color: var(--muted) }
.change-log td.change { overflow-wrap: anywhere; white-space: pre-line }
.change-log .before { color: #8a1c1c }
.change-log .after { color: #1d5e2b }
.change-log .arrow { color: var(--muted); padding-inline: 6px }
"#;

/// `/admin/change-log?entity=…&id=…&page=…`.
#[component]
pub fn ChangeLog(entity: ReadSignal<String>, id: ReadSignal<String>, page: ReadSignal<String>) -> Element {
    let data = match use_loader(move || change_log_page(entity(), id(), page())) {
        Ok(data) => data,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Change log" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let data = data.read();
    let page_link = |page: i64| {
        let mut query = vec![("entity", data.entity.clone()), ("id", data.id.clone())];
        query.retain(|(_, value)| !value.is_empty());
        query.push(("page", page.to_string()));
        let query: Vec<String> = query
            .into_iter()
            .map(|(name, value)| format!("{name}={}", encode(&value)))
            .collect();
        format!("/admin/change-log?{}", query.join("&"))
    };
    rsx! {
        document::Style { {CSS} }
        AdminMenu {}
        h2 { "Change log" }
        form { id: "change-log-filter", method: "get", action: "/admin/change-log",
            label {
                "Show "
                select { name: "entity",
                    option { value: "", selected: data.entity.is_empty(), "Everything" }
                    for (value , label) in ENTITIES {
                        option { key: "{value}", value, selected: data.entity == value, "{label}" }
                    }
                }
            }
            label {
                "ID "
                input { r#type: "text", name: "id", value: "{data.id}", placeholder: "T12 or D34" }
            }
            button { r#type: "submit", "Filter" }
            if !data.entity.is_empty() || !data.id.is_empty() {
                a { href: "/admin/change-log", "Show everything" }
            }
        }
        if data.bad_id {
            p { id: "bad-id", class: "error", "“{data.id}” is not an ID, so every ID is shown. Type one like T12, D34 or 7." }
        }
        if data.actions.is_empty() {
            p { id: "no-changes", class: "empty", "No changes yet." }
        }
        div { class: "change-log",
            for (i , action) in data.actions.iter().enumerate() {
                section { key: "{i}-{action.when}", class: "action",
                    h3 { class: "when mono", "{action.when}" }
                    div { class: "tbl",
                        table {
                            tbody {
                                for entry in action.entries.iter() {
                                    tr { key: "{entry.id}", class: "entry", id: "C{entry.id}",
                                        td { class: "record mono",
                                            if let Some(link) = &entry.link {
                                                a { href: "{link}", "{entry.record}" }
                                            } else {
                                                "{entry.record}"
                                            }
                                        }
                                        td { class: "field", "{entry.field}" }
                                        td { class: "change",
                                            span { class: "before", {shown(&entry.before)} }
                                            span { class: "arrow", "→" }
                                            span { class: "after", {shown(&entry.after)} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if data.pages > 1 {
            nav { class: "pager", aria_label: "Pages",
                span { class: "mono", id: "page", "{data.page} / {data.pages}" }
                if data.page > 1 {
                    a { id: "newer", href: page_link(data.page - 1), "Newer" }
                }
                if data.page < data.pages {
                    a { id: "older", href: page_link(data.page + 1), "Older" }
                }
            }
        }
    }
}

/// A value as the page shows it; `—` for unknown or absent.
fn shown(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "—".to_owned())
}

/// Percent-encode a query-string value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
