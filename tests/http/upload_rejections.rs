//! Refused uploads: whatever the reason, and however much of the body is
//! still to come when the server decides, the client gets the whole refusal
//! rather than a dropped connection (issue #47).
//!
//! Each refusal is sent several times in a row over the client's pooled
//! connections, with megabytes of body behind the point where the server
//! refuses, and must come back complete every time.

use reqwest::multipart::{Form, Part};

use crate::support::{
    TestApp,
    add_test::{AddTestForm, generated},
    fixtures::{NewDocument, NewTest, sample_pdf},
    html::Page,
};

const MIB: u64 = 1024 * 1024;
/// What follows the point of refusal.
const TRAILING: u64 = 8 * MIB;
/// How often each refusal is sent.
const TIMES: usize = 3;
const EVIL: &str = "https://evil.example";

/// The status and the whole body of a response, which must arrive intact.
async fn complete(response: reqwest::Result<reqwest::Response>, what: &str) -> (u16, String) {
    let response = response.unwrap_or_else(|e| panic!("{what}: no response: {e:?}"));
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .unwrap_or_else(|e| panic!("{what}: the response was cut off: {e:?}"));
    (status, body)
}

fn refusal(body: &str) -> String {
    Page::parse(body).text("#refusal")
}

// --------------------------------------------------------------- Add test

#[tokio::test]
async fn add_test_refusals_arrive_whole_however_much_body_is_left() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let other = app.log_in().await;
    let big = |form: AddTestForm| form.generated_file("T_Big.pdf", TRAILING);

    for _ in 0..TIMES {
        // No session: refused before any of the body is read.
        let form = big(app.add_test_form(&cookie).await.test(&NewTest::known()));
        let (status, body) = complete(
            Ok(app.submit_add_test_as(None, &app.base_url, form, "publish").await),
            "anonymous",
        )
        .await;
        assert_eq!(status, 401, "anonymous: {body}");
        assert!(body.contains("Admin login required"), "anonymous: {body}");

        // Another session's CSRF token: refused after the first field.
        let form = big(app.add_test_form(&other).await.test(&NewTest::known()));
        let (status, body) = complete(Ok(app.submit_add_test(&cookie, form, "publish").await), "foreign token").await;
        assert_eq!(status, 403, "foreign token: {body}");
        assert!(body.contains("This form has expired"), "foreign token: {body}");

        // Another site's Origin: refused by the middleware, before the handler.
        let form = big(app.add_test_form(&cookie).await.test(&NewTest::known()));
        let (status, body) = complete(
            Ok(app.submit_add_test_as(Some(&cookie), EVIL, form, "publish").await),
            "Origin",
        )
        .await;
        assert_eq!(status, 403, "Origin: {body}");
        assert!(body.contains("This form has expired"), "Origin: {body}");

        // A disallowed type, ahead of megabytes of good files.
        let form = app
            .add_test_form(&cookie)
            .await
            .test(&NewTest::known())
            .file("setup.exe", b"MZ")
            .generated_file("T_Big.pdf", TRAILING);
        let (status, body) = complete(Ok(app.submit_add_test(&cookie, form, "publish").await), "type").await;
        assert_eq!(status, 415, "type: {body}");
        assert!(refusal(&body).starts_with("“setup.exe” can’t be added"), "type: {body}");

        // A dropdown left on "Choose…".
        let form = big(app
            .add_test_form(&cookie)
            .await
            .test(&NewTest::known())
            .unchoose("host"));
        let (status, body) = complete(Ok(app.submit_add_test(&cookie, form, "publish").await), "dropdown").await;
        assert_eq!(status, 400, "dropdown: {body}");
        assert_eq!(refusal(&body), "Choose the Host, or Unknown.");
    }

    // A file over 100 MB, with more files behind it.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .generated_file("T_Huge.pdf", 100 * MIB + 1)
        .generated_file("S_Big.pdf", TRAILING);
    let (status, body) = complete(Ok(app.submit_add_test(&cookie, form, "publish").await), "size").await;
    assert_eq!(status, 413, "size: {body}");
    assert_eq!(
        refusal(&body),
        "“T_Huge.pdf” can’t be added: files can be at most 100 MB."
    );

    assert!(app.stored_keys().await.is_empty(), "nothing stored is left behind");
    assert!(app.change_log().await.is_empty(), "nothing was saved");
    // The connections are still good for the real thing.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_x.pdf", &sample_pdf(2000));
    app.add_test(&cookie, form, "publish").await;
}

// ------------------------------------------- one-file posts over the limit

/// A post with one file: the text `fields`, `padding` bytes of a field the
/// endpoint ignores, then `file`, named `name`, of `len` bytes starting
/// with `head`.
struct OneFile {
    path: &'static str,
    fields: Vec<(&'static str, String)>,
    padding: u64,
    file: &'static str,
    name: &'static str,
    head: &'static [u8],
    len: u64,
}

impl OneFile {
    fn padded(mut self, padding: u64) -> Self {
        self.padding = padding;
        self
    }

    fn file(mut self, name: &'static str, head: &'static [u8], len: u64) -> Self {
        (self.name, self.head, self.len) = (name, head, len);
        self
    }

    fn field(mut self, name: &'static str, value: &str) -> Self {
        for field in &mut self.fields {
            if field.0 == name {
                field.1 = value.to_owned();
            }
        }
        self
    }

    async fn send(self, app: &TestApp, cookie: Option<&str>, origin: &str) -> reqwest::Result<reqwest::Response> {
        let mut form = Form::new();
        for (name, value) in self.fields {
            form = form.text(name, value);
        }
        if self.padding > 0 {
            form = form.part(
                "padding",
                Part::stream_with_length(generated(b"", self.padding), self.padding),
            );
        }
        let form = form.part(
            self.file.to_owned(),
            Part::stream_with_length(generated(self.head, self.len), self.len).file_name(self.name.to_owned()),
        );
        let mut request = app.http.post(app.url(self.path)).header("origin", origin);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.multipart(form).send().await
    }
}

/// A post over the limit: a whole 413 that closes the connection, whose
/// refusal is returned.
async fn too_large(response: reqwest::Result<reqwest::Response>, what: &str) -> String {
    let closing = response
        .as_ref()
        .ok()
        .and_then(|r| r.headers().get("connection").cloned());
    let (status, body) = complete(response, what).await;
    assert_eq!(status, 413, "{what}: {body}");
    assert_eq!(
        closing.as_ref().and_then(|v| v.to_str().ok()),
        Some("close"),
        "{what}: the rest is not read"
    );
    refusal(&body)
}

/// A one-file endpoint's posts over its limit (about 101 MB), each answered
/// with a 413 that arrives whole, never "cut off". Which is reached first,
/// the file's 100 MB or the post's limit, is a race (multer reads every
/// chunk that is ready at once): both must answer the same.
async fn one_file_posts_over_the_limit(app: &TestApp, cookie: &str, post: impl Fn() -> OneFile) {
    let base = app.base_url.as_str();
    // A file over 100 MB: either limit may be reached first.
    let response = post()
        .file("scan.pdf", b"%PDF-", 120 * MIB)
        .send(app, Some(cookie), base)
        .await;
    assert_eq!(
        too_large(response, "file over 100 MB").await,
        "“scan.pdf” can’t be added: files can be at most 100 MB."
    );
    // The post's limit reached first, for certain: 2 MiB of other fields and
    // then a file of exactly 100 MiB.
    let response = post()
        .padded(2 * MIB)
        .file("scan.pdf", b"%PDF-", 100 * MIB)
        .send(app, Some(cookie), base)
        .await;
    assert_eq!(
        too_large(response, "limit reached in the file").await,
        "“scan.pdf” can’t be added: files can be at most 100 MB."
    );
    // The post's limit reached before the file.
    let response = post().padded(102 * MIB).send(app, Some(cookie), base).await;
    assert_eq!(
        too_large(response, "limit reached before the file").await,
        "That is too much to upload: files can be at most 100 MB."
    );
}

// ------------------------------------------------------------- Upload PDF

#[tokio::test]
async fn upload_pdf_refusals_arrive_whole_however_much_body_is_left() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let other = app.log_in().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let foreign = app.csrf_on("/logout", Some(&other)).await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(2000);
    let document = app
        .insert_document(&[test], NewDocument::pdf("Test", &pdf))
        .await
        .to_string();
    let base = app.base_url.as_str();
    let post = || OneFile {
        path: "/api/admin/documents/upload-pdf",
        fields: vec![
            ("csrf", csrf.clone()),
            ("document", document.clone()),
            ("return_to", "/admin/needs-review".to_owned()),
        ],
        padding: 0,
        file: "pdf",
        name: "scan.pdf",
        head: b"%PDF-",
        len: TRAILING,
    };

    for _ in 0..TIMES {
        let response = post().send(&app, None, base).await;
        let (status, body) = complete(response, "anonymous").await;
        assert_eq!(status, 401, "anonymous: {body}");

        let response = post().field("csrf", &foreign).send(&app, Some(&cookie), base).await;
        let (status, body) = complete(response, "foreign token").await;
        assert_eq!(status, 403, "foreign token: {body}");

        let response = post().send(&app, Some(&cookie), EVIL).await;
        let (status, body) = complete(response, "Origin").await;
        assert_eq!(status, 403, "Origin: {body}");

        let response = post()
            .file("scan.pdf", b"PK\x03\x04", TRAILING)
            .send(&app, Some(&cookie), base)
            .await;
        let (status, body) = complete(response, "not a PDF").await;
        assert_eq!(status, 415, "not a PDF: {body}");
        assert_eq!(refusal(&body), "“scan.pdf” is not a PDF.");

        let response = post().field("document", "999999").send(&app, Some(&cookie), base).await;
        let (status, body) = complete(response, "unknown Document").await;
        assert_eq!(status, 404, "unknown Document: {body}");
        assert_eq!(refusal(&body), "D999999 does not exist.");
    }

    one_file_posts_over_the_limit(&app, &cookie, post).await;

    assert!(app.change_log().await.is_empty(), "nothing changed");
    let keys = app.stored_keys().await;
    assert!(
        !keys.contains(&format!("documents/{document}/pdf")),
        "no PDF was stored: {keys:?}"
    );
}

// ------------------------------------------------------- Replace original

#[tokio::test]
async fn replace_original_refusals_arrive_whole_however_much_body_is_left() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let other = app.log_in().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let foreign = app.csrf_on("/logout", Some(&other)).await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(2000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let base = app.base_url.as_str();
    let post = || OneFile {
        path: "/api/admin/documents/replace-original",
        fields: vec![
            ("csrf", csrf.clone()),
            ("test", test.to_string()),
            ("document", document.to_string()),
        ],
        padding: 0,
        file: "original",
        name: "Better.pdf",
        head: b"%PDF-",
        len: TRAILING,
    };

    for _ in 0..TIMES {
        let response = post().send(&app, None, base).await;
        let (status, body) = complete(response, "anonymous").await;
        assert_eq!(status, 401, "anonymous: {body}");

        let response = post().field("csrf", &foreign).send(&app, Some(&cookie), base).await;
        let (status, body) = complete(response, "foreign token").await;
        assert_eq!(status, 403, "foreign token: {body}");

        let response = post().send(&app, Some(&cookie), EVIL).await;
        let (status, body) = complete(response, "Origin").await;
        assert_eq!(status, 403, "Origin: {body}");

        let response = post()
            .file("setup.exe", b"MZ", TRAILING)
            .send(&app, Some(&cookie), base)
            .await;
        let (status, body) = complete(response, "type").await;
        assert_eq!(status, 415, "type: {body}");
        assert!(refusal(&body).starts_with("“setup.exe” can’t be added"), "type: {body}");

        let response = post().field("document", "999999").send(&app, Some(&cookie), base).await;
        let (status, body) = complete(response, "unknown Document").await;
        assert_eq!(status, 404, "unknown Document: {body}");
        assert_eq!(refusal(&body), "D999999 does not exist or is in the Trash.");
    }

    one_file_posts_over_the_limit(&app, &cookie, post).await;

    assert!(app.change_log().await.is_empty(), "nothing changed");
    // The Original is still the one it had.
    let original = app
        .get(&format!("/tests/{test}/documents/{document}/original"))
        .await
        .bytes()
        .await
        .unwrap();
    assert_eq!(original.as_ref(), pdf.as_slice());
}
