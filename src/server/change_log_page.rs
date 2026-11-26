//! The Change log page's data (the page is `crate::app::change_log`).
//!
//! Entries are grouped into Admin actions by `changed_at`, which is the
//! writing transaction's start (`now()`), and paged by action, so an action
//! is never split across pages. The filter narrows both to one entity type
//! and, optionally, one ID.

use std::collections::HashMap;

use axum::http::HeaderMap;
use dioxus::prelude::ServerFnError;
use sqlx::{FromRow, PgPool};

use super::{AppState, auth, internal_error, trash::local_time_sql};
use crate::app::{
    change_log::{ChangeLogPage, ENTITIES, LoggedAction, LoggedEntry},
    test_edit::edit_path,
};

/// Admin actions per page.
const ACTIONS_PER_PAGE: i64 = 50;

pub async fn page(
    state: &AppState,
    headers: &HeaderMap,
    entity: &str,
    id: &str,
    page: &str,
) -> Result<ChangeLogPage, ServerFnError> {
    auth::require_admin(state, headers).await?;
    let filter = Filter::parse(entity, id);
    let page = page.trim().parse::<i64>().unwrap_or(1);
    load(&state.db, filter, page).await.map_err(internal_error)
}

struct Filter {
    entity: Option<&'static str>,
    id_text: String,
    id: Option<i64>,
    bad_id: bool,
}

impl Filter {
    /// An entity type from the list, and an ID typed as `12`, `T12` or
    /// `D12`. A `T` or `D` also picks the type when none is chosen.
    fn parse(entity: &str, id: &str) -> Self {
        let mut entity = ENTITIES.iter().map(|(value, _)| *value).find(|value| *value == entity);
        let id_text = id.trim().to_owned();
        let (prefix, digits) = match id_text.chars().next() {
            Some('T' | 't') => (Some("test"), &id_text[1..]),
            Some('D' | 'd') => (Some("document"), &id_text[1..]),
            _ => (None, id_text.as_str()),
        };
        let id = digits.trim().parse::<i64>().ok().filter(|id| *id > 0);
        let bad_id = !id_text.is_empty() && id.is_none();
        if id.is_some() && entity.is_none() {
            entity = prefix;
        }
        Self {
            entity,
            id_text,
            id,
            bad_id,
        }
    }
}

#[derive(FromRow)]
struct Row {
    id: i64,
    /// Microseconds, the action's key.
    at: i64,
    shown: String,
    entity: String,
    entity_id: i64,
    field: String,
    before: Option<String>,
    after: Option<String>,
}

async fn load(db: &PgPool, filter: Filter, page: i64) -> anyhow::Result<ChangeLogPage> {
    let filtered = "($1::text IS NULL OR entity = $1) AND ($2::bigint IS NULL OR entity_id = $2)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(DISTINCT changed_at) FROM change_log WHERE {filtered}"
    )))
    .bind(filter.entity)
    .bind(filter.id)
    .fetch_one(db)
    .await?;
    let pages = ((total + ACTIONS_PER_PAGE - 1) / ACTIONS_PER_PAGE).max(1);
    let page = page.clamp(1, pages);
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH actions AS (
             SELECT DISTINCT changed_at FROM change_log WHERE {filtered}
             ORDER BY changed_at DESC
             LIMIT $3 OFFSET $4
         )
         SELECT id, (extract(epoch FROM changed_at) * 1000000)::bigint AS at,
                {} AS shown, entity, entity_id, field, before, after
         FROM change_log
         WHERE changed_at IN (SELECT changed_at FROM actions) AND {filtered}
         ORDER BY changed_at DESC, id",
        local_time_sql("changed_at")
    )))
    .bind(filter.entity)
    .bind(filter.id)
    .bind(ACTIONS_PER_PAGE)
    .bind((page - 1) * ACTIONS_PER_PAGE)
    .fetch_all(db)
    .await?;

    let links = links(db, &rows).await?;
    let mut actions: Vec<(i64, LoggedAction)> = Vec::new();
    for row in rows {
        let entry = LoggedEntry {
            id: row.id,
            record: record_name(&row.entity, row.entity_id),
            link: links.get(&(row.entity.clone(), row.entity_id)).cloned(),
            entity: row.entity,
            entity_id: row.entity_id,
            field: row.field.replace('_', " "),
            before: row.before,
            after: row.after,
        };
        match actions.last_mut() {
            Some((at, action)) if *at == row.at => action.entries.push(entry),
            _ => actions.push((
                row.at,
                LoggedAction {
                    when: row.shown,
                    entries: vec![entry],
                },
            )),
        }
    }
    Ok(ChangeLogPage {
        entity: filter.entity.unwrap_or_default().to_owned(),
        id: filter.id_text,
        bad_id: filter.bad_id,
        page,
        pages,
        total,
        actions: actions.into_iter().map(|(_, action)| action).collect(),
    })
}

/// How the page names a record: `T12`, `D34`, `Facet value 7`.
fn record_name(entity: &str, id: i64) -> String {
    match entity {
        "test" => format!("T{id}"),
        "document" => format!("D{id}"),
        "facet_value" => format!("Facet value {id}"),
        "document_kind" => format!("Document kind {id}"),
        other => format!("{other} {id}"),
    }
}

/// Where each logged record that still exists can be seen: a Test's edit
/// page, a Document on the edit page of its first untrashed Test, the
/// Trash for either when it is there, a Facet value on its Facet's page,
/// a kind on the Document kinds page.
async fn links(db: &PgPool, rows: &[Row]) -> Result<HashMap<(String, i64), String>, sqlx::Error> {
    let ids = |entity: &str| -> Vec<i64> {
        let mut ids: Vec<i64> = rows
            .iter()
            .filter(|r| r.entity == entity)
            .map(|r| r.entity_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let mut links = HashMap::new();

    let tests: Vec<(i64, bool)> = sqlx::query_as("SELECT id, trashed_at IS NOT NULL FROM tests WHERE id = ANY($1)")
        .bind(ids("test"))
        .fetch_all(db)
        .await?;
    for (id, trashed) in tests {
        let link = if trashed {
            format!("/admin/trash#T{id}")
        } else {
            edit_path(id)
        };
        links.insert(("test".to_owned(), id), link);
    }

    let documents: Vec<(i64, bool, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT d.id, d.trashed_at IS NOT NULL,
                (SELECT min(td.test_id) FROM test_documents td JOIN tests t ON t.id = td.test_id
                 WHERE td.document_id = d.id AND t.trashed_at IS NULL),
                (SELECT min(td.test_id) FROM test_documents td WHERE td.document_id = d.id)
         FROM documents d WHERE d.id = ANY($1)",
    )
    .bind(ids("document"))
    .fetch_all(db)
    .await?;
    for (id, trashed, untrashed_test, any_test) in documents {
        let link = match (trashed, untrashed_test, any_test) {
            (true, _, _) => format!("/admin/trash#D{id}"),
            (false, Some(test), _) => format!("{}#D{id}", edit_path(test)),
            // Out of sight with its Tests, all in the Trash.
            (false, None, Some(test)) => format!("/admin/trash#T{test}"),
            (false, None, None) => continue,
        };
        links.insert(("document".to_owned(), id), link);
    }

    let values: Vec<(i64, String)> = sqlx::query_as("SELECT id, facet::text FROM facet_values WHERE id = ANY($1)")
        .bind(ids("facet_value"))
        .fetch_all(db)
        .await?;
    for (id, facet) in values {
        links.insert(
            ("facet_value".to_owned(), id),
            format!("/admin/facets?facet={facet}#v{id}"),
        );
    }

    let kinds: Vec<i64> = sqlx::query_scalar("SELECT id FROM document_kinds WHERE id = ANY($1)")
        .bind(ids("document_kind"))
        .fetch_all(db)
        .await?;
    for id in kinds {
        links.insert(("document_kind".to_owned(), id), format!("/admin/document-kinds#K{id}"));
    }
    Ok(links)
}
