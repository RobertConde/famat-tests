//! `famat import`'s special cases (#31), on the committed fixture set: the
//! Hidden duplicates, broken and non-test files, the Master Keys, Word+PDF
//! pairs, the unpacked bundle, the dropped shortcut and bundle zip, the
//! attach decisions, Needs review on import, the reconciliation report and
//! the PDF backfill. See `import.rs` for the fixture's Tests and Documents.

use crate::{
    import::{DOCUMENTS, HIDDEN_DOCUMENTS, SUPPLIED_PDFS},
    support::{TestApp, html::links_in, import::fixture},
};

/// A Test's Document links in the results: (text, href).
async fn links(app: &TestApp, test: i64) -> Vec<(String, String)> {
    let page = app.get_page("/").await;
    links_in(page.one(&format!("tr#T{test} td.documents")))
}

fn link(text: &str, href: &str) -> (String, String) {
    (text.to_owned(), href.to_owned())
}

/// The Hidden Documents and their notes, read from the table (there is no
/// Admin page for a Document's note yet).
async fn hidden_notes(app: &TestApp) -> Vec<(i64, Option<String>)> {
    sqlx::query_as("SELECT id, note FROM documents WHERE NOT published ORDER BY id")
        .fetch_all(&app.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn duplicates_broken_files_and_non_tests_are_imported_hidden_with_a_note() {
    let app = TestApp::start().await;
    app.import_fixtures();

    let notes = hidden_notes(&app).await;
    assert_eq!(
        notes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        HIDDEN_DOCUMENTS,
        "{notes:?}"
    );
    let note = |id: i64| notes.iter().find(|(d, _)| *d == id).unwrap().1.clone().unwrap();
    // From the AI's Duplicate Of, and from an attach decision.
    assert_eq!(note(1176), "duplicate of 1162");
    assert_eq!(note(3455), "duplicate of 5937");
    assert!(note(5617).starts_with("broken: OLE container corrupt"), "{notes:?}");
    assert!(
        note(6606).starts_with("not a test: sheet said Solutions: file is a blank answer form"),
        "{notes:?}"
    );

    // Visitors see neither the copy of the Master Key nor the duplicate.
    assert_eq!(
        links(&app, 1500).await,
        [
            link("Test", "/tests/1500/documents/1160/pdf"),
            link("Answers", "/tests/1500/documents/1162/pdf"),
        ]
    );
    assert_eq!(
        links(&app, 3100).await,
        [link("Test", "/tests/3100/documents/5937/pdf")]
    );
    for (test, document) in [(1500, 1176), (3100, 3455)] {
        for file in ["pdf", "original"] {
            let response = app.get(&format!("/tests/{test}/documents/{document}/{file}")).await;
            assert_eq!(response.status(), 404, "D{document}'s {file}");
        }
    }
    // A Test whose only Document is Hidden fails the publish rule.
    assert!(!app.search_ids("/").await.contains(&3150));

    // Admins see them, Hidden, on their Tests.
    let cookie = app.admin().await;
    let visibility: Vec<(i64, String)> = app
        .saved_documents(&cookie, 1500)
        .await
        .into_iter()
        .map(|d| (d.id, d.visibility))
        .collect();
    assert_eq!(
        visibility,
        [
            (1160, "Published".to_owned()),
            (1162, "Published".to_owned()),
            (1176, "Hidden".to_owned())
        ]
    );
}

#[tokio::test]
async fn a_master_key_is_one_document_on_each_of_its_tests() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // The unpacked Hustle answer key covers the Geometry and Algebra II
    // Hustles: one Document on both Tests.
    assert_eq!(app.search_ids("/?test_type=Hustle").await, [3714, 3713]);
    assert_eq!(
        links(&app, 3714).await,
        [
            link("Test", "/tests/3714/documents/10372/pdf"),
            link(".doc", "/tests/3714/documents/10372/original"),
            link("Answers", "/tests/3714/documents/10364/pdf"),
            link(".doc", "/tests/3714/documents/10364/original"),
        ]
    );
    for test in [3713, 3714] {
        let (_, bytes) = app.download_original(test, 10364).await;
        assert!(bytes == fixture("unpacked/8053/Hustle/2013 State Hustle Answers.doc"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = 10364")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn a_word_and_pdf_pair_is_one_document_with_a_supplied_pdf() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // 3623 is the PDF of the Word Original 5642 (an attach-check), and
    // 10365's PDF came with it in its bundle.
    assert_eq!(
        links(&app, 2600).await,
        [
            link("Solutions", "/tests/2600/documents/5642/pdf"),
            link(".doc", "/tests/2600/documents/5642/original"),
        ]
    );
    for ((test, document), (_, pdf)) in [(2600, 5642), (3713, 10365)].into_iter().zip(SUPPLIED_PDFS) {
        let response = app.get(&format!("/tests/{test}/documents/{document}/pdf")).await;
        assert_eq!(response.status(), 200, "D{document}'s PDF");
        assert_eq!(response.headers()["content-type"], "application/pdf");
        assert!(
            response.bytes().await.unwrap() == fixture(pdf),
            "D{document}'s PDF is {pdf}"
        );
        let original = DOCUMENTS.iter().find(|(id, _, _)| *id == document).unwrap().2;
        assert!(app.download_original(test, document).await.1 == fixture(original));
    }
    // The PDF is no Document of its own.
    for path in ["/documents/3623/pdf", "/documents/3623/original"] {
        assert_eq!(app.get(path).await.status(), 404, "{path}");
    }
    let cookie = app.admin().await;
    let documents = app.saved_documents(&cookie, 2600).await;
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].pdf_status, "PDF supplied");
}

#[tokio::test]
async fn a_bundle_is_imported_as_its_documents_and_the_zip_is_not() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // 8053's old Test is succeeded by the Tests of its Documents.
    assert!(!app.search_ids("/").await.contains(&2700));
    for path in ["/documents/8053/pdf", "/documents/8053/original"] {
        assert_eq!(app.get(path).await.status(), 404, "{path}");
    }
    assert_eq!(
        links(&app, 3713).await,
        [
            link("Test", "/tests/3713/documents/10365/pdf"),
            link(".docx", "/tests/3713/documents/10365/original"),
            link("Answers", "/tests/3713/documents/10364/pdf"),
            link(".doc", "/tests/3713/documents/10364/original"),
        ]
    );
    let (name, _) = app.download_original(3713, 10365).await;
    assert_eq!(name, "T3713 D10365 2013 State - FAMAT - Algebra II Hustle - Test.docx");

    // 6293 is no bundle: it stays a zip, with no PDF.
    let (name, bytes) = app.download_original(3160, 6293).await;
    assert_eq!(
        name,
        "T3160 D6293 State - FAMAT - Open Computer Programming - Other.zip"
    );
    assert!(bytes == fixture("docs/6293 S_Solutions.zip"));
    let status: String = sqlx::query_scalar("SELECT pdf_status::text FROM documents WHERE id = 6293")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(status, "not_convertible");
}

#[tokio::test]
async fn attach_decisions_put_documents_on_the_right_tests() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // attach: 3415's own labels had no Division; it joins 3414's Test.
    assert_eq!(app.search_ids("/?test_type=Gemini").await, [2800]);
    assert_eq!(
        links(&app, 2800).await,
        [
            link("Test", "/tests/2800/documents/3415/pdf"),
            link("Solutions", "/tests/2800/documents/3414/pdf"),
        ]
    );
    // new test: 3915 and 3916 have the old T2900's labels, but are a Test of
    // their own.
    assert_eq!(
        app.search_ids("/?test_type=Mu+Applications&year=1998").await,
        [2900, 3709]
    );
    assert_eq!(
        links(&app, 2900).await,
        [link("Solutions", "/tests/2900/documents/6510/pdf")]
    );
    assert_eq!(
        links(&app, 3709).await,
        [
            link("Test", "/tests/3709/documents/3916/pdf"),
            link("Solutions", "/tests/3709/documents/3915/pdf"),
        ]
    );
}

#[tokio::test]
async fn flagged_documents_need_review_with_their_reasons() {
    let app = TestApp::start().await;
    app.import_fixtures();

    let flagged: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, needs_review_comment FROM documents WHERE needs_review ORDER BY id")
            .fetch_all(&app.db)
            .await
            .unwrap();
    let mu_test = "Attach-check: a 1998 \"Mu Test\" pair (3915 Solutions, 3916 Test), not Applications.";
    let expected = [
        (768, "AI labels flagged: AI changed sheet: File Type 'Solutions'→'Test'"),
        (
            3907,
            "Imported as Calculus Applications: neither the file name, the AI evidence nor the file says whether \
             this is Calculus Applications or Mu Applications.",
        ),
        (3915, mu_test),
        (3916, mu_test),
        (5617, "AI labels flagged: low confidence | broken"),
        (6510, "Attach-check: page 1 is a 1997-titled grid inside a 1998 file."),
        (10372, "AI labels: needs review: no Division printed (folder Geo)"),
    ];
    assert_eq!(flagged, expected.map(|(id, comment)| (id, comment.to_owned())),);
}

#[tokio::test]
async fn after_import_the_queue_holds_a_job_for_each_word_and_powerpoint_original() {
    let app = TestApp::start().await;
    let summary = app.import_fixtures();

    let jobs: Vec<i64> = sqlx::query_scalar("SELECT document_id FROM pdf_jobs ORDER BY document_id")
        .fetch_all(&app.db)
        .await
        .unwrap();
    // Every Word Original without a supplied PDF (5642 and 10365 have one);
    // not the zip, nor any PDF.
    assert_eq!(jobs, [3910, 5617, 9800, 10364, 10372]);
    let preparing: Vec<i64> = sqlx::query_scalar("SELECT id FROM documents WHERE pdf_status = 'preparing' ORDER BY id")
        .fetch_all(&app.db)
        .await
        .unwrap();
    assert_eq!(preparing, jobs);
    assert!(
        summary.contains("  PDF conversions: 5 queued, one for each of the 5 Originals being prepared."),
        "{summary}"
    );
}

#[tokio::test]
async fn the_report_accounts_for_every_fixture_file_and_flags_the_missing_one() {
    let app = TestApp::start().await;
    let run = app.import_fixtures_run();

    // Every file of the docs export and of the unpacked directory has its
    // line, saying what became of it.
    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import");
    let mut names = Vec::new();
    for entry in std::fs::read_dir(fixtures.join("docs")).unwrap() {
        names.push(entry.unwrap().file_name().to_string_lossy().into_owned());
    }
    let mut pending = vec![fixtures.join("unpacked")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(&fixtures).unwrap().to_string_lossy().into_owned();
                names.push(relative);
            }
        }
    }
    assert_eq!(names.len(), 52);
    let section = run
        .report
        .split("\nSource files (52):\n")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .unwrap_or_else(|| panic!("no Source files section in:\n{}", run.report));
    for name in &names {
        let lines = section
            .lines()
            .filter(|l| l.starts_with(&format!("  {name}: ")))
            .count();
        assert_eq!(lines, 1, "{name} has {lines} lines in:\n{section}");
    }
    assert_eq!(section.lines().count(), 52);

    for line in [
        "  1176 1989 Answers.pdf: imported Hidden: D1176 on T1500; duplicate of 1162",
        "  1162 1989 Answers.pdf: imported: D1162 on T1500, T1501, T3708",
        "  3623 2005 Solutions.pdf: imported as a PDF: the supplied PDF of D5642: tid 3623 is its Original as a PDF",
        "  unpacked/8053/Hustle/Algebra/Algebra 2 Hustle MAO States 2013.pdf: imported as a PDF: the supplied \
         PDF of D10365, which it came with",
        "  unpacked/8053/Hustle/2013 State Hustle Answers.doc: imported: D10364 on T3713, T3714",
        "  8053 T_Hustle.zip: dropped: a bundle zip, not imported itself: the 3 Documents in it are imported as \
         D10364, D10365 and D10372",
        "  1769 Open Tests.lnk: dropped: a Windows shortcut (.lnk), not a Document",
        "  .htaccess: dropped: not a Document: the name does not start with a tid",
        "  unpacked/8053/Hustle/~$13 State Hustle Answers.doc: FAILED: in the unpacked directory, but \
         unpacked.csv does not list it",
        // Every old Test, with where its Documents went.
        "  T1: kept",
        "  T3: kept; D770 is on T3707",
        "  T1500: kept; D1176 is Hidden (duplicate of 1162)",
        "  T2700: not kept; succeeded by T3713, T3714; D8053 was dropped: a bundle zip, not imported itself: the \
         3 Documents in it are imported as D10364, D10365 and D10372",
        "  D3415: attach to the Test of D3414: moved onto the Test of D3414; its own labels said 2001 State · \
         FAMAT · — Gemini (why: same header 'GEMINI / FAMAT State Convention 2001'; questions match 3414's \
         solutions)",
    ] {
        assert!(
            run.report.lines().any(|l| l == line),
            "no line {line:?} in:\n{}",
            run.report
        );
    }

    // The summary, on stdout too: counts, the checksums, and the problems,
    // among them the deliberately missing 5618.
    for line in [
        "  Source files: 52 — 42 imported, 4 imported Hidden, 2 imported as supplied PDFs, 3 dropped, 1 failed, \
         0 not accounted for.",
        "  Hidden Documents: 4 — duplicates 2, broken 1, not a test 1.",
        "  Old Tests: 28 — 23 kept with all their Documents, 3 kept with some Documents elsewhere, 1 not kept \
         (their Documents' Tests succeed them), 1 with Documents missing.",
        "  Garage: 48 of 48 objects stored and read back, each matching its source's size and SHA-256; Garage \
         holds 48 of them and 0 objects else.",
        "  Problems (4):",
        "    MISSING File Number 5618: labelled, but there is no such file in the docs export",
        "    T1888: INCOMPLETE; D5617 is Hidden (broken: OLE container corrupt (98% zero bytes); textutil and \
         LibreOffice give only garbage); D5618 is missing: labelled, but there is no such file in the docs \
         export",
        "    review-core.csv flags D5618 (broken), which is not imported",
    ] {
        assert!(
            run.stdout.lines().any(|l| l == line),
            "no line {line:?} in:\n{}",
            run.stdout
        );
        assert!(
            run.report.lines().any(|l| l == line),
            "no line {line:?} in:\n{}",
            run.report
        );
    }
}
