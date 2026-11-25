//! Admin passcode auth: argon2 hash from env, in-memory sessions, HMAC CSRF.
//!
//! Throwaway: sessions live in a HashMap (v1 would put them in Postgres) and
//! there is no login rate limiting.

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use dioxus::prelude::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::LoginForm;

const COOKIE: &str = "famat_session";
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
const CSRF_TTL_SECS: u64 = 3600;

static PASSCODE_HASH: LazyLock<String> = LazyLock::new(|| {
    std::env::var("ADMIN_PASSCODE_HASH")
        .expect("set ADMIN_PASSCODE_HASH to an argon2 PHC string (cargo run --example hash_passcode --features server)")
});

static SECRET: LazyLock<Vec<u8>> = LazyLock::new(|| match std::env::var("SESSION_SECRET") {
    Ok(s) => s.into_bytes(),
    Err(_) => random_bytes(32),
});

static SESSIONS: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

pub fn init() {
    let _ = PasswordHash::new(&PASSCODE_HASH).expect("ADMIN_PASSCODE_HASH is not a valid PHC string");
    LazyLock::force(&SECRET);
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("getrandom");
    b
}

fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

/// The valid session id in the request, if any.
fn valid_session(headers: &HeaderMap) -> Option<String> {
    let id = session_id(headers)?;
    let sessions = SESSIONS.lock().unwrap();
    match sessions.get(&id) {
        Some(exp) if *exp > Instant::now() => Some(id),
        _ => None,
    }
}

pub fn is_admin(headers: &HeaderMap) -> bool {
    valid_session(headers).is_some()
}

pub fn require_admin(headers: &HeaderMap) -> Result<(), HttpError> {
    is_admin(headers).or_unauthorized("Admin login required")
}

// ------------------------------------------------------------------ CSRF

/// Tokens are `<unix ts>.<hex hmac(secret, bind|ts)>`, where `bind` is the
/// session id (or "anon" before login), so a token can't be replayed across
/// sessions and expires after an hour.
fn sign(bind: &str, ts: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(&SECRET).unwrap();
    mac.update(format!("{bind}|{ts}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn bind_for(headers: &HeaderMap) -> String {
    valid_session(headers).unwrap_or_else(|| "anon".into())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub fn csrf_for(headers: &HeaderMap) -> String {
    let ts = now();
    format!("{ts}.{}", sign(&bind_for(headers), ts))
}

/// Check the token and, when the browser sends one, that Origin matches Host.
pub fn verify_csrf(headers: &HeaderMap, token: &str) -> bool {
    if let (Some(origin), Some(host)) = (headers.get(header::ORIGIN), headers.get(header::HOST)) {
        let origin = origin.to_str().unwrap_or("");
        let host = host.to_str().unwrap_or("");
        let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
        if origin_host != host {
            return false;
        }
    }
    let Some((ts, mac)) = token.split_once('.') else { return false };
    let Ok(ts) = ts.parse::<u64>() else { return false };
    if now().saturating_sub(ts) > CSRF_TTL_SECS {
        return false;
    }
    let mut m = Hmac::<Sha256>::new_from_slice(&SECRET).unwrap();
    m.update(format!("{}|{ts}", bind_for(headers)).as_bytes());
    let Ok(mac) = hex::decode(mac) else { return false };
    m.verify_slice(&mac).is_ok() // constant-time compare
}

// ------------------------------------------------------------------ handlers

pub fn see_other(location: &str, set_cookie: Option<String>) -> Response {
    let mut b = Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, location);
    if let Some(c) = set_cookie {
        b = b.header(header::SET_COOKIE, c);
    }
    b.body(Body::empty()).unwrap()
}

pub async fn login(headers: &HeaderMap, form: LoginForm) -> Response {
    if !verify_csrf(headers, &form.csrf) {
        return see_other("/login?error=csrf", None);
    }
    // argon2 is deliberately slow; keep it off the async workers.
    let ok = tokio::task::spawn_blocking(move || {
        let hash = PasswordHash::new(&PASSCODE_HASH).unwrap();
        Argon2::default().verify_password(form.passcode.as_bytes(), &hash).is_ok()
    })
    .await
    .unwrap_or(false);
    if !ok {
        return see_other("/login?error=passcode", None);
    }

    let id = hex::encode(random_bytes(32));
    SESSIONS.lock().unwrap().insert(id.clone(), Instant::now() + SESSION_TTL);
    let cookie = format!(
        "{COOKIE}={id}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    see_other("/admin", Some(cookie))
}

pub fn logout(headers: &HeaderMap, csrf: &str) -> Response {
    if !verify_csrf(headers, csrf) {
        return see_other("/admin?uploaded=", None);
    }
    if let Some(id) = session_id(headers) {
        SESSIONS.lock().unwrap().remove(&id);
    }
    let clear = format!("{COOKIE}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0");
    see_other("/", Some(clear))
}
