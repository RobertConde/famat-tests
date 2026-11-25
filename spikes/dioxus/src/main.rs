//! Throwaway spike for issue #14: Dioxus 0.7 fullstack on axum.
//!
//! - `/`       public search (SSR, works without JS, hydrates)
//! - `/login`  Admin passcode login (argon2, session cookie, CSRF)
//! - `/admin`  Admin-only page with a file upload into Garage
//! - `/download/zip?keys=a,b` plain axum route streaming a zip from Garage

#[allow(unused_imports)] // HeaderMap is only used by server-side extractors
use dioxus::fullstack::{Form, HeaderMap, Loading, MultipartFormData};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
mod server;

fn main() {
    #[cfg(not(feature = "server"))]
    dioxus::launch(App);

    #[cfg(feature = "server")]
    dioxus::serve(|| async move { server::router(App).await });
}

#[derive(Routable, Clone, Debug, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Shell)]
    #[route("/?:year&:level")]
    Search { year: String, level: String },
    #[route("/login?:error")]
    Login { error: String },
    #[route("/admin?:uploaded")]
    Admin { uploaded: String },
}

#[component]
fn App() -> Element {
    rsx! {
        document::Title { "FAMAT Test Bank (spike)" }
        document::Style { {CSS} }
        Router::<Route> {}
    }
}

const CSS: &str = "body{font:16px system-ui;max-width:48rem;margin:2rem auto;padding:0 1rem}\
nav a{margin-right:1rem} li{margin:.25rem 0} .muted{color:#666} .err{color:#b00}";

#[component]
fn Shell() -> Element {
    rsx! {
        nav {
            Link { to: Route::Search { year: String::new(), level: String::new() }, "Search" }
            Link { to: Route::Admin { uploaded: String::new() }, "Admin" }
        }
        HydrationMarker {}
        Outlet::<Route> {}
    }
}

/// Shows "hydrated" once the WASM client takes over. Server HTML never has it.
#[component]
fn HydrationMarker() -> Element {
    let mut hydrated = use_signal(|| false);
    use_effect(move || hydrated.set(true));
    rsx! {
        p { class: "muted", id: "js-state",
            if hydrated() { "JS: hydrated" } else { "JS: server-rendered HTML" }
        }
    }
}

// ---------------------------------------------------------------- search

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestRow {
    pub id: u32,
    pub year: u16,
    pub competition: String,
    pub level: String,
    pub test_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub years: Vec<u16>,
    pub levels: Vec<String>,
    pub tests: Vec<TestRow>,
}

/// The Facet filter runs on the server; in v1 this would be a sqlx query.
#[get("/api/search?year&level")]
async fn search(year: String, level: String) -> Result<SearchResults> {
    Ok(server::data::search(&year, &level))
}

#[component]
fn Search(year: ReadSignal<String>, level: ReadSignal<String>) -> Element {
    let results = use_loader(move || search(year(), level()))?;
    let results = results.read();
    let nav = navigator();

    rsx! {
        h1 { "Search Tests" }
        // A plain GET form: without JS the button reloads the page with the
        // query string; with JS, onchange navigates client-side instead.
        form { method: "get", action: "/",
            label { "Year "
                select {
                    name: "year",
                    onchange: move |e| { nav.push(Route::Search { year: e.value(), level: level() }); },
                    option { value: "", "Any" }
                    for y in results.years.iter() {
                        option { value: "{y}", selected: year() == y.to_string(), "{y}" }
                    }
                }
            }
            " "
            label { "Division/Level "
                select {
                    name: "level",
                    onchange: move |e| { nav.push(Route::Search { year: year(), level: e.value() }); },
                    option { value: "", "Any" }
                    for l in results.levels.iter() {
                        option { value: "{l}", selected: level() == *l, "{l}" }
                    }
                }
            }
            " "
            button { r#type: "submit", "Filter" }
        }
        p { id: "count", "{results.tests.len()} Tests" }
        ul { id: "results",
            for t in results.tests.iter() {
                li { key: "{t.id}",
                    "{t.year} {t.competition}: {t.level} {t.test_type} "
                    a { href: "/download/zip?keys=sample-a.txt,sample-b.txt", "zip" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- login

#[derive(Deserialize, Serialize)]
pub struct LoginForm {
    pub csrf: String,
    pub passcode: String,
}

/// A CSRF token for the current visitor (bound to their session if any).
#[get("/api/csrf", headers: HeaderMap)]
async fn csrf_token() -> Result<String> {
    Ok(server::auth::csrf_for(&headers))
}

/// Native form POST target. Returns a 303 so it works with JS disabled.
#[post("/api/login", headers: HeaderMap)]
async fn login(form: Form<LoginForm>) -> Result<dioxus::fullstack::response::Response> {
    Ok(server::auth::login(&headers, form.0).await)
}

#[derive(Deserialize, Serialize)]
pub struct CsrfForm {
    pub csrf: String,
}

#[post("/api/logout", headers: HeaderMap)]
async fn logout(form: Form<CsrfForm>) -> Result<dioxus::fullstack::response::Response> {
    Ok(server::auth::logout(&headers, &form.0.csrf))
}

#[component]
fn Login(error: ReadSignal<String>) -> Element {
    let csrf = use_loader(csrf_token)?;
    rsx! {
        h1 { "Admin login" }
        if !error().is_empty() {
            p { class: "err", id: "login-error", "Login failed: {error}" }
        }
        form { method: "post", action: "/api/login",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            label { "Passcode " input { r#type: "password", name: "passcode", required: true } }
            " "
            button { r#type: "submit", "Log in" }
        }
    }
}

// ---------------------------------------------------------------- admin

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdminInfo {
    pub csrf: String,
    pub objects: Vec<(String, i64)>,
}

// Note: with the default `Result<T>` (CapturedError) the HTTP status is
// always 500 even for `or_unauthorized`; `ServerFnError` keeps the 401.
#[get("/api/admin/info", headers: HeaderMap)]
async fn admin_info() -> Result<AdminInfo, ServerFnError> {
    server::auth::require_admin(&headers)?;
    Ok(AdminInfo {
        csrf: server::auth::csrf_for(&headers),
        objects: server::s3::list().await?,
    })
}

/// Multipart upload. The hidden `csrf` field must come before the file.
#[post("/api/admin/upload", headers: HeaderMap)]
async fn upload(form: MultipartFormData) -> Result<dioxus::fullstack::response::Response> {
    Ok(server::s3::upload_form(&headers, form).await)
}

#[component]
fn Admin(uploaded: ReadSignal<String>) -> Element {
    // The axum middleware already redirects anonymous full-page loads. This
    // covers client-side navigation, where no page request reaches the server.
    let info = match use_loader(admin_info) {
        Ok(info) => info,
        Err(Loading::Pending(p)) => return Err(Loading::Pending(p).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h1 { "Admin" }
                p { "You must " a { href: "/login", "log in" } " first." }
            };
        }
    };
    let info = info.read();

    rsx! {
        h1 { "Admin" }
        if !uploaded().is_empty() {
            p { id: "uploaded", "Uploaded {uploaded}" }
        }
        h2 { "Upload a Document" }
        form { method: "post", action: "/api/admin/upload", enctype: "multipart/form-data",
            input { r#type: "hidden", name: "csrf", value: "{info.csrf}" }
            input { r#type: "file", name: "file", required: true }
            " "
            button { r#type: "submit", "Upload" }
        }
        h2 { "Objects in Garage" }
        ul { id: "objects",
            for (key, size) in info.objects.iter() {
                li { key: "{key}", "{key} ({size} bytes)" }
            }
        }
        form { method: "post", action: "/api/logout",
            input { r#type: "hidden", name: "csrf", value: "{info.csrf}" }
            button { r#type: "submit", "Log out" }
        }
    }
}
