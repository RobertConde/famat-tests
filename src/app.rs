//! The Dioxus app, shared by the server (SSR) and the WASM client.
//!
//! Everything here compiles for both targets; server-only code lives in
//! `crate::server` and is only referenced from server-function bodies.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod admin;
use admin::{AdminMenu, Login, Logout};

#[derive(Routable, Clone, Debug, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Shell)]
    #[route("/")]
    Search {},
    #[route("/login?:error")]
    Login { error: String },
    #[route("/logout")]
    Logout {},
}

#[component]
pub fn App() -> Element {
    rsx! {
        document::Title { "FAMAT Test Bank" }
        document::Link { rel: "preconnect", href: "https://fonts.googleapis.com" }
        document::Link { rel: "stylesheet", href: FONTS }
        document::Style { {CSS} }
        Router::<Route> {}
    }
}

const FONTS: &str = "https://fonts.googleapis.com/css2?family=Public+Sans:wght@400;500;600;700\
                     &family=IBM+Plex+Mono:wght@400;500&display=swap";

/// The look of the original FAMAT archive (final design: branch
/// `prototype/search-page`, `template.html`). Light only, sans-serif only.
const CSS: &str = r#"
:root { --paper: #fff; --ink: #18202c; --muted: #5d6675; --rule: #dfe3e8; --head: #f4f6f8;
  --accent: #1f4e9c; --faint: #a4acb8;
  --f-body: "Public Sans", system-ui, sans-serif; --f-mono: "IBM Plex Mono", ui-monospace, monospace;
  color-scheme: light }
* { box-sizing: border-box }
body { margin: 0; background: var(--paper); color: var(--ink); font: 15px/1.45 var(--f-body);
  padding: 0 max(16px, env(safe-area-inset-right, 0px)) 24px max(16px, env(safe-area-inset-left, 0px)) }
main { max-width: 1180px; margin-inline: auto }
h1 { font-size: 1.35rem; font-weight: 700; margin: 0; padding-block: 18px 12px; text-align: center }
button, select, input { font: inherit; color: inherit }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px }
.mono { font-family: var(--f-mono); font-variant-numeric: tabular-nums }
.filters { width: fit-content; max-width: 100%; margin: 4px auto 0; border: 1px solid var(--rule); border-radius: 6px;
  padding: 12px 16px 14px; display: grid; grid-template-columns: max-content minmax(0, 17rem); gap: 8px 12px; align-items: center }
.filters legend { font-weight: 700; padding-inline: 4px }
.filters .l { text-align: right; font-weight: 600; font-size: .9rem }
.filters select { width: 100%; min-width: 0; padding: 5px 8px; background: var(--paper); border: 1px solid var(--rule);
  border-radius: 5px; font-size: .9rem; color: var(--ink) }
.filters .checks { display: flex; flex-wrap: wrap; gap: 4px 14px; font-size: .9rem }
.filters .checks label { display: flex; gap: 5px; align-items: center }
.filters .actions { grid-column: 1 / -1; display: flex; justify-content: flex-end; gap: 12px; align-items: center; padding-top: 4px }
.filters .reset { font-weight: 400 }
.filters .go { border: 1px solid var(--accent); background: var(--accent); color: var(--paper); border-radius: 5px;
  padding: 5px 18px; font-weight: 600; cursor: pointer }
.count { text-align: center; font-size: .85rem; color: var(--muted); padding-block: 10px; margin: 0 }
.count b { color: var(--ink) }
.tbl { overflow-x: auto; border: 1px solid var(--rule); border-radius: 6px }
table { border-collapse: collapse; width: 100% }
th { text-align: left; font-size: .75rem; color: var(--muted); font-weight: 600; background: var(--head); padding: 7px 10px;
  white-space: nowrap; border-bottom: 1px solid var(--rule) }
td { padding: 7px 10px; border-bottom: 1px solid var(--rule); vertical-align: top }
tr:last-child td { border-bottom: 0 }
.u { color: var(--faint) }
.docs { display: grid; gap: 2px }
a.original { font-weight: 400; font-size: .8rem; color: var(--muted); margin-left: 6px }
a { color: var(--accent); text-decoration: none; font-weight: 500 }
a:hover { text-decoration: underline }
.pager { display: flex; gap: 8px; align-items: center; justify-content: flex-end; padding-block: 10px; font-size: .85rem }
.pager a, .pager .off { border: 1px solid var(--rule); background: var(--paper); border-radius: 5px; padding: 3px 10px;
  color: var(--ink); font-weight: 400 }
.pager a:hover { background: var(--head); text-decoration: none }
.pager .off { color: var(--faint) }
.empty { padding: 28px; text-align: center; color: var(--muted) }
.credit { text-align: center; color: var(--muted); font-size: .85rem; padding-block: 28px 8px; border-top: 1px solid var(--rule); margin-top: 24px }
@media (max-width: 700px) {
  .filters { width: 100%; grid-template-columns: max-content minmax(0, 1fr) }
  .tbl { border: 0 }
  thead { display: none }
  tr { display: grid; grid-template-columns: auto 1fr; gap: 2px 10px; padding: 10px 0; border-bottom: 1px solid var(--rule) }
  td { border: 0; padding: 0 }
  td[data-l]::before { content: attr(data-l) ": "; color: var(--muted); font-size: .8rem }
  td.year { grid-row: span 2; font-weight: 600 }
  td.wide { grid-column: 1 / -1 }
}
"#;

#[component]
fn Shell() -> Element {
    rsx! {
        main {
            h1 { "FAMAT Test Bank" }
            Outlet::<Route> {}
            footer { class: "credit", "Made by and maintained by Rob from Doral Academy" }
        }
    }
}

// ------------------------------------------------------------------ search

/// A Facet value as shown on a Test. Month may also be not applicable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FacetValue {
    Known(String),
    Unknown,
    /// Month only: State, National and Mail-in have no month.
    NotApplicable,
}

/// One Published Document of a Test, as linked from the results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentLink {
    pub id: i64,
    pub kind: String,
    pub label: Option<String>,
    /// The Original's extension (without the dot) when the Original is not a
    /// PDF, for the small download link beside the PDF one.
    pub original_extension: Option<String>,
}

/// One Test in the search results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestRow {
    pub id: i64,
    pub year: Option<i16>,
    pub month: FacetValue,
    pub competition: FacetValue,
    pub host: FacetValue,
    pub division_level: FacetValue,
    pub test_type: FacetValue,
    pub documents: Vec<DocumentLink>,
}

/// A search, as the query string states it. Facet fields hold the raw
/// value: empty for Any, [`UNKNOWN`] for Unknown, otherwise a Facet value's
/// name (or a year).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub year: String,
    pub month: String,
    pub competition: String,
    pub division_level: String,
    pub test_type: String,
    pub host: String,
    pub includes_test: bool,
    pub includes_answers: bool,
    pub includes_solutions: bool,
}

/// The query-string value of a dropdown's **Unknown** option.
pub const UNKNOWN: &str = "unknown";

/// The values each dropdown offers, in Facet display order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Years of Published Tests, newest first.
    pub years: Vec<i16>,
    pub months: Vec<String>,
    pub competitions: Vec<String>,
    pub division_levels: Vec<String>,
    pub test_types: Vec<String>,
    pub hosts: Vec<String>,
}

/// One page of search results, with what the form needs to show the search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: SearchQuery,
    pub options: SearchOptions,
    /// Matching Tests on every page.
    pub total: i64,
    /// 1-based; always within `1..=pages`.
    pub page: i64,
    pub pages: i64,
    /// Links to the neighbouring pages of the same search.
    pub previous: Option<String>,
    pub next: Option<String>,
    pub tests: Vec<TestRow>,
}

/// A page of Published Tests matching the search in the request's query
/// string, in results order. Public: it never returns Hidden or Trashed
/// Tests or Documents, so it needs no authorization.
///
/// It reads the raw query string itself (during SSR, the page's), because
/// the router decodes the query before splitting it, which loses `&`, `=`
/// and `+` inside values.
#[get("/api/tests",
      state: dioxus::fullstack::extract::Extension<crate::server::AppState>,
      raw_query: dioxus::fullstack::extract::RawQuery)]
pub async fn search_tests() -> Result<SearchResults, ServerFnError> {
    crate::server::search::search(&state.db, raw_query.0.as_deref())
        .await
        .map_err(crate::server::internal_error)
}

/// The search page. The form is a plain GET form and the pager plain links,
/// so every search is a server-rendered page load and works without JS.
#[component]
fn Search() -> Element {
    let results = use_loader(search_tests)?;
    let results = results.read();
    let SearchResults { query, options, .. } = &*results;
    let years: Vec<String> = options.years.iter().map(i16::to_string).collect();
    let total = results.total;
    let total_text = thousands(total);

    rsx! {
        AdminMenu {}
        form { id: "filters", method: "get", action: "/",
            fieldset { class: "filters",
                legend { "Search Options" }
                FacetSelect { name: "year", label: "Year", options: years, selected: query.year.clone() }
                FacetSelect { name: "month", label: "Month", options: options.months.clone(), selected: query.month.clone() }
                FacetSelect {
                    name: "competition",
                    label: "Competition",
                    options: options.competitions.clone(),
                    selected: query.competition.clone(),
                }
                FacetSelect {
                    name: "division_level",
                    label: "Division/Level",
                    options: options.division_levels.clone(),
                    selected: query.division_level.clone(),
                }
                FacetSelect {
                    name: "test_type",
                    label: "Test type",
                    options: options.test_types.clone(),
                    selected: query.test_type.clone(),
                }
                FacetSelect { name: "host", label: "Host", options: options.hosts.clone(), selected: query.host.clone() }
                span { class: "l", "Includes:" }
                span { class: "checks",
                    Include { value: "test", label: "Test", checked: query.includes_test }
                    Include { value: "answers", label: "Answers", checked: query.includes_answers }
                    Include { value: "solutions", label: "Solutions", checked: query.includes_solutions }
                }
                div { class: "actions",
                    a { class: "reset", href: "/", "Clear" }
                    button { class: "go", r#type: "submit", "Search" }
                }
            }
        }
        p { id: "count", class: "count",
            b { "{total_text}" }
            if total == 1 { " test" } else { " tests" }
        }
        if results.tests.is_empty() {
            div { class: "empty", "No tests match. Clear a filter." }
        } else {
            div { class: "tbl",
                table { id: "results",
                    thead {
                        tr {
                            th { "Year" }
                            th { "Event" }
                            th { "Host" }
                            th { "Division/Level" }
                            th { "Test type" }
                            th { "Documents" }
                        }
                    }
                    tbody {
                        for test in results.tests.iter() {
                            Row { key: "{test.id}", test: test.clone() }
                        }
                    }
                }
            }
        }
        if results.pages > 1 {
            nav { class: "pager", aria_label: "Pages",
                span { class: "mono", id: "page", "{results.page} / {results.pages}" }
                PageLink { href: results.previous.clone(), rel: "prev", label: "Previous" }
                PageLink { href: results.next.clone(), rel: "next", label: "Next" }
            }
        }
    }
}

/// One stacked `Label:` dropdown: Any, the values, then Unknown.
#[component]
fn FacetSelect(name: &'static str, label: &'static str, options: Vec<String>, selected: String) -> Element {
    let id = format!("f-{name}");
    rsx! {
        label { class: "l", r#for: "{id}", "{label}:" }
        select { id: "{id}", name,
            option { value: "", selected: selected.is_empty(), "Any" }
            for value in options.iter() {
                option { key: "{value}", value: "{value}", selected: *value == selected, "{value}" }
            }
            option { value: UNKNOWN, selected: selected == UNKNOWN, "Unknown" }
        }
    }
}

/// One **Includes** checkbox.
#[component]
fn Include(value: &'static str, label: &'static str, checked: bool) -> Element {
    rsx! {
        label {
            input { r#type: "checkbox", id: "f-{value}", name: "includes", value, checked }
            "{label}"
        }
    }
}

/// Previous or Next: a link, or a faint label on the first or last page.
#[component]
fn PageLink(href: Option<String>, rel: &'static str, label: &'static str) -> Element {
    match href {
        Some(href) => rsx! {
            a { id: "{rel}", href: "{href}", rel, "{label}" }
        },
        None => rsx! {
            span { class: "off", aria_disabled: "true", "{label}" }
        },
    }
}

/// A result row. Cells carry labels for the stacked layout on phones.
#[component]
fn Row(test: TestRow) -> Element {
    rsx! {
        tr { id: "T{test.id}",
            td { class: "year mono",
                match test.year {
                    Some(year) => rsx! { "{year}" },
                    None => rsx! { Unknown {} },
                }
            }
            td { class: "event", "data-l": "Event",
                Event { month: test.month.clone(), competition: test.competition.clone() }
            }
            td { class: "host", "data-l": "Host", Facet { value: test.host.clone() } }
            td { class: "division-level", "data-l": "Division/Level",
                Facet { value: test.division_level.clone() }
            }
            td { class: "test-type", "data-l": "Test type", Facet { value: test.test_type.clone() } }
            td { class: "documents wide",
                div { class: "docs",
                    for document in test.documents.iter() {
                        span { key: "{document.id}", class: "document",
                            a { href: "/tests/{test.id}/documents/{document.id}/pdf",
                                "{document.kind}"
                                if let Some(label) = &document.label { " ({label})" }
                            }
                            if let Some(extension) = &document.original_extension {
                                " "
                                a {
                                    class: "original",
                                    href: "/tests/{test.id}/documents/{document.id}/original",
                                    title: "Download the Original",
                                    if extension.is_empty() { "original" } else { ".{extension}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// "{Month} {Competition}", e.g. "January Regional" or just "State". An
/// unknown Month still shows its dash; a not-applicable one shows nothing.
#[component]
fn Event(month: FacetValue, competition: FacetValue) -> Element {
    rsx! {
        match month {
            FacetValue::NotApplicable => rsx! {},
            month => rsx! { Facet { value: month } " " },
        }
        Facet { value: competition }
    }
}

#[component]
fn Facet(value: FacetValue) -> Element {
    match value {
        FacetValue::Known(name) => rsx! { "{name}" },
        FacetValue::Unknown | FacetValue::NotApplicable => rsx! { Unknown {} },
    }
}

/// Unknown values show as a faint dash.
#[component]
fn Unknown() -> Element {
    rsx! { span { class: "u", title: "Unknown", "—" } }
}

/// 1234567 → "1,234,567".
fn thousands(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
