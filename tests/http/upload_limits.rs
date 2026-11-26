//! The upload limits (issue #48): a file can be at most 90 MB on every
//! upload, and a post of several files, as a browser without JavaScript
//! sends them, at most about 95 MB in total, so every request fits under
//! Cloudflare's 100 MB.

use reqwest::multipart::{Form as Multipart, Part};

use crate::support::{
    TestApp,
    add_test::generated,
    fixtures::{NewDocument, NewTest, sample_pdf},
    html::Page,
    staging::File,
};

const MB: u64 = 1_000_000;
const MAX: u64 = 90 * MB;

fn refusal(body: &str) -> String {
    Page::parse(body).text("#refusal")
}

fn too_big(name: &str) -> String {
    format!("“{name}” can’t be added: files can be at most 90 MB.")
}

/// A generated file part of `len` bytes starting with `%PDF-`.
fn big_pdf(name: &str, len: u64) -> Part {
    Part::stream_with_length(generated(b"%PDF-", len), len)
        .file_name(name.to_owned())
        .mime_str("application/octet-stream")
        .unwrap()
}

async fn post(app: &TestApp, cookie: &str, path: &str, form: Multipart) -> (u16, String) {
    let response = app
        .http
        .post(app.url(path))
        .header("origin", &app.base_url)
        .header("cookie", cookie)
        .multipart(form)
        .send()
        .await
        .expect("POST");
    let status = response.status().as_u16();
    (status, response.text().await.expect("the whole answer"))
}

/// The size of a Document's file as stored.
async fn stored_size(app: &TestApp, document: i64, role: &str) -> i64 {
    sqlx::query_scalar("SELECT byte_size FROM document_files WHERE document_id = $1 AND role = $2::file_role")
        .bind(document)
        .bind(role)
        .fetch_one(&app.db)
        .await
        .expect("a stored file")
}

// Add test's own case is in `add_test.rs`.

#[tokio::test]
async fn add_files_takes_a_90_mb_file_and_refuses_one_byte_more() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let cookie = app.admin().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let form = |len: u64| {
        Multipart::new()
            .text("csrf", csrf.clone())
            .text("test", test.to_string())
            .part("files", big_pdf("S_Calculus.pdf", len))
            .text("save", "publish")
    };

    let (status, body) = post(&app, &cookie, "/api/admin/tests/add-files", form(MAX + 1)).await;
    assert_eq!(status, 413, "{body}");
    assert_eq!(refusal(&body), too_big("S_Calculus.pdf"));
    assert!(app.stored_keys().await.is_empty());

    let (status, body) = post(&app, &cookie, "/api/admin/tests/add-files", form(MAX)).await;
    assert_eq!(status, 303, "{body}");
    let document: i64 = sqlx::query_scalar("SELECT document_id FROM test_documents WHERE test_id = $1")
        .bind(test)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(stored_size(&app, document, "original").await, MAX as i64);
}

#[tokio::test]
async fn replace_original_takes_a_90_mb_file_and_refuses_one_byte_more() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(2000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let form = |len: u64| {
        Multipart::new()
            .text("csrf", csrf.clone())
            .text("test", test.to_string())
            .text("document", document.to_string())
            .part("original", big_pdf("Better.pdf", len))
    };
    let path = "/api/admin/documents/replace-original";

    let (status, body) = post(&app, &cookie, path, form(MAX + 1)).await;
    assert_eq!(status, 413, "{body}");
    assert_eq!(refusal(&body), too_big("Better.pdf"));
    assert_eq!(
        stored_size(&app, document, "original").await,
        2000,
        "the old one is kept"
    );

    let (status, body) = post(&app, &cookie, path, form(MAX)).await;
    assert_eq!(status, 303, "{body}");
    assert_eq!(stored_size(&app, document, "original").await, MAX as i64);
}

#[tokio::test]
async fn upload_pdf_takes_a_90_mb_file_and_refuses_one_byte_more() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(2000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let form = |len: u64| {
        Multipart::new()
            .text("csrf", csrf.clone())
            .text("document", document.to_string())
            .text("return_to", "/admin/needs-review")
            .part("pdf", big_pdf("scan.pdf", len))
    };
    let path = "/api/admin/documents/upload-pdf";

    let (status, body) = post(&app, &cookie, path, form(MAX + 1)).await;
    assert_eq!(status, 413, "{body}");
    assert_eq!(refusal(&body), too_big("scan.pdf"));

    let (status, body) = post(&app, &cookie, path, form(MAX)).await;
    assert_eq!(status, 303, "{body}");
    assert_eq!(stored_size(&app, document, "pdf").await, MAX as i64);
}

#[tokio::test]
async fn staging_takes_a_90_mb_file_and_refuses_one_byte_more() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;

    let answer = app
        .stage(&cookie, &csrf, "T_Calculus.pdf", File::Generated(b"%PDF-", MAX + 1))
        .await;
    assert_eq!(answer.refusal(413), too_big("T_Calculus.pdf"));
    assert!(app.stored_keys().await.is_empty());

    let id = app
        .stage(&cookie, &csrf, "T_Calculus.pdf", File::Generated(b"%PDF-", MAX))
        .await
        .id();
    let form = app.add_test_form(&cookie).await.test(&NewTest::known()).staged(&id);
    let test = app.add_test(&cookie, form, "publish").await;
    let document = app.saved_documents(&cookie, test).await[0].id;
    assert_eq!(stored_size(&app, document, "original").await, MAX as i64);
}

/// Without JavaScript, every file is in one post: one over about 95 MB is
/// refused, saying to upload fewer files at a time, though each file is
/// under 90 MB.
#[tokio::test]
async fn a_post_of_several_files_over_95_mb_is_refused_with_a_message() {
    let app = TestApp::start().await;
    let test = app
        .insert_test(&NewTest {
            year: Some(2001),
            ..NewTest::known()
        })
        .await;
    let cookie = app.admin().await;
    let message = "That is too much to upload at once: one upload can be at most about 95 MB in total. \
                   Upload fewer files at a time (add the rest with + Add files on the Test’s edit page), \
                   or turn on JavaScript, which uploads each file on its own.";

    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .generated_file("T_Calculus.pdf", 48 * MB)
        .generated_file("S_Calculus.pdf", 48 * MB);
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 413);
    assert_eq!(refusal(&response.text().await.unwrap()), message);

    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let form = Multipart::new()
        .text("csrf", csrf)
        .text("test", test.to_string())
        .part("files", big_pdf("T_Calculus.pdf", 48 * MB))
        .part("files", big_pdf("S_Calculus.pdf", 48 * MB))
        .text("save", "publish");
    let (status, body) = post(&app, &cookie, "/api/admin/tests/add-files", form).await;
    assert_eq!(status, 413, "{body}");
    assert_eq!(refusal(&body), message);

    assert!(app.stored_keys().await.is_empty(), "nothing stored is left behind");
    assert!(app.change_log().await.is_empty());
}
