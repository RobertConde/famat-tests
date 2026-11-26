//! Facet values: each listed Facet's Admin-curated values, their display
//! order and aliases, and the Facet values page that curates them (the page
//! is `crate::app::facet_values`).
//!
//! - **Aliases** are former names, kept on rename and merge so that old
//!   links and searches still find the same Tests ([`FacetValueList::find`]).
//! - **Reorder** renumbers the whole Facet `1..n`, so ties left by the import
//!   disappear the first time an Admin reorders.
//! - **Merge** moves every Test, Trashed ones too, onto the surviving value;
//!   the old name and its aliases become the survivor's aliases.
//! - **Delete** is refused while any Test (Trashed ones too) uses the value.
//!
//! Every write is one transaction with its change log entries: a value's
//! `name`, `aliases` (comma-separated) and `display_order`, and the Facet
//! field of each Test a merge moves.

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use dioxus::prelude::ServerFnError;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::{
    AppState, add_test, auth,
    change_log::{self, Change, Entity},
    documents::escape_html,
    internal_error,
};
use crate::app::{
    UNKNOWN,
    facet_values::{
        AddValueForm, DeleteValueForm, FacetTab, FacetValueRow, FacetValuesPage, MergeValueForm, MoveValueForm,
        RenameValueForm, ReorderForm,
    },
};

/// The Facets with a value list, as `(form field and Postgres facet, label)`,
/// in search form order.
pub const LISTED_FACETS: [(&str, &str); 5] = [
    ("month", "Month"),
    ("competition", "Competition"),
    ("division_level", "Division/Level"),
    ("test_type", "Test type"),
    ("host", "Host"),
];

/// The longest name a value may have; download names are capped near 150.
const MAX_NAME_CHARS: usize = 100;

fn label_of(facet: &str) -> Option<&'static str> {
    LISTED_FACETS.iter().find(|(f, _)| *f == facet).map(|(_, label)| *label)
}

// ---------------------------------------------------------------- the list

/// Every Facet value and alias, for reading dropdowns and search URLs.
pub struct FacetValueList {
    /// `(facet, id, name)`, by Facet in display order.
    pub values: Vec<(String, i64, String)>,
    /// `(facet, alias, value id)`
    pub aliases: Vec<(String, String, i64)>,
}

impl FacetValueList {
    pub async fn load(db: &PgPool) -> Result<Self, sqlx::Error> {
        Ok(Self {
            values: sqlx::query_as(
                "SELECT facet::text, id, name FROM facet_values ORDER BY facet, display_order, name",
            )
            .fetch_all(db)
            .await?,
            aliases: sqlx::query_as("SELECT facet::text, alias, facet_value_id FROM facet_value_aliases")
                .fetch_all(db)
                .await?,
        })
    }

    /// `(id, name)` of `facet`'s values, in display order.
    pub fn of(&self, facet: &str) -> impl Iterator<Item = (i64, &str)> + use<'_> {
        let facet = facet.to_owned();
        self.values
            .iter()
            .filter(move |(f, _, _)| *f == facet)
            .map(|(_, id, name)| (*id, name.as_str()))
    }

    /// The names of `facet`'s values, in display order.
    pub fn names(&self, facet: &str) -> Vec<String> {
        self.of(facet).map(|(_, name)| name.to_owned()).collect()
    }

    pub fn name(&self, facet: &str, id: i64) -> Option<&str> {
        self.of(facet).find(|(v, _)| *v == id).map(|(_, name)| name)
    }

    /// The value `name` means in `facet`: a value of that name, else one
    /// with that alias (a former name), else the same ignoring case.
    pub fn find(&self, facet: &str, name: &str) -> Option<i64> {
        let named = |same: &dyn Fn(&str) -> bool| {
            self.of(facet).find(|(_, n)| same(n)).map(|(id, _)| id).or_else(|| {
                self.aliases
                    .iter()
                    .find(|(f, a, _)| f == facet && same(a))
                    .map(|(_, _, id)| *id)
            })
        };
        let lower = name.to_lowercase();
        named(&|n| n == name).or_else(|| named(&|n| n.to_lowercase() == lower))
    }
}

/// Create a value at the end of its Facet's display order, and log it.
pub async fn create(tx: &mut Transaction<'_, Postgres>, facet: &str, name: &str) -> Result<i64, sqlx::Error> {
    let (id, display_order): (i64, i32) = sqlx::query_as(
        "INSERT INTO facet_values (facet, name, display_order)
         SELECT $1::facet, $2, coalesce(max(display_order), 0) + 1 FROM facet_values WHERE facet = $1::facet
         RETURNING id, display_order",
    )
    .bind(facet)
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    change_log::record_created(
        tx,
        Entity::FacetValue,
        id,
        [
            Change::set("facet", facet),
            Change::set("name", name),
            Change::set("display_order", display_order),
        ],
    )
    .await?;
    Ok(id)
}

// -------------------------------------------------------------------- page

#[derive(FromRow)]
struct ValueRecord {
    id: i64,
    name: String,
    tests: i64,
    trashed_tests: i64,
    aliases: Vec<String>,
}

/// The Facet values page's data for `facet` (Month when empty or unknown).
pub async fn page(state: &AppState, headers: &HeaderMap, facet: &str) -> Result<FacetValuesPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    let (facet, label) = LISTED_FACETS
        .into_iter()
        .find(|(f, _)| *f == facet)
        .unwrap_or(LISTED_FACETS[0]);
    // A value's ID is unique across Facets, so it can only appear in its own
    // Facet's column.
    let values: Vec<ValueRecord> = sqlx::query_as(
        "SELECT v.id, v.name,
                count(t.id) AS tests,
                count(t.id) FILTER (WHERE t.trashed_at IS NOT NULL) AS trashed_tests,
                coalesce((SELECT array_agg(a.alias ORDER BY a.alias) FROM facet_value_aliases a
                          WHERE a.facet_value_id = v.id), '{}') AS aliases
         FROM facet_values v
         LEFT JOIN tests t ON v.id IN (t.month_id, t.competition_id, t.division_level_id, t.test_type_id, t.host_id)
         WHERE v.facet = $1::facet
         GROUP BY v.id
         ORDER BY v.display_order, v.name",
    )
    .bind(facet)
    .fetch_all(&state.db)
    .await
    .map_err(internal_error)?;
    Ok(FacetValuesPage {
        csrf,
        facet: facet.to_owned(),
        label: label.to_owned(),
        tabs: LISTED_FACETS
            .into_iter()
            .map(|(facet, label)| FacetTab {
                facet: facet.to_owned(),
                label: label.to_owned(),
            })
            .collect(),
        values: values
            .into_iter()
            .map(|v| FacetValueRow {
                id: v.id,
                name: v.name,
                tests: v.tests,
                trashed_tests: v.trashed_tests,
                aliases: v.aliases,
            })
            .collect(),
    })
}

// ------------------------------------------------------------------ writes

/// A refused change, shown as a page that links back to the Facet.
struct Refusal {
    status: StatusCode,
    message: String,
    /// The Facet to go back to; the first one when unknown.
    facet: String,
}

impl Refusal {
    fn new(status: StatusCode, facet: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            facet: facet.to_owned(),
        }
    }

    fn response(&self) -> Response {
        let message = escape_html(&self.message);
        let facet = label_of(&self.facet).map_or(LISTED_FACETS[0].0, |_| self.facet.as_str());
        let page = format!(
            "<!DOCTYPE html>\
<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Not saved · FAMAT Test Bank</title>\
<style>body{{font:16px/1.4 system-ui,sans-serif;margin:0;color:#1a1a1a;background:#fff}}\
main{{max-width:64rem;margin:0 auto;padding:1rem}}#refusal{{color:#b00020}}</style></head>\
<body><main><h1>Nothing was saved</h1>\
<p id=\"refusal\">{message}</p>\
<p><a href=\"/admin/facets?facet={facet}\">Back to Facet values</a></p>\
</main></body></html>"
        );
        (self.status, [(header::CACHE_CONTROL, "no-store")], Html(page)).into_response()
    }
}

/// What a write did: where to send the Admin, or why nothing was saved.
type Outcome = Result<String, Refusal>;

/// Run one Admin write: check the session and CSRF token, then `write` in a
/// transaction that commits only if it succeeds. Answers 303 to the page.
async fn write<F>(state: &AppState, headers: &HeaderMap, csrf: &str, write: F) -> Result<Response, ServerFnError>
where
    F: AsyncFnOnce(&mut Transaction<'static, Postgres>) -> anyhow::Result<Outcome>,
{
    auth::require_admin_form(state, headers, csrf).await?;
    let result: anyhow::Result<Outcome> = async {
        let mut tx = state.db.begin().await?;
        let outcome = write(&mut tx).await?;
        if outcome.is_ok() {
            tx.commit().await?;
        }
        Ok(outcome)
    }
    .await;
    match result.map_err(internal_error)? {
        Ok(location) => Ok(auth::see_other(&location, None)),
        Err(refusal) => Ok(refusal.response()),
    }
}

fn facet_page(facet: &str, id: i64) -> String {
    format!("/admin/facets?facet={facet}#v{id}")
}

/// A value, locked for the rest of the transaction.
struct Locked {
    id: i64,
    facet: String,
    name: String,
    display_order: i32,
    aliases: Vec<String>,
}

impl Locked {
    fn label(&self) -> &'static str {
        label_of(&self.facet).unwrap_or("Facet")
    }
}

async fn lock(tx: &mut Transaction<'_, Postgres>, id: &str) -> anyhow::Result<Result<Locked, Refusal>> {
    let gone = || {
        Refusal::new(
            StatusCode::NOT_FOUND,
            "",
            "That value no longer exists. Reload the page.",
        )
    };
    let Ok(id) = id.trim().parse::<i64>() else {
        return Ok(Err(gone()));
    };
    let found: Option<(String, String, i32)> =
        sqlx::query_as("SELECT facet::text, name, display_order FROM facet_values WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((facet, name, display_order)) = found else {
        return Ok(Err(gone()));
    };
    let aliases = aliases_of(tx, id).await?;
    Ok(Ok(Locked {
        id,
        facet,
        name,
        display_order,
        aliases,
    }))
}

async fn aliases_of(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT alias FROM facet_value_aliases WHERE facet_value_id = $1 ORDER BY alias")
        .bind(id)
        .fetch_all(&mut **tx)
        .await
}

/// How the change log shows a value's aliases: `None` when it has none.
fn alias_list(aliases: &[String]) -> Option<String> {
    (!aliases.is_empty()).then(|| aliases.join(", "))
}

/// A name an Admin typed, trimmed, or why it can't be one.
fn typed_name(facet: &str, name: &str) -> Result<String, Refusal> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let label = label_of(facet).unwrap_or("value");
    if name.is_empty() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            facet,
            format!("Type a name for the {label}."),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            facet,
            format!("A name can be at most {MAX_NAME_CHARS} characters."),
        ));
    }
    // The search URLs say Unknown with this word.
    if name.eq_ignore_ascii_case(UNKNOWN) {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            facet,
            "“Unknown” is how the search says a value is not known; choose another name.",
        ));
    }
    Ok(name)
}

/// The value of `facet` that is named `name` or has it as an alias, ignoring
/// case: `(id, name, whether it matched an alias)`.
async fn named(
    tx: &mut Transaction<'_, Postgres>,
    facet: &str,
    name: &str,
) -> Result<Option<(i64, String, bool)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, false FROM facet_values WHERE facet = $1::facet AND lower(name) = lower($2)
         UNION ALL
         SELECT v.id, v.name, true FROM facet_value_aliases a JOIN facet_values v ON v.id = a.facet_value_id
         WHERE a.facet = $1::facet AND lower(a.alias) = lower($2)
         ORDER BY 3, 1
         LIMIT 1",
    )
    .bind(facet)
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
}

/// `POST /api/admin/facet-values`: **+ Add value**, at the end of the Facet.
pub async fn add(state: &AppState, headers: &HeaderMap, form: AddValueForm) -> Result<Response, ServerFnError> {
    write(state, headers, &form.csrf, async |tx| {
        let facet = form.facet.as_str();
        let Some(label) = label_of(facet) else {
            return Ok(Err(Refusal::new(StatusCode::BAD_REQUEST, "", "Choose a Facet.")));
        };
        let name = match typed_name(facet, &form.name) {
            Ok(name) => name,
            Err(refusal) => return Ok(Err(refusal)),
        };
        // Serialize adds to one Facet, so two can't both take one name.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('facet_values:' || $1))")
            .bind(facet)
            .execute(&mut **tx)
            .await?;
        if let Some((_, existing, alias)) = named(tx, facet, &name).await? {
            let message = if alias {
                format!("“{name}” is a former name of the {label} “{existing}”.")
            } else {
                format!("There is already a {label} “{existing}”.")
            };
            return Ok(Err(Refusal::new(StatusCode::CONFLICT, facet, message)));
        }
        let id = create(tx, facet, &name).await?;
        Ok(Ok(facet_page(facet, id)))
    })
    .await
}

/// `POST /api/admin/facet-values/rename`: the old name becomes an alias.
/// Renaming to one of the value's own aliases takes that alias back; a name
/// another value has (or had) is refused, since that is a merge.
pub async fn rename(state: &AppState, headers: &HeaderMap, form: RenameValueForm) -> Result<Response, ServerFnError> {
    write(state, headers, &form.csrf, async |tx| {
        let value = match lock(tx, &form.id).await? {
            Ok(value) => value,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let (facet, label) = (value.facet.as_str(), value.label());
        let name = match typed_name(facet, &form.name) {
            Ok(name) => name,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if name == value.name {
            return Ok(Ok(facet_page(facet, value.id)));
        }
        if let Some((other, existing, alias)) = named(tx, facet, &name).await?
            && other != value.id
        {
            let message = if alias {
                format!(
                    "“{name}” is a former name of the {label} “{existing}”. \
                     To make them one, merge “{}” into “{existing}”.",
                    value.name
                )
            } else {
                format!(
                    "There is already a {label} “{existing}”. To make them one, merge “{}” into it.",
                    value.name
                )
            };
            return Ok(Err(Refusal::new(StatusCode::CONFLICT, facet, message)));
        }
        sqlx::query("UPDATE facet_values SET name = $2 WHERE id = $1")
            .bind(value.id)
            .bind(&name)
            .execute(&mut **tx)
            .await?;
        // The new name is no longer a former one; the old name now is.
        sqlx::query("DELETE FROM facet_value_aliases WHERE facet = $1::facet AND alias = $2")
            .bind(facet)
            .bind(&name)
            .execute(&mut **tx)
            .await?;
        add_alias(tx, facet, &value.name, value.id).await?;
        let aliases = aliases_of(tx, value.id).await?;
        change_log::record(
            tx,
            Entity::FacetValue,
            value.id,
            [
                Change::new("name", &value.name, &name),
                Change::new("aliases", alias_list(&value.aliases), alias_list(&aliases)),
            ],
        )
        .await?;
        Ok(Ok(facet_page(facet, value.id)))
    })
    .await
}

async fn add_alias(tx: &mut Transaction<'_, Postgres>, facet: &str, alias: &str, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO facet_value_aliases (facet, alias, facet_value_id) VALUES ($1::facet, $2, $3)
         ON CONFLICT (facet, alias) DO UPDATE SET facet_value_id = excluded.facet_value_id",
    )
    .bind(facet)
    .bind(alias)
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// `POST /api/admin/facet-values/merge`: **Merge into…** another value of
/// the same Facet, named (or found by a former name) in `into`.
pub async fn merge(state: &AppState, headers: &HeaderMap, form: MergeValueForm) -> Result<Response, ServerFnError> {
    write(state, headers, &form.csrf, async |tx| {
        let value = match lock(tx, &form.id).await? {
            Ok(value) => value,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let (facet, label) = (value.facet.as_str(), value.label());
        let into = form.into.split_whitespace().collect::<Vec<_>>().join(" ");
        if into.is_empty() {
            let message = format!("Choose the {label} to merge “{}” into.", value.name);
            return Ok(Err(Refusal::new(StatusCode::BAD_REQUEST, facet, message)));
        }
        let survivor = match named(tx, facet, &into).await? {
            Some((id, _, _)) if id == value.id => {
                let message = format!("“{}” can’t be merged into itself. Choose another {label}.", value.name);
                return Ok(Err(Refusal::new(StatusCode::BAD_REQUEST, facet, message)));
            }
            Some((id, _, _)) => match lock(tx, &id.to_string()).await? {
                Ok(survivor) => survivor,
                Err(refusal) => return Ok(Err(refusal)),
            },
            None => {
                let message = format!("There is no {label} “{into}”.");
                return Ok(Err(Refusal::new(StatusCode::BAD_REQUEST, facet, message)));
            }
        };

        // Every Test moves, Trashed ones too, so the old value can go.
        let moved: Vec<i64> = sqlx::query_scalar(
            "UPDATE tests SET
                 month_id          = CASE WHEN month_id          = $1 THEN $2 ELSE month_id END,
                 competition_id    = CASE WHEN competition_id    = $1 THEN $2 ELSE competition_id END,
                 division_level_id = CASE WHEN division_level_id = $1 THEN $2 ELSE division_level_id END,
                 test_type_id      = CASE WHEN test_type_id      = $1 THEN $2 ELSE test_type_id END,
                 host_id           = CASE WHEN host_id           = $1 THEN $2 ELSE host_id END,
                 updated_at        = now()
             WHERE $1 IN (month_id, competition_id, division_level_id, test_type_id, host_id)
             RETURNING id",
        )
        .bind(value.id)
        .bind(survivor.id)
        .fetch_all(&mut **tx)
        .await?;
        add_test::replace_held_value(tx, facet, value.id, survivor.id).await?;

        // The old name and its aliases become the survivor's.
        sqlx::query("UPDATE facet_value_aliases SET facet_value_id = $2 WHERE facet_value_id = $1")
            .bind(value.id)
            .bind(survivor.id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM facet_values WHERE id = $1")
            .bind(value.id)
            .execute(&mut **tx)
            .await?;
        add_alias(tx, facet, &value.name, survivor.id).await?;
        // A former name that is the survivor's own name is no alias.
        sqlx::query("DELETE FROM facet_value_aliases WHERE facet = $1::facet AND alias = $2")
            .bind(facet)
            .bind(&survivor.name)
            .execute(&mut **tx)
            .await?;
        let aliases = aliases_of(tx, survivor.id).await?;

        let mut moved = moved;
        moved.sort_unstable();
        for test in moved {
            let change = Change::new(field_of(facet), &value.name, &survivor.name);
            change_log::record(tx, Entity::Test, test, [change]).await?;
        }
        change_log::record_deleted(
            tx,
            Entity::FacetValue,
            value.id,
            [
                Change::cleared("facet", facet),
                Change::cleared("name", &value.name),
                Change::cleared("display_order", value.display_order),
                Change::cleared("aliases", alias_list(&value.aliases)),
                Change::set("merged_into", &survivor.name),
            ],
        )
        .await?;
        change_log::record(
            tx,
            Entity::FacetValue,
            survivor.id,
            [Change::new(
                "aliases",
                alias_list(&survivor.aliases),
                alias_list(&aliases),
            )],
        )
        .await?;
        Ok(Ok(facet_page(facet, survivor.id)))
    })
    .await
}

/// The change log field of a Test's Facet, as Add test names it.
fn field_of(facet: &str) -> &'static str {
    LISTED_FACETS
        .iter()
        .find(|(f, _)| *f == facet)
        .map_or("facet", |(f, _)| *f)
}

/// `POST /api/admin/facet-values/delete`: only a value no Test uses (Trashed
/// Tests count) and no held upload chose.
pub async fn delete(state: &AppState, headers: &HeaderMap, form: DeleteValueForm) -> Result<Response, ServerFnError> {
    write(state, headers, &form.csrf, async |tx| {
        let value = match lock(tx, &form.id).await? {
            Ok(value) => value,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let facet = value.facet.as_str();
        // A Test taking the value meanwhile waits on the row lock `lock`
        // holds, then fails its foreign key check once the value is gone.
        let tests: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tests
             WHERE $1 IN (month_id, competition_id, division_level_id, test_type_id, host_id)",
        )
        .bind(value.id)
        .fetch_one(&mut **tx)
        .await?;
        if tests > 0 {
            let message = format!(
                "“{}” can’t be deleted: {} use{} it. Merge it into another {} instead.",
                value.name,
                if tests == 1 {
                    "1 Test".to_owned()
                } else {
                    format!("{tests} Tests")
                },
                if tests == 1 { "s" } else { "" },
                value.label()
            );
            return Ok(Err(Refusal::new(StatusCode::CONFLICT, facet, message)));
        }
        if add_test::held_uses(tx, facet, value.id).await? {
            let message = format!(
                "“{}” can’t be deleted: files waiting on Add test’s duplicate warning use it.",
                value.name
            );
            return Ok(Err(Refusal::new(StatusCode::CONFLICT, facet, message)));
        }
        sqlx::query("DELETE FROM facet_values WHERE id = $1")
            .bind(value.id)
            .execute(&mut **tx)
            .await?;
        change_log::record_deleted(
            tx,
            Entity::FacetValue,
            value.id,
            [
                Change::cleared("facet", facet),
                Change::cleared("name", &value.name),
                Change::cleared("display_order", value.display_order),
                Change::cleared("aliases", alias_list(&value.aliases)),
            ],
        )
        .await?;
        Ok(Ok(format!("/admin/facets?facet={facet}")))
    })
    .await
}

/// `POST /api/admin/facet-values/move`: the up/down buttons (no JS).
pub async fn move_value(state: &AppState, headers: &HeaderMap, form: MoveValueForm) -> Result<Response, ServerFnError> {
    write(state, headers, &form.csrf, async |tx| {
        let value = match lock(tx, &form.id).await? {
            Ok(value) => value,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let facet = value.facet.clone();
        let mut order = current_order(tx, &facet).await?;
        let Some(i) = order.iter().position(|(id, _)| *id == value.id) else {
            return Ok(Ok(facet_page(&facet, value.id)));
        };
        match form.direction.as_str() {
            "up" if i > 0 => order.swap(i, i - 1),
            "down" if i + 1 < order.len() => order.swap(i, i + 1),
            "up" | "down" => {}
            _ => return Ok(Err(Refusal::new(StatusCode::BAD_REQUEST, &facet, "Choose up or down."))),
        }
        renumber(tx, order).await?;
        Ok(Ok(facet_page(&facet, value.id)))
    })
    .await
}

/// `POST /api/admin/facet-values/reorder`: a drag, as the Facet's value IDs
/// in their new order (comma-separated). IDs that aren't the Facet's are
/// ignored, and values it leaves out keep their order after the listed ones,
/// so a value added meanwhile is not lost.
pub async fn reorder(state: &AppState, headers: &HeaderMap, form: ReorderForm) -> Result<Response, ServerFnError> {
    write(state, headers, &form.csrf, async |tx| {
        let facet = form.facet.as_str();
        if label_of(facet).is_none() {
            return Ok(Err(Refusal::new(StatusCode::BAD_REQUEST, "", "Choose a Facet.")));
        }
        let Ok(wanted) = form
            .order
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::parse::<i64>)
            .collect::<Result<Vec<_>, _>>()
        else {
            return Ok(Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                facet,
                "That order is not a list of values.",
            )));
        };
        let mut current = current_order(tx, facet).await?;
        let mut order = Vec::with_capacity(current.len());
        for id in wanted {
            if let Some(i) = current.iter().position(|(v, _)| *v == id) {
                order.push(current.remove(i));
            }
        }
        order.extend(current);
        renumber(tx, order).await?;
        Ok(Ok(format!("/admin/facets?facet={facet}")))
    })
    .await
}

/// `(id, display_order)` of a Facet's values in display order, locked.
async fn current_order(tx: &mut Transaction<'_, Postgres>, facet: &str) -> Result<Vec<(i64, i32)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, display_order FROM facet_values WHERE facet = $1::facet
         ORDER BY display_order, name FOR UPDATE",
    )
    .bind(facet)
    .fetch_all(&mut **tx)
    .await
}

/// Give the values `1..n` in the order given, logging each one that moved.
async fn renumber(tx: &mut Transaction<'_, Postgres>, order: Vec<(i64, i32)>) -> Result<(), sqlx::Error> {
    for (position, (id, before)) in (1..).zip(order) {
        if position == before {
            continue;
        }
        sqlx::query("UPDATE facet_values SET display_order = $2 WHERE id = $1")
            .bind(id)
            .bind(position)
            .execute(&mut **tx)
            .await?;
        change_log::record(
            tx,
            Entity::FacetValue,
            id,
            [Change::new("display_order", before, position)],
        )
        .await?;
    }
    Ok(())
}
