//! **Edit selected** and **Merge into…**, server side (the page is
//! `crate::app::bulk_edit`).
//!
//! - **Edit selected** ([`edit`]) sets one Facet value (a typed **+ New
//!   value** is created, as on Add test), or ticks or clears Needs review,
//!   on every selected Test, in one transaction. Each Test is judged on its
//!   own: changed (and logged), unchanged (already so), or refused. A
//!   Published Test that the edit would leave with its Year and Competition
//!   both unknown is refused by the publish rule, as is a Test that no
//!   longer exists or is in the Trash. The answer is a 303 back to the bulk
//!   edit page, whose query string carries the summary.
//! - **Merge into…** ([`merge`]) moves every Document of one Test onto
//!   another, and puts the first, left empty, in the Trash (Hidden, with
//!   `trashed_at` set). A Document already on both stays one row per Test,
//!   and one on other Tests too keeps them. Each moved Document's `tests`
//!   and the merged Test are logged.

use std::collections::HashMap;

use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};
use dioxus::prelude::ServerFnError;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::{
    AppState,
    add_test::{self, Choice, Refusal, Resolved},
    auth,
    change_log::{self, Change, Entity},
    facet_values::{FacetValueList, LISTED_FACETS},
    internal_error,
    search::{Params, Viewer, href},
    test_edit::parse_id,
};
use crate::{
    app::{
        bulk_edit::{BULK_EDIT_PATH, BulkEditForm, BulkEditPage, BulkTest, MergeForm, Outcome},
        test_edit::edit_path,
    },
    download::{parse_selection, selection_value},
};

// -------------------------------------------------------------------- page

pub async fn page(
    state: &AppState,
    headers: &HeaderMap,
    raw_query: Option<&str>,
) -> Result<BulkEditPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    let params = Params::parse(raw_query.unwrap_or(""));
    let ids = params.selected();
    let back = safe_back(&params.first("back"))
        .unwrap_or_else(|| href(&params.search_query(Viewer::Admin), params.page(), &ids));
    page_data(&state.db, csrf, &ids, back, outcome(&params))
        .await
        .map_err(internal_error)
}

#[derive(FromRow)]
struct SelectedRecord {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    host: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
    published: bool,
    needs_review: bool,
    documents: i64,
}

async fn page_data(
    db: &PgPool,
    csrf: String,
    ids: &[i64],
    back: String,
    outcome: Option<Outcome>,
) -> anyhow::Result<BulkEditPage> {
    let records: Vec<SelectedRecord> = sqlx::query_as(
        "SELECT t.id, t.year, t.month_applicable, m.name AS month, c.name AS competition, h.name AS host,
                dl.name AS division_level, tt.name AS test_type, t.published, t.needs_review,
                (SELECT count(*) FROM test_documents td JOIN documents d ON d.id = td.document_id
                 WHERE td.test_id = t.id AND d.trashed_at IS NULL) AS documents
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id
         WHERE t.id = ANY($1) AND t.trashed_at IS NULL
         ORDER BY t.id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    let tests: Vec<BulkTest> = records
        .into_iter()
        .map(|t| BulkTest {
            id: t.id,
            description: add_test::describe(
                t.year,
                t.month_applicable.then_some(t.month.as_deref()),
                t.competition.as_deref(),
                t.host.as_deref(),
                t.division_level.as_deref(),
                t.test_type.as_deref(),
            ),
            published: t.published,
            needs_review: t.needs_review,
            documents: t.documents,
        })
        .collect();
    let values = FacetValueList::load(db).await?;
    Ok(BulkEditPage {
        csrf,
        selection: selection_value(tests.iter().map(|t| t.id)),
        tests,
        facets: add_test::facet_options(db, &values).await?,
        back,
        outcome,
    })
}

/// A link back that stays on this site: a path, never `//host` or a URL.
fn safe_back(back: &str) -> Option<String> {
    (back.starts_with('/') && !back.starts_with("//") && !back.contains('\\')).then(|| back.to_owned())
}

// ------------------------------------------------------------ the summary

/// Why a Test was left as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refused {
    /// The publish rule: a Published Test needs its Year or Competition.
    Publish,
    /// No such Test, or it is in the Trash.
    Gone,
}

impl Refused {
    const ALL: [Self; 2] = [Self::Publish, Self::Gone];

    fn code(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Gone => "gone",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Publish => {
                "It is Published, and this would leave its Year and Competition both unknown. \
                 Hide it first, or choose a known value."
            }
            Self::Gone => "It does not exist, or it is in the Trash.",
        }
    }
}

struct Summary {
    done: String,
    changed: Vec<i64>,
    unchanged: Vec<i64>,
    refused: Vec<(i64, Refused)>,
}

/// The bulk edit page of `selection`, linking back to `back`, showing
/// `summary` if given.
fn page_href(selection: &[i64], back: &str, summary: Option<&Summary>) -> String {
    let ids = |ids: &[i64]| selection_value(ids.iter().copied());
    let mut pairs = vec![("sel", ids(selection)), ("back", back.to_owned())];
    if let Some(summary) = summary {
        let refused = summary
            .refused
            .iter()
            .map(|(id, why)| format!("{id}:{}", why.code()))
            .collect::<Vec<_>>()
            .join(",");
        pairs.extend([
            ("done", summary.done.clone()),
            ("changed", ids(&summary.changed)),
            ("unchanged", ids(&summary.unchanged)),
            ("refused", refused),
        ]);
    }
    match serde_urlencoded::to_string(&pairs) {
        Ok(query) => format!("{BULK_EDIT_PATH}?{query}"),
        Err(_) => BULK_EDIT_PATH.to_owned(),
    }
}

/// The summary in the page's query string, if it has one.
fn outcome(params: &Params) -> Option<Outcome> {
    let done = params.first("done");
    if done.is_empty() {
        return None;
    }
    let ids = |key: &str| parse_selection(&params.first(key)).unwrap_or_default();
    let refused = params
        .first("refused")
        .split(',')
        .filter_map(|part| {
            let (id, code) = part.split_once(':')?;
            let why = Refused::ALL.into_iter().find(|why| why.code() == code)?;
            Some((parse_id(id)?, why.reason().to_owned()))
        })
        .collect();
    Some(Outcome {
        done,
        changed: ids("changed"),
        unchanged: ids("unchanged"),
        refused,
    })
}

// ------------------------------------------------------------ Edit selected

/// What one bulk edit does to each Test.
enum Edit {
    Year(Option<i16>),
    /// A listed Facet (`month`, `competition`, …) and its choice.
    Facet(&'static str, Choice),
    /// Tick (with a reason to add, if any) or clear.
    Review {
        tick: bool,
        comment: Option<String>,
    },
}

impl BulkEditForm {
    fn fields(&self) -> HashMap<String, String> {
        [
            ("year", &self.year),
            ("year_new", &self.year_new),
            ("month", &self.month),
            ("month_new", &self.month_new),
            ("competition", &self.competition),
            ("competition_new", &self.competition_new),
            ("division_level", &self.division_level),
            ("division_level_new", &self.division_level_new),
            ("test_type", &self.test_type),
            ("test_type_new", &self.test_type_new),
            ("host", &self.host),
            ("host_new", &self.host_new),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.clone()))
        .collect()
    }

    fn edit(&self, values: &FacetValueList) -> Result<Edit, Refusal> {
        match (self.review.as_str(), self.facet.as_str()) {
            ("tick", _) => {
                let comment = self.needs_review_comment.trim();
                Ok(Edit::Review {
                    tick: true,
                    comment: (!comment.is_empty()).then(|| comment.to_owned()),
                })
            }
            ("clear", _) => Ok(Edit::Review {
                tick: false,
                comment: None,
            }),
            ("", "year") => Ok(Edit::Year(add_test::year_choice(&self.fields())?)),
            ("", facet) => {
                let (facet, label) = LISTED_FACETS
                    .into_iter()
                    .find(|(f, _)| *f == facet)
                    .ok_or_else(|| Refusal::bad_request("Choose a Facet to set, or Needs review."))?;
                Ok(Edit::Facet(
                    facet,
                    add_test::choice(values, &self.fields(), facet, label)?,
                ))
            }
            _ => Err(Refusal::bad_request("Choose Tick or Clear Needs review.")),
        }
    }
}

/// A Test's labels, as stored, with names for the change log.
#[derive(Clone, FromRow)]
struct Labels {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month_id: Option<i64>,
    month: Option<String>,
    competition_id: Option<i64>,
    competition: Option<String>,
    division_level_id: Option<i64>,
    division_level: Option<String>,
    test_type_id: Option<i64>,
    test_type: Option<String>,
    host_id: Option<i64>,
    host: Option<String>,
    published: bool,
    needs_review: bool,
    needs_review_comment: Option<String>,
}

impl Labels {
    /// The publish rule: Year or Competition known.
    fn publishable(&self) -> bool {
        self.year.is_some() || self.competition_id.is_some()
    }

    fn same(&self, other: &Self) -> bool {
        (
            self.year,
            self.month_applicable,
            self.month_id,
            self.competition_id,
            self.division_level_id,
            self.test_type_id,
            self.host_id,
            self.needs_review,
            &self.needs_review_comment,
        ) == (
            other.year,
            other.month_applicable,
            other.month_id,
            other.competition_id,
            other.division_level_id,
            other.test_type_id,
            other.host_id,
            other.needs_review,
            &other.needs_review_comment,
        )
    }

    /// How the change log shows the Month.
    fn month_name(&self) -> Option<String> {
        if self.month_applicable {
            self.month.clone()
        } else {
            Some("Not applicable".to_owned())
        }
    }
}

/// A Facet edit with its value resolved (created if new).
enum Setting {
    Year(Option<i16>),
    Facet {
        facet: &'static str,
        value: Resolved,
        not_applicable: bool,
    },
    Review {
        tick: bool,
        comment: Option<String>,
    },
}

impl Setting {
    /// e.g. "Host set to Lawton Chiles".
    fn done(&self) -> String {
        match self {
            Self::Year(year) => format!(
                "Year set to {}",
                year.map_or_else(|| "Unknown".to_owned(), |y| y.to_string())
            ),
            Self::Facet {
                facet,
                value,
                not_applicable,
            } => {
                let label = LISTED_FACETS
                    .iter()
                    .find(|(f, _)| f == facet)
                    .map_or(*facet, |(_, label)| *label);
                let name = match (&value.name, not_applicable) {
                    (_, true) => "Not applicable",
                    (Some(name), false) => name,
                    (None, false) => "Unknown",
                };
                format!("{label} set to {name}")
            }
            Self::Review { tick: true, .. } => "Needs review ticked".to_owned(),
            Self::Review { tick: false, .. } => "Needs review cleared".to_owned(),
        }
    }

    fn apply(&self, before: &Labels) -> Labels {
        let mut after = before.clone();
        match self {
            Self::Year(year) => after.year = *year,
            Self::Facet {
                facet,
                value,
                not_applicable,
            } => {
                let (id, name) = (value.id, value.name.clone());
                match *facet {
                    "month" => {
                        after.month_applicable = !not_applicable;
                        (after.month_id, after.month) = (id, name);
                    }
                    "competition" => (after.competition_id, after.competition) = (id, name),
                    "division_level" => (after.division_level_id, after.division_level) = (id, name),
                    "test_type" => (after.test_type_id, after.test_type) = (id, name),
                    _ => (after.host_id, after.host) = (id, name),
                }
            }
            Self::Review { tick: false, .. } => {
                after.needs_review = false;
                after.needs_review_comment = None;
            }
            Self::Review { tick: true, comment } => {
                after.needs_review = true;
                after.needs_review_comment = add_reason(before.needs_review_comment.as_deref(), comment.as_deref());
            }
        }
        after
    }
}

/// A Test's Needs review comment with `reason` added on its own line,
/// unless it already has that line.
fn add_reason(comment: Option<&str>, reason: Option<&str>) -> Option<String> {
    match (comment, reason) {
        (Some(comment), Some(reason)) if !comment.lines().any(|line| line.trim() == reason) => {
            Some(format!("{comment}\n{reason}"))
        }
        (None, Some(reason)) => Some(reason.to_owned()),
        (comment, _) => comment.map(str::to_owned),
    }
}

/// `POST /api/admin/tests/bulk-edit`.
pub async fn edit(state: &AppState, headers: &HeaderMap, form: BulkEditForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let mut ids = parse_selection(&form.sel).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(*id));
    let back = safe_back(&form.back).unwrap_or_else(|| "/".to_owned());
    let here = page_href(&ids, &back, None);
    if ids.is_empty() {
        let refusal = Refusal::bad_request("Select at least one Test on the search page first.");
        return Ok(refusal.back_to(back, "Back to the search").response());
    }
    let values = FacetValueList::load(&state.db).await.map_err(internal_error)?;
    let edit = match form.edit(&values) {
        Ok(edit) => edit,
        Err(refusal) => return Ok(refusal.back_to(here, "Back to Edit selected").response()),
    };

    let summary = async {
        let mut tx = state.db.begin().await?;
        let summary = apply(&mut tx, &ids, edit).await?;
        tx.commit().await?;
        anyhow::Ok(summary)
    }
    .await
    .map_err(internal_error)?;
    Ok(auth::see_other(&page_href(&ids, &back, Some(&summary)), None))
}

async fn apply(tx: &mut Transaction<'static, Postgres>, ids: &[i64], edit: Edit) -> anyhow::Result<Summary> {
    let edit = match edit {
        Edit::Year(year) => Setting::Year(year),
        Edit::Facet(facet, choice) => Setting::Facet {
            facet,
            value: add_test::resolve(tx, facet, &choice).await?,
            not_applicable: choice == Choice::NotApplicable,
        },
        Edit::Review { tick, comment } => Setting::Review { tick, comment },
    };
    // Locked in ID order, so two bulk edits can't deadlock.
    let found: Vec<Labels> = sqlx::query_as(
        "SELECT t.id, t.year, t.month_applicable, t.month_id, m.name AS month,
                t.competition_id, c.name AS competition, t.division_level_id, dl.name AS division_level,
                t.test_type_id, tt.name AS test_type, t.host_id, h.name AS host,
                t.published, t.needs_review, t.needs_review_comment
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         WHERE t.id = ANY($1) AND t.trashed_at IS NULL
         ORDER BY t.id
         FOR UPDATE OF t",
    )
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut found: HashMap<i64, Labels> = found.into_iter().map(|t| (t.id, t)).collect();

    let mut summary = Summary {
        done: edit.done(),
        changed: Vec::new(),
        unchanged: Vec::new(),
        refused: Vec::new(),
    };
    for &id in ids {
        let Some(before) = found.remove(&id) else {
            summary.refused.push((id, Refused::Gone));
            continue;
        };
        let after = edit.apply(&before);
        if after.same(&before) {
            summary.unchanged.push(id);
            continue;
        }
        if before.published && before.publishable() && !after.publishable() {
            summary.refused.push((id, Refused::Publish));
            continue;
        }
        sqlx::query(
            "UPDATE tests
             SET year = $2, month_id = $3, month_applicable = $4, competition_id = $5,
                 division_level_id = $6, test_type_id = $7, host_id = $8,
                 needs_review = $9, needs_review_comment = $10, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(after.year)
        .bind(after.month_id)
        .bind(after.month_applicable)
        .bind(after.competition_id)
        .bind(after.division_level_id)
        .bind(after.test_type_id)
        .bind(after.host_id)
        .bind(after.needs_review)
        .bind(&after.needs_review_comment)
        .execute(&mut **tx)
        .await?;
        change_log::record(
            tx,
            Entity::Test,
            id,
            [
                Change::new("year", before.year, after.year),
                Change::new("month", before.month_name(), after.month_name()),
                Change::new("competition", &before.competition, &after.competition),
                Change::new("division_level", &before.division_level, &after.division_level),
                Change::new("test_type", &before.test_type, &after.test_type),
                Change::new("host", &before.host, &after.host),
                Change::new("needs_review", before.needs_review, after.needs_review),
                Change::new(
                    "needs_review_comment",
                    &before.needs_review_comment,
                    &after.needs_review_comment,
                ),
            ],
        )
        .await?;
        summary.changed.push(id);
    }
    Ok(summary)
}

// -------------------------------------------------------------- Merge into…

/// A Document's Tests as the change log shows them: `T12, T40`.
fn test_list(tests: &[i64]) -> Option<String> {
    (!tests.is_empty()).then(|| {
        tests
            .iter()
            .map(|t| Entity::Test.display_id(*t))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// `POST /api/admin/tests/merge`.
pub async fn merge(state: &AppState, headers: &HeaderMap, form: MergeForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let Some(from) = parse_id(&form.from) else {
        return Ok(Refusal::new(StatusCode::NOT_FOUND, "That Test does not exist.")
            .back_to("/".to_owned(), "Back to the search")
            .response());
    };
    let refuse = |status, message: String| {
        Refusal::new(status, message)
            .back_to(edit_path(from), "Back to the Test")
            .response()
    };
    let Some(into) = parse_id(&form.into) else {
        return Ok(refuse(
            StatusCode::BAD_REQUEST,
            "Type the ID of the Test to merge into, like T123.".to_owned(),
        ));
    };
    if from == into {
        return Ok(refuse(
            StatusCode::BAD_REQUEST,
            format!("T{from} can't be merged into itself."),
        ));
    }

    let merged = async {
        let mut tx = state.db.begin().await?;
        let merged = merge_in(&mut tx, from, into).await?;
        if merged.is_ok() {
            tx.commit().await?;
        }
        anyhow::Ok(merged)
    }
    .await
    .map_err(internal_error)?;
    match merged {
        Ok(()) => Ok(auth::see_other(&edit_path(into), None)),
        Err(missing) => Ok(refuse(
            StatusCode::NOT_FOUND,
            format!("T{missing} does not exist, or it is in the Trash."),
        )),
    }
}

/// Merge `from` into `into`, or say which of them is missing.
async fn merge_in(tx: &mut Transaction<'static, Postgres>, from: i64, into: i64) -> anyhow::Result<Result<(), i64>> {
    // Both Tests, locked in ID order: attaching a Document to `from` waits
    // for this, and then finds it in the Trash.
    let tests: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT id, published FROM tests WHERE id = ANY($1) AND trashed_at IS NULL ORDER BY id FOR UPDATE",
    )
    .bind([from, into])
    .fetch_all(&mut **tx)
    .await?;
    let published = |id: i64| tests.iter().find(|(t, _)| *t == id).map(|(_, published)| *published);
    let Some(from_published) = published(from) else {
        return Ok(Err(from));
    };
    if published(into).is_none() {
        return Ok(Err(into));
    }

    // Every Document of `from`, Trashed ones too, so a Document restored
    // from the Trash later is on a Test that is not. Locked, as Also on
    // Tests… locks them, so attaches and detaches wait.
    let documents: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM documents
         WHERE id IN (SELECT document_id FROM test_documents WHERE test_id = $1)
         ORDER BY id FOR UPDATE",
    )
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    let before: Vec<(i64, Vec<i64>)> = sqlx::query_as(
        "SELECT document_id, array_agg(test_id ORDER BY test_id)
         FROM test_documents WHERE document_id = ANY($1)
         GROUP BY document_id ORDER BY document_id",
    )
    .bind(&documents)
    .fetch_all(&mut **tx)
    .await?;
    // A Document already on `into` keeps its one row there.
    sqlx::query(
        "INSERT INTO test_documents (test_id, document_id)
         SELECT $2, document_id FROM test_documents WHERE test_id = $1
         ON CONFLICT DO NOTHING",
    )
    .bind(from)
    .bind(into)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM test_documents WHERE test_id = $1")
        .bind(from)
        .execute(&mut **tx)
        .await?;
    for (document, before) in before {
        let mut after: Vec<i64> = before.iter().copied().filter(|t| *t != from).collect();
        if !after.contains(&into) {
            after.push(into);
            after.sort_unstable();
        }
        change_log::record(
            tx,
            Entity::Document,
            document,
            [Change::new("tests", test_list(&before), test_list(&after))],
        )
        .await?;
    }

    sqlx::query("UPDATE tests SET published = false, trashed_at = now(), updated_at = now() WHERE id = $1")
        .bind(from)
        .execute(&mut **tx)
        .await?;
    change_log::record(
        tx,
        Entity::Test,
        from,
        [
            Change::new("merged_into", None::<String>, Entity::Test.display_id(into)),
            Change::new("published", from_published, false),
            Change::new("trashed", false, true),
        ],
    )
    .await?;
    Ok(Ok(()))
}
