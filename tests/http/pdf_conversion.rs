//! Real conversions through LibreOffice (slow, and LibreOffice must be
//! installed): the fixture `.doc`, `.docx` and `.pptx` files become valid
//! PDFs, a corrupt file fails with Needs review, and a conversion past the
//! timeout is killed with every process it started.
//!
//! Ignored by default; run them with `scripts/test.sh pdf_conversion --
//! --ignored`. `PDF_SOFFICE` picks another LibreOffice than `soffice`.

use std::{path::PathBuf, time::Duration};

use crate::support::{TestApp, fixtures::NewTest, html::text_in, pdf::eventually};

const WAIT: Duration = Duration::from_secs(240);

async fn start(env: &[(&str, &str)]) -> (TestApp, String) {
    let mut env = env.to_vec();
    env.push(("PDF_WORKERS", "2"));
    let app = TestApp::start_with(&env).await;
    let cookie = app.admin().await;
    (app, cookie)
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/pdf/{name}")).unwrap()
}

async fn add(app: &TestApp, cookie: &str, files: &[(&str, Vec<u8>)]) -> (i64, Vec<i64>) {
    let mut form = app.add_test_form(cookie).await.test(&NewTest::known());
    for (name, bytes) in files {
        form = form.file(name, bytes);
    }
    let test = app.add_test(cookie, form, "publish").await;
    let ids = app.saved_documents(cookie, test).await.iter().map(|d| d.id).collect();
    (test, ids)
}

async fn settled(app: &TestApp, cookie: &str, test: i64) -> Vec<String> {
    eventually(WAIT, "the conversions to finish", || async {
        let statuses: Vec<String> = app
            .saved_documents(cookie, test)
            .await
            .into_iter()
            .map(|d| d.pdf_status)
            .collect();
        (!statuses.iter().any(|s| s == "PDF being prepared")).then_some(statuses)
    })
    .await
}

#[tokio::test]
#[ignore = "slow: needs LibreOffice; run with `scripts/test.sh pdf_conversion -- --ignored`"]
async fn the_doc_docx_and_pptx_fixtures_convert_to_valid_pdfs() {
    let (app, cookie) = start(&[]).await;
    let files = ["sample.doc", "sample.docx", "sample.pptx"].map(|name| (name, fixture(name)));
    let (test, documents) = add(&app, &cookie, &files).await;

    assert_eq!(settled(&app, &cookie, test).await, ["PDF supplied"; 3]);
    let mut pages = Vec::new();
    for document in documents {
        let response = app.get(&format!("/tests/{test}/documents/{document}/pdf")).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/pdf");
        let pdf = response.bytes().await.unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        let parsed = lopdf::Document::load_mem(&pdf).expect("the PDF parses");
        pages.push(parsed.get_pages().len());
    }
    assert_eq!(pages, [1, 1, 2], "one page per Word file, one per slide");
}

#[tokio::test]
#[ignore = "slow: needs LibreOffice; run with `scripts/test.sh pdf_conversion -- --ignored`"]
async fn a_corrupt_word_file_fails_with_needs_review_and_its_error() {
    let (app, cookie) = start(&[]).await;
    let mut corrupt = b"PK\x03\x04".to_vec();
    corrupt.extend((0..4000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
    let (test, documents) = add(&app, &cookie, &[("Test.docx", corrupt)]).await;

    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed"]);
    let page = app.get_page_as("/admin/needs-review", Some(&cookie)).await;
    let comment = text_in(page.one(&format!("table#failed tr#D{}", documents[0])), "td.comment");
    assert!(comment.starts_with("PDF conversion failed: "), "{comment}");
}

#[tokio::test]
#[ignore = "slow: needs LibreOffice; run with `scripts/test.sh pdf_conversion -- --ignored`"]
async fn libreoffice_past_the_timeout_is_killed_with_every_process_it_started() {
    let work_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("pdf-work")
        .join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&work_dir).unwrap();
    let work = work_dir.display().to_string();
    let (app, cookie) = start(&[("PDF_TIMEOUT_SECS", "1"), ("PDF_WORK_DIR", &work)]).await;
    // 30,000 pages take LibreOffice far longer than a second, even on a
    // fast machine (3,000 take about 3 s on an M5).
    let mut long = String::from("{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Times New Roman;}}\\f0\\fs24\n");
    for i in 0..30_000 {
        long.push_str(&format!("Question {i}: if 2x + 3 = {i}, what is x?\\par\\page\n"));
    }
    long.push('}');
    let (test, documents) = add(&app, &cookie, &[("Test.rtf", long.into_bytes())]).await;

    assert_eq!(settled(&app, &cookie, test).await, ["PDF failed"]);
    let page = app.get_page_as("/admin/needs-review", Some(&cookie)).await;
    let comment = text_in(page.one(&format!("table#failed tr#D{}", documents[0])), "td.comment");
    assert_eq!(
        comment,
        "PDF conversion failed: LibreOffice took longer than 1 s and was stopped"
    );
    eventually(Duration::from_secs(10), "LibreOffice to be gone", || async {
        let ps = std::process::Command::new("ps")
            .args(["-axo", "stat=,command="])
            .output()
            .unwrap();
        let ps = String::from_utf8_lossy(&ps.stdout).into_owned();
        let left: Vec<&str> = ps
            .lines()
            .filter(|line| line.contains(&work) && !line.trim_start().starts_with('Z'))
            .collect();
        left.is_empty().then_some(())
    })
    .await;
    let leftovers: Vec<_> = std::fs::read_dir(&work_dir).unwrap().collect();
    assert!(leftovers.is_empty(), "job directories are removed: {leftovers:?}");
    let _ = std::fs::remove_dir_all(&work_dir);
}
