//! Server-only code: compiled with `--features server`, never into WASM.

pub mod auth;
pub mod data;
pub mod s3;
pub mod zip;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use dioxus::prelude::*;

/// The full axum router: Dioxus SSR + server functions, plus plain routes.
pub async fn router(app: fn() -> Element) -> anyhow::Result<Router> {
    // Fail fast on missing config instead of on first request.
    auth::init();
    s3::init();

    Ok(dioxus::server::router(app)
        // Plain axum route, not a server function (ADR 0001).
        .route("/download/zip", get(zip::download_zip))
        .layer(middleware::from_fn(admin_gate))
        // axum's Multipart defaults to a 2 MB body limit.
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024)))
}

/// Redirect anonymous full-page loads of `/admin*` to `/login` before Dioxus
/// renders anything.
async fn admin_gate(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if (path == "/admin" || path.starts_with("/admin/")) && !auth::is_admin(req.headers()) {
        return Redirect::to("/login").into_response();
    }
    next.run(req).await
}
