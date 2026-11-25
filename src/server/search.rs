//! Public search over Published Tests.
//!
//! The search lives in the query string, so searches are linkable and work
//! without JS:
//!
//! - `year`, `month`, `competition`, `division_level`, `test_type`, `host`:
//!   empty or absent for Any, `unknown` for Unknown, otherwise a Facet value's
//!   name (a number for `year`). A value that names nothing matches nothing.
//! - `includes`: `test`, `answers` or `solutions`, repeatable. Each one keeps
//!   only Tests with a Published Document whose kind contains questions,
//!   answers or solutions.
//! - `page`: 1-based, [`PAGE_SIZE`] Tests per page.

use std::collections::HashMap;

use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};

use crate::app::{DocumentLink, FacetValue, SearchOptions, SearchQuery, SearchResults, TestRow, UNKNOWN};

pub const PAGE_SIZE: i64 = 50;

/// One page of the search that `raw_query` (a URL query string, without the
/// `?`) asks for.
pub async fn search(db: &PgPool, raw_query: Option<&str>) -> Result<SearchResults, sqlx::Error> {
    let params = Params::parse(raw_query.unwrap_or(""));
    let query = params.search_query();
    let values = FacetValues::load(db).await?;
    let filters = Filters::resolve(&query, &values);

    let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM tests t");
    filters.push_where(&mut count);
    let total: i64 = count.build_query_scalar().fetch_one(db).await?;

    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = params.page().clamp(1, pages);
    let tests = matching_tests(db, &filters, page).await?;
    let link = |page: i64| Some(href(&query, page));

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
        previous: if page > 1 { link(page - 1) } else { None },
        next: if page < pages { link(page + 1) } else { None },
        tests,
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
        }
    }

    fn page(&self) -> i64 {
        self.first("page").parse().unwrap_or(1)
    }
}

/// The search page's URL for `query` at `page`: only the filters in use, so
/// links stay short.
fn href(query: &SearchQuery, page: i64) -> String {
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
    if page != "1" {
        pairs.push(("page", &page));
    }
    match serde_urlencoded::to_string(&pairs) {
        Ok(query) if !query.is_empty() => format!("/?{query}"),
        _ => "/".to_owned(),
    }
}

// ------------------------------------------------------------------ filters

/// Every Facet value, by Facet, in display order.
struct FacetValues(Vec<(String, i64, String)>);

impl FacetValues {
    async fn load(db: &PgPool) -> Result<Self, sqlx::Error> {
        Ok(Self(
            sqlx::query_as(
                "SELECT facet::text, id, name FROM facet_values
                 ORDER BY facet, display_order, name",
            )
            .fetch_all(db)
            .await?,
        ))
    }

    fn names(&self, facet: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(f, _, _)| f == facet)
            .map(|(_, _, name)| name.clone())
            .collect()
    }

    fn id(&self, facet: &str, name: &str) -> Option<i64> {
        self.0
            .iter()
            .find(|(f, _, n)| f == facet && n == name)
            .map(|(_, id, _)| *id)
    }
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
    fn resolve(query: &SearchQuery, values: &FacetValues) -> Self {
        let facet = |facet: &str, column, unknown, raw: &str| FacetFilter {
            column,
            unknown,
            filter: Filter::from_raw(raw, |name| values.id(facet, name)),
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
    sql.push(
        " ORDER BY t.year DESC NULLS LAST,
                   m.display_order NULLS LAST, m.name,
                   c.display_order NULLS LAST, c.name,
                   h.display_order NULLS LAST, h.name,
                   dl.display_order NULLS LAST, dl.name,
                   tt.display_order NULLS LAST, tt.name,
                   t.id
          LIMIT ",
    )
    .push_bind(PAGE_SIZE)
    .push(" OFFSET ")
    .push_bind((page - 1) * PAGE_SIZE);
    let tests: Vec<TestRecord> = sql.build_query_as().fetch_all(db).await?;

    let ids: Vec<i64> = tests.iter().map(|t| t.id).collect();
    let documents: Vec<DocumentRecord> = sqlx::query_as(
        "SELECT td.test_id, d.id, k.name AS kind, d.label,
                CASE WHEN o.content_type <> 'application/pdf' AND o.extension <> 'pdf'
                     THEN o.extension END AS original_extension
         FROM test_documents td
         JOIN documents d      ON d.id = td.document_id
         JOIN document_kinds k ON k.id = d.kind_id
         LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
         WHERE td.test_id = ANY($1) AND d.published AND d.trashed_at IS NULL
         ORDER BY k.display_order, k.name, d.label NULLS FIRST, d.id",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;

    let mut by_test: HashMap<i64, Vec<DocumentLink>> = HashMap::new();
    for d in documents {
        by_test.entry(d.test_id).or_default().push(DocumentLink {
            id: d.id,
            kind: d.kind,
            label: d.label,
            original_extension: d.original_extension,
        });
    }

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
