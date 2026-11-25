//! The Dioxus app, shared by the server (SSR) and the WASM client.
//!
//! Everything here compiles for both targets; server-only code lives in
//! `crate::server` and is only referenced from server-function bodies.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Routable, Clone, Debug, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Shell)]
    #[route("/")]
    Search {},
}

#[component]
pub fn App() -> Element {
    rsx! {
        document::Title { "FAMAT Test Bank" }
        document::Style { {CSS} }
        Router::<Route> {}
    }
}

const CSS: &str = "\
body{font:16px/1.4 system-ui,sans-serif;margin:0;color:#1a1a1a;background:#fff}\
main{max-width:64rem;margin:0 auto;padding:1rem}\
table{border-collapse:collapse;width:100%}\
th,td{text-align:left;padding:.35rem .5rem;border-bottom:1px solid #ddd;vertical-align:top}\
.unknown{color:#aaa}\
.documents .document{margin-right:.75rem}\
.documents .original{font-size:.8em}\
footer{text-align:center;color:#666;font-size:.875rem;padding:2rem 1rem}";

#[component]
fn Shell() -> Element {
    rsx! {
        main {
            h1 { "FAMAT Test Bank" }
            Outlet::<Route> {}
        }
        footer { "Made by and maintained by Rob from Doral Academy" }
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

/// Published Tests, in results order. Public: it never returns Hidden or
/// Trashed Tests or Documents, so it needs no authorization.
#[get("/api/tests", state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn search_tests() -> Result<Vec<TestRow>, ServerFnError> {
    crate::server::search::published_tests(&state.db)
        .await
        .map_err(crate::server::internal_error)
}

#[component]
fn Search() -> Element {
    let tests = use_loader(search_tests)?;
    let tests = tests.read();
    let count = tests.len();

    rsx! {
        p { id: "count", if count == 1 { "1 test" } else { "{count} tests" } }
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
                for test in tests.iter() {
                    tr { key: "{test.id}", id: "T{test.id}",
                        td { class: "year",
                            match test.year {
                                Some(year) => rsx! { "{year}" },
                                None => rsx! { Unknown {} },
                            }
                        }
                        td { class: "event",
                            Event { month: test.month.clone(), competition: test.competition.clone() }
                        }
                        td { class: "host", Facet { value: test.host.clone() } }
                        td { class: "division-level", Facet { value: test.division_level.clone() } }
                        td { class: "test-type", Facet { value: test.test_type.clone() } }
                        td { class: "documents",
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
    }
}

/// "{Month} {Competition}", e.g. "January Regional" or just "State".
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
    rsx! { span { class: "unknown", title: "Unknown", "–" } }
}
