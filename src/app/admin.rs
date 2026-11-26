//! Admin login and logout, and the Admin menu shown on the search page.
//!
//! Login and logout are native form posts answered with explicit 303s, so
//! they work without JavaScript (ADR 0001). The logic is server-only, in
//! `crate::server::auth`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{Form, HeaderMap, Loading, response::Response};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Posted natively by the browser; only the server reads it.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct LoginForm {
    pub csrf: String,
    pub passcode: String,
}

/// A form that carries nothing but its CSRF token.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Serialize, Deserialize)]
pub struct CsrfForm {
    pub csrf: String,
}

/// What a logged-in Admin's pages need from the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdminSession {
    /// The CSRF token for this session's forms.
    pub csrf: String,
}

/// A CSRF token for the visitor's next form. Public: the login form needs one.
#[get("/api/csrf",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn csrf_token() -> Result<String, ServerFnError> {
    crate::server::auth::csrf_for(&state, &headers)
        .await
        .map_err(crate::server::internal_error)
}

/// The caller's Admin session. An Admin endpoint: anonymous callers get 401.
#[get("/api/admin/session",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn admin_session() -> Result<AdminSession, ServerFnError> {
    let session = crate::server::auth::require_admin(&state, &headers).await?;
    Ok(AdminSession {
        csrf: crate::server::auth::csrf_token(&state.csrf_key, Some(&session)),
    })
}

#[post("/api/login",
    headers: HeaderMap,
    client: crate::server::auth::ClientIp,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn login(form: Form<LoginForm>) -> Result<Response, ServerFnError> {
    let Form(LoginForm { csrf, passcode }) = form;
    Ok(crate::server::auth::login(&state, &headers, client, &csrf, passcode).await)
}

#[post("/api/logout",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
pub async fn logout(form: Form<CsrfForm>) -> Result<Response, ServerFnError> {
    crate::server::auth::logout(&state, &headers, &form.0.csrf).await
}

const CSS: &str = "\
#admin-menu ul{list-style:none;display:flex;flex-wrap:wrap;gap:.25rem 1rem;margin:0 0 1rem;padding:.5rem .75rem;background:#f3f3f3;border-radius:4px}\
.error{color:#b00020}\
form.admin label{margin-right:.5rem}";

/// `/login`, with `?error=passcode` after a wrong passcode.
#[component]
pub fn Login(error: ReadSignal<String>) -> Element {
    let csrf = use_loader(csrf_token)?;
    rsx! {
        document::Style { {CSS} }
        h2 { "Admin login" }
        if error() == "passcode" {
            p { id: "login-error", class: "error", "That passcode is not right." }
        }
        form { id: "login", class: "admin", method: "post", action: "/api/login",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            label {
                "Passcode "
                input {
                    r#type: "password",
                    name: "passcode",
                    required: true,
                    autocomplete: "current-password",
                    autofocus: true,
                }
            }
            button { r#type: "submit", "Log in" }
        }
    }
}

/// `/logout`: a button, since logging out is a form post.
#[component]
pub fn Logout() -> Element {
    let session = match use_loader(admin_session) {
        Ok(session) => session,
        Err(Loading::Pending(pending)) => return Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => {
            return rsx! {
                h2 { "Log out" }
                p { id: "not-logged-in", "You are not logged in. " a { href: "/login", "Log in" } }
            };
        }
    };
    let csrf = session.read().csrf.clone();
    rsx! {
        h2 { "Log out" }
        form { id: "logout", class: "admin", method: "post", action: "/api/logout",
            input { r#type: "hidden", name: "csrf", value: "{csrf}" }
            button { r#type: "submit", "Log out" }
        }
    }
}

/// The Admin menu, as `(label, link)`. Links to pages that later tickets
/// build are placeholders until then.
const MENU: [(&str, &str); 7] = [
    ("Add test", "/admin/tests/new"),
    ("Facet values", "/admin/facets"),
    ("Document kinds", "/admin/document-kinds"),
    ("Needs review", "/?needs_review=on"),
    ("Trash", "/admin/trash"),
    ("Change log", "/admin/change-log"),
    ("Log out", "/logout"),
];

/// The Admin menu, shown only to a logged-in Admin. Renders nothing for
/// everyone else.
#[component]
pub fn AdminMenu() -> Element {
    match use_loader(admin_session) {
        Ok(_) => rsx! {
            document::Style { {CSS} }
            nav { id: "admin-menu", aria_label: "Admin",
                ul {
                    for (label, href) in MENU {
                        li { key: "{href}", a { href, "{label}" } }
                    }
                }
            }
        },
        Err(Loading::Pending(pending)) => Err(Loading::Pending(pending).into()),
        Err(Loading::Failed(_)) => rsx! {},
    }
}
