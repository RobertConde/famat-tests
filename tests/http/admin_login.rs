//! Admin login with the shared passcode, set by `famat set-passcode`.

use crate::support::{
    TestApp,
    admin::{PASSCODE, session_cookie, set_cookies},
    html::{Page, links_in},
};

const MENU: [&str; 7] = [
    "Add test",
    "Facet values",
    "Document kinds",
    "Needs review",
    "Trash",
    "Change log",
    "Log out",
];

fn menu(page: &Page) -> Vec<String> {
    page.select("#admin-menu")
        .into_iter()
        .flat_map(links_in)
        .map(|(text, _)| text)
        .collect()
}

#[tokio::test]
async fn logging_in_sets_a_30_day_secure_cookie_and_shows_the_admin_menu() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);

    let anonymous = app.get_page("/").await;
    assert!(menu(&anonymous).is_empty(), "no Admin menu before logging in");

    let response = app.submit_login(PASSCODE, &[]).await;

    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/");
    let cookies = set_cookies(&response);
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    let (token, flags) = cookies[0]
        .strip_prefix("famat_session=")
        .and_then(|rest| rest.split_once("; "))
        .expect("a famat_session cookie");
    assert!(token.len() >= 32, "a long random token: {token:?}");
    assert_eq!(flags, "HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=2592000");

    let cookie = session_cookie(&response).unwrap();
    let page = app.get_page_as("/", Some(&cookie)).await;
    assert_eq!(menu(&page), MENU);
    let log_out = links_in(page.one("#admin-menu"))
        .into_iter()
        .find(|(text, _)| text == "Log out")
        .unwrap();
    assert_eq!(log_out.1, "/logout");
}

#[tokio::test]
async fn a_wrong_passcode_is_rejected() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);

    let response = app.submit_login("not the passcode", &[]).await;

    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/login?error=passcode");
    assert!(set_cookies(&response).is_empty());
    let page = app.get_page("/login?error=passcode").await;
    assert_eq!(page.text("#login-error"), "That passcode is not right.");
}

#[tokio::test]
async fn nobody_can_log_in_before_a_passcode_is_set() {
    let app = TestApp::start().await;

    let response = app.submit_login("", &[]).await;

    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/login?error=passcode");
    assert!(set_cookies(&response).is_empty());
}

#[tokio::test]
async fn a_forged_csrf_token_is_rejected() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);
    let real = app.csrf_on("/login", None).await;
    let (ts, mac) = real.split_once('.').unwrap();
    let flipped = if mac.starts_with('0') { "1" } else { "0" };
    let forged_mac = format!("{ts}.{flipped}{}", &mac[1..]);
    let forged_time = format!("{}.{mac}", ts.parse::<u64>().unwrap() + 1);

    for forged in [forged_mac.as_str(), forged_time.as_str(), "", "nonsense"] {
        let response = app
            .post_form("/api/login", &[("csrf", forged), ("passcode", PASSCODE)], &[])
            .await;
        assert_eq!(response.status(), 403, "token {forged:?}");
        assert!(set_cookies(&response).is_empty(), "token {forged:?}");
    }

    // Leaving the token out altogether is refused too.
    let response = app.post_form("/api/login", &[("passcode", PASSCODE)], &[]).await;
    assert!(response.status().is_client_error(), "{}", response.status());
    assert!(set_cookies(&response).is_empty());
}

#[tokio::test]
async fn a_cross_site_origin_is_rejected() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);

    for origin in ["https://evil.example", "null", "http://127.0.0.1"] {
        // A valid token and the right passcode, but posted from elsewhere.
        let response = app.submit_login(PASSCODE, &[("origin", origin)]).await;
        assert_eq!(response.status(), 403, "Origin {origin}");
        assert!(set_cookies(&response).is_empty(), "Origin {origin}");
    }

    // The Origin check covers every write, not only the login form.
    let cookie = app.log_in().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let response = app
        .post_form(
            "/api/logout",
            &[("csrf", &csrf)],
            &[("cookie", &cookie), ("origin", "https://evil.example")],
        )
        .await;
    assert_eq!(response.status(), 403);
    assert_eq!(app.get_as("/api/admin/session", Some(&cookie)).await.status(), 200);
}

#[tokio::test]
async fn the_sixth_failed_login_within_a_minute_from_one_ip_is_refused() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);
    let visitor = [("cf-connecting-ip", "203.0.113.7")];

    for attempt in 1..=5 {
        let response = app.submit_login("wrong guess", &visitor).await;
        assert_eq!(response.status(), 303, "attempt {attempt}");
        assert_eq!(response.headers()["location"], "/login?error=passcode");
    }

    // Even the right passcode is refused now.
    let response = app.submit_login(PASSCODE, &visitor).await;
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "60");
    assert!(set_cookies(&response).is_empty());

    // Other visitors are not affected.
    let response = app
        .submit_login(PASSCODE, &[("cf-connecting-ip", "198.51.100.20")])
        .await;
    assert_eq!(response.status(), 303);
    assert!(session_cookie(&response).is_some());
}

#[tokio::test]
async fn an_ipv6_visitor_is_limited_across_their_slash_64() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);

    for host in 1..=5 {
        let ip = format!("2001:db8:1:2::{host}");
        let response = app.submit_login("wrong guess", &[("cf-connecting-ip", &ip)]).await;
        assert_eq!(response.status(), 303, "{ip}");
    }

    let response = app
        .submit_login(PASSCODE, &[("cf-connecting-ip", "2001:db8:1:2:ffff::9")])
        .await;
    assert_eq!(response.status(), 429);
    let response = app
        .submit_login(PASSCODE, &[("cf-connecting-ip", "2001:db8:1:3::1")])
        .await;
    assert_eq!(response.status(), 303);
}

#[tokio::test]
async fn without_cf_connecting_ip_the_limit_applies_to_the_socket_address() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);

    for attempt in 1..=5 {
        let response = app.submit_login("wrong guess", &[]).await;
        assert_eq!(response.status(), 303, "attempt {attempt}");
    }

    let response = app.submit_login(PASSCODE, &[]).await;
    assert_eq!(response.status(), 429);
    // The same machine through Cloudflare is a different visitor.
    let response = app
        .submit_login(PASSCODE, &[("cf-connecting-ip", "198.51.100.20")])
        .await;
    assert_eq!(response.status(), 303);
}

#[tokio::test]
async fn successful_logins_do_not_count_towards_the_limit() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);
    let visitor = [("cf-connecting-ip", "203.0.113.7")];

    for _ in 0..4 {
        assert_eq!(app.submit_login("wrong guess", &visitor).await.status(), 303);
    }
    for _ in 0..3 {
        let response = app.submit_login(PASSCODE, &visitor).await;
        assert!(session_cookie(&response).is_some());
    }
    // The fifth failure is still allowed; only the sixth is refused.
    let response = app.submit_login("wrong guess", &visitor).await;
    assert_eq!(response.headers()["location"], "/login?error=passcode");
    assert_eq!(app.submit_login(PASSCODE, &visitor).await.status(), 429);
}

#[tokio::test]
async fn after_set_passcode_existing_sessions_no_longer_work() {
    let app = TestApp::start().await;
    let first = app.admin().await;
    let second = app.log_in().await;
    for cookie in [&first, &second] {
        assert_eq!(app.get_as("/api/admin/session", Some(cookie)).await.status(), 200);
    }

    app.set_passcode("a brand new passcode");

    for cookie in [&first, &second] {
        assert_eq!(app.get_as("/api/admin/session", Some(cookie)).await.status(), 401);
        let page = app.get_page_as("/", Some(cookie)).await;
        assert!(menu(&page).is_empty(), "no Admin menu for an ended session");
    }
    let old = app.submit_login(PASSCODE, &[]).await;
    assert_eq!(old.headers()["location"], "/login?error=passcode");
    let new = app.submit_login("a brand new passcode", &[]).await;
    let cookie = session_cookie(&new).expect("the new passcode logs in");
    assert_eq!(menu(&app.get_page_as("/", Some(&cookie)).await), MENU);
}

#[tokio::test]
async fn anonymous_calls_to_admin_endpoints_get_401_or_403_not_500() {
    let app = TestApp::start().await;
    app.set_passcode(PASSCODE);
    let anonymous_csrf = app.csrf_on("/login", None).await;

    // No session, or one the server never issued.
    for cookie in [None, Some("famat_session=0123456789abcdef"), Some("famat_session=")] {
        let response = app.get_as("/api/admin/session", cookie).await;
        assert_eq!(response.status(), 401, "cookie {cookie:?}");

        let mut headers = vec![];
        if let Some(cookie) = cookie {
            headers.push(("cookie", cookie));
        }
        let response = app
            .post_form("/api/logout", &[("csrf", &anonymous_csrf)], &headers)
            .await;
        assert_eq!(response.status(), 401, "cookie {cookie:?}");
    }

    // A logged-in Admin's write still needs a token bound to their session.
    let cookie = app.log_in().await;
    for csrf in [anonymous_csrf.as_str(), "", "1.00"] {
        let response = app
            .post_form("/api/logout", &[("csrf", csrf)], &[("cookie", &cookie)])
            .await;
        assert_eq!(response.status(), 403, "token {csrf:?}");
    }
    assert_eq!(app.get_as("/api/admin/session", Some(&cookie)).await.status(), 200);
}

#[tokio::test]
async fn logging_out_ends_the_session() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;

    let page = app.get_page_as("/logout", Some(&cookie)).await;
    let form = page.one("form#logout");
    assert_eq!(form.value().attr("method"), Some("post"));
    assert_eq!(form.value().attr("action"), Some("/api/logout"));
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let response = app
        .post_form("/api/logout", &[("csrf", &csrf)], &[("cookie", &cookie)])
        .await;

    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/");
    assert_eq!(
        set_cookies(&response),
        vec!["famat_session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0"]
    );
    assert_eq!(app.get_as("/api/admin/session", Some(&cookie)).await.status(), 401);
    assert!(menu(&app.get_page_as("/", Some(&cookie)).await).is_empty());
    let page = app.get_page_as("/logout", Some(&cookie)).await;
    assert!(page.select("form#logout").is_empty());
    assert_eq!(page.text("#not-logged-in"), "You are not logged in. Log in");
}

#[tokio::test]
async fn the_login_page_works_without_javascript() {
    let app = TestApp::start().await;

    let page = app.get_page("/login").await;

    let form = page.one("form#login");
    assert_eq!(form.value().attr("method"), Some("post"));
    assert_eq!(form.value().attr("action"), Some("/api/login"));
    assert_eq!(page.select("form#login input[type=password][name=passcode]").len(), 1);
    assert_eq!(page.select("form#login input[type=hidden][name=csrf]").len(), 1);
    assert!(page.select("#login-error").is_empty());
}
