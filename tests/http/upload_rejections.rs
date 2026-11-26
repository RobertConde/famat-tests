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

// ------------------------------------------------------------- Upload PDF

/// An Upload PDF post of `len` bytes starting with `head`, for `document`.
async fn upload_pdf(
    app: &TestApp,
    cookie: Option<&str>,
    origin: &str,
    csrf: &str,
    document: &str,
    head: &[u8],
    len: u64,
) -> reqwest::Result<reqwest::Response> {
    let form = Form::new()
        .text("csrf", csrf.to_owned())
        .text("document", document.to_owned())
        .text("return_to", "/admin/needs-review")
        .part(
            "pdf",
            Part::stream_with_length(generated(head, len), len).file_name("scan.pdf"),
        );
    let mut request = app
        .http
        .post(app.url("/api/admin/documents/upload-pdf"))
        .header("origin", origin);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    request.multipart(form).send().await
}

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

    for _ in 0..TIMES {
        let response = upload_pdf(&app, None, base, &csrf, &document, b"%PDF-", TRAILING).await;
        let (status, body) = complete(response, "anonymous").await;
        assert_eq!(status, 401, "anonymous: {body}");

        let response = upload_pdf(&app, Some(&cookie), base, &foreign, &document, b"%PDF-", TRAILING).await;
        let (status, body) = complete(response, "foreign token").await;
        assert_eq!(status, 403, "foreign token: {body}");

        let response = upload_pdf(&app, Some(&cookie), EVIL, &csrf, &document, b"%PDF-", TRAILING).await;
        let (status, body) = complete(response, "Origin").await;
        assert_eq!(status, 403, "Origin: {body}");

        let response = upload_pdf(&app, Some(&cookie), base, &csrf, &document, b"PK\x03\x04", TRAILING).await;
        let (status, body) = complete(response, "not a PDF").await;
        assert_eq!(status, 415, "not a PDF: {body}");
        assert_eq!(refusal(&body), "“scan.pdf” is not a PDF.");

        let response = upload_pdf(&app, Some(&cookie), base, &csrf, "999999", b"%PDF-", TRAILING).await;
        let (status, body) = complete(response, "unknown Document").await;
        assert_eq!(status, 404, "unknown Document: {body}");
        assert_eq!(refusal(&body), "D999999 does not exist.");
    }

    // Over the whole post's limit (about 101 MB): the server stops reading
    // there, but the refusal still arrives.
    let response = upload_pdf(&app, Some(&cookie), base, &csrf, &document, b"%PDF-", 120 * MIB).await;
    let closing = response
        .as_ref()
        .ok()
        .and_then(|r| r.headers().get("connection").cloned());
    let (status, body) = complete(response, "size").await;
    assert_eq!(
        closing.as_ref().and_then(|v| v.to_str().ok()),
        Some("close"),
        "the rest is not read"
    );
    assert_eq!(status, 413, "size: {body}");
    assert_eq!(
        refusal(&body),
        "“scan.pdf” can’t be added: files can be at most 100 MB."
    );

    assert!(app.change_log().await.is_empty(), "nothing changed");
    let keys = app.stored_keys().await;
    assert!(
        !keys.contains(&format!("documents/{document}/pdf")),
        "no PDF was stored: {keys:?}"
    );
}
