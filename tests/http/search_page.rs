//! The public search page, as seen with JavaScript off.

use crate::support::{
    TestApp,
    fixtures::{Month, NewDocument, NewTest, sample_pdf},
    html::{Page, links_in, text, text_in},
};

// ------------------------------------------------------------------ helpers

/// The Test IDs in the results table, top to bottom.
fn result_ids(page: &Page) -> Vec<i64> {
    page.select("#results tbody tr")
        .iter()
        .map(|row| {
            let id = row.value().attr("id").expect("result rows have an id");
            id.strip_prefix('T').unwrap().parse().unwrap()
        })
        .collect()
}

impl TestApp {
    /// The Test IDs a search URL lists on its page.
    async fn search(&self, path: &str) -> Vec<i64> {
        result_ids(&self.get_page(path).await)
    }

    /// A known Test with one Published Document of each of `kinds`.
    async fn test_with_documents(&self, kinds: &[&'static str]) -> i64 {
        let test = self.insert_test(&NewTest::known()).await;
        for kind in kinds {
            self.insert_document(&[test], NewDocument::pdf(kind, &sample_pdf(500)))
                .await;
        }
        test
    }
}

/// The options of a dropdown, as `(value, text)`.
fn options(page: &Page, name: &str) -> Vec<(String, String)> {
    page.select(&format!("#filters select[name={name:?}] option"))
        .iter()
        .map(|o| (o.value().attr("value").unwrap().to_owned(), text(*o)))
        .collect()
}

fn texts(options: &[(String, String)]) -> Vec<&str> {
    options.iter().map(|(_, text)| text.as_str()).collect()
}

fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort();
    ids
}

// --------------------------------------------------------------- rendering

#[tokio::test]
async fn a_published_test_is_listed_in_the_server_rendered_html() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let document = app
        .insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(2_000)))
        .await;

    let page = app.get_page("/").await;

    assert_eq!(page.text("#count"), "1 test");
    let headings: Vec<String> = page.select("#results thead th").into_iter().map(text).collect();
    assert_eq!(
        headings,
        [
            "", // the page's select checkbox
            "Year",
            "Event",
            "Host",
            "Division/Level",
            "Test type",
            "Documents",
            "Download"
        ]
    );
    let row = page.one(&format!("#results tbody tr#T{test}"));
    assert_eq!(text_in(row, ".year"), "1993");
    assert_eq!(text_in(row, ".event"), "February Invitational");
    assert_eq!(text_in(row, ".host"), "Plant City");
    assert_eq!(text_in(row, ".division-level"), "Calculus/Mu");
    assert_eq!(text_in(row, ".test-type"), "Individual");
    assert_eq!(
        links_in(page.one(&format!("tr#T{test} td.documents"))),
        vec![("Test".to_owned(), format!("/tests/{test}/documents/{document}/pdf"))]
    );
    assert_eq!(page.text("footer"), "Made by and maintained by Rob from Doral Academy");
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
    assert_eq!(text_in(unknown, ".year"), "—");
    assert_eq!(text_in(unknown, ".event"), "— Regional");
    assert_eq!(text_in(unknown, ".host"), "—");
    assert_eq!(text_in(unknown, ".division-level"), "—");
    assert_eq!(text_in(unknown, ".test-type"), "—");
}

#[tokio::test]
async fn the_form_offers_any_the_values_in_display_order_and_unknown() {
    let app = TestApp::start().await;
    // Display order is creation order in the fixtures; none of these is
    // alphabetical, so an alphabetical sort would show.
    for (facet, values) in [
        ("month", &["November", "January", "February"][..]),
        ("competition", &["Invitational", "Regional", "State"][..]),
        ("division_level", &["Algebra I", "Geometry", "Algebra II"][..]),
        ("test_type", &["Individual", "Team", "Ciphering"][..]),
        ("host", &["FAMAT", "MAO", "Archbishop McCarthy"][..]),
    ] {
        for value in values {
            app.facet_value(facet, value).await;
        }
    }
    for year in [2001, 2019, 1993] {
        app.insert_test(&NewTest {
            year: Some(year),
            ..NewTest::known()
        })
        .await;
    }
    app.insert_test(&NewTest {
        year: Some(1987),
        published: false,
        ..NewTest::known()
    })
    .await;

    let page = app.get_page("/").await;

    let labels: Vec<String> = page.select("#filters label.l").into_iter().map(text).collect();
    assert_eq!(
        labels,
        [
            "Year:",
            "Month:",
            "Competition:",
            "Division/Level:",
            "Test type:",
            "Host:"
        ]
    );
    let names: Vec<&str> = page
        .select("#filters select")
        .iter()
        .map(|s| s.value().attr("name").unwrap())
        .collect();
    assert_eq!(
        names,
        ["year", "month", "competition", "division_level", "test_type", "host"]
    );

    // Only years that Published Tests have, newest first.
    assert_eq!(
        texts(&options(&page, "year")),
        ["Any", "2019", "2001", "1993", "Unknown"]
    );
    assert_eq!(
        texts(&options(&page, "month")),
        ["Any", "November", "January", "February", "Unknown"]
    );
    assert_eq!(
        texts(&options(&page, "competition")),
        ["Any", "Invitational", "Regional", "State", "Unknown"]
    );
    assert_eq!(
        texts(&options(&page, "division_level")),
        ["Any", "Algebra I", "Geometry", "Algebra II", "Calculus/Mu", "Unknown"]
    );
    assert_eq!(
        texts(&options(&page, "test_type")),
        ["Any", "Individual", "Team", "Ciphering", "Unknown"]
    );
    assert_eq!(
        texts(&options(&page, "host")),
        ["Any", "FAMAT", "MAO", "Archbishop McCarthy", "Plant City", "Unknown"]
    );

    let includes: Vec<String> = page.select("#filters .checks label").into_iter().map(text).collect();
    assert_eq!(includes, ["Test", "Answers", "Solutions"]);
    assert_eq!(page.one("#filters a.reset").value().attr("href"), Some("/"));
    assert_eq!(page.text("#filters button[type=submit]"), "Search");
}

// ----------------------------------------------------------------- filters

#[tokio::test]
async fn each_facet_filter_keeps_only_tests_with_that_value() {
    let app = TestApp::start().await;
    let base = app.insert_test(&NewTest::known()).await;
    let year = app
        .insert_test(&NewTest {
            year: Some(2004),
            ..NewTest::known()
        })
        .await;
    let month = app
        .insert_test(&NewTest {
            month: Month::Known("January"),
            ..NewTest::known()
        })
        .await;
    let competition = app
        .insert_test(&NewTest {
            competition: Some("Regional"),
            ..NewTest::known()
        })
        .await;
    let division_level = app
        .insert_test(&NewTest {
            division_level: Some("Geometry"),
            ..NewTest::known()
        })
        .await;
    let test_type = app
        .insert_test(&NewTest {
            test_type: Some("Team"),
            ..NewTest::known()
        })
        .await;
    let host = app
        .insert_test(&NewTest {
            host: Some("FAMAT"),
            ..NewTest::known()
        })
        .await;
    let all = sorted(vec![base, year, month, competition, division_level, test_type, host]);
    let all_but = |id: i64| all.iter().copied().filter(|&t| t != id).collect::<Vec<_>>();

    for (param, base_value, other_value, other) in [
        ("year", "1993", "2004", year),
        ("month", "February", "January", month),
        ("competition", "Invitational", "Regional", competition),
        ("division_level", "Calculus%2FMu", "Geometry", division_level),
        ("test_type", "Individual", "Team", test_type),
        ("host", "Plant+City", "FAMAT", host),
    ] {
        assert_eq!(
            app.search(&format!("/?{param}={other_value}")).await,
            vec![other],
            "{param}={other_value}"
        );
        assert_eq!(
            sorted(app.search(&format!("/?{param}={base_value}")).await),
            all_but(other),
            "{param}={base_value}"
        );
    }

    // Filters combine.
    assert_eq!(
        app.search("/?year=1993&host=FAMAT&test_type=Individual").await,
        vec![host]
    );
    // A value that names nothing matches nothing.
    let page = app.get_page("/?host=Nowhere+High").await;
    assert_eq!(page.text("#count"), "0 tests");
    assert!(result_ids(&page).is_empty());
    assert_eq!(page.text(".empty"), "No tests match. Clear a filter.");
}

#[tokio::test]
async fn any_matches_every_test() {
    let app = TestApp::start().await;
    let known = app.insert_test(&NewTest::known()).await;
    let unknown = app
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
    let state = app
        .insert_test(&NewTest {
            month: Month::NotApplicable,
            competition: Some("State"),
            ..NewTest::known()
        })
        .await;

    // What the form sends with every dropdown on Any and nothing ticked.
    let ids = app
        .search("/?year=&month=&competition=&division_level=&test_type=&host=")
        .await;

    assert_eq!(sorted(ids), sorted(vec![known, unknown, state]));
}

#[tokio::test]
async fn unknown_keeps_only_tests_missing_that_facet() {
    let app = TestApp::start().await;
    let known = app.insert_test(&NewTest::known()).await;
    let year = app
        .insert_test(&NewTest {
            year: None,
            ..NewTest::known()
        })
        .await;
    let month = app
        .insert_test(&NewTest {
            month: Month::Unknown,
            ..NewTest::known()
        })
        .await;
    let competition = app
        .insert_test(&NewTest {
            competition: None,
            ..NewTest::known()
        })
        .await;
    let division_level = app
        .insert_test(&NewTest {
            division_level: None,
            ..NewTest::known()
        })
        .await;
    let test_type = app
        .insert_test(&NewTest {
            test_type: None,
            ..NewTest::known()
        })
        .await;
    let host = app
        .insert_test(&NewTest {
            host: None,
            ..NewTest::known()
        })
        .await;
    // State has no month, which is not the same as an unknown one.
    let state = app
        .insert_test(&NewTest {
            month: Month::NotApplicable,
            competition: Some("State"),
            ..NewTest::known()
        })
        .await;

    for (param, unknown) in [
        ("year", year),
        ("month", month),
        ("competition", competition),
        ("division_level", division_level),
        ("test_type", test_type),
        ("host", host),
    ] {
        assert_eq!(
            app.search(&format!("/?{param}=unknown")).await,
            vec![unknown],
            "{param}=unknown"
        );
    }
    let known_month = app.search("/?month=February").await;
    assert!(known_month.contains(&known));
    assert!(!known_month.contains(&state));
}

#[tokio::test]
async fn includes_keeps_tests_with_a_document_containing_each_ticked_part() {
    let app = TestApp::start().await;
    let test_only = app.test_with_documents(&["Test"]).await;
    let condensed = app.test_with_documents(&["Condensed Test"]).await;
    let answers_only = app.test_with_documents(&["Answers"]).await;
    let test_and_answers = app.test_with_documents(&["Test", "Answers"]).await;
    // Neither of these matches any Includes box.
    app.test_with_documents(&["Errata", "Other"]).await;
    app.insert_test(&NewTest::known()).await;

    assert_eq!(
        sorted(app.search("/?includes=test").await),
        sorted(vec![test_only, condensed, test_and_answers])
    );
    assert_eq!(
        sorted(app.search("/?includes=answers").await),
        sorted(vec![answers_only, test_and_answers])
    );
    assert!(app.search("/?includes=solutions").await.is_empty());
    // Several ticks all have to hold.
    assert_eq!(
        app.search("/?includes=test&includes=answers").await,
        vec![test_and_answers]
    );
}

#[tokio::test]
async fn combined_counts_as_test_answers_and_solutions_and_solutions_as_answers() {
    let app = TestApp::start().await;
    let combined = app.test_with_documents(&["Combined Test+Solutions"]).await;
    let solutions = app.test_with_documents(&["Solutions"]).await;
    let test_only = app.test_with_documents(&["Test"]).await;

    assert_eq!(
        sorted(app.search("/?includes=test").await),
        sorted(vec![combined, test_only])
    );
    assert_eq!(
        sorted(app.search("/?includes=answers").await),
        sorted(vec![combined, solutions])
    );
    assert_eq!(
        sorted(app.search("/?includes=solutions").await),
        sorted(vec![combined, solutions])
    );
    assert_eq!(
        app.search("/?includes=test&includes=answers&includes=solutions").await,
        vec![combined]
    );
}

#[tokio::test]
async fn includes_ignores_hidden_and_trashed_documents() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let with_hidden = app.test_with_documents(&["Test"]).await;
    app.insert_document(
        &[with_hidden],
        NewDocument {
            published: false,
            ..NewDocument::pdf("Solutions", &pdf)
        },
    )
    .await;
    let with_trashed = app.test_with_documents(&["Test"]).await;
    let trashed = app
        .insert_document(&[with_trashed], NewDocument::pdf("Solutions", &pdf))
        .await;
    app.trash_document(trashed).await;
    let with_solutions = app.test_with_documents(&["Solutions"]).await;

    assert_eq!(app.search("/?includes=solutions").await, vec![with_solutions]);
    let page = app.get_page("/").await;
    assert_eq!(
        links_in(page.one(&format!("tr#T{with_trashed} td.documents"))).len(),
        1,
        "only the Test is listed"
    );
}

// -------------------------------------------------------- hidden and trash

#[tokio::test]
async fn hidden_and_trashed_tests_never_appear() {
    let app = TestApp::start().await;
    let published = app.test_with_documents(&["Test"]).await;
    let hidden = app
        .insert_test(&NewTest {
            year: Some(1999),
            published: false,
            ..NewTest::known()
        })
        .await;
    app.insert_document(&[hidden], NewDocument::pdf("Test", &sample_pdf(500)))
        .await;
    let trashed = app.test_with_documents(&["Test"]).await;
    app.trash_test(trashed).await;

    for path in [
        "/",
        "/?year=1993",
        "/?year=1999",
        "/?host=Plant+City",
        "/?includes=test",
        "/?page=2",
    ] {
        let page = app.get_page(path).await;
        let ids = result_ids(&page);
        assert!(!ids.contains(&hidden), "{path} lists the Hidden Test");
        assert!(!ids.contains(&trashed), "{path} lists the Trashed Test");
    }
    let page = app.get_page("/").await;
    assert_eq!(result_ids(&page), vec![published]);
    assert_eq!(page.text("#count"), "1 test");
    assert!(!texts(&options(&page, "year")).contains(&"1999"));
}

// --------------------------------------------------------- order and pages

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

    assert_eq!(app.search("/").await, vec![jan_2021, nov_2020, jan_2020, year_unknown]);
}

#[tokio::test]
async fn within_a_month_tests_follow_competition_host_division_and_type_order() {
    let app = TestApp::start().await;
    // Display orders that differ from the alphabet.
    for (facet, values) in [
        ("month", &["January", "February"][..]),
        ("competition", &["Regional", "Invitational", "State", "National"][..]),
        ("host", &["MAO", "FAMAT"][..]),
        ("division_level", &["Geometry", "Algebra II"][..]),
        ("test_type", &["Team", "Individual"][..]),
    ] {
        for value in values {
            app.facet_value(facet, value).await;
        }
    }
    let test = |month, competition, host, division_level, test_type| NewTest {
        year: Some(2010),
        month,
        competition: Some(competition),
        host: Some(host),
        division_level: Some(division_level),
        test_type: Some(test_type),
        published: true,
    };
    let jan = Month::Known("January");
    let feb = Month::Known("February");
    let na = Month::NotApplicable;
    // Inserted in reverse of the expected order. Month-less Tests (unknown or
    // not applicable) come last, then follow Competition order.
    let expected_last_first = [
        test(na, "National", "MAO", "Geometry", "Team"),
        test(na, "State", "MAO", "Geometry", "Team"),
        test(Month::Unknown, "Invitational", "MAO", "Geometry", "Team"),
        test(feb, "Regional", "MAO", "Geometry", "Team"),
        test(jan, "Invitational", "MAO", "Geometry", "Team"),
        test(jan, "Regional", "FAMAT", "Geometry", "Team"),
        test(jan, "Regional", "MAO", "Algebra II", "Team"),
        test(jan, "Regional", "MAO", "Geometry", "Individual"),
        test(jan, "Regional", "MAO", "Geometry", "Team"),
    ];
    let mut expected = Vec::new();
    for new in &expected_last_first {
        expected.push(app.insert_test(new).await);
    }
    expected.reverse();

    assert_eq!(app.search("/").await, expected);
}

#[tokio::test]
async fn results_come_fifty_to_a_page_with_previous_and_next() {
    let app = TestApp::start().await;
    // 120 Tests, one per year, so the full order is newest first; plus some
    // the search filters out.
    let mut expected = Vec::new();
    for year in 1901..=2020 {
        expected.push(
            app.insert_test(&NewTest {
                year: Some(year),
                ..NewTest::known()
            })
            .await,
        );
    }
    expected.reverse();
    for year in [1950, 1990] {
        app.insert_test(&NewTest {
            year: Some(year),
            host: Some("FAMAT"),
            ..NewTest::known()
        })
        .await;
    }

    let first = app.get_page("/?host=Plant+City").await;
    assert_eq!(first.text("#count"), "120 tests");
    assert_eq!(first.text("#page"), "1 / 3");
    assert_eq!(result_ids(&first), expected[..50]);
    // The pager's buttons submit the results form, which keeps the search.
    assert!(first.select("#prev").is_empty(), "no Previous on page 1");
    let next = first.form("#picks").press("#next").submit_url();
    assert_eq!(next, "/?host=Plant+City&page=2");

    let second = app.get_page(&next).await;
    assert_eq!(second.text("#page"), "2 / 3");
    assert_eq!(result_ids(&second), expected[50..100]);
    let previous = second.form("#picks").press("#prev").submit_url();
    assert_eq!(previous, "/?host=Plant+City&page=1");
    assert_eq!(app.search(&previous).await, expected[..50]);
    let next = second.form("#picks").press("#next").submit_url();

    let third = app.get_page(&next).await;
    assert_eq!(third.text("#count"), "120 tests");
    assert_eq!(third.text("#page"), "3 / 3");
    assert_eq!(result_ids(&third), expected[100..]);
    assert!(third.select("#next").is_empty(), "no Next on the last page");

    // A page past the end shows the last one.
    assert_eq!(app.search("/?host=Plant+City&page=9").await, expected[100..]);
    // One page needs no pager.
    let one_page = app.get_page("/?host=FAMAT").await;
    assert_eq!(result_ids(&one_page).len(), 2);
    assert!(one_page.select(".pager").is_empty());
}

// ------------------------------------------------------------- without JS

#[tokio::test]
async fn submitting_the_form_without_js_returns_the_results_and_keeps_the_choices() {
    let app = TestApp::start().await;
    let wanted = app.test_with_documents(&["Test", "Solutions"]).await;
    // Filtered out by Includes: Solutions and by Host.
    app.test_with_documents(&["Test"]).await;
    let other_host = app
        .insert_test(&NewTest {
            host: Some("FAMAT"),
            ..NewTest::known()
        })
        .await;
    app.insert_document(&[other_host], NewDocument::pdf("Solutions", &sample_pdf(500)))
        .await;

    let home = app.get_page("/").await;
    let url = home
        .form("#filters")
        .select("year", "1993")
        .select("host", "Plant City")
        .check("includes", "solutions", true)
        .submit_url();
    assert_eq!(
        url,
        "/?year=1993&month=&competition=&division_level=&test_type=&host=Plant+City&includes=solutions"
    );
    let results = app.get_page(&url).await;

    assert_eq!(result_ids(&results), vec![wanted]);
    assert_eq!(results.text("#count"), "1 test");
    // The form shows the search it ran, so resubmitting it runs it again.
    assert_eq!(results.form("#filters").submit_url(), url);
    assert_eq!(
        results.text("#filters select[name=host] option[selected]"),
        "Plant City"
    );
    assert!(
        results
            .one("#filters input[name=includes][value=solutions]")
            .value()
            .attr("checked")
            .is_some()
    );

    // Unknown is a choice like any other.
    let url = results.form("#filters").select("host", "unknown").submit_url();
    let page = app.get_page(&url).await;
    assert!(result_ids(&page).is_empty());
    assert_eq!(page.text("#filters select[name=host] option[selected]"), "Unknown");
}

#[tokio::test]
async fn values_with_reserved_url_characters_round_trip_through_the_form() {
    let app = TestApp::start().await;
    let odd = app
        .insert_test(&NewTest {
            host: Some("A&M = Prep + 50% Club"),
            ..NewTest::known()
        })
        .await;
    app.insert_test(&NewTest::known()).await;

    let home = app.get_page("/").await;
    let url = home
        .form("#filters")
        .select("host", "A&M = Prep + 50% Club")
        .submit_url();
    let page = app.get_page(&url).await;

    assert_eq!(result_ids(&page), vec![odd]);
    assert_eq!(
        page.text("#filters select[name=host] option[selected]"),
        "A&M = Prep + 50% Club"
    );
}
