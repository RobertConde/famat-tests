//! **Staged uploads** (issue #48): with JavaScript, Add test, **+ Add
//! files** and **Replace original** upload each file in its own request to
//! the staging endpoint, then post the form with the staged files'
//! references. What that saves must be exactly what a direct upload saves,
//! and staged files nobody saves are swept.

use reqwest::multipart::Form as Multipart;

use crate::support::{
    TestApp,
    add_test::{AddTestForm, added_test},
    fixtures::{NewDocument, NewTest, sample_docx, sample_pdf},
    html::Page,
    staging::{File, STAGE},
};

const EVIL: &str = "https://evil.example";
const MB: u64 = 1_000_000;
const ALLOWED: &str = "only PDF, DOC, DOCX, PPT, PPTX, RTF, ODT and ZIP files are allowed";
const GONE: &str =
    "A file uploaded for this form was already saved, or waited more than a day. Choose the files again.";

fn refusal(body: &str) -> String {
    Page::parse(body).text("#refusal")
}

/// The keys of staged objects among this test's.
async fn staged_keys(app: &TestApp) -> Vec<String> {
    app.stored_keys()
        .await
        .into_iter()
        .filter(|k| k.starts_with("staged/"))
        .collect()
}

/// A CSRF token of the Admin's session.
async fn csrf(app: &TestApp, cookie: &str) -> String {
    app.csrf_on("/logout", Some(cookie)).await
}

// -------------------------------------------------------- the endpoint

#[tokio::test]
async fn staging_needs_an_admin_session_a_csrf_token_and_this_sites_origin() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let other = app.log_in().await;
    let token = csrf(&app, &cookie).await;
    let foreign = csrf(&app, &other).await;
    let pdf = sample_pdf(2000);
    let base = app.base_url.as_str();

    let anonymous = app.stage_as(None, base, &token, "T_x.pdf", File::Bytes(&pdf)).await;
    assert_eq!(anonymous.status(), 401);
    let foreign = app
        .stage_as(Some(&cookie), base, &foreign, "T_x.pdf", File::Bytes(&pdf))
        .await;
    assert_eq!(foreign.status(), 403);
    let no_token = app
        .stage_as(Some(&cookie), base, "", "T_x.pdf", File::Bytes(&pdf))
        .await;
    assert_eq!(no_token.status(), 403);
    let evil = app
        .stage_as(Some(&cookie), EVIL, &token, "T_x.pdf", File::Bytes(&pdf))
        .await;
    assert_eq!(evil.status(), 403, "another site's Origin");
    assert!(app.stored_keys().await.is_empty());
    assert!(app.staged_rows().await.is_empty());

    // The real thing.
    let answer = app.stage(&cookie, &token, "T_x.pdf", File::Bytes(&pdf)).await;
    let id = answer.id();
    assert_eq!(id.len(), 32, "{id}");
    assert_eq!(answer.json["name"], "T_x.pdf");
    assert_eq!(answer.json["byte_size"], 2000);
    assert_eq!(staged_keys(&app).await, [format!("staged/{id}")]);
    assert_eq!(
        app.stored_object(&format!("{}staged/{id}", app.key_prefix)).await,
        pdf,
        "stored as sent"
    );
    assert!(app.change_log().await.is_empty(), "staging saves nothing");
}

#[tokio::test]
async fn staging_refuses_other_types_and_files_over_90_mb_with_a_message() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let token = csrf(&app, &cookie).await;

    for name in ["setup.exe", "notes.txt", "README"] {
        let answer = app.stage(&cookie, &token, name, File::Bytes(b"MZ")).await;
        assert_eq!(answer.refusal(415), format!("“{name}” can’t be added: {ALLOWED}."));
    }

    let answer = app
        .stage(&cookie, &token, "T_Huge.pdf", File::Generated(b"%PDF-", 90 * MB + 1))
        .await;
    assert_eq!(
        answer.refusal(413),
        "“T_Huge.pdf” can’t be added: files can be at most 90 MB."
    );
    assert!(app.stored_keys().await.is_empty(), "nothing is kept of a refused file");
    assert!(app.staged_rows().await.is_empty());

    // A post with no file.
    let response = app
        .http
        .post(app.url(STAGE))
        .header("origin", &app.base_url)
        .header("cookie", &cookie)
        .multipart(Multipart::new().text("csrf", token.clone()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "Choose a file to upload.");

    let answer = app
        .stage(&cookie, &token, "T_Big.pdf", File::Generated(b"%PDF-", 90 * MB))
        .await;
    let id = answer.id();
    assert_eq!(answer.json["byte_size"], 90 * MB);
    assert_eq!(
        app.staged_rows().await,
        [(id, "T_Big.pdf".to_owned(), Some(90_000_000))]
    );
}

// ----------------------------------------------------------- committing

/// Run the same Add test twice, on two fresh servers: once posting the
/// files, as without JavaScript, once staging each and posting references,
/// as with it. Everything the two save must match.
#[tokio::test]
async fn a_staged_set_saves_the_same_documents_as_a_direct_upload() {
    let (team_pdf, team_docx) = (sample_pdf(30_000), sample_docx(31_000));
    let (sol_docx, sol_pdf) = (sample_docx(32_000), sample_pdf(33_000));
    let notes = sample_docx(5_000);
    let zip = sample_docx(6_000);
    let answers = sample_pdf(9 * 1024 * 1024 + 7);
    let files: [(&str, &[u8]); 7] = [
        // The PDF first: the Word file becomes the Original.
        ("Geometry Team.pdf", &team_pdf),
        ("Geometry Team.docx", &team_docx),
        // The Word file first, names differing in case.
        ("S_Geometry Team.docx", &sol_docx),
        ("s_geometry team.PDF", &sol_pdf),
        ("Geometry Notes.doc", &notes),
        ("Geometry figures.zip", &zip),
        // Over one 8 MiB part.
        ("Geometry key.pdf", &answers),
    ];
    let fields = [("kind_0", "Errata"), ("label_1", "author's PDF"), ("label_6", "Key")];

    let direct = TestApp::start().await;
    let staged = TestApp::start().await;
    let mut saved = Vec::new();
    for (app, staging) in [(&direct, false), (&staged, true)] {
        let cookie = app.admin().await;
        let mut form = app.add_test_form(&cookie).await.test(&NewTest::known());
        if staging {
            let token = csrf(app, &cookie).await;
            for (name, bytes) in files {
                let id = app.stage_bytes(&cookie, &token, name, bytes).await;
                form = form.staged(&id);
            }
            assert_eq!(staged_keys(app).await.len(), files.len());
        } else {
            for (name, bytes) in files {
                form = form.file(name, bytes);
            }
        }
        for (name, value) in fields {
            form = form.field(name, value);
        }
        let test = app.add_test(&cookie, form, "publish").await;
        let documents = app.saved_documents(&cookie, test).await;
        let mut downloads = Vec::new();
        for document in &documents {
            for role in ["original", "pdf"] {
                let response = app
                    .get(&format!("/tests/{test}/documents/{}/{role}", document.id))
                    .await;
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .map(|v| v.to_str().unwrap().to_owned());
                downloads.push((status, content_type, response.bytes().await.unwrap().to_vec()));
            }
        }
        let jobs: Vec<i64> = sqlx::query_scalar("SELECT document_id FROM pdf_jobs ORDER BY document_id")
            .fetch_all(&app.db)
            .await
            .unwrap();
        saved.push((
            test,
            documents,
            downloads,
            app.change_log().await,
            app.stored_keys().await,
            jobs,
        ));
    }

    let (direct_saved, staged_saved) = (&saved[0], &saved[1]);
    assert_eq!(direct_saved.0, staged_saved.0, "the same Test");
    assert_eq!(
        direct_saved.1, staged_saved.1,
        "the same Documents, kinds, labels and PDF statuses"
    );
    assert_eq!(direct_saved.1.len(), 5, "two Word+PDF pairs");
    assert!(direct_saved.2 == staged_saved.2, "the same downloads");
    assert_eq!(direct_saved.3, staged_saved.3, "the same change log");
    assert_eq!(direct_saved.4, staged_saved.4, "the same objects, none of them staged");
    assert_eq!(direct_saved.5, staged_saved.5, "the same conversions queued");
    assert_eq!(direct_saved.5.len(), 1, "only the lone Word file waits for a PDF");
    assert!(staged.staged_rows().await.is_empty());
}

#[tokio::test]
async fn a_staged_file_is_saved_once_and_a_refused_form_can_be_posted_again() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let token = csrf(&app, &cookie).await;
    let pdf = sample_pdf(2000);
    let id = app.stage_bytes(&cookie, &token, "T_Calculus.pdf", &pdf).await;

    // Refused (a dropdown left on Choose…): nothing is saved, and the file
    // stays staged for the same form.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .unchoose("host")
        .staged(&id);
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 400);
    assert_eq!(refusal(&response.text().await.unwrap()), "Choose the Host, or Unknown.");
    assert_eq!(app.stored_keys().await, [format!("staged/{id}")]);

    let form = app.add_test_form(&cookie).await.test(&NewTest::known()).staged(&id);
    let test = app.add_test(&cookie, form, "publish").await;
    let saved = app.saved_documents(&cookie, test).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].kind, "Test");
    let response = app.get(&format!("/tests/{test}/documents/{}/pdf", saved[0].id)).await;
    assert!(response.bytes().await.unwrap() == pdf);
    assert!(staged_keys(&app).await.is_empty(), "the staged copy is gone");
    assert!(app.staged_rows().await.is_empty());

    // Posting it again (a second click, a second tab) saves nothing more.
    let log = app.change_log().await;
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            year: Some(2001),
            ..NewTest::known()
        })
        .staged(&id);
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 409);
    assert_eq!(refusal(&response.text().await.unwrap()), GONE);
    assert_eq!(app.change_log().await, log);

    // Nor does an ID that was never staged, nor a file and a reference
    // whose second is bad: whatever was stored for the first is removed.
    let other = app.stage_bytes(&cookie, &token, "S_Calculus.pdf", &pdf).await;
    let keys = app.stored_keys().await;
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            year: Some(2002),
            ..NewTest::known()
        })
        .staged(&other)
        .staged("0123456789abcdef0123456789abcdef");
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 409);
    assert_eq!(app.stored_keys().await, keys, "only the staged file, still staged");
    assert_eq!(app.change_log().await, log);
}

#[tokio::test]
async fn a_staged_set_can_be_held_for_the_duplicate_warning_and_added_to_that_test() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let existing = app.insert_test(&NewTest::known()).await;
    let token = csrf(&app, &cookie).await;
    let (docx, pdf) = (sample_docx(3000), sample_pdf(2000));
    let word = app.stage_bytes(&cookie, &token, "S_Calc.docx", &docx).await;
    let its_pdf = app.stage_bytes(&cookie, &token, "S_Calc.pdf", &pdf).await;

    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .staged(&word)
        .staged(&its_pdf);
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 303);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.contains("pending="), "{location}");
    assert!(
        staged_keys(&app).await.is_empty(),
        "held under its Document, no longer staged"
    );

    let page = app.get_page_as(&location, Some(&cookie)).await;
    assert_eq!(page.text("ul.held"), "S_Calc.docx + S_Calc.pdf → Solutions");
    let pending = location.rsplit('=').next().unwrap();
    let csrf = app.csrf_on(&location, Some(&cookie)).await;
    let response = app
        .post_form(
            "/api/admin/uploads/finish",
            &[
                ("csrf", &csrf),
                ("pending", pending),
                ("action", &format!("add:{existing}")),
            ],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(added_test(response.headers()["location"].to_str().unwrap()), existing);
    let saved = app.saved_documents(&cookie, existing).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].pdf_status, "PDF supplied");
    let supplied = app
        .get(&format!("/tests/{existing}/documents/{}/pdf", saved[0].id))
        .await;
    assert!(supplied.bytes().await.unwrap() == pdf);
}

#[tokio::test]
async fn add_files_and_replace_original_take_staged_files_too() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let first_pdf = sample_pdf(1000);
    let first = app.insert_document(&[test], NewDocument::pdf("Test", &first_pdf)).await;
    let cookie = app.admin().await;
    let token = csrf(&app, &cookie).await;
    let post = |path: &'static str, form: Multipart| {
        app.http
            .post(app.url(path))
            .header("origin", &app.base_url)
            .header("cookie", &cookie)
            .multipart(form)
            .send()
    };

    // + Add files: a Word+PDF pair and a lone PDF, staged.
    let (docx, pdf, key) = (sample_docx(2000), sample_pdf(3000), sample_pdf(4000));
    let ids = [
        app.stage_bytes(&cookie, &token, "S_Calc.docx", &docx).await,
        app.stage_bytes(&cookie, &token, "S_Calc.pdf", &pdf).await,
        app.stage_bytes(&cookie, &token, "calc key.pdf", &key).await,
    ];
    let mut form = Multipart::new()
        .text("csrf", token.clone())
        .text("test", test.to_string());
    for id in &ids {
        form = form.text("staged", id.clone());
    }
    let form = form.text("label_2", "Part 2").text("save", "publish");
    let response = post("/api/admin/tests/add-files", form).await.unwrap();
    assert_eq!(response.status(), 303, "{}", response.text().await.unwrap());
    let (solutions, answers) = (first + 1, first + 2);
    for (document, role, bytes) in [
        (solutions, "original", &docx),
        (solutions, "pdf", &pdf),
        (answers, "pdf", &key),
    ] {
        let response = app.get(&format!("/tests/{test}/documents/{document}/{role}")).await;
        assert_eq!(response.status(), 200, "D{document} {role}");
        assert!(response.bytes().await.unwrap() == *bytes, "D{document} {role}");
    }
    let labels: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT d.id, k.name, d.label FROM documents d JOIN document_kinds k ON k.id = d.kind_id ORDER BY d.id",
    )
    .fetch_all(&app.db)
    .await
    .unwrap();
    assert_eq!(
        labels[1..],
        [
            (solutions, "Solutions".to_owned(), None),
            (answers, "Answers".to_owned(), Some("Part 2".to_owned()))
        ]
    );
    assert!(staged_keys(&app).await.is_empty());

    // Replace original: a staged file over the first Document's PDF Original.
    let better = sample_docx(5000);
    let id = app.stage_bytes(&cookie, &token, "Better.docx", &better).await;
    let replace = |id: String| {
        Multipart::new()
            .text("csrf", token.clone())
            .text("test", test.to_string())
            .text("document", first.to_string())
            .text("staged", id)
    };
    let response = post("/api/admin/documents/replace-original", replace(id.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), 303, "{}", response.text().await.unwrap());
    let original = app.get(&format!("/tests/{test}/documents/{first}/original")).await;
    assert_eq!(
        original.headers()["content-type"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert!(original.bytes().await.unwrap() == better);
    let status: String = sqlx::query_scalar("SELECT pdf_status::text FROM documents WHERE id = $1")
        .bind(first)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(status, "preparing", "queued for conversion afresh");
    let logged: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT after FROM change_log WHERE entity = 'document' AND entity_id = $1 AND field = 'original'",
    )
    .bind(first)
    .fetch_all(&app.db)
    .await
    .unwrap();
    assert_eq!(logged.len(), 1);
    assert!(
        logged[0]
            .as_deref()
            .unwrap()
            .starts_with("Better.docx (5000 bytes, sha256 "),
        "{logged:?}"
    );
    assert!(staged_keys(&app).await.is_empty());

    // Used once only.
    let response = post("/api/admin/documents/replace-original", replace(id))
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(refusal(&response.text().await.unwrap()), GONE);
}

// ------------------------------------------------------------ cleanup

#[tokio::test]
async fn staged_files_never_saved_are_swept_after_a_day() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let token = csrf(&app, &cookie).await;
    let pdf = sample_pdf(2000);
    let old = app.stage_bytes(&cookie, &token, "T_old.pdf", &pdf).await;
    let recent = app.stage_bytes(&cookie, &token, "T_recent.pdf", &pdf).await;
    app.age_staged(&old, 25).await;
    app.age_staged(&recent, 23).await;

    // Too old to save, even before the sweep has run.
    let form = app.add_test_form(&cookie).await.test(&NewTest::known()).staged(&old);
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 409);
    assert_eq!(refusal(&response.text().await.unwrap()), GONE);

    // That post swept it; the one under a day old stays.
    assert_eq!(staged_keys(&app).await, [format!("staged/{recent}")]);
    let rows: Vec<String> = app.staged_rows().await.into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(rows, std::slice::from_ref(&recent));

    // Staging sweeps too.
    app.age_staged(&recent, 25).await;
    let fresh = app.stage_bytes(&cookie, &token, "T_fresh.pdf", &pdf).await;
    assert_eq!(staged_keys(&app).await, [format!("staged/{fresh}")]);
    let rows: Vec<String> = app.staged_rows().await.into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(rows, [fresh]);
    assert!(app.change_log().await.is_empty());
}

// ------------------------------------------------------ without JavaScript

#[tokio::test]
async fn without_javascript_the_page_says_one_upload_holds_about_95_mb_and_small_batches_work() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    for (path, form) in [
        ("/admin/tests/new".to_owned(), "form#add-test"),
        (
            format!("/admin/tests/{}/edit", app.insert_test(&NewTest::known()).await),
            "form#add-files",
        ),
    ] {
        let page = app.get_page_as(&path, Some(&cookie)).await;
        let hint = page.text(&format!("{form} .hint"));
        assert!(hint.contains("up to 90 MB each"), "{hint}");
        assert!(hint.contains("at most about 95 MB in total"), "{hint}");
        assert_eq!(
            page.select(&format!("{form} input[type=file][name=files]")).len(),
            1,
            "{path}: the files are posted natively"
        );
    }

    // A small batch in one post, as before.
    let form: AddTestForm = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            year: Some(2001),
            ..NewTest::known()
        })
        .file("T_a.pdf", &sample_pdf(1000))
        .file("S_a.pdf", &sample_pdf(1100));
    let test = app.add_test(&cookie, form, "publish").await;
    assert_eq!(app.saved_kinds(&cookie, test).await, ["Test", "Solutions"]);
    assert!(staged_keys(&app).await.is_empty(), "nothing went through staging");
}
