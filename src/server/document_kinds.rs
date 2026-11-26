//! The Document kinds page, server side (the page is
//! `crate::app::document_kinds`): rename, add, reorder, tick what each kind
//! contains, and delete a kind no Document uses.
//!
//! Every action is a native form post that checks the session and its CSRF
//! token, writes in one transaction with its change log entries
//! ([`Entity::DocumentKind`]), and answers 303 back to the page, or an HTML
//! refusal a person reads in the browser.
//!
//! Nothing about the kinds is cached: the search's Includes filters, the
//! download menus and the zips read `document_kinds` on every request, so
//! they follow a change at once.
//!
//! Each write locks every kind row first (there are only a handful), so two
//! Admins' changes apply one after the other.

use std::collections::HashMap;

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use dioxus::prelude::ServerFnError;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::{
    AppState, auth,
    change_log::{self, Change, Entity},
    documents::escape_html,
    internal_error,
};
use crate::{
    app::document_kinds::{
        AddKindForm, DeleteKindForm, DocumentKindsPage, KindRow, MAX_NAME_CHARS, MoveKindForm, ReorderKindsForm,
        SaveKindForm,
    },
    download::Contents,
};

/// Where every action answers to.
const PAGE: &str = "/admin/document-kinds";

// -------------------------------------------------------------------- page

/// The page's data: every kind in display order, with its usage.
pub async fn page(state: &AppState, headers: &HeaderMap) -> Result<DocumentKindsPage, ServerFnError> {
    let session = auth::require_admin(state, headers).await?;
    let csrf = auth::csrf_token(&state.csrf_key, Some(&session));
    let kinds = rows(&state.db).await.map_err(internal_error)?;
    Ok(DocumentKindsPage { csrf, kinds })
}

#[derive(FromRow)]
struct RowRecord {
    id: i64,
    name: String,
    contains_questions: bool,
    contains_answers: bool,
    contains_solutions: bool,
    documents: i64,
    waiting: i64,
}

async fn rows(db: &PgPool) -> Result<Vec<KindRow>, sqlx::Error> {
    let records: Vec<RowRecord> = sqlx::query_as(
        "SELECT k.id, k.name, k.contains_questions, k.contains_answers, k.contains_solutions,
                (SELECT count(*) FROM documents d WHERE d.kind_id = k.id) AS documents,
                (SELECT count(*) FROM pending_uploads p
                 CROSS JOIN LATERAL jsonb_array_elements(p.plan->'documents') pd
                 WHERE (pd->>'kind_id')::bigint = k.id) AS waiting
         FROM document_kinds k
         ORDER BY k.display_order, k.name, k.id",
    )
    .fetch_all(db)
    .await?;
    Ok(records
        .into_iter()
        .map(|r| KindRow {
            id: r.id,
            name: r.name,
            contains: Contents {
                questions: r.contains_questions,
                answers: r.contains_answers,
                solutions: r.contains_solutions,
            },
            documents: r.documents,
            waiting: r.waiting,
        })
        .collect())
}

// ----------------------------------------------------------------- actions

/// `+ Add`: a new kind at the end of the display order.
pub async fn add(state: &AppState, headers: &HeaderMap, form: AddKindForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let contents = contents(form.questions, form.answers, form.solutions);
    respond(add_kind(&state.db, &form.name, contents).await)
}

async fn add_kind(db: &PgPool, name: &str, contents: Contents) -> Result<Response, Refusal> {
    let mut tx = db.begin().await?;
    let kinds = lock_kinds(&mut tx).await?;
    let name = valid_name(name, &kinds, None)?;
    let display_order = kinds.iter().map(|k| k.display_order).max().unwrap_or(0) + 1;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO document_kinds (name, display_order, contains_questions, contains_answers, contains_solutions)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(&name)
    .bind(display_order)
    .bind(contents.questions)
    .bind(contents.answers)
    .bind(contents.solutions)
    .fetch_one(&mut *tx)
    .await?;
    change_log::record_created(
        &mut tx,
        Entity::DocumentKind,
        id,
        [
            Change::set("name", &name),
            Change::set("display_order", display_order),
            Change::set("contains_questions", contents.questions),
            Change::set("contains_answers", contents.answers),
            Change::set("contains_solutions", contents.solutions),
        ],
    )
    .await?;
    tx.commit().await?;
    Ok(auth::see_other(PAGE, None))
}

/// **Save** on a row: its name and what it contains.
pub async fn save(state: &AppState, headers: &HeaderMap, form: SaveKindForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    let contents = contents(form.questions, form.answers, form.solutions);
    respond(save_kind(&state.db, form.id, &form.name, contents).await)
}

async fn save_kind(db: &PgPool, id: i64, name: &str, contents: Contents) -> Result<Response, Refusal> {
    let mut tx = db.begin().await?;
    let kinds = lock_kinds(&mut tx).await?;
    let kind = find(&kinds, id)?;
    let name = valid_name(name, &kinds, Some(id))?;
    sqlx::query(
        "UPDATE document_kinds
         SET name = $2, contains_questions = $3, contains_answers = $4, contains_solutions = $5
         WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(contents.questions)
    .bind(contents.answers)
    .bind(contents.solutions)
    .execute(&mut *tx)
    .await?;
    change_log::record(
        &mut tx,
        Entity::DocumentKind,
        id,
        [
            Change::new("name", &kind.name, &name),
            Change::new("contains_questions", kind.contains_questions, contents.questions),
            Change::new("contains_answers", kind.contains_answers, contents.answers),
            Change::new("contains_solutions", kind.contains_solutions, contents.solutions),
        ],
    )
    .await?;
    tx.commit().await?;
    Ok(auth::see_other(PAGE, None))
}

/// ↑ / ↓ on a row (the reorder without JavaScript): swap it with its
/// neighbour. At either end it stays where it is.
pub async fn move_kind(state: &AppState, headers: &HeaderMap, form: MoveKindForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    respond(move_by(&state.db, form.id, &form.direction).await)
}

async fn move_by(db: &PgPool, id: i64, direction: &str) -> Result<Response, Refusal> {
    let mut tx = db.begin().await?;
    let kinds = lock_kinds(&mut tx).await?;
    find(&kinds, id)?;
    let mut order: Vec<i64> = kinds.iter().map(|k| k.id).collect();
    let i = order.iter().position(|&k| k == id).unwrap_or_default();
    let neighbour = match direction {
        "up" => i.checked_sub(1),
        "down" => Some(i + 1).filter(|&j| j < order.len()),
        _ => return Err(Refusal::bad_request("Choose ↑ or ↓.")),
    };
    if let Some(j) = neighbour {
        order.swap(i, j);
    }
    renumber(&mut tx, &kinds, &order).await?;
    tx.commit().await?;
    Ok(auth::see_other(PAGE, None))
}

/// The drag reorder: the page posts every kind's ID in the new order.
pub async fn reorder(state: &AppState, headers: &HeaderMap, form: ReorderKindsForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    respond(reorder_to(&state.db, &form.order).await)
}

async fn reorder_to(db: &PgPool, order: &str) -> Result<Response, Refusal> {
    let mut tx = db.begin().await?;
    let kinds = lock_kinds(&mut tx).await?;
    let order: Option<Vec<i64>> = order.split(',').map(|id| id.trim().parse().ok()).collect();
    let mut sorted = order.clone().unwrap_or_default();
    sorted.sort_unstable();
    let mut ids: Vec<i64> = kinds.iter().map(|k| k.id).collect();
    ids.sort_unstable();
    let Some(order) = order.filter(|_| sorted == ids) else {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "The Document kinds changed since the page was loaded. Reload it and drag again.",
        ));
    };
    renumber(&mut tx, &kinds, &order).await?;
    tx.commit().await?;
    Ok(auth::see_other(PAGE, None))
}

/// **Delete**: only a kind no Document uses, including Hidden Documents,
/// Documents in Trash, and files waiting in a duplicate warning.
pub async fn delete(state: &AppState, headers: &HeaderMap, form: DeleteKindForm) -> Result<Response, ServerFnError> {
    auth::require_admin_form(state, headers, &form.csrf).await?;
    respond(delete_kind(&state.db, form.id).await)
}

async fn delete_kind(db: &PgPool, id: i64) -> Result<Response, Refusal> {
    let mut tx = db.begin().await?;
    let kinds = lock_kinds(&mut tx).await?;
    let kind = find(&kinds, id)?;
    // The row lock makes a Document being added with this kind wait, so
    // these counts cannot go stale before the delete.
    let (documents, waiting): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM documents WHERE kind_id = $1),
                (SELECT count(*) FROM pending_uploads p
                 CROSS JOIN LATERAL jsonb_array_elements(p.plan->'documents') pd
                 WHERE (pd->>'kind_id')::bigint = $1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if documents > 0 {
        let documents = if documents == 1 {
            "1 Document uses it".to_owned()
        } else {
            format!("{documents} Documents use it")
        };
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!(
                "“{}” can’t be deleted: {documents} (counting Hidden ones and the Trash). \
                 Give them another kind first.",
                kind.name
            ),
        ));
    }
    if waiting > 0 {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!(
                "“{}” can’t be deleted: files waiting on Add test’s duplicate warning use it. \
                 Finish or discard that upload first.",
                kind.name
            ),
        ));
    }
    if kinds.len() == 1 {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "The last Document kind can’t be deleted: every Document needs one.",
        ));
    }
    sqlx::query("DELETE FROM document_kinds WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    change_log::record_deleted(
        &mut tx,
        Entity::DocumentKind,
        id,
        [
            Change::cleared("name", &kind.name),
            Change::cleared("display_order", kind.display_order),
            Change::cleared("contains_questions", kind.contains_questions),
            Change::cleared("contains_answers", kind.contains_answers),
            Change::cleared("contains_solutions", kind.contains_solutions),
        ],
    )
    .await?;
    tx.commit().await?;
    Ok(auth::see_other(PAGE, None))
}

// ----------------------------------------------------------------- helpers

#[derive(FromRow)]
struct Kind {
    id: i64,
    name: String,
    display_order: i32,
    contains_questions: bool,
    contains_answers: bool,
    contains_solutions: bool,
}

/// Every kind, in display order, locked until the transaction ends.
async fn lock_kinds(tx: &mut Transaction<'_, Postgres>) -> Result<Vec<Kind>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, display_order, contains_questions, contains_answers, contains_solutions
         FROM document_kinds
         ORDER BY display_order, name, id
         FOR UPDATE",
    )
    .fetch_all(&mut **tx)
    .await
}

fn find(kinds: &[Kind], id: i64) -> Result<&Kind, Refusal> {
    kinds.iter().find(|k| k.id == id).ok_or_else(|| {
        Refusal::new(
            StatusCode::NOT_FOUND,
            "That Document kind no longer exists. Reload the page.",
        )
    })
}

/// Give the kinds display orders 1, 2, 3… in `order`, logging each one that
/// changed.
async fn renumber(tx: &mut Transaction<'_, Postgres>, kinds: &[Kind], order: &[i64]) -> Result<(), sqlx::Error> {
    let before: HashMap<i64, i32> = kinds.iter().map(|k| (k.id, k.display_order)).collect();
    for (position, &id) in (1..).zip(order) {
        let old = before[&id];
        if old == position {
            continue;
        }
        sqlx::query("UPDATE document_kinds SET display_order = $2 WHERE id = $1")
            .bind(id)
            .bind(position)
            .execute(&mut **tx)
            .await?;
        change_log::record(
            tx,
            Entity::DocumentKind,
            id,
            [Change::new("display_order", old, position)],
        )
        .await?;
    }
    Ok(())
}

/// The name with its spaces tidied (trimmed, runs collapsed), if it is
/// non-empty, short enough, and no other kind has it (ignoring case).
fn valid_name(name: &str, kinds: &[Kind], except: Option<i64>) -> Result<String, Refusal> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(Refusal::bad_request("Give the Document kind a name."));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Refusal::bad_request(format!(
            "A Document kind’s name can be at most {MAX_NAME_CHARS} characters."
        )));
    }
    if kinds
        .iter()
        .any(|k| Some(k.id) != except && k.name.to_lowercase() == name.to_lowercase())
    {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("There is already a Document kind “{name}”."),
        ));
    }
    Ok(name)
}

fn contents(questions: bool, answers: bool, solutions: bool) -> Contents {
    Contents {
        questions,
        answers,
        solutions,
    }
}

fn respond(result: Result<Response, Refusal>) -> Result<Response, ServerFnError> {
    Ok(result.unwrap_or_else(|refusal| refusal.response()))
}

/// Why an action was not done, as a page a person reads after a form post.
struct Refusal {
    status: StatusCode,
    message: String,
}

impl Refusal {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn response(&self) -> Response {
        let message = escape_html(&self.message);
        let page = format!(
            "<!DOCTYPE html>\
<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Not saved · FAMAT Test Bank</title>\
<style>body{{font:16px/1.4 system-ui,sans-serif;margin:0;color:#1a1a1a;background:#fff}}\
main{{max-width:64rem;margin:0 auto;padding:1rem}}#refusal{{color:#b00020}}</style></head>\
<body><main><h1>Nothing was changed</h1>\
<p id=\"refusal\">{message}</p>\
<p><a href=\"{PAGE}\">Back to Document kinds</a></p>\
</main></body></html>"
        );
        (self.status, [(header::CACHE_CONTROL, "no-store")], Html(page)).into_response()
    }
}

impl From<sqlx::Error> for Refusal {
    fn from(e: sqlx::Error) -> Self {
        // Two Admins adding the same name at once: the unique constraint
        // catches what the check above could not.
        if let Some(db) = e.as_database_error()
            && db.is_unique_violation()
        {
            return Self::new(StatusCode::CONFLICT, "There is already a Document kind with that name.");
        }
        dioxus::logger::tracing::error!("document kinds: {e:#}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong on the server. Try again.",
        )
    }
}
