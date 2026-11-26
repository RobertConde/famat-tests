//! The Dioxus app, shared by the server (SSR) and the WASM client.
//!
//! Everything here compiles for both targets; server-only code lives in
//! `crate::server` and is only referenced from server-function bodies.

use std::{collections::HashSet, rc::Rc};

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

pub mod add_test;
mod admin;
pub mod upload_rules;
use add_test::AddTest;
use admin::{AdminMenu, Login, Logout};

use crate::download::{
    BULK_ZIP_PATH, Contents, Format, GroupCounts, KindGroup, MAX_BULK_TESTS, selection_value, zip_href,
};

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
    #[route("/admin/tests/new?:added&:pending&:notice")]
    AddTest { added: String, pending: String, notice: String },
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
  --accent: #1f4e9c; --accent-soft: #e8eef9; --faint: #a4acb8;
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
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap }
/* Not a scroll container: it would clip the download menus of the last rows. */
.tbl { border: 1px solid var(--rule); border-radius: 6px }
th:first-child { border-top-left-radius: 5px }
th:last-child { border-top-right-radius: 5px }
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
.split { position: relative; display: inline-flex; border: 1px solid var(--rule); border-radius: 6px; background: var(--paper) }
.split > a, .split > button, .split summary { padding: 4px 8px; cursor: pointer; color: var(--ink); font-weight: 400; display: inline-flex;
  align-items: center; border-radius: 5px }
.split > a, .split > button { border-radius: 5px 0 0 5px }
.split > button { border: 0; background: none }
.split details { display: flex }
.split summary { list-style: none; border-left: 1px solid var(--rule); padding: 4px 6px; color: var(--muted); border-radius: 0 5px 5px 0 }
.split summary::-webkit-details-marker { display: none }
.split > a:hover, .split > button:hover, .split summary:hover { background: var(--head); text-decoration: none }
.split svg { width: 16px; height: 16px }
.scrim { position: fixed; inset: 0; z-index: 19 }
.menu { position: absolute; top: calc(100% + 4px); right: 0; z-index: 20; min-width: 210px; background: var(--paper);
  border: 1px solid var(--rule); border-radius: 8px; box-shadow: 0 8px 24px rgb(15 23 42 / .14); padding: 4px; display: grid; text-align: left }
.menu > a, .menu > button { padding: 6px 10px; border-radius: 5px; display: flex; justify-content: space-between; gap: 16px; font-size: .9rem;
  color: var(--ink); font-weight: 400 }
.menu > button { border: 0; background: none; cursor: pointer; text-align: left }
.menu > a:hover, .menu > a:focus-visible, .menu > button:hover, .menu > button:focus-visible { background: var(--head); text-decoration: none }
.menu .n { color: var(--muted); font-variant-numeric: tabular-nums }
.menu hr { border: 0; border-top: 1px solid var(--rule); margin: 4px 0 }
.menu .fmt { display: flex; gap: 2px; padding: 4px 6px; align-items: center; font-size: .8rem; color: var(--muted) }
.menu .fmt span { margin-right: auto }
.menu .fmt a { border-radius: 4px; padding: 3px 8px; font-size: .8rem; color: var(--ink); font-weight: 400 }
.menu .fmt a:hover { background: var(--head); text-decoration: none }
.menu .fmt a[aria-current="true"] { background: var(--accent-soft); color: var(--accent); font-weight: 600 }
.pager { display: flex; gap: 8px; align-items: center; justify-content: flex-end; padding-block: 10px; font-size: .85rem }
.pager button, .pager .off { border: 1px solid var(--rule); background: var(--paper); border-radius: 5px; padding: 3px 10px;
  color: var(--ink); font-weight: 400; cursor: pointer }
.pager button:hover { background: var(--head) }
.bulk { display: flex; flex-wrap: wrap; align-items: center; justify-content: center; gap: 8px 12px; padding: 8px; margin-bottom: 8px;
  background: var(--head); border-radius: 6px; font-size: .9rem }
.bulk .note { color: var(--muted) }
.linkish { border: 0; background: none; color: var(--accent); cursor: pointer; padding: 0; font-weight: 500 }
.linkish:hover { text-decoration: underline }
/* Without JS the bar is as the page was loaded: once a tick changes, show it
   (if hidden) with its Update selection button. */
.bulk.none, .bulk .update { display: none }
#picks:not(.live):has(input.pick:checked:not([checked]), input.pick[checked]:not(:checked), #pickpage:checked:not([checked])) .bulk { display: flex }
#picks:not(.live):has(input.pick:checked:not([checked]), input.pick[checked]:not(:checked), #pickpage:checked:not([checked])) .bulk .update { display: inline }
th.pick, td.pick { width: 1%; padding-right: 0 }
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
  tr { position: relative }
  td.pick { position: absolute; right: 0; top: 10px }
  .menu { right: auto; left: 0 }
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
    /// What its kind contains, for the download menu's kind groups.
    pub contents: Contents,
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
    /// The download menus' Format switch, kept for the page (`format`).
    pub format: Format,
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
    /// This page under each Format, for the Format switch without JS.
    pub format_links: Vec<(Format, String)>,
    /// This page with nothing selected.
    pub clear_selection: String,
    pub tests: Vec<TestRow>,
    /// Every matching Test, when they would fit in one bulk zip
    /// ([`MAX_BULK_TESTS`]), so the page can tell whether all are selected.
    pub result_ids: Option<Vec<i64>>,
    /// The selected Tests that are Published and untrashed, on any page.
    pub selection: Vec<SelectedTest>,
}

/// A Test selected for a bulk download, with its Documents' counts for the
/// bulk download menu.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectedTest {
    pub id: i64,
    pub counts: GroupCounts,
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

/// The search page. The search form is a plain GET form, so every search
/// is a server-rendered page load and works without JS.
///
/// The results, the bulk bar and the pager are one more GET form,
/// `#picks`, carrying the search and the selection: each row's checkbox is a
/// `sel` field, and Tests selected on other pages are a hidden one. Paging,
/// Select all and the bulk downloads are its submit buttons, so ticks
/// travel with them without JS. Hydrated, the selection is a signal: the
/// bulk bar follows each tick at once.
#[component]
fn Search() -> Element {
    let results = use_loader(search_tests)?;
    let results = results.read();
    let SearchResults { query, options, .. } = &*results;
    let years: Vec<String> = options.years.iter().map(i16::to_string).collect();
    let total = results.total;
    let total_text = thousands(total);
    let format = query.format;
    let chosen = use_signal(|| format);
    use_context_provider(|| DownloadFormat {
        chosen,
        links: results.format_links.clone(),
    });
    let selection = use_signal(|| results.selection.clone());
    use_context_provider(|| Selection(selection));
    let mut live = use_signal(|| false);
    use_effect(move || live.set(true));

    let page_ids: Vec<i64> = results.tests.iter().map(|t| t.id).collect();
    let selected_ids: Vec<i64> = selection.read().iter().map(|t| t.id).collect();
    let off_page = selection_value(selected_ids.iter().copied().filter(|id| !page_ids.contains(id)));
    let all_selected = selection_value(selected_ids);
    let page_rows: Vec<SelectedTest> = results.tests.iter().map(SelectedTest::of).collect();

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
                if format != Format::default() {
                    input { r#type: "hidden", name: "format", value: format.slug() }
                }
                // A new search keeps the selection.
                if !all_selected.is_empty() {
                    input { r#type: "hidden", name: "sel", value: "{all_selected}" }
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
        form {
            id: "picks",
            class: if live() { "live" } else { "" },
            method: "get",
            action: "/",
            SearchFields { query: query.clone() }
            if chosen() != Format::default() {
                input { r#type: "hidden", name: "format", value: chosen().slug() }
            }
            if !off_page.is_empty() {
                input { r#type: "hidden", name: "sel", value: "{off_page}" }
            }
            BulkBar {
                total,
                result_ids: results.result_ids.clone(),
                clear: results.clear_selection.clone(),
                page: results.page,
            }
            if results.tests.is_empty() {
                div { class: "empty", "No tests match. Clear a filter." }
            } else {
                div { class: "tbl",
                    table { id: "results",
                        thead {
                            tr {
                                th { class: "pick", PagePick { rows: page_rows } }
                                th { "Year" }
                                th { "Event" }
                                th { "Host" }
                                th { "Division/Level" }
                                th { "Test type" }
                                th { "Documents" }
                                th { span { class: "sr", "Download" } }
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
                    PageButton {
                        page: (results.page > 1).then(|| results.page - 1),
                        id: "prev",
                        label: "Previous",
                    }
                    PageButton {
                        page: (results.page < results.pages).then(|| results.page + 1),
                        id: "next",
                        label: "Next",
                    }
                }
            }
        }
    }
}

/// The search as hidden fields of the selection form, so its submit
/// buttons keep it.
#[component]
fn SearchFields(query: SearchQuery) -> Element {
    let fields: Vec<(&'static str, String)> = [
        ("year", query.year),
        ("month", query.month),
        ("competition", query.competition),
        ("division_level", query.division_level),
        ("test_type", query.test_type),
        ("host", query.host),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .chain(
        [
            (query.includes_test, "test"),
            (query.includes_answers, "answers"),
            (query.includes_solutions, "solutions"),
        ]
        .into_iter()
        .filter(|(included, _)| *included)
        .map(|(_, value)| ("includes", value.to_owned())),
    )
    .collect();
    rsx! {
        for (name , value) in fields {
            input { r#type: "hidden", name, value }
        }
    }
}

/// The Tests selected for a bulk download, shared by the rows, the page
/// checkbox and the bulk bar.
#[derive(Clone, Copy)]
struct Selection(Signal<Vec<SelectedTest>>);

impl Selection {
    fn contains(&self, id: i64) -> bool {
        self.0.read().iter().any(|t| t.id == id)
    }

    /// Select or deselect a Test.
    fn set(&mut self, test: SelectedTest, selected: bool) {
        let mut tests = self.0.write();
        tests.retain(|t| t.id != test.id);
        if selected {
            tests.push(test);
        }
    }
}

impl SelectedTest {
    fn of(test: &TestRow) -> Self {
        Self {
            id: test.id,
            counts: GroupCounts::of(test.documents.iter().map(|d| d.contents)),
        }
    }
}

/// The header checkbox: selects every Test on this page. Its value lists
/// them, so it works without JS too, though only to select.
#[component]
fn PagePick(rows: Vec<SelectedTest>) -> Element {
    let mut selection = use_context::<Selection>();
    let all = !rows.is_empty() && rows.iter().all(|t| selection.contains(t.id));
    let value = selection_value(rows.iter().map(|t| t.id));
    rsx! {
        input {
            r#type: "checkbox",
            id: "pickpage",
            name: "sel",
            value,
            checked: all,
            aria_label: "Select all on this page",
            onchange: move |event| {
                for test in &rows {
                    selection.set(*test, event.checked());
                }
            },
        }
    }
}

/// Shown while anything is selected: "N selected", the split download
/// button for the selection, Select all N results and Clear selection.
///
/// Without JS the bar is as the page was loaded, so it also has an
/// "Update selection" button, shown (by CSS) once a checkbox changes.
#[component]
fn BulkBar(total: i64, result_ids: Option<Vec<i64>>, clear: String, page: i64) -> Element {
    let mut selection = use_context::<Selection>();
    let selected = selection.0.read().clone();
    let count = selected.len();
    let counts = selected
        .iter()
        .fold(GroupCounts::default(), |sum, t| sum.plus(t.counts));
    let selected_ids: HashSet<i64> = selected.iter().map(|t| t.id).collect();
    // How many matching Tests aren't selected yet, when that can be known.
    let unselected = result_ids.map(|ids| ids.iter().filter(|id| !selected_ids.contains(id)).count());
    let can_select_all = unselected.is_some_and(|n| n > 0 && count + n <= MAX_BULK_TESTS);
    let over_cap = count > MAX_BULK_TESTS;
    // Too many to select or download them all.
    let capped = over_cap || (unselected != Some(0) && !can_select_all);
    let count_text = thousands(count as i64);
    let total_text = thousands(total);

    rsx! {
        div { id: "bulk", class: if count > 0 { "bulk" } else { "bulk none" },
            if count > 0 {
                span { id: "selected",
                    b { "{count_text}" }
                    " selected"
                }
                if !over_cap {
                    DownloadButton { target: Target::Selection, counts }
                }
                if can_select_all {
                    button {
                        class: "linkish",
                        id: "select-all",
                        r#type: "submit",
                        name: "select",
                        value: "all",
                        "Select all {total_text} results"
                    }
                }
                if capped {
                    span { class: "note", "Up to {MAX_BULK_TESTS} tests can be downloaded together." }
                }
                a {
                    class: "linkish",
                    id: "clear-selection",
                    href: clear,
                    onclick: move |event| {
                        event.prevent_default();
                        selection.0.set(Vec::new());
                    },
                    "Clear selection"
                }
            }
            button {
                class: "linkish update",
                r#type: "submit",
                name: "page",
                value: "{page}",
                "Update selection"
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

/// Previous or Next: a submit button of the selection form, so the
/// selection goes along, or a faint label on the first or last page.
#[component]
fn PageButton(page: Option<i64>, id: &'static str, label: &'static str) -> Element {
    match page {
        Some(page) => rsx! {
            button { id, r#type: "submit", name: "page", value: "{page}", "{label}" }
        },
        None => rsx! {
            span { class: "off", aria_disabled: "true", "{label}" }
        },
    }
}

/// A result row. Cells carry labels for the stacked layout on phones.
#[component]
fn Row(test: TestRow) -> Element {
    let mut selection = use_context::<Selection>();
    let picked = SelectedTest::of(&test);
    rsx! {
        tr { id: "T{test.id}",
            td { class: "pick",
                input {
                    r#type: "checkbox",
                    class: "pick",
                    name: "sel",
                    value: "{test.id}",
                    checked: selection.contains(test.id),
                    aria_label: "Select this test",
                    onchange: move |event| selection.set(picked, event.checked()),
                }
            }
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
            td { class: "download wide",
                if !test.documents.is_empty() {
                    DownloadButton { target: Target::Test(test.id), counts: picked.counts }
                }
            }
        }
    }
}

/// The page's Format switch, shared by every row's download menu: the
/// chosen Format, and this page's URL under each one (the switch's links
/// without JS).
#[derive(Clone)]
struct DownloadFormat {
    chosen: Signal<Format>,
    links: Vec<(Format, String)>,
}

/// What a split download button downloads.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    /// One Test's zip: its items are links.
    Test(i64),
    /// The bulk zip of the selection: its items are submit buttons of the
    /// selection form, so they send the ticks as they are, even without JS.
    Selection,
}

/// A Notion-style split download button: the icon zips all of the
/// target's Documents in the chosen Format, and ▾ opens a menu of kind
/// groups (with `counts`, only the non-empty ones) and the Format switch.
///
/// Without JS the menu is a native `<details>` disclosure of plain links
/// (or submit buttons), and the Format switch reloads the page with
/// `?format=`. Hydrated, it opens on click only, moves focus to its first
/// item, and closes on an outside click, Esc (focus returns to ▾) or a
/// choice; the Format switch then updates every button without a reload.
#[component]
fn DownloadButton(target: Target, counts: GroupCounts) -> Element {
    let DownloadFormat { mut chosen, links } = use_context::<DownloadFormat>();
    let mut open = use_signal(|| false);
    let mut toggle = use_signal(|| None::<Rc<MountedData>>);
    let mut first_item = use_signal(|| None::<Rc<MountedData>>);
    use_effect(move || {
        if open()
            && let Some(item) = first_item.peek().clone()
        {
            spawn(async move {
                let _ = item.set_focus(true).await;
            });
        }
    });
    let mut close_and_refocus = move || {
        open.set(false);
        if let Some(toggle) = toggle.peek().clone() {
            spawn(async move {
                let _ = toggle.set_focus(true).await;
            });
        }
    };

    let format = chosen();
    let title = format!("Download all ({})", format.label());

    rsx! {
        div { class: "split",
            match target {
                Target::Test(test_id) => rsx! {
                    a {
                        class: "all",
                        href: zip_href(test_id, KindGroup::All, format),
                        aria_label: "Download all documents",
                        title,
                        DownloadIcon {}
                    }
                },
                Target::Selection => rsx! {
                    button {
                        class: "all",
                        r#type: "submit",
                        formaction: BULK_ZIP_PATH,
                        aria_label: "Download all documents",
                        title,
                        DownloadIcon {}
                    }
                },
            }
            details {
                open: open(),
                onkeydown: move |event| {
                    if event.key() == Key::Escape && open() {
                        event.prevent_default();
                        close_and_refocus();
                    }
                },
                summary {
                    aria_label: "Choose what to download",
                    title: "Choose what to download",
                    onmounted: move |event| toggle.set(Some(event.data())),
                    onclick: move |event| {
                        event.prevent_default();
                        open.toggle();
                    },
                    "▾"
                }
                if open() {
                    div { class: "scrim", onclick: move |_| open.set(false) }
                }
                div { class: "menu",
                    for (i , (group , count)) in counts.menu().into_iter().enumerate() {
                        match target {
                            Target::Test(test_id) => rsx! {
                                a {
                                    key: "{group.slug()}",
                                    class: "group",
                                    href: zip_href(test_id, group, format),
                                    onmounted: move |event| {
                                        if i == 0 {
                                            first_item.set(Some(event.data()));
                                        }
                                    },
                                    onclick: move |_| close_and_refocus(),
                                    "{group.label()}"
                                    span { class: "n", "{count}" }
                                }
                            },
                            Target::Selection => rsx! {
                                button {
                                    key: "{group.slug()}",
                                    class: "group",
                                    r#type: "submit",
                                    formaction: BULK_ZIP_PATH,
                                    name: "group",
                                    value: group.slug(),
                                    onmounted: move |event| {
                                        if i == 0 {
                                            first_item.set(Some(event.data()));
                                        }
                                    },
                                    onclick: move |_| close_and_refocus(),
                                    "{group.label()}"
                                    span { class: "n", "{count}" }
                                }
                            },
                        }
                    }
                    hr {}
                    div { class: "fmt", role: "group", aria_label: "Format",
                        span { "Format" }
                        for (option , href) in links {
                            a {
                                key: "{option.slug()}",
                                href,
                                aria_current: if option == format { "true" } else { "false" },
                                onclick: move |event| {
                                    event.prevent_default();
                                    chosen.set(option);
                                },
                                "{option.label()}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DownloadIcon() -> Element {
    rsx! {
        svg {
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "M12 4v11" }
            path { d: "m7 10 5 5 5-5" }
            path { d: "M5 20h14" }
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
