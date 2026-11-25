//! Download names, as `Content-Disposition` gives them:
//! `T{test id} D{doc id} {Year} {Month} {Competition} - {Host} - {Division/Level} {Test type} - {Document kind}[ ({label})].{ext}`

use crate::support::{
    TestApp,
    download::disposition,
    fixtures::{Month, NewDocument, NewTest, sample_docx, sample_pdf},
    html::links_in,
};

/// The name the PDF of a Document downloads as, from one of its Tests.
async fn pdf_name(app: &TestApp, test: i64, document: i64) -> String {
    download_name(app, &format!("/tests/{test}/documents/{document}/pdf")).await
}

/// The name the Original of a Document downloads as, from one of its Tests.
async fn original_name(app: &TestApp, test: i64, document: i64) -> String {
    download_name(app, &format!("/tests/{test}/documents/{document}/original")).await
}

async fn download_name(app: &TestApp, path: &str) -> String {
    let response = app.get(path).await;
    assert_eq!(response.status(), 200, "GET {path}");
    let disposition = disposition(&response);
    assert!(disposition.fallback.is_ascii(), "{disposition:?}");
    disposition.name
}

#[tokio::test]
async fn a_fully_known_test_names_every_part() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let word = app
        .insert_document(&[test], NewDocument::docx("Solutions", &docx, Some(&pdf)))
        .await;

    assert_eq!(
        pdf_name(&app, test, document).await,
        format!("T{test} D{document} 1993 February Invitational - Plant City - Calculus-Mu Individual - Test.pdf")
    );
    assert_eq!(
        pdf_name(&app, test, word).await,
        format!("T{test} D{word} 1993 February Invitational - Plant City - Calculus-Mu Individual - Solutions.pdf")
    );
    assert_eq!(
        original_name(&app, test, word).await,
        format!("T{test} D{word} 1993 February Invitational - Plant City - Calculus-Mu Individual - Solutions.docx")
    );
}

#[tokio::test]
async fn unknown_facets_are_left_out_with_their_separators() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let no_year_or_host = app
        .insert_test(&NewTest {
            year: None,
            host: None,
            ..NewTest::known()
        })
        .await;
    let no_month_or_test_type = app
        .insert_test(&NewTest {
            month: Month::Unknown,
            test_type: None,
            ..NewTest::known()
        })
        .await;
    let no_division_or_test_type = app
        .insert_test(&NewTest {
            division_level: None,
            test_type: None,
            ..NewTest::known()
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

    let cases = [
        (
            no_year_or_host,
            "February Invitational - Calculus-Mu Individual - Answers.pdf",
        ),
        (
            no_month_or_test_type,
            "1993 Invitational - Plant City - Calculus-Mu - Answers.pdf",
        ),
        (
            no_division_or_test_type,
            "1993 February Invitational - Plant City - Answers.pdf",
        ),
        (nothing_known, "- Answers.pdf"),
    ];
    for (test, rest) in cases {
        let document = app.insert_document(&[test], NewDocument::pdf("Answers", &pdf)).await;
        assert_eq!(
            pdf_name(&app, test, document).await,
            format!("T{test} D{document} {rest}")
        );
    }
}

#[tokio::test]
async fn a_state_test_has_no_month() {
    let app = TestApp::start().await;
    let test = app
        .insert_test(&NewTest {
            year: Some(2003),
            month: Month::NotApplicable,
            competition: Some("State"),
            division_level: Some("Precalculus/Alpha"),
            test_type: Some("Equations and Inequalities"),
            host: Some("FAMAT"),
            published: true,
        })
        .await;
    let document = app
        .insert_document(&[test], NewDocument::pdf("Solutions", &sample_pdf(1_000)))
        .await;

    assert_eq!(
        pdf_name(&app, test, document).await,
        format!(
            "T{test} D{document} 2003 State - FAMAT - Precalculus-Alpha Equations and Inequalities - Solutions.pdf"
        )
    );
}

#[tokio::test]
async fn a_label_follows_the_kind_in_parentheses() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let labelled = app
        .insert_document(
            &[test],
            NewDocument {
                label: Some("2 per page"),
                ..NewDocument::docx("Test", &docx, Some(&pdf))
            },
        )
        .await;

    let base = format!(
        "T{test} D{labelled} 1993 February Invitational - Plant City - Calculus-Mu Individual - Test (2 per page)"
    );
    assert_eq!(pdf_name(&app, test, labelled).await, format!("{base}.pdf"));
    assert_eq!(original_name(&app, test, labelled).await, format!("{base}.docx"));
}

#[tokio::test]
async fn test_type_parentheses_slashes_and_characters_windows_rejects_are_cleaned_up() {
    let app = TestApp::start().await;
    let test = app
        .insert_test(&NewTest {
            test_type: Some("(Individual)"),
            host: Some("Lake \"Brantley\": <High>? *|\\"),
            ..NewTest::known()
        })
        .await;
    let document = app
        .insert_document(
            &[test],
            NewDocument {
                label: Some("Part 1/2"),
                ..NewDocument::pdf("Test", &sample_pdf(1_000))
            },
        )
        .await;

    assert_eq!(
        pdf_name(&app, test, document).await,
        format!(
            "T{test} D{document} 1993 February Invitational - Lake Brantley High - Calculus-Mu Individual - Test (Part 1-2).pdf"
        )
    );
}

#[tokio::test]
async fn a_document_on_several_tests_takes_the_id_of_the_test_it_is_downloaded_from() {
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
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let master_key = app
        .insert_document(&[geometry, algebra], NewDocument::docx("Answers", &docx, Some(&pdf)))
        .await;

    // Each row links to the Document through its own Test.
    let page = app.get_page("/").await;
    for test in [geometry, algebra] {
        let hrefs: Vec<String> = links_in(page.one(&format!("tr#T{test}")))
            .into_iter()
            .map(|(_, href)| href)
            .collect();
        assert_eq!(
            hrefs,
            vec![
                format!("/tests/{test}/documents/{master_key}/pdf"),
                format!("/tests/{test}/documents/{master_key}/original"),
            ]
        );
    }

    assert_eq!(
        pdf_name(&app, geometry, master_key).await,
        format!(
            "T{geometry} D{master_key} 1993 February Invitational - Plant City - Geometry Individual - Answers.pdf"
        )
    );
    assert_eq!(
        original_name(&app, algebra, master_key).await,
        format!(
            "T{algebra} D{master_key} 1993 February Invitational - Plant City - Algebra II Individual - Answers.docx"
        )
    );
}

#[tokio::test]
async fn a_very_long_name_is_capped_without_losing_the_ids_kind_or_extension() {
    let app = TestApp::start().await;
    let host = "Lake Brantley High School Mathematics Department and Friends of the Seminole County Schools";
    let test_type = "Applications of Integration, Differential Equations, Sequences, Series and Polar Coordinates";
    let test = app
        .insert_test(&NewTest {
            host: Some(host),
            test_type: Some(test_type),
            ..NewTest::known()
        })
        .await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let document = app
        .insert_document(
            &[test],
            NewDocument {
                label: Some("Part 2"),
                ..NewDocument::docx("Combined Test+Solutions", &docx, Some(&pdf))
            },
        )
        .await;

    let pdf_name = pdf_name(&app, test, document).await;
    let original_name = original_name(&app, test, document).await;

    for (name, extension) in [(&pdf_name, "pdf"), (&original_name, "docx")] {
        assert!(
            name.chars().count() <= 150,
            "{} characters: {name}",
            name.chars().count()
        );
        assert!(name.chars().count() >= 140, "shortened no more than needed: {name}");
        assert!(
            name.starts_with(&format!(
                "T{test} D{document} 1993 February Invitational - Lake Brantley High"
            )),
            "{name}"
        );
        assert!(name.contains(" - Calculus-Mu Applications of Integration"), "{name}");
        assert!(
            name.ends_with(&format!(" - Combined Test+Solutions (Part 2).{extension}")),
            "{name}"
        );
    }
    assert_eq!(
        pdf_name.strip_suffix(".pdf"),
        original_name.strip_suffix(".docx"),
        "the PDF and its Original share the base name"
    );
}

#[tokio::test]
async fn names_are_made_at_download_time_so_a_renamed_facet_value_applies_at_once() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let document = app
        .insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(1_000)))
        .await;
    assert!(pdf_name(&app, test, document).await.contains(" - Plant City - "));

    app.rename_facet_value("host", "Plant City", "Plant City High").await;

    assert_eq!(
        pdf_name(&app, test, document).await,
        format!("T{test} D{document} 1993 February Invitational - Plant City High - Calculus-Mu Individual - Test.pdf")
    );
}

#[tokio::test]
async fn a_non_ascii_name_is_sent_as_rfc_5987_with_an_ascii_fallback() {
    let app = TestApp::start().await;
    let test = app
        .insert_test(&NewTest {
            host: Some("Académie Doral"),
            ..NewTest::known()
        })
        .await;
    let document = app
        .insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(1_000)))
        .await;

    let response = app.get(&format!("/tests/{test}/documents/{document}/pdf")).await;
    let disposition = disposition(&response);

    assert_eq!(
        disposition.name,
        format!("T{test} D{document} 1993 February Invitational - Académie Doral - Calculus-Mu Individual - Test.pdf")
    );
    assert_eq!(
        disposition.fallback,
        format!("T{test} D{document} 1993 February Invitational - Acad_mie Doral - Calculus-Mu Individual - Test.pdf")
    );
}
