//! The old PHP site's URLs, answered with redirects so old bookmarks and
//! links keep working (spec, "Public search page"; #32):
//!
//! - `GET /scripts/download.php?tid=N` → 301 to `/documents/N/pdf` (which
//!   then picks the Test).
//! - `GET /docs/<N> <name>` (a file in the old directory listing) → 301 to
//!   `/documents/N/original`, the file that URL served. `/docs/` itself →
//!   301 to the search page.
//! - `GET /index.php?year=…&category=…&level=…&style=…&host=…&submit=…`, and
//!   the same query on `/` (the old form submitted to whichever of the two
//!   showed it) → 301 to the new search, with values translated
//!   ([`old_search`]).
//!
//! A `tid` that names no Published Document of a Published Test redirects
//! to the search page with a 302 instead, since an Admin may publish it
//! later. Anything unreadable redirects to the search page with a 301.
//! Repeated query keys take the last value, as PHP did.

use axum::{
    Extension, Router,
    extract::{Path, RawQuery, Request},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use sqlx::PgPool;

use super::{
    AppState,
    documents::server_error,
    facet_values::FacetValueList,
    import::{
        RawLabels,
        tidy::{Month, tidy_labels},
    },
    search::{current_names, href},
};
use crate::app::{SearchQuery, UNKNOWN};

/// The old search's own keys: a query with any of them on `/` is an old one.
/// (`year` and `host` kept their names.)
const OLD_SEARCH_KEYS: [&str; 4] = ["category", "level", "style", "submit"];

/// The routes of the old site's pages and files.
pub fn routes() -> Router {
    Router::new()
        .route("/scripts/download.php", get(download))
        .route("/index.php", get(index))
        .route("/docs", get(docs_listing))
        .route("/docs/", get(docs_listing))
        .route("/docs/{*file}", get(docs_file))
}

/// Middleware: an old search submitted to `/` (it has `category`, `level`,
/// `style` or `submit`, which the new search never uses) redirects like
/// `/index.php` does. Everything else passes through.
pub async fn old_search_at_root(request: Request, next: Next) -> Response {
    let old = matches!(*request.method(), Method::GET | Method::HEAD)
        && request.uri().path() == "/"
        && request
            .uri()
            .query()
            .is_some_and(|query| parse(query).iter().any(|(k, _)| OLD_SEARCH_KEYS.contains(&k.as_str())));
    if !old {
        return next.run(request).await;
    }
    let Some(state) = request.extensions().get::<AppState>().cloned() else {
        return server_error("no AppState on the request");
    };
    index(Extension(state), RawQuery(request.uri().query().map(str::to_owned))).await
}

/// `GET /scripts/download.php?tid=N`: Document N's PDF.
async fn download(Extension(state): Extension<AppState>, RawQuery(query): RawQuery) -> Response {
    let params = parse(query.as_deref().unwrap_or(""));
    match document_id(last(&params, "tid")) {
        Some(id) => to_document(&state.db, id, "pdf").await,
        None => moved("/"),
    }
}

/// `GET /docs/<N> <name>`: Document N's Original, the file the old listing
/// linked to.
async fn docs_file(Extension(state): Extension<AppState>, Path(file): Path<String>) -> Response {
    let tid = file.split_once(' ').map_or(file.as_str(), |(tid, _)| tid);
    match document_id(tid) {
        Some(id) => to_document(&state.db, id, "original").await,
        None => moved("/"),
    }
}

/// `GET /docs/`: the old "All Documents" listing. The search lists them now.
async fn docs_listing() -> Response {
    moved("/")
}

/// `GET /index.php?…`: the same search on the new page.
async fn index(Extension(state): Extension<AppState>, RawQuery(query): RawQuery) -> Response {
    let params = parse(query.as_deref().unwrap_or(""));
    let values = match FacetValueList::load(&state.db).await {
        Ok(values) => values,
        Err(e) => return server_error(e),
    };
    moved(&href(&old_search(&params, &values), 1, &[]))
}

/// Document `id`'s `file` (`pdf` or `original`), or the search page if it
/// isn't a Published Document of a Published Test.
async fn to_document(db: &PgPool, id: i64, file: &str) -> Response {
    let published: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM test_documents td
             JOIN tests t     ON t.id = td.test_id
             JOIN documents d ON d.id = td.document_id
             WHERE d.id = $1
               AND d.published AND d.trashed_at IS NULL
               AND t.published AND t.trashed_at IS NULL
         )",
    )
    .bind(id)
    .fetch_one(db)
    .await;
    match published {
        Ok(true) => moved(&format!("/documents/{id}/{file}")),
        Ok(false) => (StatusCode::FOUND, [(header::LOCATION, "/")]).into_response(),
        Err(e) => server_error(e),
    }
}

/// The new search for an old one's query.
///
/// - `category` is Competition and Month: "Invitationals - January" is
///   Invitational in January. Its parts, `level` (Division/Level), `style`
///   (Test type), `year` and `host` are tidied as the import tidies the old
///   labels: plural events become singular, Month is dropped for State,
///   National and Mail-in, Test type parentheses are dropped, "Washington
///   State" is State hosted by Washington Mu Alpha Theta.
/// - `?` and `-` (and year `0`) become Unknown; absent or blank is Any, as
///   is a year that isn't one.
/// - Values are then written by their current names (aliases and case
///   resolve); a name that is no value passes through as is. The exception is
///   Test type "Applications", which the import split into Calculus
///   Applications and Mu Applications: unless it is a value, it becomes Any.
pub fn old_search(params: &[(String, String)], values: &FacetValueList) -> SearchQuery {
    let category = last(params, "category");
    let (comp_type, comp_month) = category.split_once(" - ").unwrap_or((category, ""));
    let raw = RawLabels {
        file_type: String::new(),
        year: String::new(),
        comp_type: comp_type.to_owned(),
        comp_month: comp_month.to_owned(),
        division: last(params, "level").to_owned(),
        test: last(params, "style").to_owned(),
        host: last(params, "host").to_owned(),
    };
    // Only a bad year or File Type fails, and both are left empty.
    let Ok(tidied) = tidy_labels(&raw) else {
        return SearchQuery::default();
    };
    let test = tidied.test;
    let or_unknown = |tidied: Option<String>, raw: &str| {
        tidied.unwrap_or_else(|| if raw.trim().is_empty() { "" } else { UNKNOWN }.to_owned())
    };

    let year = match last(params, "year") {
        "" => "",
        "0" | "?" | "-" => UNKNOWN,
        year if year.parse::<i16>().is_ok_and(|y| (1900..=2100).contains(&y)) => year,
        _ => "",
    };
    let month = match test.month {
        Month::Known(month) => month,
        Month::Unknown => or_unknown(None, &raw.comp_month),
        Month::NotApplicable => String::new(),
    };
    let mut query = current_names(
        SearchQuery {
            year: year.to_owned(),
            month,
            competition: or_unknown(test.competition, &raw.comp_type),
            division_level: or_unknown(test.division_level, &raw.division),
            test_type: or_unknown(test.test_type, &raw.test),
            host: or_unknown(test.host, &raw.host),
            ..SearchQuery::default()
        },
        values,
    );
    if query.test_type.eq_ignore_ascii_case("Applications") && values.find("test_type", &query.test_type).is_none() {
        query.test_type.clear();
    }
    query
}

/// The Document ID `tid` states, if it is one.
fn document_id(tid: &str) -> Option<i64> {
    tid.parse().ok().filter(|id| *id > 0)
}

/// A 301 to `location`.
fn moved(location: &str) -> Response {
    (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, location)]).into_response()
}

/// The decoded `key=value` pairs of a query string; none if it is malformed.
fn parse(query: &str) -> Vec<(String, String)> {
    serde_urlencoded::from_str(query).unwrap_or_default()
}

/// The last value of `key`, trimmed (PHP kept the last); empty if absent.
fn last<'a>(params: &'a [(String, String)], key: &str) -> &'a str {
    params
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.trim())
}
