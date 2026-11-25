//! The public search page, as seen with JavaScript off.

use crate::support::{
    TestApp,
    fixtures::{Month, NewDocument, NewTest, sample_pdf},
    html::{links_in, text_in},
};

#[tokio::test]
async fn a_published_test_is_listed_in_the_server_rendered_html() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let document = app
        .insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(2_000)))
        .await;

    let page = app.get_page("/").await;

    assert_eq!(page.text("#count"), "1 test");
    let row = page.one(&format!("#results tbody tr#T{test}"));
    assert_eq!(text_in(row, ".year"), "1993");
    assert_eq!(text_in(row, ".event"), "February Invitational");
    assert_eq!(text_in(row, ".host"), "Plant City");
    assert_eq!(text_in(row, ".division-level"), "Calculus/Mu");
    assert_eq!(text_in(row, ".test-type"), "Individual");
    assert_eq!(
        links_in(row),
        vec![("Test".to_owned(), format!("/tests/{test}/documents/{document}/pdf"))]
    );
}

#[tokio::test]
async fn hidden_tests_are_not_listed() {
    let app = TestApp::start().await;
    let published = app.insert_test(&NewTest::known()).await;
    let hidden = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;

    let page = app.get_page("/").await;

    assert_eq!(page.text("#count"), "1 test");
    assert_eq!(page.select(&format!("tr#T{published}")).len(), 1);
    assert!(page.select(&format!("tr#T{hidden}")).is_empty());
}

#[tokio::test]
async fn unknown_facets_show_a_dash_and_state_has_no_month() {
    let app = TestApp::start().await;
    let state = app
        .insert_test(&NewTest {
            year: Some(2019),
            month: Month::NotApplicable,
            competition: Some("State"),
            division_level: Some("Geometry"),
            test_type: Some("Team"),
            host: Some("FAMAT"),
            published: true,
        })
        .await;
    let unknown = app
        .insert_test(&NewTest {
            year: None,
            month: Month::Unknown,
            competition: Some("Regional"),
            division_level: None,
            test_type: None,
            host: None,
            published: true,
        })
        .await;

    let page = app.get_page("/").await;

    let state = page.one(&format!("tr#T{state}"));
    assert_eq!(text_in(state, ".event"), "State");
    let unknown = page.one(&format!("tr#T{unknown}"));
    assert_eq!(text_in(unknown, ".year"), "–");
    assert_eq!(text_in(unknown, ".event"), "– Regional");
    assert_eq!(text_in(unknown, ".host"), "–");
    assert_eq!(text_in(unknown, ".division-level"), "–");
    assert_eq!(text_in(unknown, ".test-type"), "–");
}

#[tokio::test]
async fn tests_are_ordered_newest_year_first_then_by_season() {
    let app = TestApp::start().await;
    // Season order comes from the Month display order, not the alphabet.
    let november = app.facet_value("month", "November").await;
    let january = app.facet_value("month", "January").await;
    assert!(november < january, "fixture values are ordered by creation");

    let jan_2020 = app
        .insert_test(&NewTest {
            year: Some(2020),
            month: Month::Known("January"),
            ..NewTest::known()
        })
        .await;
    let nov_2020 = app
        .insert_test(&NewTest {
            year: Some(2020),
            month: Month::Known("November"),
            ..NewTest::known()
        })
        .await;
    let year_unknown = app
        .insert_test(&NewTest {
            year: None,
            ..NewTest::known()
        })
        .await;
    let jan_2021 = app
        .insert_test(&NewTest {
            year: Some(2021),
            month: Month::Known("January"),
            ..NewTest::known()
        })
        .await;

    let page = app.get_page("/").await;

    let ids: Vec<String> = page
        .select("#results tbody tr")
        .iter()
        .map(|row| row.value().attr("id").unwrap().to_owned())
        .collect();
    let expected: Vec<String> = [jan_2021, nov_2020, jan_2020, year_unknown]
        .iter()
        .map(|id| format!("T{id}"))
        .collect();
    assert_eq!(ids, expected);
}
