//! **Old URLs**: links and bookmarks into the old PHP site redirect to the
//! same Document or search here.

use crate::support::{
    TestApp,
    fixtures::{Month, NewDocument, NewTest, sample_docx, sample_pdf},
};

/// GET `path` and assert it answers `status` with `location`.
async fn assert_redirect(app: &TestApp, path: &str, status: u16, location: &str) {
    let response = app.get(path).await;
    assert_eq!(response.status(), status, "GET {path}");
    assert_eq!(response.headers()["location"], location, "GET {path}");
}

/// A Published Test of 1999 January Invitational, Berkeley Prep, Algebra I
/// Individual, with one Published Word Document. Returns `(test, document)`.
async fn berkeley_prep(app: &TestApp) -> (i64, i64) {
    let test = app
        .insert_test(&NewTest {
            year: Some(1999),
            month: Month::Known("January"),
            competition: Some("Invitational"),
            division_level: Some("Algebra I"),
            test_type: Some("Individual"),
            host: Some("Berkeley Prep"),
            published: true,
        })
        .await;
    let document = app
        .insert_document(
            &[test],
            NewDocument::docx("Test", &sample_docx(1_000), Some(&sample_pdf(1_000))),
        )
        .await;
    (test, document)
}

/// Give a Facet value a former name, as an Admin's rename leaves one.
async fn add_alias(app: &TestApp, facet: &str, name: &str, alias: &str) {
    let id = app.facet_value(facet, name).await;
    sqlx::query("INSERT INTO facet_value_aliases (facet, alias, facet_value_id) VALUES ($1::facet, $2, $3)")
        .bind(facet)
        .bind(alias)
        .bind(id)
        .execute(&app.db)
        .await
        .expect("insert alias");
}

#[tokio::test]
async fn an_old_download_link_redirects_to_the_documents_pdf() {
    let app = TestApp::start().await;
    let (test, document) = berkeley_prep(&app).await;

    let old = format!("/scripts/download.php?tid={document}");
    assert_redirect(&app, &old, 301, &format!("/documents/{document}/pdf")).await;
    // …which then picks the Test.
    assert_redirect(
        &app,
        &format!("/documents/{document}/pdf"),
        307,
        &format!("/tests/{test}/documents/{document}/pdf"),
    )
    .await;
}

#[tokio::test]
async fn an_old_download_link_to_a_missing_or_hidden_document_redirects_to_the_search() {
    let app = TestApp::start().await;
    let (test, document) = berkeley_prep(&app).await;
    let hidden = app
        .insert_document(
            &[test],
            NewDocument {
                published: false,
                ..NewDocument::pdf("Answers", &sample_pdf(1_000))
            },
        )
        .await;

    // It might be published later, so the redirect is not permanent.
    for tid in [hidden, document + 1000] {
        assert_redirect(&app, &format!("/scripts/download.php?tid={tid}"), 302, "/").await;
    }
    for query in ["", "?tid=", "?tid=abc", "?tid=-3", "?tid=%ZZ"] {
        assert_redirect(&app, &format!("/scripts/download.php{query}"), 301, "/").await;
    }
}

#[tokio::test]
async fn an_old_docs_file_redirects_to_the_documents_original() {
    let app = TestApp::start().await;
    let (_, document) = berkeley_prep(&app).await;

    // As the old directory listing linked it, spaces encoded.
    let old = format!("/docs/{document}%20Berkeley%20Prep%20Algebra%20I%20(Individual)%20Test.doc");
    assert_redirect(&app, &old, 301, &format!("/documents/{document}/original")).await;

    assert_redirect(&app, &format!("/docs/{}%20Gone.pdf", document + 1000), 302, "/").await;
    for path in ["/docs", "/docs/", "/docs/?C=M;O=A", "/docs/style.css", "/docs/%20x"] {
        assert_redirect(&app, path, 301, "/").await;
    }
}

#[tokio::test]
async fn an_old_search_redirects_to_the_same_search_with_values_translated() {
    let app = TestApp::start().await;
    let (test, _) = berkeley_prep(&app).await;

    let old = "/index.php?year=1999&category=Invitationals+-+January&level=Algebra+I\
               &style=%28Individual%29&host=Berkeley+Prep&submit=Search";
    let new = "/?year=1999&month=January&competition=Invitational&division_level=Algebra+I\
               &test_type=Individual&host=Berkeley+Prep";
    assert_redirect(&app, old, 301, new).await;

    let page = app.get_page(new).await;
    assert_eq!(
        page.select(&format!("tr#T{test}")).len(),
        1,
        "the new search finds the Test"
    );

    // The old form also submitted to `/` when it was shown there.
    assert_redirect(&app, &old.replace("/index.php", "/"), 301, new).await;
    // Fields left on their blank option are absent; an empty year is Any.
    assert_redirect(
        &app,
        "/?year=&category=Invitationals+-+January&submit=Search",
        301,
        "/?month=January&competition=Invitational",
    )
    .await;
}

#[tokio::test]
async fn an_old_search_by_a_renamed_value_or_in_another_case_uses_the_current_names() {
    let app = TestApp::start().await;
    berkeley_prep(&app).await;
    app.insert_test(&NewTest {
        host: Some("Lawton Chiles"),
        ..NewTest::known()
    })
    .await;
    add_alias(&app, "host", "Lawton Chiles", "Chiles").await;
    app.rename_facet_value("division_level", "Algebra I", "Algebra 1").await;
    add_alias(&app, "division_level", "Algebra 1", "Algebra I").await;

    assert_redirect(
        &app,
        "/index.php?host=Chiles&submit=Search",
        301,
        "/?host=Lawton+Chiles",
    )
    .await;
    assert_redirect(
        &app,
        "/index.php?level=Algebra+I&host=berkeley+prep&submit=Search",
        301,
        "/?division_level=Algebra+1&host=Berkeley+Prep",
    )
    .await;
}

#[tokio::test]
async fn an_old_search_by_a_value_that_names_nothing_passes_it_through() {
    let app = TestApp::start().await;
    berkeley_prep(&app).await;

    assert_redirect(
        &app,
        "/index.php?category=Regionals+-+March&style=Mystery+Test&host=Nowhere+High&submit=Search",
        301,
        "/?month=March&competition=Regional&test_type=Mystery+Test&host=Nowhere+High",
    )
    .await;
}

#[tokio::test]
async fn old_categories_and_unknowns_translate_as_the_import_tidies_them() {
    let app = TestApp::start().await;
    app.insert_test(&NewTest {
        month: Month::NotApplicable,
        competition: Some("National"),
        test_type: Some("State Bowl Nationals"),
        ..NewTest::known()
    })
    .await;
    app.insert_test(&NewTest {
        month: Month::NotApplicable,
        competition: Some("State"),
        host: Some("Washington Mu Alpha Theta"),
        ..NewTest::known()
    })
    .await;

    for (old, new) in [
        // Month is not applicable to State, National and Mail-in.
        ("category=Nationals", "/?competition=National"),
        ("category=States", "/?competition=State"),
        ("category=Mail-in", "/?competition=Mail-in"),
        (
            "category=Washington+State",
            "/?competition=State&host=Washington+Mu+Alpha+Theta",
        ),
        ("style=State+Bowl+%28Nationals%29", "/?test_type=State+Bowl+Nationals"),
        // `?`, `-` and year 0 are Unknown.
        (
            "category=Invitationals+-+%3F",
            "/?month=unknown&competition=Invitational",
        ),
        ("category=%3F+-+March", "/?month=March&competition=unknown"),
        (
            "level=%3F&style=%3F&host=-",
            "/?division_level=unknown&test_type=unknown&host=unknown",
        ),
        ("year=0", "/?year=unknown"),
        // A year that isn't one is Any.
        ("year=nineteen&level=Calculus%2FMu", "/?division_level=Calculus%2FMu"),
        // The import split Applications into Calculus and Mu Applications.
        (
            "level=Calculus%2FMu&style=Applications",
            "/?division_level=Calculus%2FMu",
        ),
        // PHP kept the last of a repeated key.
        ("host=Doral&host=Plant+City", "/?host=Plant+City"),
        ("submit=Search", "/"),
    ] {
        assert_redirect(&app, &format!("/index.php?{old}&submit=Search"), 301, new).await;
    }
    assert_redirect(&app, "/index.php", 301, "/").await;
    assert_redirect(&app, "/index.php?%ZZ", 301, "/").await;
}

#[tokio::test]
async fn a_new_search_at_the_root_is_not_redirected() {
    let app = TestApp::start().await;
    berkeley_prep(&app).await;

    for path in ["/", "/?year=1999&host=Berkeley+Prep"] {
        let response = app.get(path).await;
        assert_eq!(response.status(), 200, "GET {path}");
    }
}
