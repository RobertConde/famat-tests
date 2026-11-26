//! Server-only code: compiled with `--features server`, never into WASM.

pub mod add_test;
pub mod auth;
pub mod change_log;
pub mod cli;
pub mod config;
pub mod db;
pub mod documents;
pub mod download_name;
pub mod import;
pub mod search;
pub mod storage;
pub mod zip;

use axum::{
    Extension, Router,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use dioxus::prelude::ServerFnError;
use sqlx::PgPool;

use crate::app;
use config::Config;
use storage::Storage;

/// Shared by every request; handlers and server functions take it as an
/// `Extension<AppState>`.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub storage: Storage,
    pub csrf_key: auth::CsrfKey,
}

/// The full axum router: Dioxus SSR and server functions, plus plain routes
/// for downloads (ADR 0001: downloads are not server functions).
pub async fn router(config: Config) -> anyhow::Result<Router> {
    let db = db::connect(&config.database).await?;
    let storage = Storage::new(&config.storage);
    let csrf_key = auth::CsrfKey::load(&db).await?;
    let state = AppState { db, storage, csrf_key };

    Ok(dioxus::server::router(app::App)
        .route("/tests/{test_id}/documents/{id}/pdf", get(documents::pdf))
        .route("/tests/{test_id}/documents/{id}/original", get(documents::original))
        .route("/tests/{test_id}/zip", get(zip::test_zip))
        .route("/documents/{id}/pdf", get(documents::pdf_of_first_test))
        .route("/documents/{id}/original", get(documents::original_of_first_test))
        .route("/healthz", get(health))
        .layer(middleware::from_fn(auth::same_origin_writes))
        .layer(Extension(state)))
}

/// 200 when the database answers. For container health checks.
async fn health(Extension(state): Extension<AppState>) -> Response {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => "ok".into_response(),
        Err(e) => {
            dioxus::logger::tracing::error!("health check: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response()
        }
    }
}

/// Log an unexpected error and give the caller a bare 500, so database or
/// storage details never reach the public.
pub fn internal_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus::logger::tracing::error!("{e:#}");
    ServerFnError::new("internal error")
}
