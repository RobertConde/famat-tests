//! The PDF conversion queue, with a stand-in for LibreOffice
//! ([`StubConverter`]): Add test queues conversions, at most two run at
//! once, each with its own profile; a timeout kills the converter's whole
//! process group; a failure after one retry sets failed and Needs review;
//! and the Admin's Upload PDF, Retry PDF and Retry all failed.
//!
//! Real LibreOffice conversions are in `pdf_conversion` (slow).

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use sha2::{Digest, Sha256};

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest},
    html::{Page, text, text_in},
    pdf::{StubConverter, alive, eventually, minimal_pdf},
};

const WAIT: Duration = Duration::from_secs(30);

/// Start the app with the stub converter, and log in.
async fn start(stub: &StubConverter, timeout_secs: u64) -> (TestApp, String) {
    let env = stub.server_env(timeout_secs);
    let env: Vec<(&str, &str)> = env.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let app = TestApp::start_with(&env).await;
    let cookie = app.admin().await;
    (app, cookie)
}

/// Add a Published Test with these files through the Add test form, and
/// return its ID and its Documents' IDs in upload order.
async fn add(app: &TestApp, cookie: &str, files: &[(&str, &[u8])]) -> (i64, Vec<i64>) {
    let mut form = app.add_test_form(cookie).await.test(&NewTest::known());
    for (name, bytes) in files {
        form = form.file(name, bytes);
    }
    let test = app.add_test(cookie, form, "publish").await;
    let ids = app.saved_documents(cookie, test).await.iter().map(|d| d.id).collect();
    (test, ids)
}

/// Each Document's PDF status as the Add test summary shows it, e.g. "PDF
/// being prepared", "PDF supplied" (a PDF is stored), "PDF failed".
async fn statuses(app: &TestApp, cookie: &str, test: i64) -> Vec<String> {
    app.saved_documents(cookie, test)
        .await
        .into_iter()
        .map(|d| d.pdf_status)
        .collect()
}

/// Wait until every Document of the Test has a status other than "PDF
/// being prepared", and return them.
async fn settled(app: &TestApp, cookie: &str, test: i64) -> Vec<String> {
    eventually(WAIT, "the conversions to finish", || async {
        let statuses = statuses(app, cookie, test).await;
        (!statuses.iter().any(|s| s == "PDF being prepared")).then_some(statuses)
    })
    .await
}

async fn problems(app: &TestApp, cookie: &str) -> Page {
    app.get_page_as("/admin/needs-review", Some(cookie)).await
}

/// The bytes of a download, asserting a 200 and its content type.
async fn download(app: &TestApp, path: &str, content_type: &str) -> Vec<u8> {
    let response = app.get(path).await;
    assert_eq!(response.status(), 200, "GET {path}");
    assert_eq!(response.headers()["content-type"], content_type, "GET {path}");
    response.bytes().await.unwrap().to_vec()
}

async fn needs_review(app: &TestApp, document: i64) -> (bool, Option<String>) {
    sqlx::query_as("SELECT needs_review, needs_review_comment FROM documents WHERE id = $1")
        .bind(document)
        .fetch_one(&app.db)
        .await
        .unwrap()
}

/// The change log entries for one Document's field, as `(before, after)`.
async fn logged(app: &TestApp, document: i64, field: &str) -> Vec<(Option<String>, Option<String>)> {
    app.change_log()
        .await
        .into_iter()
        .filter(|(entity, id, f, _, _)| entity == "document" && *id == document && f == field)
        .map(|(_, _, _, before, after)| (before, after))
        .collect()
}

async fn post_retry(app: &TestApp, cookie: &str, csrf: &str, document: i64) -> reqwest::Response {
    let document = document.to_string();
    app.post_form(
        "/api/admin/documents/retry-pdf",
        &[
            ("csrf", csrf),
            ("document", &document),
            ("return_to", "/admin/needs-review"),
        ],
        &[("cookie", cookie)],
    )
    .await
}

async fn post_upload(
    app: &TestApp,
    cookie: &str,
    csrf: &str,
    document: i64,
    name: &str,
    pdf: &[u8],
) -> reqwest::Response {
    let form = Form::new()
        .text("csrf", csrf.to_owned())
        .text("document", document.to_string())
        .text("return_to", "/admin/needs-review")
        .part("pdf", Part::bytes(pdf.to_vec()).file_name(name.to_owned()));
    app.http
        .post(app.url("/api/admin/documents/upload-pdf"))
        .header("origin", &app.base_url)
        .header("cookie", cookie)
        .multipart(form)
        .send()
        .await
        .unwrap()
}

fn location(response: &reqwest::Response) -> &str {
    response.headers()["location"].to_str().unwrap()
}

// ----------------------------------------------------------------- queuing

#[tokio::test]
async fn add_test_queues_a_conversion_and_the_pdf_replaces_the_being_prepared_page() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let original = b"ok 2\nthe questions".as_slice();
    let (test, documents) = add(&app, &cookie, &[("Test.docx", original)]).await;
    let pdf_path = format!("/tests/{test}/documents/{}/pdf", documents[0]);

    // While the job runs, the public page says so and offers the Original.
    let waiting = app.get_page(&pdf_path).await;
    assert!(waiting.text("#pdf-status").contains("being prepared"));
    let link = waiting.one("a#original").value().attr("href").unwrap().to_owned();
    assert_eq!(download(&app, &link, DOCX).await, original);
    let row = problems(&app, &cookie).await;
    assert!(
        text(row.one(&format!("table#preparing tr#D{}", documents[0]))).contains("D"),
        "listed as being prepared"
    );

    assert_eq!(settled(&app, &cookie, test).await, ["PDF supplied"]);
    assert_eq!(download(&app, &pdf_path, "application/pdf").await, stub.pdf);
    assert_eq!(download(&app, &link, DOCX).await, original, "the Original is kept");
    let sha256: Vec<u8> =
        sqlx::query_scalar("SELECT sha256 FROM document_files WHERE document_id = $1 AND role = 'pdf'")
            .bind(documents[0])
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(sha256, Sha256::digest(&stub.pdf).to_vec());

    let runs = stub.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].input, "original.docx",
        "the Original's own name never reaches the command"
    );
    assert!(runs[0].math_type_off, "MathTypeToMath is off in the profile");
    assert!(!runs[0].profile.exists(), "the profile is thrown away");
    assert!(problems(&app, &cookie).await.select("table#preparing").is_empty());
}

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

#[tokio::test]
async fn at_most_two_conversions_run_at_once_and_no_two_share_a_profile() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let files: Vec<(String, &[u8])> = (1..=5)
        .map(|i| (format!("Part {i}.docx"), b"ok 1".as_slice()))
        .collect();
    let files: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), *b)).collect();
    let (test, _) = add(&app, &cookie, &files).await;

    assert_eq!(settled(&app, &cookie, test).await, ["PDF supplied"; 5]);
    let runs = stub.runs();
    assert_eq!(runs.len(), 5);
    let most = runs.iter().map(|r| r.running).max().unwrap();
    assert_eq!(most, 2, "two at once, never more: {runs:?}");
    let mut profiles: Vec<_> = runs.iter().map(|r| r.profile.clone()).collect();
    profiles.sort();
    profiles.dedup();
    assert_eq!(profiles.len(), 5, "each run has its own profile: {runs:?}");
    assert!(runs.iter().all(|r| r.math_type_off && !r.profile.exists()));
}

#[tokio::test]
async fn at_startup_every_document_being_prepared_gets_a_job_and_extensionless_files_are_sniffed() {
    let stub = StubConverter::new();
    stub.set_mode("ok");
    let (mut app, _) = start(&stub, 30).await;
    let test = app.insert_test(&NewTest::known()).await;
    let docx = std::fs::read("tests/fixtures/pdf/sample.docx").unwrap();
    let jpeg = std::fs::read("tests/fixtures/pdf/gradient.jpg").unwrap();
    let png = std::fs::read("tests/fixtures/pdf/gradient.png").unwrap();
    let pdf = minimal_pdf("an extensionless PDF");
    // Inserted as the import would, with no job.
    let word = app
        .insert_document(&[test], NewDocument::docx("Test", &docx, None))
        .await;
    let unnamed_word = app.insert_document(&[test], unnamed(&docx)).await;
    let unnamed_pdf = app.insert_document(&[test], unnamed(&pdf)).await;
    let unnamed_jpeg = app.insert_document(&[test], unnamed(&jpeg)).await;
    let png_doc = app
        .insert_document(
            &[test],
            NewDocument {
                content_type: "image/png",
                extension: "png",
                ..NewDocument::docx("Test", &png, None)
            },
        )
        .await;

    app.restart().await;
    let ids = [word, unnamed_word, unnamed_pdf, unnamed_jpeg, png_doc];
    let pdfs = eventually(WAIT, "every Document's PDF", || async {
        let mut pdfs = Vec::new();
        for id in ids {
            let response = app.get(&format!("/tests/{test}/documents/{id}/pdf")).await;
            if response.headers()["content-type"] != "application/pdf" {
                return None;
            }
            pdfs.push(response.bytes().await.unwrap().to_vec());
        }
        Some(pdfs)
    })
    .await;

    let mut inputs: Vec<String> = stub.runs().into_iter().map(|r| r.input).collect();
    inputs.sort();
    assert_eq!(
        inputs,
        ["original.docx", "original.docx"],
        "the extensionless Word file is a .docx"
    );
    assert_eq!(pdfs[0], stub.pdf);
    assert_eq!(pdfs[1], stub.pdf);
    assert_eq!(pdfs[2], pdf, "an extensionless PDF passes through unchanged");
    assert!(pdfs[3..].iter().all(|image| image.starts_with(b"%PDF-")));
    assert!(
        pdfs[3].windows(jpeg.len()).any(|w| w == jpeg),
        "a JPEG is embedded byte for byte"
    );
}

/// A Document whose Original has no extension, as the import may find.
fn unnamed(bytes: &[u8]) -> NewDocument<'_> {
    NewDocument {
        content_type: "application/octet-stream",
        extension: "",
        ..NewDocument::docx("Test", bytes, None)
    }
}

#[tokio::test]
async fn a_zip_is_not_convertible_and_is_never_converted() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let (test, documents) = add(&app, &cookie, &[("Code.zip", b"PK\x05\x06"), ("Test.docx", b"ok")]).await;

    assert_eq!(
        settled(&app, &cookie, test).await,
        ["no PDF (not convertible)", "PDF supplied"]
    );
    assert_eq!(stub.runs().len(), 1, "only the Word file was converted");
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let refused = post_retry(&app, &cookie, &csrf, documents[0]).await;
    assert_eq!(refused.status(), 409);
    assert!(refused.text().await.unwrap().contains("not convertible"));
}

// ---------------------------------------------------------------- failures

#[tokio::test]
async fn a_timeout_kills_the_whole_process_group_and_after_one_retry_fails_with_needs_review() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 1).await;
    let (test, documents) = add(&app, &cookie, &[("Test.docx", b"hang")]).await;
    let document = documents[0];

    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed"]);
    let runs = stub.runs();
    assert_eq!(runs.len(), 2, "tried once, then retried once");
    for run in &runs {
        let child = stub.child_of(run);
        eventually(Duration::from_secs(5), "the converter and its child to die", || async {
            (!alive(run.pid) && !alive(child)).then_some(())
        })
        .await;
    }

    let page = problems(&app, &cookie).await;
    let row = page.one(&format!("table#failed tr#D{document}"));
    let comment = text_in(row, "td.comment");
    assert_eq!(
        comment,
        "PDF conversion failed: LibreOffice took longer than 1 s and was stopped"
    );
    assert_eq!(needs_review(&app, document).await, (true, Some(comment)));

    // The public sees only the Original.
    let public = app.get_page(&format!("/tests/{test}/documents/{document}/pdf")).await;
    assert!(public.text("#pdf-status").contains("no PDF"));
    assert!(public.select("a#original").len() == 1);
}

#[tokio::test]
async fn a_corrupt_file_fails_with_the_error_recorded_and_retry_pdf_converts_it() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let (test, documents) = add(&app, &cookie, &[("Test.docx", b"fail")]).await;
    let document = documents[0];

    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed"]);
    assert_eq!(stub.runs().len(), 2);
    let page = problems(&app, &cookie).await;
    let comment = text_in(page.one(&format!("table#failed tr#D{document}")), "td.comment");
    assert_eq!(
        comment,
        "PDF conversion failed: LibreOffice made no PDF (exit status: 1): stub: this file is broken"
    );

    // The Admin fixes the cause and presses Retry PDF on the problems page.
    stub.set_mode("ok");
    let form = page.one(&format!("tr#D{document} form.retry-pdf"));
    assert_eq!(form.value().attr("action"), Some("/api/admin/documents/retry-pdf"));
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let response = post_retry(&app, &cookie, &csrf, document).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/admin/needs-review");

    assert_eq!(settled(&app, &cookie, test).await, ["PDF supplied"]);
    assert_eq!(stub.runs().len(), 3);
    let pdf = format!("/tests/{test}/documents/{document}/pdf");
    assert_eq!(download(&app, &pdf, "application/pdf").await, stub.pdf);
    assert_eq!(
        needs_review(&app, document).await,
        (false, None),
        "Needs review is cleared"
    );
    assert!(problems(&app, &cookie).await.select("#no-failed").len() == 1);
    assert_eq!(
        logged(&app, document, "pdf_status").await,
        [
            (None, Some("preparing".to_owned())),
            (Some("failed".to_owned()), Some("preparing".to_owned()))
        ]
    );
}

#[tokio::test]
async fn a_pdf_that_does_not_parse_is_a_failure() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let (test, documents) = add(&app, &cookie, &[("Test.docx", b"garbage")]).await;

    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed"]);
    let page = problems(&app, &cookie).await;
    let comment = text_in(page.one(&format!("table#failed tr#D{}", documents[0])), "td.comment");
    assert!(
        comment.starts_with("PDF conversion failed: the PDF LibreOffice made does not parse"),
        "{comment}"
    );
}

#[tokio::test]
async fn a_failure_keeps_an_earlier_needs_review_reason() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let test = app.insert_test(&NewTest::known()).await;
    let document = app
        .insert_document(&[test], NewDocument::docx("Test", b"fail", None))
        .await;
    sqlx::query(
        "UPDATE documents SET needs_review = true, needs_review_comment = 'AI: unsure of the kind' WHERE id = $1",
    )
    .bind(document)
    .execute(&app.db)
    .await
    .unwrap();
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    assert_eq!(post_retry(&app, &cookie, &csrf, document).await.status(), 303);

    let comment = eventually(WAIT, "the failure", || async {
        let (_, comment) = needs_review(&app, document).await;
        comment.filter(|c| c.contains("PDF conversion failed"))
    })
    .await;
    assert_eq!(
        comment,
        "AI: unsure of the kind\nPDF conversion failed: LibreOffice made no PDF (exit status: 1): stub: this file is broken"
    );

    stub.set_mode("ok");
    assert_eq!(post_retry(&app, &cookie, &csrf, document).await.status(), 303);
    let review = eventually(WAIT, "the PDF", || async {
        let review = needs_review(&app, document).await;
        (review.1.as_deref() == Some("AI: unsure of the kind")).then_some(review)
    })
    .await;
    assert!(review.0, "still Needs review for its other reason");
}

// ---------------------------------------------------------- Admin actions

#[tokio::test]
async fn retry_all_failed_queues_every_failed_document_again() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let (test, documents) = add(&app, &cookie, &[("Test.docx", b"fail"), ("Answers.docx", b"fail")]).await;
    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed", "PDF failed"]);

    stub.set_mode("ok");
    let page = problems(&app, &cookie).await;
    assert_eq!(text(page.one("#failed-heading")), "Failed (2)");
    let form = page.one("form.retry-failed");
    assert_eq!(form.value().attr("action"), Some("/api/admin/pdfs/retry-failed"));
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let response = app
        .post_form(
            "/api/admin/pdfs/retry-failed",
            &[("csrf", &csrf), ("return_to", "/admin/needs-review")],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/admin/needs-review");

    assert_eq!(settled(&app, &cookie, test).await, ["PDF supplied", "PDF supplied"]);
    for document in documents {
        assert_eq!(
            logged(&app, document, "pdf_status").await.last().unwrap(),
            &(Some("failed".to_owned()), Some("preparing".to_owned()))
        );
    }
}

#[tokio::test]
async fn upload_pdf_replaces_a_failed_conversion_and_the_status_becomes_ready() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let (test, documents) = add(&app, &cookie, &[("Test.docx", b"fail")]).await;
    let document = documents[0];
    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed"]);
    let (_, comment) = needs_review(&app, document).await;

    let page = problems(&app, &cookie).await;
    let form = page.one(&format!("tr#D{document} form.upload-pdf"));
    assert_eq!(form.value().attr("enctype"), Some("multipart/form-data"));
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;

    // Not a PDF: refused, nothing changes.
    let refused = post_upload(&app, &cookie, &csrf, document, "scan.docx", b"PK\x03\x04 not a pdf").await;
    assert_eq!(refused.status(), 415);
    assert!(refused.text().await.unwrap().contains("“scan.docx” is not a PDF."));
    assert_eq!(statuses(&app, &cookie, test).await, ["PDF failed"]);

    let by_hand = minimal_pdf("printed from Word by hand");
    let response = post_upload(&app, &cookie, &csrf, document, "by hand.pdf", &by_hand).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/admin/needs-review");

    assert_eq!(statuses(&app, &cookie, test).await, ["PDF supplied"]);
    let pdf = format!("/tests/{test}/documents/{document}/pdf");
    assert_eq!(download(&app, &pdf, "application/pdf").await, by_hand);
    assert_eq!(needs_review(&app, document).await, (false, None));
    let sha = hex(&Sha256::digest(&by_hand));
    let after = format!("by hand.pdf ({} bytes, sha256 {sha})", by_hand.len());
    assert_eq!(logged(&app, document, "pdf").await, [(None, Some(after))]);
    assert_eq!(
        logged(&app, document, "pdf_status").await.last().unwrap(),
        &(Some("failed".to_owned()), Some("ready".to_owned()))
    );
    assert_eq!(
        logged(&app, document, "needs_review").await,
        [(Some("true".to_owned()), Some("false".to_owned()))]
    );
    assert_eq!(logged(&app, document, "needs_review_comment").await, [(comment, None)]);
    assert_eq!(stub.runs().len(), 2, "no conversion after the upload");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn the_pdf_actions_refuse_anonymous_callers_and_foreign_tokens() {
    let stub = StubConverter::new();
    let (app, cookie) = start(&stub, 30).await;
    let (_, documents) = add(&app, &cookie, &[("Test.docx", b"fail")]).await;
    let document = documents[0].to_string();
    let other = app.log_in().await;
    let foreign = app.csrf_on("/logout", Some(&other)).await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;

    let anonymous = app.get_page("/admin/needs-review").await;
    assert!(anonymous.select("table").is_empty());
    assert!(text(anonymous.one("#not-logged-in")).contains("not logged in"));
    assert_eq!(app.get("/api/admin/pdf-problems").await.status(), 401);

    for path in ["/api/admin/documents/retry-pdf", "/api/admin/pdfs/retry-failed"] {
        let fields = [("csrf", csrf.as_str()), ("document", document.as_str())];
        assert_eq!(
            app.post_form(path, &fields, &[]).await.status(),
            401,
            "{path} anonymous"
        );
        let fields = [("csrf", foreign.as_str()), ("document", document.as_str())];
        let response = app.post_form(path, &fields, &[("cookie", &cookie)]).await;
        assert_eq!(response.status(), 403, "{path} with another session's token");
        let fields = [("csrf", csrf.as_str()), ("document", document.as_str())];
        let headers = [("cookie", cookie.as_str()), ("origin", "https://evil.example")];
        assert_eq!(
            app.post_form(path, &fields, &headers).await.status(),
            403,
            "{path} cross-site"
        );
    }

    let pdf = minimal_pdf("x");
    let anonymous = Form::new()
        .text("csrf", csrf.clone())
        .text("document", document.clone())
        .part("pdf", Part::bytes(pdf.clone()).file_name("x.pdf"));
    let response = app
        .http
        .post(app.url("/api/admin/documents/upload-pdf"))
        .header("origin", &app.base_url)
        .multipart(anonymous)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let id = documents[0];
    let response = post_upload(&app, &cookie, &foreign, id, "x.pdf", &pdf).await;
    assert_eq!(response.status(), 403);

    assert!(logged(&app, id, "pdf").await.is_empty(), "no PDF was logged");
    let keys = app.stored_keys().await;
    assert!(
        !keys.contains(&format!("documents/{id}/pdf")),
        "nothing was stored: {keys:?}"
    );
}
