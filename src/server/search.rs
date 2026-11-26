//! Public search over Published Tests.
//!
//! The search lives in the query string, so searches are linkable and work
//! without JS:
//!
//! - `year`, `month`, `competition`, `division_level`, `test_type`, `host`:
//!   empty or absent for Any, `unknown` for Unknown, otherwise a Facet value's
//!   name (a number for `year`). A former name (an alias, kept on rename and
//!   merge) finds the same value, as does the name in another case, and the
//!   page then shows the current name. A value that names nothing matches
//!   nothing.
//! - `includes`: `test`, `answers` or `solutions`, repeatable. Each one keeps
//!   only Tests with a Published Document whose kind contains questions,
//!   answers or solutions.
//! - `page`: 1-based, [`PAGE_SIZE`] Tests per page.
//! - `format`: the download menus' Format switch, `pdf` (default),
//!   `original` or `both`. It filters nothing; links keep it for the page.
//! - `sel`: the selected Test IDs, comma-separated and/or repeated (see
//!   [`selection_value`]). It filters nothing either. Only Published,
//!   untrashed Tests stay selected; anything that isn't an ID is ignored.
//! - `select=all`: also select every Test the search matches, when that
//!   leaves at most [`MAX_BULK_TESTS`] selected.

use std::collections::{HashMap, HashSet};

use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};

use super::facet_values::FacetValueList;
use crate::{
    app::{DocumentLink, FacetValue, SearchOptions, SearchQuery, SearchResults, SelectedTest, TestRow, UNKNOWN},
    download::{Contents, Format, GroupCounts, MAX_BULK_TESTS, parse_selection, selection_value},
};

pub const PAGE_SIZE: i64 = 50;

/// The results order, over `tests t` joined to its Facet values as `m`,
/// `c`, `h`, `dl` and `tt` (see [`matching_tests`]).
pub(super) const RESULTS_ORDER: &str = "t.year DESC NULLS LAST,
    m.display_order NULLS LAST, m.name,
    c.display_order NULLS LAST, c.name,
    h.display_order NULLS LAST, h.name,
    dl.display_order NULLS LAST, dl.name,
    tt.display_order NULLS LAST, tt.name,
    t.id";

/// One page of the search that `raw_query` (a URL query string, without the
/// `?`) asks for.
pub async fn search(db: &PgPool, raw_query: Option<&str>) -> Result<SearchResults, sqlx::Error> {
    let params = Params::parse(raw_query.unwrap_or(""));
    let values = FacetValueList::load(db).await?;
    let query = current_names(params.search_query(), &values);
    let filters = Filters::resolve(&query, &values);

    let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM tests t");
    filters.push_where(&mut count);
    let total: i64 = count.build_query_scalar().fetch_one(db).await?;

    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = params.page().clamp(1, pages);
    let tests = matching_tests(db, &filters, page).await?;

    // Every match, while they would fit in one bulk zip, so the page knows
    // whether to offer Select all.
    let result_ids = match usize::try_from(total) {
        Ok(total) if total <= MAX_BULK_TESTS => Some(matching_ids(db, &filters).await?),
        _ => None,
    };
    let mut wanted = params.selected();
    if params.first("select") == "all"
        && let Some(result_ids) = &result_ids
    {
        let have: HashSet<i64> = wanted.iter().copied().collect();
        let mut all = wanted.clone();
        all.extend(result_ids.iter().filter(|id| !have.contains(id)));
        if all.len() <= MAX_BULK_TESTS {
            wanted = all;
        }
    }
    let selection = selected_tests(db, &wanted).await?;
    let selected: Vec<i64> = selection.iter().map(|t| t.id).collect();

    Ok(SearchResults {
        options: SearchOptions {
            years: published_years(db).await?,
            months: values.names("month"),
            competitions: values.names("competition"),
            division_levels: values.names("division_level"),
            test_types: values.names("test_type"),
            hosts: values.names("host"),
        },
        total,
        page,
        pages,
        format_links: Format::ALL
            .into_iter()
            .map(|format| {
                (
                    format,
                    href(
                        &SearchQuery {
                            format,
                            ..query.clone()
                        },
                        page,
                        &selected,
                    ),
                )
            })
            .collect(),
        clear_selection: href(&query, page, &[]),
        tests,
        result_ids,
        selection,
        query,
    })
}

// ------------------------------------------------------------- query string

/// The decoded `key=value` pairs of a query string, in order.
struct Params(Vec<(String, String)>);

impl Params {
    fn parse(raw: &str) -> Self {
        // Malformed input (e.g. bad UTF-8 escapes) is treated as no search.
        Self(serde_urlencoded::from_str(raw).unwrap_or_default())
    }

    /// The first value of `key`, trimmed; empty if absent.
    fn first(&self, key: &str) -> String {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_owned())
            .unwrap_or_default()
    }

    fn includes(&self, value: &str) -> bool {
        self.0.iter().any(|(k, v)| k == "includes" && v == value)
    }

    fn search_query(&self) -> SearchQuery {
        SearchQuery {
            year: self.first("year"),
            month: self.first("month"),
            competition: self.first("competition"),
            division_level: self.first("division_level"),
            test_type: self.first("test_type"),
            host: self.first("host"),
            includes_test: self.includes("test"),
            includes_answers: self.includes("answers"),
            includes_solutions: self.includes("solutions"),
            format: Format::from_slug(&self.first("format")).unwrap_or_default(),
        }
    }

    fn page(&self) -> i64 {
        self.first("page").parse().unwrap_or(1)
    }

    /// The Test IDs in every `sel` value, each once, in order. Values that
    /// aren't IDs are ignored.
    fn selected(&self) -> Vec<i64> {
        let mut seen = HashSet::new();
        self.0
            .iter()
            .filter(|(k, _)| k == "sel")
            .flat_map(|(_, v)| parse_selection(v).unwrap_or_default())
            .filter(|id| seen.insert(*id))
            .collect()
    }
}

/// The search page's URL for `query` at `page` with `selected` Tests: only
/// the filters in use, so links stay short.
fn href(query: &SearchQuery, page: i64, selected: &[i64]) -> String {
    let page = page.to_string();
    let mut pairs: Vec<(&str, &str)> = [
        ("year", &query.year),
        ("month", &query.month),
        ("competition", &query.competition),
        ("division_level", &query.division_level),
        ("test_type", &query.test_type),
        ("host", &query.host),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(key, value)| (key, value.as_str()))
    .collect();
    for (included, value) in [
        (query.includes_test, "test"),
        (query.includes_answers, "answers"),
        (query.includes_solutions, "solutions"),
    ] {
        if included {
            pairs.push(("includes", value));
        }
    }
    // In form order: the hidden `format` field follows the checkboxes.
    if query.format != Format::default() {
        pairs.push(("format", query.format.slug()));
    }
    let selected = selection_value(selected.iter().copied());
    if !selected.is_empty() {
        pairs.push(("sel", &selected));
    }
    if page != "1" {
        pairs.push(("page", &page));
    }
    match serde_urlencoded::to_string(&pairs) {
        Ok(query) if !query.is_empty() => format!("/?{query}"),
        _ => "/".to_owned(),
    }
}

// ------------------------------------------------------------------ filters

/// The search with each Facet value written by its current name, so a
/// search by a former name (or in another case) shows its value chosen in
/// the dropdown and links with the name it has now.
fn current_names(mut query: SearchQuery, values: &FacetValueList) -> SearchQuery {
    for (facet, raw) in [
        ("month", &mut query.month),
        ("competition", &mut query.competition),
        ("division_level", &mut query.division_level),
        ("test_type", &mut query.test_type),
        ("host", &mut query.host),
    ] {
        if raw.is_empty() || raw == UNKNOWN {
            continue;
        }
        if let Some(name) = values.find(facet, raw).and_then(|id| values.name(facet, id)) {
            *raw = name.to_owned();
        }
    }
    query
}

/// What one dropdown asks for.
#[derive(Clone, Copy)]
enum Filter<T> {
    Any,
    Unknown,
    Is(T),
    /// The value names nothing, so nothing matches.
    Nothing,
}

impl<T> Filter<T> {
    fn from_raw(raw: &str, resolve: impl FnOnce(&str) -> Option<T>) -> Self {
        match raw {
            "" => Filter::Any,
            UNKNOWN => Filter::Unknown,
            raw => resolve(raw).map_or(Filter::Nothing, Filter::Is),
        }
    }
}

/// A Facet with a value list: its column on `tests` and how to tell unknown.
struct FacetFilter {
    column: &'static str,
    unknown: &'static str,
    filter: Filter<i64>,
}

struct Filters {
    year: Filter<i16>,
    facets: Vec<FacetFilter>,
    /// `document_kinds` flag columns every match must have a Document for.
    includes: Vec<&'static str>,
}

impl Filters {
    fn resolve(query: &SearchQuery, values: &FacetValueList) -> Self {
        let facet = |facet: &str, column, unknown, raw: &str| FacetFilter {
            column,
            unknown,
            filter: Filter::from_raw(raw, |name| values.find(facet, name)),
        };
        let mut includes = Vec::new();
        if query.includes_test {
            includes.push("contains_questions");
        }
        if query.includes_answers {
            includes.push("contains_answers");
        }
        if query.includes_solutions {
            includes.push("contains_solutions");
        }
        Self {
            year: Filter::from_raw(&query.year, |year| year.parse().ok()),
            facets: vec![
                // An unknown Month is not the same as a not-applicable one.
                facet(
                    "month",
                    "t.month_id",
                    "t.month_applicable AND t.month_id IS NULL",
                    &query.month,
                ),
                facet(
                    "competition",
                    "t.competition_id",
                    "t.competition_id IS NULL",
                    &query.competition,
                ),
                facet(
                    "division_level",
                    "t.division_level_id",
                    "t.division_level_id IS NULL",
                    &query.division_level,
                ),
                facet(
                    "test_type",
                    "t.test_type_id",
                    "t.test_type_id IS NULL",
                    &query.test_type,
                ),
                facet("host", "t.host_id", "t.host_id IS NULL", &query.host),
            ],
            includes,
        }
    }

    /// ` WHERE …` over `tests t`: Published, untrashed and matching.
    fn push_where(&self, sql: &mut QueryBuilder<Postgres>) {
        sql.push(" WHERE t.published AND t.trashed_at IS NULL");
        match self.year {
            Filter::Any => {}
            Filter::Unknown => {
                sql.push(" AND t.year IS NULL");
            }
            Filter::Is(year) => {
                sql.push(" AND t.year = ").push_bind(year);
            }
            Filter::Nothing => {
                sql.push(" AND false");
            }
        }
        for facet in &self.facets {
            match facet.filter {
                Filter::Any => {}
                Filter::Unknown => {
                    sql.push(format_args!(" AND ({})", facet.unknown));
                }
                Filter::Is(id) => {
                    sql.push(format_args!(" AND {} = ", facet.column)).push_bind(id);
                }
                Filter::Nothing => {
                    sql.push(" AND false");
                }
            }
        }
        for flag in &self.includes {
            sql.push(format_args!(
                " AND EXISTS (SELECT 1 FROM test_documents td
                              JOIN documents d      ON d.id = td.document_id
                              JOIN document_kinds k ON k.id = d.kind_id
                              WHERE td.test_id = t.id AND d.published AND d.trashed_at IS NULL
                                AND k.{flag})"
            ));
        }
    }
}

// ------------------------------------------------------------------ results

#[derive(FromRow)]
struct TestRecord {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    host: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
}

#[derive(FromRow)]
struct DocumentRecord {
    test_id: i64,
    id: i64,
    kind: String,
    label: Option<String>,
    /// NULL when the Original is a PDF.
    original_extension: Option<String>,
    contains_questions: bool,
    contains_answers: bool,
    contains_solutions: bool,
}

/// One page of matching Tests with their Published, untrashed Documents.
///
/// Order: Year newest first, then Month in season order (month-less last),
/// Competition, Host, Division/Level and Test type, each in Facet display
/// order; unknowns sort last.
async fn matching_tests(db: &PgPool, filters: &Filters, page: i64) -> Result<Vec<TestRow>, sqlx::Error> {
    let mut sql = QueryBuilder::<Postgres>::new(
        "SELECT t.id, t.year, t.month_applicable,
                m.name AS month, c.name AS competition, h.name AS host,
                dl.name AS division_level, tt.name AS test_type
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id",
    );
    filters.push_where(&mut sql);
    sql.push(format_args!(" ORDER BY {RESULTS_ORDER} LIMIT "))
        .push_bind(PAGE_SIZE)
        .push(" OFFSET ")
        .push_bind((page - 1) * PAGE_SIZE);
    let tests: Vec<TestRecord> = sql.build_query_as().fetch_all(db).await?;

    let ids: Vec<i64> = tests.iter().map(|t| t.id).collect();
    let mut by_test = published_documents(db, &ids).await?;
    Ok(tests
        .into_iter()
        .map(|t| TestRow {
            id: t.id,
            year: t.year,
            month: if t.month_applicable {
                facet_value(t.month)
            } else {
                FacetValue::NotApplicable
            },
            competition: facet_value(t.competition),
            host: facet_value(t.host),
            division_level: facet_value(t.division_level),
            test_type: facet_value(t.test_type),
            documents: by_test.remove(&t.id).unwrap_or_default(),
        })
        .collect())
}

/// The IDs of every matching Test, in no particular order.
async fn matching_ids(db: &PgPool, filters: &Filters) -> Result<Vec<i64>, sqlx::Error> {
    let mut sql = QueryBuilder::<Postgres>::new("SELECT t.id FROM tests t");
    filters.push_where(&mut sql);
    sql.build_query_scalar().fetch_all(db).await
}

/// Those of the Tests `ids` that are Published and untrashed, by ID, with
/// how many Published, untrashed Documents each has in each kind group.
async fn selected_tests(db: &PgPool, ids: &[i64]) -> Result<Vec<SelectedTest>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let visible: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM tests WHERE id = ANY($1) AND published AND trashed_at IS NULL ORDER BY id")
            .bind(ids)
            .fetch_all(db)
            .await?;
    let mut by_test = published_documents(db, &visible).await?;
    Ok(visible
        .into_iter()
        .map(|id| SelectedTest {
            id,
            counts: GroupCounts::of(by_test.remove(&id).unwrap_or_default().iter().map(|d| d.contents)),
        })
        .collect())
}

/// The Published, untrashed Documents of each of the Tests `ids`, in
/// results order (kind, label, ID).
async fn published_documents(db: &PgPool, ids: &[i64]) -> Result<HashMap<i64, Vec<DocumentLink>>, sqlx::Error> {
    let documents: Vec<DocumentRecord> = sqlx::query_as(
        "SELECT td.test_id, d.id, k.name AS kind, d.label,
                CASE WHEN o.content_type <> 'application/pdf' AND o.extension <> 'pdf'
                     THEN o.extension END AS original_extension,
                k.contains_questions, k.contains_answers, k.contains_solutions
         FROM test_documents td
         JOIN documents d      ON d.id = td.document_id
         JOIN document_kinds k ON k.id = d.kind_id
         LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
         WHERE td.test_id = ANY($1) AND d.published AND d.trashed_at IS NULL
         ORDER BY k.display_order, k.name, d.label NULLS FIRST, d.id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;

    let mut by_test: HashMap<i64, Vec<DocumentLink>> = HashMap::new();
    for d in documents {
        by_test.entry(d.test_id).or_default().push(DocumentLink {
            id: d.id,
            kind: d.kind,
            label: d.label,
            original_extension: d.original_extension,
            contents: Contents {
                questions: d.contains_questions,
                answers: d.contains_answers,
                solutions: d.contains_solutions,
            },
        });
    }
    Ok(by_test)
}

/// Years that Published Tests have, newest first.
async fn published_years(db: &PgPool) -> Result<Vec<i16>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT year FROM tests
         WHERE published AND trashed_at IS NULL AND year IS NOT NULL
         ORDER BY year DESC",
    )
    .fetch_all(db)
    .await
}

/// A Facet value from a nullable column: NULL means unknown.
fn facet_value(name: Option<String>) -> FacetValue {
    name.map_or(FacetValue::Unknown, FacetValue::Known)
}
