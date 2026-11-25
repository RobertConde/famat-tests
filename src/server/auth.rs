//! Admin login with the shared passcode.
//!
//! - **Passcode:** only an argon2id hash is stored, set by `famat set-passcode`
//!   ([`set_passcode`]). Setting it ends every session.
//! - **Sessions:** a random token in the `famat_session` cookie; Postgres
//!   keeps its SHA-256 and expiry, 30 days after login.
//! - **CSRF:** every form carries an HMAC token bound to the session (or to
//!   "anonymous" on the login page), and every non-GET request whose `Origin`
//!   is another site is refused ([`same_origin_writes`]).
//! - **Rate limit:** 5 failed logins per minute per client, where the client
//!   is `CF-Connecting-IP` (Cloudflare's view of the visitor), falling back to
//!   the socket address.
//!
//! Every Admin server function authorizes itself with [`require_admin`] (reads)
//! or [`require_admin_form`] (writes): page middleware does not cover `/api/*`
//! (ADR 0001).

use std::{
    convert::Infallible,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    body::Body,
    extract::{ConnectInfo, FromRequestParts, Request},
    http::{HeaderMap, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dioxus::prelude::ServerFnError;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use super::AppState;

pub const COOKIE: &str = "famat_session";
const SESSION_DAYS: i32 = 30;
/// A form older than this must be reloaded before it can be submitted.
const CSRF_TTL_SECS: u64 = 24 * 3600;
/// Failed logins allowed per client in [`RATE_WINDOW_SECS`].
const MAX_FAILURES: i64 = 5;
const RATE_WINDOW_SECS: f64 = 60.0;
pub const MIN_PASSCODE_CHARS: usize = 8;

/// The key that signs CSRF tokens, loaded once at startup. It lives in
/// Postgres, so every server process signs alike.
#[derive(Clone)]
pub struct CsrfKey(Vec<u8>);

impl CsrfKey {
    pub async fn load(db: &PgPool) -> anyhow::Result<Self> {
        let key: Vec<u8> = sqlx::query_scalar("SELECT csrf_key FROM admin_auth")
            .fetch_one(db)
            .await
            .context("loading the CSRF key")?;
        Ok(Self(key))
    }

    fn mac(&self, bind: &str, ts: u64) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes any key length");
        mac.update(format!("{bind}|{ts}").as_bytes());
        mac
    }
}

// ------------------------------------------------------------------ sessions

/// A valid Admin session, proven by the request's cookie.
#[derive(Clone, Debug)]
pub struct Session {
    token_sha256: Vec<u8>,
}

impl Session {
    /// What CSRF tokens issued to this session are bound to.
    fn csrf_binding(&self) -> String {
        format!("session:{}", hex::encode(&self.token_sha256))
    }
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// The session the request's cookie proves, if it is current.
pub async fn current_session(db: &PgPool, headers: &HeaderMap) -> Result<Option<Session>, sqlx::Error> {
    let Some(token) = session_token(headers) else {
        return Ok(None);
    };
    let token_sha256 = Sha256::digest(token.as_bytes()).to_vec();
    let found: Option<bool> =
        sqlx::query_scalar("SELECT true FROM admin_sessions WHERE token_sha256 = $1 AND expires_at > now()")
            .bind(&token_sha256)
            .fetch_optional(db)
            .await?;
    Ok(found.map(|_| Session { token_sha256 }))
}

/// For every Admin server function: the caller's session, or a 401.
pub async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<Session, ServerFnError> {
    match current_session(&state.db, headers).await {
        Ok(Some(session)) => Ok(session),
        Ok(None) => Err(http_error(StatusCode::UNAUTHORIZED, "Admin login required")),
        Err(e) => Err(super::internal_error(e)),
    }
}

/// For every Admin write (a form post): [`require_admin`], plus the form's
/// CSRF token, or a 403. The Origin check has already run on every write
/// ([`same_origin_writes`]).
pub async fn require_admin_form(state: &AppState, headers: &HeaderMap, csrf: &str) -> Result<Session, ServerFnError> {
    let session = require_admin(state, headers).await?;
    if !verify_csrf(&state.csrf_key, Some(&session), csrf) {
        return Err(http_error(StatusCode::FORBIDDEN, FORBIDDEN_MESSAGE));
    }
    Ok(session)
}

fn http_error(status: StatusCode, message: &str) -> ServerFnError {
    ServerFnError::ServerError {
        message: message.to_owned(),
        code: status.as_u16(),
        details: None,
    }
}

// ---------------------------------------------------------------------- CSRF

const FORBIDDEN_MESSAGE: &str =
    "This form has expired or came from another site. Go back, reload the page and try again.";

/// A token for a form: `<unix time>.<hex HMAC(key, binding|time)>`, bound to
/// the session (or to "anonymous" before login), so a token cannot be
/// replayed across sessions, and valid for [`CSRF_TTL_SECS`].
///
/// An anonymous token only guards the login form, and anyone can get one; the
/// Origin check and `SameSite=Strict` are what stop a cross-site login.
pub fn csrf_token(key: &CsrfKey, session: Option<&Session>) -> String {
    let ts = unix_now();
    let mac = key.mac(&binding(session), ts).finalize().into_bytes();
    format!("{ts}.{}", hex::encode(mac))
}

fn verify_csrf(key: &CsrfKey, session: Option<&Session>, token: &str) -> bool {
    let Some((ts, mac)) = token.split_once('.') else {
        return false;
    };
    let (Ok(ts), Ok(mac)) = (ts.parse::<u64>(), hex::decode(mac)) else {
        return false;
    };
    let now = unix_now();
    if ts > now + 60 || now.saturating_sub(ts) > CSRF_TTL_SECS {
        return false;
    }
    // Constant-time comparison.
    key.mac(&binding(session), ts).verify_slice(&mac).is_ok()
}

fn binding(session: Option<&Session>) -> String {
    session.map_or_else(|| "anonymous".to_owned(), Session::csrf_binding)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The CSRF token for the visitor's next form, bound to their session if
/// they have one.
pub async fn csrf_for(state: &AppState, headers: &HeaderMap) -> Result<String, sqlx::Error> {
    let session = current_session(&state.db, headers).await?;
    Ok(csrf_token(&state.csrf_key, session.as_ref()))
}

/// Middleware: refuse any non-GET request whose `Origin` is not this site.
///
/// Browsers send `Origin` on every POST, so a cross-site form or `fetch` is
/// caught here. Requests without one (curl, scripts) still need a valid CSRF
/// token.
pub async fn same_origin_writes(request: Request, next: Next) -> Response {
    if !request.method().is_safe() && !same_origin(&request) {
        return refuse(StatusCode::FORBIDDEN, FORBIDDEN_MESSAGE);
    }
    next.run(request).await
}

fn same_origin(request: &Request) -> bool {
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return true;
    };
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.strip_prefix("https://").or_else(|| o.strip_prefix("http://")));
    match (origin_host, host) {
        (Some(origin_host), Some(host)) => origin_host.eq_ignore_ascii_case(host) || is_dx_devserver(origin_host),
        // Includes `Origin: null` (sandboxed frames, some redirects).
        _ => false,
    }
}

/// Under `dx serve` (debug builds only), the browser talks to the dev
/// server, whose proxy rewrites `Host` to this server's own address, so a
/// same-site post arrives with the dev server's address as its `Origin`.
fn is_dx_devserver(origin_host: &str) -> bool {
    if !cfg!(debug_assertions) || !dioxus::cli_config::is_cli_enabled() {
        return false;
    }
    let Some(devserver) = dioxus::cli_config::devserver_raw_addr() else {
        return false;
    };
    let port = devserver.port();
    [
        devserver.to_string(),
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
    ]
    .iter()
    .any(|allowed| origin_host.eq_ignore_ascii_case(allowed))
}

// --------------------------------------------------------------- the client

/// Who is logging in, for the rate limit: `CF-Connecting-IP` when present
/// (the tunnel is the only way in, so the header comes from Cloudflare),
/// otherwise the socket's address.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let from_cloudflare = parts
            .headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<IpAddr>().ok());
        let from_socket = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        Ok(Self(from_cloudflare.or(from_socket)))
    }
}

impl ClientIp {
    /// The rate-limit bucket. An IPv6 visitor usually controls a whole /64,
    /// so that is one bucket.
    fn bucket(self) -> String {
        match self.0.map(|ip| ip.to_canonical()) {
            Some(IpAddr::V4(ip)) => ip.to_string(),
            Some(IpAddr::V6(ip)) => {
                let s = ip.segments();
                format!("{}/64", Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
            None => "unknown".to_owned(),
        }
    }
}

// ------------------------------------------------------------ login, logout

/// `POST /api/login`, a native form post. Answers 303 to `/` with the
/// session cookie, 303 back to the login page for a wrong passcode, 403 for
/// a bad CSRF token and 429 after too many failures.
pub async fn login(state: &AppState, headers: &HeaderMap, client: ClientIp, csrf: &str, passcode: String) -> Response {
    match try_login(state, headers, client, csrf, passcode).await {
        Ok(response) => response,
        Err(e) => {
            dioxus::logger::tracing::error!("login: {e:#}");
            refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

async fn try_login(
    state: &AppState,
    headers: &HeaderMap,
    client: ClientIp,
    csrf: &str,
    passcode: String,
) -> anyhow::Result<Response> {
    let session = current_session(&state.db, headers).await?;
    if !verify_csrf(&state.csrf_key, session.as_ref(), csrf) {
        return Ok(refuse(StatusCode::FORBIDDEN, FORBIDDEN_MESSAGE));
    }

    let bucket = client.bucket();
    let Some(attempt) = begin_attempt(&state.db, &bucket).await? else {
        dioxus::logger::tracing::warn!("login refused: too many failures from {bucket}");
        let mut response = refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many failed logins. Wait a minute and try again.",
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, header::HeaderValue::from_static("60"));
        return Ok(response);
    };

    let hash: Option<String> = sqlx::query_scalar("SELECT passcode_hash FROM admin_auth")
        .fetch_one(&state.db)
        .await?;
    let Some(hash) = hash else {
        dioxus::logger::tracing::warn!("login attempted, but no passcode is set: run `famat set-passcode`");
        return Ok(see_other("/login?error=passcode", None));
    };
    if !verify_passcode(hash.clone(), passcode).await {
        return Ok(see_other("/login?error=passcode", None));
    }

    // The passcode was right, so this attempt is not a failure.
    sqlx::query("DELETE FROM login_attempts WHERE id = $1")
        .bind(attempt)
        .execute(&state.db)
        .await?;

    let token = hex::encode(random_bytes::<32>());
    // Only if the passcode is still the one just checked: `FOR SHARE` waits
    // for a concurrent `set-passcode` to commit, so no session outlives it.
    let created = sqlx::query(
        "INSERT INTO admin_sessions (token_sha256, expires_at)
         SELECT $1, now() + make_interval(days => $3)
         FROM admin_auth WHERE passcode_hash = $2
         FOR SHARE",
    )
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(&hash)
    .bind(SESSION_DAYS)
    .execute(&state.db)
    .await?
    .rows_affected();
    if created == 0 {
        return Ok(see_other("/login?error=passcode", None));
    }
    sqlx::query("DELETE FROM admin_sessions WHERE expires_at <= now()")
        .execute(&state.db)
        .await?;

    let cookie = format!(
        "{COOKIE}={token}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
        i64::from(SESSION_DAYS) * 24 * 3600
    );
    Ok(see_other("/", Some(cookie)))
}

/// Count this attempt against the client, unless it already has
/// [`MAX_FAILURES`] in the window. Returns the attempt's ID, which the
/// caller deletes if the passcode turns out right. Recording it before
/// checking the passcode means parallel guesses cannot slip past the limit.
async fn begin_attempt(db: &PgPool, bucket: &str) -> Result<Option<i64>, sqlx::Error> {
    let mut tx = db.begin().await?;
    // Serialize attempts per client.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(bucket)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM login_attempts WHERE attempted_at <= now() - make_interval(secs => $1)")
        .bind(RATE_WINDOW_SECS)
        .execute(&mut *tx)
        .await?;
    let failures: i64 = sqlx::query_scalar("SELECT count(*) FROM login_attempts WHERE client = $1")
        .bind(bucket)
        .fetch_one(&mut *tx)
        .await?;
    if failures >= MAX_FAILURES {
        tx.commit().await?;
        return Ok(None);
    }
    let id = sqlx::query_scalar("INSERT INTO login_attempts (client) VALUES ($1) RETURNING id")
        .bind(bucket)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// argon2 is deliberately slow, so it runs off the async workers.
async fn verify_passcode(hash: String, passcode: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|hash| Argon2::default().verify_password(passcode.as_bytes(), &hash).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// `POST /api/logout`, a native form post and an Admin write: ends the
/// session and answers 303 to `/` with the cookie cleared.
pub async fn logout(state: &AppState, headers: &HeaderMap, csrf: &str) -> Result<Response, ServerFnError> {
    let session = require_admin_form(state, headers, csrf).await?;
    sqlx::query("DELETE FROM admin_sessions WHERE token_sha256 = $1")
        .bind(&session.token_sha256)
        .execute(&state.db)
        .await
        .map_err(super::internal_error)?;
    let clear = format!("{COOKIE}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0");
    Ok(see_other("/", Some(clear)))
}

// ------------------------------------------------------------ set-passcode

/// Store a new passcode's argon2id hash and end every session, in one
/// transaction. Returns how many sessions ended.
pub async fn set_passcode(db: &PgPool, passcode: &str) -> anyhow::Result<u64> {
    let passcode = passcode.to_owned();
    let hash = tokio::task::spawn_blocking(move || {
        let salt = SaltString::encode_b64(&random_bytes::<16>()).map_err(|e| anyhow::anyhow!("salt: {e}"))?;
        // The crate's default is argon2id with OWASP's parameters.
        Argon2::default()
            .hash_password(passcode.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|e| anyhow::anyhow!("hashing the passcode: {e}"))
    })
    .await??;

    let mut tx = db.begin().await?;
    sqlx::query("UPDATE admin_auth SET passcode_hash = $1, passcode_set_at = now()")
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
    let ended = sqlx::query("DELETE FROM admin_sessions")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(ended)
}

// ----------------------------------------------------------------- helpers

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    bytes
}

/// A 303, which a browser follows with a GET (ADR 0001: Dioxus would turn a
/// 2xx form response into a redirect to the Referer).
fn see_other(location: &str, set_cookie: Option<String>) -> Response {
    let mut response = Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, location);
    if let Some(cookie) = set_cookie {
        response = response.header(header::SET_COOKIE, cookie);
    }
    response.body(Body::empty()).unwrap_or_else(server_error)
}

/// A refusal a person reads in the browser after a native form post.
fn refuse(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        message.to_owned(),
    )
        .into_response()
}

fn server_error(e: impl std::fmt::Display) -> Response {
    dioxus::logger::tracing::error!("{e:#}");
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}
