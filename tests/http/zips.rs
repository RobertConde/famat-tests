//! Per-Test zips, streamed from `/tests/{id}/zip?group=…&format=…`:
//! `T{id} {Year} {Month} {Competition} - {Host} - {Division/Level} {Test type}[ - {Kind group}].zip`,
//! with entries named by the Document filename scheme.

use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::support::{
    TestApp,
    download::disposition,
    fixtures::{Month, NewDocument, NewTest, sample_docx, sample_pdf},
    zip::{entries, names},
};

/// The base of every download name of a `NewTest::known()` Test.
const KNOWN: &str = "1993 February Invitational - Plant City - Calculus-Mu Individual";

/// GET a zip that must exist: its name and bytes.
async fn zip(app: &TestApp, path: &str) -> (String, Vec<u8>) {
    let response = app.get(path).await;
    assert_eq!(response.status(), 200, "GET {path}");
    assert_eq!(response.headers()["content-type"], "application/zip", "GET {path}");
    let disposition = disposition(&response);
    assert_eq!(disposition.kind, "attachment", "GET {path}");
    (disposition.name, response.bytes().await.unwrap().to_vec())
}

/// One Document of the Test below: its kind and the `(extension, bytes)` of
/// the files each Format puts in the zip.
struct Expected {
    id: i64,
    kind: &'static str,
    pdf: Vec<(&'static str, Vec<u8>)>,
    original: Vec<(&'static str, Vec<u8>)>,
    both: Vec<(&'static str, Vec<u8>)>,
}

#[tokio::test]
async fn each_kind_group_and_format_zips_exactly_its_documents_under_their_download_names() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let test_pdf = sample_pdf(3_000);
    let answers_docx = sample_docx(4_000);
    let answers_pdf = sample_pdf(4_100);
    let solutions_docx = sample_docx(5_000);
    let solutions_pdf = sample_pdf(5_100);
    let combined_pdf = sample_pdf(6_000);
    let errata_pdf = sample_pdf(700);
    let other_zip = sample_docx(8_000);

    // Inserted in kind display order, which is the zip's order too.
    let mut documents = Vec::new();
    let id = app.insert_document(&[test], NewDocument::pdf("Test", &test_pdf)).await;
    documents.push(Expected {
        id,
        kind: "Test",
        pdf: vec![("pdf", test_pdf.clone())],
        original: vec![("pdf", test_pdf.clone())],
        both: vec![("pdf", test_pdf.clone())],
    });
    let id = app
        .insert_document(&[test], NewDocument::docx("Answers", &answers_docx, Some(&answers_pdf)))
        .await;
    documents.push(Expected {
        id,
        kind: "Answers",
        pdf: vec![("pdf", answers_pdf.clone())],
        original: vec![("docx", answers_docx.clone())],
        both: vec![("pdf", answers_pdf.clone()), ("docx", answers_docx.clone())],
    });
    let id = app
        .insert_document(
            &[test],
            NewDocument::docx("Solutions", &solutions_docx, Some(&solutions_pdf)),
        )
        .await;
    documents.push(Expected {
        id,
        kind: "Solutions",
        pdf: vec![("pdf", solutions_pdf.clone())],
        original: vec![("docx", solutions_docx.clone())],
        both: vec![("pdf", solutions_pdf.clone()), ("docx", solutions_docx.clone())],
    });
    let id = app
        .insert_document(&[test], NewDocument::pdf("Combined Test+Solutions", &combined_pdf))
        .await;
    documents.push(Expected {
        id,
        kind: "Combined Test+Solutions",
        pdf: vec![("pdf", combined_pdf.clone())],
        original: vec![("pdf", combined_pdf.clone())],
        both: vec![("pdf", combined_pdf.clone())],
    });
    let id = app
        .insert_document(&[test], NewDocument::pdf("Errata", &errata_pdf))
        .await;
    documents.push(Expected {
        id,
        kind: "Errata",
        pdf: vec![("pdf", errata_pdf.clone())],
        original: vec![("pdf", errata_pdf.clone())],
        both: vec![("pdf", errata_pdf.clone())],
    });
    // A ZIP has no PDF, so "PDF" falls back to it.
    let id = app
        .insert_document(&[test], NewDocument::zip("Other", &other_zip))
        .await;
    documents.push(Expected {
        id,
        kind: "Other",
        pdf: vec![("zip", other_zip.clone())],
        original: vec![("zip", other_zip.clone())],
        both: vec![("zip", other_zip.clone())],
    });

    let groups: [(&str, Option<&str>, &[&str]); 5] = [
        (
            "all",
            None,
            &[
                "Test",
                "Answers",
                "Solutions",
                "Combined Test+Solutions",
                "Errata",
                "Other",
            ],
        ),
        ("tests", Some("Tests"), &["Test", "Combined Test+Solutions"]),
        ("answers", Some("Answers"), &["Answers"]),
        (
            "solutions",
            Some("Solutions"),
            &["Solutions", "Combined Test+Solutions"],
        ),
        ("other", Some("Errata & other"), &["Errata", "Other"]),
    ];
    for (group, suffix, kinds) in groups {
        for format in ["pdf", "original", "both"] {
            let path = format!("/tests/{test}/zip?group={group}&format={format}");
            let (name, bytes) = zip(&app, &path).await;

            let expected_name = match suffix {
                Some(suffix) => format!("T{test} {KNOWN} - {suffix}.zip"),
                None => format!("T{test} {KNOWN}.zip"),
            };
            assert_eq!(name, expected_name, "GET {path}");

            let expected: Vec<(String, Vec<u8>)> = documents
                .iter()
                .filter(|d| kinds.contains(&d.kind))
                .flat_map(|d| {
                    let files = match format {
                        "pdf" => &d.pdf,
                        "original" => &d.original,
                        _ => &d.both,
                    };
                    files.iter().map(move |(extension, bytes)| {
                        (
                            format!("T{test} D{} {KNOWN} - {}.{extension}", d.id, d.kind),
                            bytes.clone(),
                        )
                    })
                })
                .collect();
            let actual = entries(&bytes);
            assert_eq!(
                actual.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
                expected.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>(),
                "GET {path}"
            );
            for (entry, (_, bytes)) in actual.iter().zip(&expected) {
                assert!(entry.bytes == *bytes, "GET {path}: {} has other bytes", entry.name);
                assert!(entry.stored, "GET {path}: {} is compressed", entry.name);
            }
        }
    }

    // The defaults are all Documents, as PDF.
    let (name, bytes) = zip(&app, &format!("/tests/{test}/zip")).await;
    assert_eq!(name, format!("T{test} {KNOWN}.zip"));
    let (_, all_as_pdf) = zip(&app, &format!("/tests/{test}/zip?group=all&format=pdf")).await;
    assert_eq!(names(&bytes), names(&all_as_pdf));
}

#[tokio::test]
async fn pdf_falls_back_to_the_original_when_there_is_no_ready_pdf() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let docx = sample_docx(2_000);
    let pdf = sample_pdf(2_000);
    let preparing = app
        .insert_document(&[test], NewDocument::docx("Test", &docx, None))
        .await;
    let failed = app
        .insert_document(
            &[test],
            NewDocument {
                pdf_status: "failed",
                ..NewDocument::docx("Answers", &docx, Some(&pdf))
            },
        )
        .await;
    let ready = app
        .insert_document(&[test], NewDocument::docx("Solutions", &docx, Some(&pdf)))
        .await;

    let name = |id: i64, kind: &str, extension: &str| format!("T{test} D{id} {KNOWN} - {kind}.{extension}");
    let (_, as_pdf) = zip(&app, &format!("/tests/{test}/zip?format=pdf")).await;
    assert_eq!(
        names(&as_pdf),
        [
            name(preparing, "Test", "docx"),
            name(failed, "Answers", "docx"),
            name(ready, "Solutions", "pdf"),
        ]
    );
    let (_, both) = zip(&app, &format!("/tests/{test}/zip?format=both")).await;
    assert_eq!(
        names(&both),
        [
            name(preparing, "Test", "docx"),
            name(failed, "Answers", "docx"),
            name(ready, "Solutions", "pdf"),
            name(ready, "Solutions", "docx"),
        ],
        "a PDF that is not ready is never zipped"
    );
}

#[tokio::test]
async fn hidden_or_trashed_documents_are_never_zipped() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let test = app.insert_test(&NewTest::known()).await;
    let visible = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    app.insert_document(
        &[test],
        NewDocument {
            published: false,
            ..NewDocument::docx("Solutions", &docx, Some(&pdf))
        },
    )
    .await;
    let trashed = app
        .insert_document(&[test], NewDocument::docx("Answers", &docx, Some(&pdf)))
        .await;
    app.trash_document(trashed).await;

    for format in ["pdf", "original", "both"] {
        for group in ["all", "tests"] {
            let (_, bytes) = zip(&app, &format!("/tests/{test}/zip?group={group}&format={format}")).await;
            assert_eq!(
                names(&bytes),
                [format!("T{test} D{visible} {KNOWN} - Test.pdf")],
                "group={group}&format={format}"
            );
        }
        // Groups with only hidden or trashed Documents are empty.
        for group in ["answers", "solutions", "other"] {
            let path = format!("/tests/{test}/zip?group={group}&format={format}");
            assert_eq!(app.get(&path).await.status(), 404, "GET {path}");
        }
    }

    // Nor are the Documents of a Hidden or Trashed Test, or of no Test.
    let hidden_test = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    app.insert_document(&[hidden_test], NewDocument::pdf("Test", &pdf))
        .await;
    let trashed_test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[trashed_test], NewDocument::pdf("Test", &pdf))
        .await;
    app.trash_test(trashed_test).await;
    let only_hidden = app.insert_test(&NewTest::known()).await;
    app.insert_document(
        &[only_hidden],
        NewDocument {
            published: false,
            ..NewDocument::pdf("Test", &pdf)
        },
    )
    .await;
    for test in [hidden_test, trashed_test, only_hidden, 999_999] {
        let path = format!("/tests/{test}/zip");
        let response = app.get(&path).await;
        assert_eq!(response.status(), 404, "GET {path}");
        assert!(response.headers().get("content-disposition").is_none(), "GET {path}");
    }
}

#[tokio::test]
async fn a_missing_first_object_is_a_404_and_a_missing_later_one_fails_the_download() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let missing_first = app.insert_test(&NewTest::known()).await;
    app.insert_document(
        &[missing_first],
        NewDocument {
            stored: false,
            ..NewDocument::pdf("Test", &pdf)
        },
    )
    .await;
    app.insert_document(&[missing_first], NewDocument::pdf("Answers", &pdf))
        .await;

    let response = app.get(&format!("/tests/{missing_first}/zip")).await;
    assert_eq!(response.status(), 404);
    assert!(response.headers().get("content-disposition").is_none());

    // Once the zip has started, a missing object cuts the download off rather
    // than ending a zip that silently lacks it.
    let missing_later = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[missing_later], NewDocument::pdf("Test", &pdf))
        .await;
    app.insert_document(
        &[missing_later],
        NewDocument {
            stored: false,
            ..NewDocument::pdf("Answers", &pdf)
        },
    )
    .await;
    let response = app.get(&format!("/tests/{missing_later}/zip")).await;
    assert_eq!(response.status(), 200);
    assert!(response.bytes().await.is_err(), "the download fails");
}

#[tokio::test]
async fn an_unknown_group_or_format_is_a_400() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(1_000)))
        .await;

    for query in ["group=everything", "format=docx", "group=Tests"] {
        let path = format!("/tests/{test}/zip?{query}");
        assert_eq!(app.get(&path).await.status(), 400, "GET {path}");
    }
}

#[tokio::test]
async fn zip_names_follow_the_download_name_rules() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let state = app
        .insert_test(&NewTest {
            year: Some(2003),
            month: Month::NotApplicable,
            competition: Some("State"),
            division_level: Some("Precalculus/Alpha"),
            test_type: Some("(Equations and Inequalities)"),
            host: Some("FAMAT"),
            published: true,
        })
        .await;
    let nothing_known = app
        .insert_test(&NewTest {
            year: None,
            month: Month::Unknown,
            competition: None,
            division_level: None,
            test_type: None,
            host: None,
            published: true,
        })
        .await;
    let non_ascii = app
        .insert_test(&NewTest {
            host: Some("Académie Doral"),
            ..NewTest::known()
        })
        .await;
    for test in [state, nothing_known, non_ascii] {
        app.insert_document(&[test], NewDocument::pdf("Solutions", &pdf)).await;
    }

    let cases = [
        (
            format!("/tests/{state}/zip?group=solutions"),
            format!("T{state} 2003 State - FAMAT - Precalculus-Alpha Equations and Inequalities - Solutions.zip"),
        ),
        (format!("/tests/{nothing_known}/zip"), format!("T{nothing_known}.zip")),
        (
            format!("/tests/{nothing_known}/zip?group=solutions"),
            format!("T{nothing_known} - Solutions.zip"),
        ),
        (
            format!("/tests/{non_ascii}/zip"),
            format!("T{non_ascii} 1993 February Invitational - Académie Doral - Calculus-Mu Individual.zip"),
        ),
    ];
    for (path, expected) in cases {
        assert_eq!(zip(&app, &path).await.0, expected, "GET {path}");
    }
    let response = app.get(&format!("/tests/{non_ascii}/zip")).await;
    assert!(disposition(&response).fallback.is_ascii());
}

#[tokio::test]
async fn a_document_on_several_tests_is_named_after_the_test_it_is_zipped_from() {
    let app = TestApp::start().await;
    let geometry = app
        .insert_test(&NewTest {
            division_level: Some("Geometry"),
            ..NewTest::known()
        })
        .await;
    let algebra = app
        .insert_test(&NewTest {
            division_level: Some("Algebra II"),
            ..NewTest::known()
        })
        .await;
    let master_key = app
        .insert_document(&[geometry, algebra], NewDocument::pdf("Answers", &sample_pdf(1_000)))
        .await;

    for (test, level) in [(geometry, "Geometry"), (algebra, "Algebra II")] {
        let (_, bytes) = zip(&app, &format!("/tests/{test}/zip")).await;
        assert_eq!(
            names(&bytes),
            [format!(
                "T{test} D{master_key} 1993 February Invitational - Plant City - {level} Individual - Answers.pdf"
            )]
        );
    }
}

#[tokio::test]
async fn entries_are_dated_when_their_files_were_stored_in_eastern_time() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let docx = sample_docx(1_000);
    let pdf = sample_pdf(1_000);
    let winter = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let summer = app
        .insert_document(&[test], NewDocument::docx("Solutions", &docx, Some(&pdf)))
        .await;
    app.set_stored_at(winter, "original", "2023-02-11 15:04:06+00").await;
    app.set_stored_at(summer, "pdf", "2023-07-04 16:30:00+00").await;
    app.set_stored_at(summer, "original", "2021-11-20 23:59:58+00").await;

    let (_, bytes) = zip(&app, &format!("/tests/{test}/zip?format=both")).await;
    let dates: Vec<_> = entries(&bytes).into_iter().map(|e| (e.name, e.modified)).collect();
    assert_eq!(
        dates,
        [
            (format!("T{test} D{winter} {KNOWN} - Test.pdf"), (2023, 2, 11, 10, 4, 6)),
            (
                format!("T{test} D{summer} {KNOWN} - Solutions.pdf"),
                (2023, 7, 4, 12, 30, 0)
            ),
            (
                format!("T{test} D{summer} {KNOWN} - Solutions.docx"),
                (2021, 11, 20, 18, 59, 58)
            ),
        ]
    );
}

/// The server's resident memory, in KiB.
fn rss_kib(pid: u32) -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0)
}

#[tokio::test]
async fn memory_stays_flat_while_a_zip_of_large_objects_streams() {
    const MIB: usize = 1024 * 1024;
    const OBJECT: usize = 64 * MIB;
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    // Repeated rather than generated, so the fixture is quick to make.
    let first = sample_pdf(MIB).repeat(OBJECT / MIB);
    let second = sample_docx(MIB).repeat(OBJECT / MIB);
    app.insert_document(&[test], NewDocument::pdf("Test", &first)).await;
    app.insert_document(&[test], NewDocument::zip("Other", &second)).await;
    drop((first, second));

    // Warm up on a small zip, so the baseline includes whatever any zip
    // allocates once but none of the large objects.
    let small = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[small], NewDocument::pdf("Test", &sample_pdf(100_000)))
        .await;
    let warm_up = app.get(&format!("/tests/{small}/zip")).await;
    assert_eq!(warm_up.status(), 200);
    warm_up.bytes().await.unwrap();

    let pid = app.server_pid();
    let baseline = rss_kib(pid);
    let done = Arc::new(AtomicBool::new(false));
    let sampler = {
        let done = done.clone();
        thread::spawn(move || {
            let mut peak = 0;
            while !done.load(Ordering::Relaxed) {
                peak = peak.max(rss_kib(pid));
                thread::sleep(Duration::from_millis(20));
            }
            peak
        })
    };

    let mut response = app.get(&format!("/tests/{test}/zip")).await;
    assert_eq!(response.status(), 200);
    let mut received = 0;
    while let Some(chunk) = response.chunk().await.unwrap() {
        received += chunk.len();
    }
    done.store(true, Ordering::Relaxed);
    let peak = sampler.join().unwrap();

    assert!(received > 2 * OBJECT, "the whole zip arrived: {received} bytes");
    let growth_mib = peak.saturating_sub(baseline) / 1024;
    assert!(
        growth_mib < 32,
        "the server grew by {growth_mib} MiB (baseline {baseline} KiB, peak {peak} KiB) \
         while streaming {} MiB",
        received / MIB
    );
}
