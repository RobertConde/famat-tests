//! The Test edit page (`/admin/tests/{id}/edit`): the Edit link on each
//! result row (Admins only), saving the Test and each Document, Replace
//! original, Also on Tests…, and + Add files. Every write is checked over
//! HTTP: what the public page then shows, and the change log it wrote.

use std::time::Duration;

use reqwest::multipart::{Form as Multipart, Part};

use crate::support::{
    TestApp,
    download::disposition,
    fixtures::{NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Form, Page, text, text_in},
    pdf::{StubConverter, eventually, minimal_pdf},
};

const WAIT: Duration = Duration::from_secs(30);

fn edit_path(test: i64) -> String {
    format!("/admin/tests/{test}/edit")
}

async fn edit_page(app: &TestApp, cookie: &str, test: i64) -> Page {
    app.get_page_as(&edit_path(test), Some(cookie)).await
}

/// Send a filled-in POST form as a browser on this site would.
async fn submit(app: &TestApp, cookie: &str, form: Form<'_>) -> reqwest::Response {
    let fields = form.fields();
    let pairs: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    app.post_form(&form.action(), &pairs, &[("cookie", cookie)]).await
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("location")
        .map(|l| l.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

/// Assert a 303 back to the Test's edit page.
async fn assert_saved(response: reqwest::Response, test: i64) {
    let status = response.status();
    let location = location(&response);
    assert_eq!(status, 303, "{}", response.text().await.unwrap_or_default());
    assert!(location.starts_with(&edit_path(test)), "redirected to {location}");
}

/// The value of the option showing `shown` in the select matching `css`.
fn option_value(page: &Page, css: &str, shown: &str) -> String {
    page.select(&format!("{css} option"))
        .into_iter()
        .find(|o| text(*o) == shown)
        .unwrap_or_else(|| panic!("no option {shown:?} in {css}"))
        .value()
        .attr("value")
        .unwrap()
        .to_owned()
}

/// The change log entries for one record, as `(field, before, after)`.
async fn logged(app: &TestApp, entity: &str, id: i64) -> Vec<(String, Option<String>, Option<String>)> {
    app.change_log()
        .await
        .into_iter()
        .filter(|(e, i, _, _, _)| e == entity && *i == id)
        .map(|(_, _, field, before, after)| (field, before, after))
        .collect()
}

fn entry(field: &str, before: Option<&str>, after: Option<&str>) -> (String, Option<String>, Option<String>) {
    (field.to_owned(), before.map(str::to_owned), after.map(str::to_owned))
}

/// The public search page's row of a Test, if it shows one.
async fn public_row_text(app: &TestApp, test: i64) -> Option<String> {
    let page = app.get_page("/").await;
    page.select(&format!("tr#T{test}")).first().map(|row| text(*row))
}

/// The Documents the public row of a Test lists, as their link text.
async fn public_documents(app: &TestApp, test: i64) -> Vec<String> {
    let page = app.get_page("/").await;
    page.select(&format!("tr#T{test} .document > a:first-child"))
        .into_iter()
        .map(text)
        .collect()
}

async fn download(app: &TestApp, path: &str) -> (reqwest::StatusCode, Vec<u8>) {
    let response = app.get(path).await;
    (response.status(), response.bytes().await.unwrap().to_vec())
}

/// The text of the Document's PDF status on the edit page.
async fn pdf_status(app: &TestApp, cookie: &str, test: i64, document: i64) -> String {
    let page = edit_page(app, cookie, test).await;
    text_in(page.one(&format!("section#D{document}")), ".pdf-status")
}

/// The CSRF token on a Test's edit page.
async fn csrf(app: &TestApp, cookie: &str, test: i64) -> String {
    let page = edit_page(app, cookie, test).await;
    page.one("form#edit-test input[name=csrf]")
        .value()
        .attr("value")
        .unwrap()
        .to_owned()
}

async fn post_multipart(app: &TestApp, path: &str, cookie: Option<&str>, form: Multipart) -> reqwest::Response {
    let mut request = app
        .http
        .post(app.url(path))
        .header("origin", &app.base_url)
        .multipart(form);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    request.send().await.expect("POST multipart")
}

fn file(name: &str, bytes: &[u8]) -> Part {
    Part::bytes(bytes.to_vec())
        .file_name(name.to_owned())
        .mime_str("application/octet-stream")
        .unwrap()
}

/// Replace original, as the Document's form posts it.
async fn replace_original(
    app: &TestApp,
    cookie: Option<&str>,
    csrf: &str,
    test: i64,
    document: i64,
    name: &str,
    bytes: &[u8],
) -> reqwest::Response {
    let form = Multipart::new()
        .text("csrf", csrf.to_owned())
        .text("test", test.to_string())
        .text("document", document.to_string())
        .part("original", file(name, bytes));
    post_multipart(app, "/api/admin/documents/replace-original", cookie, form).await
}

/// **+ Add files**, as the form posts it: the files, then `fields` (per-file
/// kinds and labels), then the pressed button.
async fn add_files(
    app: &TestApp,
    cookie: Option<&str>,
    csrf: &str,
    test: i64,
    files: &[(&str, &[u8])],
    fields: &[(&str, &str)],
    save: &str,
) -> reqwest::Response {
    let mut form = Multipart::new()
        .text("csrf", csrf.to_owned())
        .text("test", test.to_string());
    for (name, bytes) in files {
        form = form.part("files", file(name, bytes));
    }
    for (name, value) in fields {
        form = form.text(name.to_string(), value.to_string());
    }
    form = form.text("save", save.to_owned());
    post_multipart(app, "/api/admin/tests/add-files", cookie, form).await
}

// ------------------------------------------------------------- the page

#[tokio::test]
async fn only_admins_see_the_edit_link_and_the_edit_page() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;

    let public = app.get_page("/").await;
    assert!(
        public.select("a.edit").is_empty(),
        "anonymous visitors see no Edit link"
    );
    let anonymous = app.get_page(&edit_path(test)).await;
    assert_eq!(anonymous.select("#not-logged-in").len(), 1);
    assert!(anonymous.select("form#edit-test").is_empty());

    let cookie = app.admin().await;
    let search = app.get_page_as("/", Some(&cookie)).await;
    let edit = search.one(&format!("tr#T{test} a.edit"));
    assert_eq!(edit.value().attr("href"), Some(edit_path(test).as_str()));

    let page = edit_page(&app, &cookie, test).await;
    assert_eq!(page.text("h2"), format!("Edit T{test}"));
    assert_eq!(
        page.text("#description"),
        "1993 February Invitational · Plant City · Calculus/Mu Individual"
    );
    // The Test's values are chosen in the dropdowns.
    for (name, shown) in [
        ("year", "1993"),
        ("month", "February"),
        ("competition", "Invitational"),
        ("division_level", "Calculus/Mu"),
        ("test_type", "Individual"),
        ("host", "Plant City"),
        ("visibility", "Published"),
    ] {
        let selected = page.one(&format!("form#edit-test select[name={name}] option[selected]"));
        assert_eq!(text(selected), shown, "{name}");
    }

    let missing = app.get_page_as("/admin/tests/999999/edit", Some(&cookie)).await;
    assert_eq!(missing.select("#no-test").len(), 1);
}

#[tokio::test]
async fn every_write_needs_an_admin_session_and_a_valid_csrf_token() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let token = csrf(&app, &cookie, test).await;
    let page = edit_page(&app, &cookie, test).await;
    let section = format!("section#D{document}");
    // Each form as the page fills it in, with the `csrf` field set to `token`.
    let forms = |token: &str| -> Vec<(String, Vec<(String, String)>)> {
        [
            page.post_form("form#edit-test").select("visibility", "hidden"),
            page.post_form(&format!("{section} form.save-document"))
                .select("visibility", "hidden"),
            page.post_form(&format!("{section} form.also-on"))
                .press(&format!("button[value='detach:{test}']")),
        ]
        .into_iter()
        .map(|form| {
            let fields = form
                .fields()
                .into_iter()
                .map(|(name, value)| {
                    let value = if name == "csrf" { token.to_owned() } else { value };
                    (name, value)
                })
                .collect();
            (form.action(), fields)
        })
        .collect()
    };
    let post = async |path: &str, fields: &[(String, String)], headers: &[(&str, &str)]| {
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        app.post_form(path, &pairs, headers).await.status()
    };
    for ((path, good), (_, bad)) in forms(&token).into_iter().zip(forms("1.bad")) {
        assert_eq!(post(&path, &good, &[]).await, 401, "anonymous POST {path}");
        assert_eq!(
            post(&path, &bad, &[("cookie", &cookie)]).await,
            403,
            "bad CSRF POST {path}"
        );
        let cross_site = [("cookie", cookie.as_str()), ("origin", "https://evil.example")];
        assert_eq!(post(&path, &good, &cross_site).await, 403, "cross-site POST {path}");
    }

    let keys = app.stored_keys().await;
    let docx = sample_docx(1_000);
    let anonymous = replace_original(&app, None, &token, test, document, "new.docx", &docx).await;
    assert_eq!(anonymous.status(), 401);
    let bad = replace_original(&app, Some(&cookie), "1.bad", test, document, "new.docx", &docx).await;
    assert_eq!(bad.status(), 403);
    let anonymous = add_files(&app, None, &token, test, &[("T_more.pdf", &pdf)], &[], "publish").await;
    assert_eq!(anonymous.status(), 401);
    let bad = add_files(
        &app,
        Some(&cookie),
        "1.bad",
        test,
        &[("T_more.pdf", &pdf)],
        &[],
        "publish",
    )
    .await;
    assert_eq!(bad.status(), 403);

    // Nothing changed.
    assert_eq!(app.stored_keys().await, keys);
    assert!(app.change_log().await.is_empty());
    assert_eq!(public_documents(&app, test).await, ["Test"]);
    let (status, bytes) = download(&app, &format!("/tests/{test}/documents/{document}/original")).await;
    assert_eq!((status.as_u16(), bytes), (200, pdf));
}

// --------------------------------------------------------------- the Test

#[tokio::test]
async fn saving_the_test_changes_its_facet_values_visibility_and_needs_review() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;

    let page = edit_page(&app, &cookie, test).await;
    let form = page
        .post_form("form#edit-test")
        .fill("year_new", "1994")
        .select("month", "not_applicable")
        .select("competition", "new")
        .fill("competition_new", "State")
        .select("host", "new")
        .fill("host_new", "FAMAT")
        .check("needs_review", "on", true)
        .fill("needs_review_comment", "Is this really Calculus?");
    assert_saved(submit(&app, &cookie, form).await, test).await;

    // The public page shows the new values, and so do download names.
    let row = public_row_text(&app, test).await.expect("still published");
    assert!(
        row.contains("1994") && row.contains("State") && row.contains("FAMAT"),
        "{row}"
    );
    assert!(!row.contains("February") && !row.contains("Plant City"), "{row}");
    let response = app.get(&format!("/tests/{test}/documents/{document}/pdf")).await;
    assert_eq!(
        disposition(&response).name,
        format!("T{test} D{document} 1994 State - FAMAT - Calculus-Mu Individual - Test.pdf")
    );

    let page = edit_page(&app, &cookie, test).await;
    assert_eq!(page.text("#description"), "1994 State · FAMAT · Calculus/Mu Individual");
    assert!(page.one("#t-needs-review").value().attr("checked").is_some());
    assert_eq!(page.text("form#edit-test textarea"), "Is this really Calculus?");

    assert_eq!(
        logged(&app, "test", test).await,
        [
            entry("year", Some("1993"), Some("1994")),
            entry("month", Some("February"), Some("Not applicable")),
            entry("competition", Some("Invitational"), Some("State")),
            entry("host", Some("Plant City"), Some("FAMAT")),
            entry("needs_review", Some("false"), Some("true")),
            entry("needs_review_comment", None, Some("Is this really Calculus?")),
        ]
    );
    // The new values were created, and logged.
    let created: Vec<_> = app
        .change_log()
        .await
        .into_iter()
        .filter(|(e, _, f, _, _)| e == "facet_value" && f == "name")
        .map(|(_, _, _, _, after)| after.unwrap())
        .collect();
    assert_eq!(created, ["State", "FAMAT"]);

    // Hidden: gone from the public page; unticking Needs review clears it.
    let page = edit_page(&app, &cookie, test).await;
    let form = page
        .post_form("form#edit-test")
        .select("visibility", "hidden")
        .check("needs_review", "on", false);
    assert_saved(submit(&app, &cookie, form).await, test).await;
    assert_eq!(public_row_text(&app, test).await, None);
    let (status, _) = download(&app, &format!("/tests/{test}/documents/{document}/pdf")).await;
    assert_eq!(status, 404);
    let tail: Vec<_> = logged(&app, "test", test).await.into_iter().skip(6).collect();
    assert_eq!(
        tail,
        [
            entry("published", Some("true"), Some("false")),
            entry("needs_review", Some("true"), Some("false")),
            entry("needs_review_comment", Some("Is this really Calculus?"), None),
        ]
    );

    // Published again, it is back.
    let page = edit_page(&app, &cookie, test).await;
    let form = page.post_form("form#edit-test").select("visibility", "published");
    assert_saved(submit(&app, &cookie, form).await, test).await;
    assert!(public_row_text(&app, test).await.is_some());
}

#[tokio::test]
async fn a_test_without_its_year_or_competition_cannot_be_published() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let cookie = app.admin().await;
    let page = edit_page(&app, &cookie, test).await;
    let form = page
        .post_form("form#edit-test")
        .select("year", "unknown")
        .select("competition", "unknown");
    let response = submit(&app, &cookie, form).await;
    assert_eq!(response.status(), 409);
    let refusal = Page::parse(&response.text().await.unwrap());
    assert!(refusal.text("#refusal").contains("Year or Competition"));
    assert!(app.change_log().await.is_empty());

    // Hidden, it saves.
    let page = edit_page(&app, &cookie, test).await;
    let form = page
        .post_form("form#edit-test")
        .select("year", "unknown")
        .select("competition", "unknown")
        .select("visibility", "hidden");
    assert_saved(submit(&app, &cookie, form).await, test).await;
    let page = edit_page(&app, &cookie, test).await;
    assert_eq!(
        page.text("#description"),
        "— February — · Plant City · Calculus/Mu Individual"
    );
}

// ----------------------------------------------------------- the Documents

#[tokio::test]
async fn saving_a_document_sets_its_labels_and_label_source_and_can_clear_needs_review() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    sqlx::query(
        "UPDATE documents SET label_source = 'ai', needs_review = true, needs_review_comment = $2
         WHERE id = $1",
    )
    .bind(document)
    .bind("AI unsure:\n<maybe> Answers & Solutions")
    .execute(&app.db)
    .await
    .unwrap();
    let cookie = app.admin().await;

    let page = edit_page(&app, &cookie, test).await;
    let section = format!("section#D{document}");
    assert_eq!(text_in(page.one(&section), ".label-source"), "labels from AI");
    // The comment comes back exactly as it is, if saved unchanged.
    let fields = page.post_form(&format!("{section} form.save-document")).fields();
    assert!(
        fields.contains(&(
            "needs_review_comment".to_owned(),
            "AI unsure:\n<maybe> Answers & Solutions".to_owned()
        )),
        "{fields:?}"
    );
    let solutions = option_value(&page, &format!("{section} select[name=kind]"), "Solutions");
    let form = page
        .post_form(&format!("{section} form.save-document"))
        .select("kind", &solutions)
        .fill("label", " 2 per  page ")
        .check("needs_review", "on", false);
    assert_saved(submit(&app, &cookie, form).await, test).await;

    assert_eq!(public_documents(&app, test).await, ["Solutions (2 per page)"]);
    let response = app.get(&format!("/tests/{test}/documents/{document}/pdf")).await;
    assert!(
        disposition(&response).name.ends_with(" - Solutions (2 per page).pdf"),
        "{:?}",
        disposition(&response)
    );
    let page = edit_page(&app, &cookie, test).await;
    assert_eq!(text_in(page.one(&section), ".label-source"), "labels by an Admin");
    assert!(
        page.one(&format!("#d{document}-needs-review"))
            .value()
            .attr("checked")
            .is_none()
    );
    assert_eq!(
        logged(&app, "document", document).await,
        [
            entry("kind", Some("Test"), Some("Solutions")),
            entry("label", None, Some("2 per page")),
            entry("needs_review", Some("true"), Some("false")),
            entry(
                "needs_review_comment",
                Some("AI unsure:\n<maybe> Answers & Solutions"),
                None
            ),
            entry("label_source", Some("ai"), Some("admin")),
        ]
    );

    // Hidden: the public row no longer lists it, and it doesn't download.
    let page = edit_page(&app, &cookie, test).await;
    let form = page
        .post_form(&format!("{section} form.save-document"))
        .select("visibility", "hidden");
    assert_saved(submit(&app, &cookie, form).await, test).await;
    assert!(public_documents(&app, test).await.is_empty());
    let (status, _) = download(&app, &format!("/tests/{test}/documents/{document}/pdf")).await;
    assert_eq!(status, 404);
    assert_eq!(
        logged(&app, "document", document).await.last().unwrap(),
        &entry("published", Some("true"), Some("false"))
    );
}

#[tokio::test]
async fn replace_original_stores_the_new_file_and_regenerates_the_pdf() {
    let stub = StubConverter::new();
    let env = stub.server_env(30);
    let env: Vec<(&str, &str)> = env.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let app = TestApp::start_with(&env).await;
    let cookie = app.admin().await;
    let test = app.insert_test(&NewTest::known()).await;
    let old_docx = sample_docx(1_000);
    let supplied = minimal_pdf("the supplied PDF");
    let document = app
        .insert_document(&[test], NewDocument::docx("Test", &old_docx, Some(&supplied)))
        .await;
    let token = csrf(&app, &cookie, test).await;
    let pdf_path = format!("/tests/{test}/documents/{document}/pdf");
    let original_path = format!("/tests/{test}/documents/{document}/original");

    // A Word file: stored at once, its PDF made afresh by the queue.
    let new_docx = b"ok\nthe better copy".to_vec();
    let response = replace_original(&app, Some(&cookie), &token, test, document, "Better.docx", &new_docx).await;
    assert_eq!(response.status(), 303, "{}", response.text().await.unwrap());
    assert_eq!(download(&app, &original_path).await.1, new_docx);
    let ready = eventually(WAIT, "the new PDF", || async {
        let status = pdf_status(&app, &cookie, test, document).await;
        (status != "PDF being prepared").then_some(status)
    })
    .await;
    assert_eq!(ready, "PDF ready");
    let (status, made) = download(&app, &pdf_path).await;
    assert_eq!((status.as_u16(), made), (200, stub.pdf.clone()));
    assert_eq!(stub.runs().len(), 1);
    let log = logged(&app, "document", document).await;
    let fields: Vec<&str> = log.iter().map(|(f, _, _)| f.as_str()).collect();
    assert_eq!(fields, ["original", "pdf", "pdf_status"]);
    assert!(
        log[0].1.as_deref().unwrap().starts_with("original.docx (1000 bytes"),
        "{log:?}"
    );
    let after = format!("Better.docx ({} bytes, sha256 ", new_docx.len());
    assert!(log[0].2.as_deref().unwrap().starts_with(&after), "{log:?}");
    assert!(log[1].1.as_deref().unwrap().starts_with("PDF ("), "{log:?}");
    assert_eq!(log[1].2, None);
    assert_eq!(log[2], entry("pdf_status", Some("ready"), Some("preparing")));

    // A PDF: it is its own PDF, and the one made before is gone.
    let new_pdf = minimal_pdf("the replacement PDF");
    let response = replace_original(&app, Some(&cookie), &token, test, document, "Better.pdf", &new_pdf).await;
    assert_eq!(response.status(), 303);
    eventually(WAIT, "the PDF Original to be ready", || async {
        (pdf_status(&app, &cookie, test, document).await == "PDF ready (the Original is a PDF)").then_some(())
    })
    .await;
    assert_eq!(download(&app, &pdf_path).await.1, new_pdf);
    assert_eq!(download(&app, &original_path).await.1, new_pdf);
    let response = app.get(&original_path).await;
    assert!(disposition(&response).name.ends_with(" - Test.pdf"));
    // A PDF Original has no small ".ext" link.
    let page = app.get_page("/").await;
    assert!(page.select(&format!("tr#T{test} a.original")).is_empty());
    assert!(!app.stored_keys().await.contains(&format!("documents/{document}/pdf")));

    // A file of a type that isn't allowed changes nothing.
    let entries = app.change_log().await.len();
    let response = replace_original(&app, Some(&cookie), &token, test, document, "notes.txt", b"hello").await;
    assert_eq!(response.status(), 415);
    let refusal = Page::parse(&response.text().await.unwrap());
    assert!(refusal.text("#refusal").contains("notes.txt"));
    assert_eq!(download(&app, &original_path).await.1, new_pdf);
    assert_eq!(app.change_log().await.len(), entries);
}

// --------------------------------------------------------- Also on Tests…

#[tokio::test]
async fn a_document_attached_to_a_second_test_shows_on_both_with_each_tests_id() {
    let app = TestApp::start().await;
    let first = app.insert_test(&NewTest::known()).await;
    let second = app
        .insert_test(&NewTest {
            division_level: Some("Precalculus/Alpha"),
            ..NewTest::known()
        })
        .await;
    let pdf = sample_pdf(1_000);
    let key = app.insert_document(&[first], NewDocument::pdf("Answers", &pdf)).await;
    let cookie = app.admin().await;

    let section = format!("section#D{key}");
    let page = edit_page(&app, &cookie, first).await;
    let detach = page.one(&format!("#D{key}-on-T{first} button"));
    assert!(
        detach.value().attr("disabled").is_some(),
        "its only Test can't be detached"
    );
    let form = page
        .post_form(&format!("{section} form.also-on"))
        .fill("attach", &format!("T{second}"))
        .press("button[value=attach]");
    assert_saved(submit(&app, &cookie, form).await, first).await;

    assert_eq!(public_documents(&app, first).await, ["Answers"]);
    assert_eq!(public_documents(&app, second).await, ["Answers"]);
    for (test, division) in [(first, "Calculus-Mu"), (second, "Precalculus-Alpha")] {
        let response = app.get(&format!("/tests/{test}/documents/{key}/pdf")).await;
        assert_eq!(
            disposition(&response).name,
            format!("T{test} D{key} 1993 February Invitational - Plant City - {division} Individual - Answers.pdf")
        );
    }
    let page = edit_page(&app, &cookie, second).await;
    assert_eq!(page.select(&format!("#D{key}-on-T{first}")).len(), 1);
    assert_eq!(page.select(&format!("#D{key}-on-T{second}")).len(), 1);

    // Detach it from the first Test.
    let form = page
        .post_form(&format!("{section} form.also-on"))
        .press(&format!("button[value='detach:{first}']"));
    assert_saved(submit(&app, &cookie, form).await, second).await;
    assert!(public_documents(&app, first).await.is_empty());
    assert_eq!(public_documents(&app, second).await, ["Answers"]);

    // Its last Test can't be detached, even by a hand-made post.
    let token = csrf(&app, &cookie, second).await;
    let (second_id, key_id, detach) = (second.to_string(), key.to_string(), format!("detach:{second}"));
    let response = app
        .post_form(
            "/api/admin/documents/tests",
            &[
                ("csrf", &token),
                ("test", &second_id),
                ("document", &key_id),
                ("action", &detach),
            ],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(response.status(), 409);
    assert_eq!(public_documents(&app, second).await, ["Answers"]);

    // A Test that doesn't exist can't be attached.
    let page = edit_page(&app, &cookie, second).await;
    let form = page
        .post_form(&format!("{section} form.also-on"))
        .fill("attach", "T999999")
        .press("button[value=attach]");
    assert_eq!(submit(&app, &cookie, form).await.status(), 404);

    assert_eq!(
        logged(&app, "document", key).await,
        [
            entry(
                "tests",
                Some(&format!("T{first}")),
                Some(&format!("T{first}, T{second}"))
            ),
            entry(
                "tests",
                Some(&format!("T{first}, T{second}")),
                Some(&format!("T{second}"))
            ),
        ]
    );
}

// ------------------------------------------------------------ + Add files

#[tokio::test]
async fn add_files_adds_documents_by_add_tests_rules() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let first = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let token = csrf(&app, &cookie, test).await;
    let page = edit_page(&app, &cookie, test).await;
    assert_eq!(page.select("form#add-files input[type=file][name=files]").len(), 1);

    let docx = sample_docx(2_000);
    let solutions_pdf = sample_pdf(3_000);
    let answers = sample_pdf(4_000);
    let response = add_files(
        &app,
        Some(&cookie),
        &token,
        test,
        &[
            ("S_Calc.docx", &docx),
            ("S_Calc.pdf", &solutions_pdf),
            ("calc key.pdf", &answers),
        ],
        &[("label_2", "Part 2")],
        "publish",
    )
    .await;
    assert_saved(response, test).await;

    let page = edit_page(&app, &cookie, test).await;
    let sections = page.select("section.document");
    assert_eq!(sections.len(), 3);
    let kinds: Vec<String> = sections.iter().map(|s| text_in(*s, "h4")).collect();
    assert_eq!(
        kinds[1..],
        [
            format!("D{} Solutions", first + 1),
            format!("D{} Answers (Part 2)", first + 2)
        ]
    );
    assert_eq!(text_in(sections[1], ".pdf-status"), "PDF ready");
    assert_eq!(text_in(sections[1], ".label-source"), "labels by an Admin");
    assert_eq!(
        public_documents(&app, test).await,
        // In kind display order, as the results always list them.
        ["Test", "Answers (Part 2)", "Solutions"]
    );
    let (_, supplied) = download(&app, &format!("/tests/{test}/documents/{}/pdf", first + 1)).await;
    assert_eq!(supplied, solutions_pdf);
    let created: Vec<i64> = app
        .change_log()
        .await
        .into_iter()
        .filter(|(e, _, f, _, _)| e == "document" && f == "id")
        .map(|(_, id, _, _, _)| id)
        .collect();
    assert_eq!(created, [first + 1, first + 2]);

    // A file of a type that isn't allowed refuses the whole upload.
    let keys = app.stored_keys().await;
    let response = add_files(
        &app,
        Some(&cookie),
        &token,
        test,
        &[("more.pdf", &pdf), ("virus.exe", b"MZ")],
        &[],
        "publish",
    )
    .await;
    assert_eq!(response.status(), 415);
    let refusal = Page::parse(&response.text().await.unwrap());
    assert_eq!(refusal.one("a").value().attr("href"), Some(edit_path(test).as_str()));
    assert_eq!(app.stored_keys().await, keys);
    assert_eq!(edit_page(&app, &cookie, test).await.select("section.document").len(), 3);
}
