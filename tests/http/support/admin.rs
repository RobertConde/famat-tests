//! Acting as an Admin: setting the passcode with the real `famat
//! set-passcode`, and logging in through the login form like a browser with
//! JavaScript off.

use std::{
    io::Write,
    process::{Command, Stdio},
};

use reqwest::header::{HeaderName, HeaderValue};

use super::{TestApp, html::Page};

/// The passcode tests set unless they need another.
pub const PASSCODE: &str = "correct horse battery staple";

impl TestApp {
    /// Run `famat set-passcode` against this test's database, with the
    /// passcode piped to stdin as a script would.
    pub fn set_passcode(&self, passcode: &str) {
        let mut child = Command::new(&self.binary)
            .arg("set-passcode")
            .env_clear()
            .env("DATABASE_URL", &self.database_url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run famat set-passcode");
        writeln!(child.stdin.take().unwrap(), "{passcode}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "famat set-passcode failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// GET a path, sending a session cookie if given.
    pub async fn get_as(&self, path: &str, cookie: Option<&str>) -> reqwest::Response {
        let mut request = self.http.get(self.url(path));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.send().await.expect("GET request")
    }

    /// GET a page with a session cookie (if given) and parse it.
    pub async fn get_page_as(&self, path: &str, cookie: Option<&str>) -> Page {
        let response = self.get_as(path, cookie).await;
        assert_eq!(response.status(), 200, "GET {path}");
        Page::parse(&response.text().await.unwrap())
    }

    /// POST a urlencoded form as a browser on this site would: with this
    /// site's `Origin`, unless `headers` sets another one.
    pub async fn post_form(&self, path: &str, form: &[(&str, &str)], headers: &[(&str, &str)]) -> reqwest::Response {
        let mut request = self.http.post(self.url(path)).form(form);
        if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("origin")) {
            request = request.header("origin", &self.base_url);
        }
        for (name, value) in headers {
            request = request.header(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        request.send().await.expect("POST request")
    }

    /// The CSRF token in the form on a page.
    pub async fn csrf_on(&self, path: &str, cookie: Option<&str>) -> String {
        let page = self.get_page_as(path, cookie).await;
        let input = page.one("form input[name=csrf]");
        input.value().attr("value").expect("a CSRF value").to_owned()
    }

    /// Submit the login form with `passcode`, as a visitor with no session.
    pub async fn submit_login(&self, passcode: &str, headers: &[(&str, &str)]) -> reqwest::Response {
        let csrf = self.csrf_on("/login", None).await;
        self.post_form("/api/login", &[("csrf", &csrf), ("passcode", passcode)], headers)
            .await
    }

    /// Log in with [`PASSCODE`] (already set) and return the `Cookie` header
    /// value that carries the session.
    pub async fn log_in(&self) -> String {
        let response = self.submit_login(PASSCODE, &[]).await;
        assert_eq!(response.status(), 303, "logging in");
        session_cookie(&response).expect("a session cookie")
    }

    /// Set [`PASSCODE`] and log in.
    pub async fn admin(&self) -> String {
        self.set_passcode(PASSCODE);
        self.log_in().await
    }
}

/// The `Set-Cookie` headers of a response.
pub fn set_cookies(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

/// `famat_session=<token>` from a response that set a non-empty session
/// cookie, ready to send back as a `Cookie` header.
pub fn session_cookie(response: &reqwest::Response) -> Option<String> {
    set_cookies(response).into_iter().find_map(|cookie| {
        let pair = cookie.split(';').next()?.trim().to_owned();
        let (name, value) = pair.split_once('=')?;
        (name == "famat_session" && !value.is_empty()).then_some(pair)
    })
}
