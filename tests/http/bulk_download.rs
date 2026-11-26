//! Multi-select and the bulk zip: row and page checkboxes, the bulk bar, and
//! `/tests/zip?sel=…&group=…&format=…`, named `FAMAT Test Bank - {N} tests[ -
//! {Kind group}].zip` with one folder per Test. All as seen with JavaScript
//! off: the selection travels in the results form (`#picks`).

use scraper::Selector;

use crate::support::{
    TestApp,
    download::disposition,
    fixtures::{Month, NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Page, text},
    zip::{entries, names},
};

// ------------------------------------------------------------------ helpers

/// The "N selected" text, if the bar shows a selection.
fn selected(page: &Page) -> Option<String> {
    page.select("#bulk #selected").first().map(|e| text(*e))
}

/// The Tests whose row checkbox is ticked, top to bottom.
fn ticked(page: &Page) -> Vec<i64> {
    page.select("#results tbody tr input.pick[checked]")
        .into_iter()
        .map(|input| input.value().attr("value").unwrap().parse().unwrap())
        .collect()
}

/// `(label, count, group)` of each kind group in the bulk menu.
fn bulk_groups(page: &Page) -> Vec<(String, String, String)> {
    let count = Selector::parse(".n").unwrap();
    page.select("#bulk .split details .menu > button")
        .into_iter()
        .map(|button| {
            let n = text(button.select(&count).next().expect("a count"));
            let label = text(button).strip_suffix(&n).unwrap().trim().to_owned();
            (label, n, button.value().attr("value").unwrap().to_owned())
        })
        .collect()
}

fn ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// GET a zip that must exist: its name and bytes.
async fn zip(app: &TestApp, path: &str) -> (String, Vec<u8>) {
    let response = app.get(path).await;
    assert_eq!(response.status(), 200, "GET {path}");
    assert_eq!(response.headers()["content-type"], "application/zip", "GET {path}");
    let disposition = disposition(&response);
    assert_eq!(disposition.kind, "attachment", "GET {path}");
    (disposition.name, response.bytes().await.unwrap().to_vec())
}

impl TestApp {
    /// `n` Published Tests of `host`, one per year from 1901, each with one
    /// small PDF Test, newest first (the results order).
    async fn tests_with_a_pdf(&self, n: i16, host: &'static str) -> Vec<i64> {
        let pdf = sample_pdf(200);
        let mut tests = Vec::new();
        for year in 1901..1901 + n {
            let test = self
                .insert_test(&NewTest {
                    year: Some(year),
                    host: Some(host),
                    ..NewTest::known()
                })
                .await;
            self.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
            tests.push(test);
        }
        tests.reverse();
        tests
    }

    /// `n` Published Tests with no Documents, straight in SQL for speed.
    async fn bare_tests(&self, n: i32) -> Vec<i64> {
        sqlx::query_scalar(
            "INSERT INTO tests (year, month_applicable, published)
             SELECT 1950, true, true FROM generate_series(1, $1) RETURNING id",
        )
        .bind(n)
        .fetch_all(&self.db)
        .await
        .expect("insert Tests")
    }

    async fn hide_test(&self, id: i64) {
        sqlx::query("UPDATE tests SET published = false WHERE id = $1")
            .bind(id)
            .execute(&self.db)
            .await
            .expect("hide Test");
    }
}

// ------------------------------------------------------------ the checkboxes

#[tokio::test]
async fn each_row_and_the_page_have_a_checkbox_and_nothing_is_selected_at_first() {
    let app = TestApp::start().await;
    let tests = app.tests_with_a_pdf(3, "Plant City").await;

    let page = app.get_page("/").await;
    for test in &tests {
        let row = page.one(&format!("tr#T{test} td.pick input.pick"));
        assert_eq!(row.value().attr("type"), Some("checkbox"));
        assert_eq!(row.value().attr("name"), Some("sel"));
        assert_eq!(row.value().attr("value"), Some(test.to_string().as_str()));
        assert_eq!(row.value().attr("aria-label"), Some("Select this test"));
    }
    assert!(ticked(&page).is_empty());

    // The page checkbox's value is every Test on the page.
    let all = page.one("#results thead th.pick input#pickpage");
    assert_eq!(all.value().attr("type"), Some("checkbox"));
    assert_eq!(all.value().attr("name"), Some("sel"));
    assert_eq!(all.value().attr("value"), Some(ids(&tests).as_str()));
    assert_eq!(all.value().attr("aria-label"), Some("Select all on this page"));
    assert!(all.value().attr("checked").is_none());

    // No bar until something is selected.
    assert_eq!(selected(&page), None);
    assert!(page.one("#bulk").value().classes().any(|c| c == "none"));
    assert!(page.select("#bulk .split, #select-all, #clear-selection").is_empty());
}

#[tokio::test]
async fn the_bar_shows_the_count_the_split_button_select_all_and_clear() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(300);
    let docx = sample_docx(300);
    let first = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[first], NewDocument::pdf("Test", &pdf)).await;
    app.insert_document(&[first], NewDocument::docx("Answers", &docx, Some(&pdf)))
        .await;
    let second = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[second], NewDocument::pdf("Combined Test+Solutions", &pdf))
        .await;
    let hidden_document = app.insert_document(&[second], NewDocument::pdf("Errata", &pdf)).await;
    app.trash_document(hidden_document).await;
    let third = app.insert_test(&NewTest::known()).await;

    let page = app.get_page(&format!("/?sel={first},{second}")).await;
    assert_eq!(selected(&page).as_deref(), Some("2 selected"));
    assert!(!page.one("#bulk").value().classes().any(|c| c == "none"));
    assert_eq!(ticked(&page), [first, second]);
    assert!(page.one("#pickpage").value().attr("checked").is_none(), "{third} isn't");

    // The same split button, counting the selected Tests' visible Documents.
    let icon = page.one("#bulk .split > button.all");
    assert_eq!(icon.value().attr("type"), Some("submit"));
    assert_eq!(icon.value().attr("formaction"), Some("/tests/zip"));
    assert_eq!(icon.value().attr("aria-label"), Some("Download all documents"));
    assert_eq!(icon.value().attr("title"), Some("Download all (PDF)"));
    let row = |label: &str, count: &str, group: &str| (label.to_owned(), count.to_owned(), group.to_owned());
    assert_eq!(
        bulk_groups(&page),
        [
            row("All documents", "3", "all"),
            row("Tests", "2", "tests"),
            row("Answers", "1", "answers"),
            row("Solutions", "1", "solutions"),
        ]
    );
    for button in page.select("#bulk .menu > button") {
        assert_eq!(button.value().attr("formaction"), Some("/tests/zip"));
        assert_eq!(button.value().attr("name"), Some("group"));
    }
    assert!(
        page.one("#bulk .split details").value().attr("open").is_none(),
        "a closed disclosure"
    );

    let select_all = page.one("#bulk button#select-all");
    assert_eq!(text(select_all), "Select all 3 results");
    assert_eq!(select_all.value().attr("name"), Some("select"));
    assert_eq!(select_all.value().attr("value"), Some("all"));
    assert_eq!(page.one("#clear-selection").value().attr("href"), Some("/"));
    assert!(page.select("#bulk .note").is_empty());

    // Clear selection is a plain link to the page without it.
    let cleared = app.get_page("/").await;
    assert_eq!(selected(&cleared), None);

    // With everything selected there is nothing more to select.
    let page = app.get_page(&format!("/?sel={first},{second},{third}")).await;
    assert_eq!(selected(&page).as_deref(), Some("3 selected"));
    assert!(page.select("#select-all").is_empty());
    assert!(page.one("#pickpage").value().attr("checked").is_some());
}

// ------------------------------------------------------- selecting, no JS

#[tokio::test]
async fn ticks_survive_paging_without_js() {
    let app = TestApp::start().await;
    let tests = app.bare_tests(60).await;
    let page_one = app.get_page("/").await;
    let first_page: Vec<i64> = page_one
        .select("#results tbody tr")
        .iter()
        .map(|row| row.value().attr("id").unwrap()[1..].parse().unwrap())
        .collect();
    let second_page: Vec<i64> = tests.iter().copied().filter(|id| !first_page.contains(id)).collect();
    assert_eq!((first_page.len(), second_page.len()), (50, 10));
    let (a, b, c) = (first_page[0], first_page[7], second_page[3]);

    // Tick two on page 1 and go to page 2: they come along.
    let url = page_one
        .form("#picks")
        .check("sel", &a.to_string(), true)
        .check("sel", &b.to_string(), true)
        .press("#next")
        .submit_url();
    assert_eq!(url, format!("/?sel={a}&sel={b}&page=2"));
    let page_two = app.get_page(&url).await;
    assert_eq!(page_two.text("#page"), "2 / 2");
    assert_eq!(selected(&page_two).as_deref(), Some("2 selected"));
    assert!(ticked(&page_two).is_empty());

    // Tick one there and go back: all three are selected.
    let url = page_two
        .form("#picks")
        .check("sel", &c.to_string(), true)
        .press("#prev")
        .submit_url();
    let page_one = app.get_page(&url).await;
    assert_eq!(page_one.text("#page"), "1 / 2");
    assert_eq!(selected(&page_one).as_deref(), Some("3 selected"));
    assert_eq!(ticked(&page_one), [a, b]);

    // Untick one: it's dropped; the one on page 2 stays.
    let url = page_one
        .form("#picks")
        .check("sel", &b.to_string(), false)
        .press("#next")
        .submit_url();
    let page_two = app.get_page(&url).await;
    assert_eq!(selected(&page_two).as_deref(), Some("2 selected"));
    assert_eq!(ticked(&page_two), [c]);

    // The page checkbox selects the whole page.
    let url = page_two
        .form("#picks")
        .check("sel", &ids(&second_page), true)
        .press("#prev")
        .submit_url();
    let page_one = app.get_page(&url).await;
    assert_eq!(selected(&page_one).as_deref(), Some("11 selected"));
    assert_eq!(ticked(&page_one), [a]);
}

#[tokio::test]
async fn without_js_update_selection_applies_ticks_and_keeps_the_page() {
    let app = TestApp::start().await;
    let tests = app.bare_tests(55).await;
    let page_two = app.get_page("/?page=2").await;
    let row: i64 = page_two.select("#results tbody tr")[0].value().attr("id").unwrap()[1..]
        .parse()
        .unwrap();
    assert!(tests.contains(&row));

    // Before anything is selected the bar is hidden, but it holds the
    // button that CSS shows once a box is ticked.
    let update = page_two.one("#bulk.none button.update");
    assert_eq!(text(update), "Update selection");
    let url = page_two
        .form("#picks")
        .check("sel", &row.to_string(), true)
        .press("#bulk button.update")
        .submit_url();
    assert_eq!(url, format!("/?page=2&sel={row}"));
    let page = app.get_page(&url).await;
    assert_eq!(page.text("#page"), "2 / 2");
    assert_eq!(selected(&page).as_deref(), Some("1 selected"));
    assert_eq!(ticked(&page), [row]);
}

#[tokio::test]
async fn the_selection_is_kept_by_a_new_search_and_by_the_format_switch() {
    let app = TestApp::start().await;
    let plant_city = app.tests_with_a_pdf(2, "Plant City").await;
    let famat = app.tests_with_a_pdf(1, "FAMAT").await[0];

    let page = app.get_page(&format!("/?sel={famat}")).await;
    let url = page.form("#filters").select("host", "Plant City").submit_url();
    assert!(url.ends_with(&format!("&sel={famat}")), "{url}");
    let page = app.get_page(&url).await;
    assert_eq!(
        selected(&page).as_deref(),
        Some("1 selected"),
        "not on this page, but selected"
    );
    assert!(ticked(&page).is_empty());
    // Selected elsewhere: a hidden field carries it.
    assert_eq!(
        page.one("#picks > input[type=hidden][name=sel]").value().attr("value"),
        Some(famat.to_string().as_str())
    );
    // Selecting all results adds them.
    let page = app
        .get_page(&page.form("#picks").press("#select-all").submit_url())
        .await;
    assert_eq!(selected(&page).as_deref(), Some("3 selected"));
    assert_eq!(ticked(&page), plant_city);

    // The Format switch links keep the selection.
    let mut all = vec![famat];
    all.extend(&plant_city);
    all.sort();
    let original = page
        .select("#bulk .fmt a")
        .into_iter()
        .find(|a| text(*a) == "Original")
        .unwrap();
    let href = original.value().attr("href").unwrap();
    assert_eq!(
        href,
        format!(
            "/?host=Plant+City&format=original&sel={}",
            ids(&all).replace(',', "%2C")
        )
    );
    let page = app.get_page(href).await;
    assert_eq!(selected(&page).as_deref(), Some("3 selected"));
    assert_eq!(
        page.one("#bulk .split > button.all").value().attr("title"),
        Some("Download all (Original)")
    );
}

#[tokio::test]
async fn select_all_n_results_selects_every_match_across_pages() {
    let app = TestApp::start().await;
    let matching = app.tests_with_a_pdf(55, "Plant City").await;
    let elsewhere = app.tests_with_a_pdf(2, "FAMAT").await;

    // Start from one Test of another search.
    let page = app.get_page(&format!("/?host=Plant+City&sel={}", elsewhere[0])).await;
    assert_eq!(page.text("#bulk #select-all"), "Select all 55 results");
    let url = page.form("#picks").press("#select-all").submit_url();
    assert_eq!(
        url,
        format!("/?host=Plant+City&sel={}&select=all", elsewhere[0]),
        "back to page 1"
    );
    let page_one = app.get_page(&url).await;
    assert_eq!(selected(&page_one).as_deref(), Some("56 selected"));
    assert_eq!(ticked(&page_one), matching[..50]);
    assert!(page_one.one("#pickpage").value().attr("checked").is_some());
    assert!(page_one.select("#select-all").is_empty(), "all are selected");

    let page_two = app.get_page(&page_one.form("#picks").press("#next").submit_url()).await;
    assert_eq!(selected(&page_two).as_deref(), Some("56 selected"));
    assert_eq!(ticked(&page_two), matching[50..]);

    // The bulk zip from there holds all of them, one folder each.
    let url = page_two.form("#picks").press("#bulk .split > button.all").submit_url();
    assert!(url.starts_with("/tests/zip?"), "{url}");
    let (name, bytes) = zip(&app, &url).await;
    assert_eq!(name, "FAMAT Test Bank - 56 tests.zip");
    let mut folders: Vec<String> = names(&bytes)
        .into_iter()
        .map(|name| name.split_once('/').expect("in a folder").0.to_owned())
        .collect();
    assert_eq!(folders.len(), 56, "one PDF each");
    folders.dedup();
    assert_eq!(folders.len(), 56, "a folder each");
    let mut expected: Vec<i64> = matching.clone();
    expected.push(elsewhere[0]);
    for test in expected {
        assert!(
            folders.iter().any(|f| f.starts_with(&format!("T{test} "))),
            "a folder for T{test}"
        );
    }
}

#[tokio::test]
async fn more_results_than_a_bulk_zip_holds_cannot_all_be_selected() {
    let app = TestApp::start().await;
    let tests = app.bare_tests(501).await;

    let page = app.get_page(&format!("/?sel={}", tests[0])).await;
    assert_eq!(page.text("#count"), "501 tests");
    assert_eq!(selected(&page).as_deref(), Some("1 selected"));
    assert!(page.select("#select-all").is_empty());
    assert_eq!(page.text("#bulk .note"), "Up to 500 tests can be downloaded together.");
    // Nor by asking.
    let page = app.get_page(&format!("/?sel={}&select=all", tests[0])).await;
    assert_eq!(selected(&page).as_deref(), Some("1 selected"));

    // Over the cap by hand: no download button.
    let page = app.get_page(&format!("/?sel={}", ids(&tests))).await;
    assert_eq!(selected(&page).as_deref(), Some("501 selected"));
    assert!(page.select("#bulk .split").is_empty());
    assert_eq!(page.text("#bulk .note"), "Up to 500 tests can be downloaded together.");
    // At the cap it's back.
    let page = app.get_page(&format!("/?sel={}", ids(&tests[..500]))).await;
    assert_eq!(selected(&page).as_deref(), Some("500 selected"));
    assert_eq!(page.select("#bulk .split").len(), 1);
}

#[tokio::test]
async fn hidden_trashed_and_unknown_tests_are_never_selected() {
    let app = TestApp::start().await;
    let visible = app.tests_with_a_pdf(1, "Plant City").await[0];
    let hidden = app.tests_with_a_pdf(1, "FAMAT").await[0];
    app.hide_test(hidden).await;
    let trashed = app.tests_with_a_pdf(1, "MAO").await[0];
    app.trash_test(trashed).await;

    let page = app
        .get_page(&format!("/?sel=oops&sel={hidden},{trashed},999999&sel={visible}"))
        .await;
    assert_eq!(selected(&page).as_deref(), Some("1 selected"));
    assert_eq!(ticked(&page), [visible]);
    // Nothing carries the others on.
    assert!(page.select("#picks > input[type=hidden][name=sel]").is_empty());
    assert!(page.one("#pickpage").value().attr("checked").is_some());
    for link in page.select("#bulk .fmt a") {
        let href = link.value().attr("href").unwrap();
        assert!(href.ends_with(&format!("sel={visible}")), "{href}");
    }
    let filters = page.form("#filters").submit_url();
    assert!(filters.ends_with(&format!("&sel={visible}")), "{filters}");

    let page = app.get_page(&format!("/?sel={hidden},{trashed}")).await;
    assert_eq!(selected(&page), None);
}

// ------------------------------------------------------------- the bulk zip

/// One Document below: its Test, kind, and the `(extension, bytes)` of the
/// files each Format zips.
struct Expected {
    test: i64,
    folder: String,
    base: &'static str,
    id: i64,
    kind: &'static str,
    pdf: Vec<(&'static str, Vec<u8>)>,
    original: Vec<(&'static str, Vec<u8>)>,
    both: Vec<(&'static str, Vec<u8>)>,
}

#[tokio::test]
async fn a_bulk_zip_has_a_folder_per_test_with_its_entries_for_each_group_and_format() {
    let app = TestApp::start().await;
    const OLD: &str = "1993 February Invitational - Plant City - Calculus-Mu Individual";
    const NEW: &str = "2001 State - FAMAT - Theta Team";
    let old = app.insert_test(&NewTest::known()).await;
    let new = app
        .insert_test(&NewTest {
            year: Some(2001),
            month: Month::NotApplicable,
            competition: Some("State"),
            division_level: Some("Theta"),
            test_type: Some("Team"),
            host: Some("FAMAT"),
            published: true,
        })
        .await;
    let folder = |test: i64, base: &str| format!("T{test} {base}");

    let test_pdf = sample_pdf(1_000);
    let answers_docx = sample_docx(1_100);
    let answers_pdf = sample_pdf(1_200);
    let errata_pdf = sample_pdf(1_300);
    let solutions_docx = sample_docx(1_400);
    let combined_pdf = sample_pdf(1_500);

    // Each Test's in kind display order; the newer Test comes first.
    let mut documents = Vec::new();
    let id = app
        .insert_document(&[new], NewDocument::docx("Solutions", &solutions_docx, None))
        .await;
    documents.push(Expected {
        test: new,
        folder: folder(new, NEW),
        base: NEW,
        id,
        kind: "Solutions",
        // No PDF yet, so "PDF" is the Original.
        pdf: vec![("docx", solutions_docx.clone())],
        original: vec![("docx", solutions_docx.clone())],
        both: vec![("docx", solutions_docx.clone())],
    });
    let id = app
        .insert_document(&[new], NewDocument::pdf("Combined Test+Solutions", &combined_pdf))
        .await;
    documents.push(Expected {
        test: new,
        folder: folder(new, NEW),
        base: NEW,
        id,
        kind: "Combined Test+Solutions",
        pdf: vec![("pdf", combined_pdf.clone())],
        original: vec![("pdf", combined_pdf.clone())],
        both: vec![("pdf", combined_pdf.clone())],
    });
    let id = app.insert_document(&[old], NewDocument::pdf("Test", &test_pdf)).await;
    documents.push(Expected {
        test: old,
        folder: folder(old, OLD),
        base: OLD,
        id,
        kind: "Test",
        pdf: vec![("pdf", test_pdf.clone())],
        original: vec![("pdf", test_pdf.clone())],
        both: vec![("pdf", test_pdf.clone())],
    });
    let id = app
        .insert_document(&[old], NewDocument::docx("Answers", &answers_docx, Some(&answers_pdf)))
        .await;
    documents.push(Expected {
        test: old,
        folder: folder(old, OLD),
        base: OLD,
        id,
        kind: "Answers",
        pdf: vec![("pdf", answers_pdf.clone())],
        original: vec![("docx", answers_docx.clone())],
        both: vec![("pdf", answers_pdf.clone()), ("docx", answers_docx.clone())],
    });
    let id = app
        .insert_document(&[old], NewDocument::pdf("Errata", &errata_pdf))
        .await;
    documents.push(Expected {
        test: old,
        folder: folder(old, OLD),
        base: OLD,
        id,
        kind: "Errata",
        pdf: vec![("pdf", errata_pdf.clone())],
        original: vec![("pdf", errata_pdf.clone())],
        both: vec![("pdf", errata_pdf.clone())],
    });

    let groups: [(&str, Option<&str>, &[&str]); 5] = [
        (
            "all",
            None,
            &["Solutions", "Combined Test+Solutions", "Test", "Answers", "Errata"],
        ),
        ("tests", Some("Tests"), &["Combined Test+Solutions", "Test"]),
        ("answers", Some("Answers"), &["Answers"]),
        (
            "solutions",
            Some("Solutions"),
            &["Solutions", "Combined Test+Solutions"],
        ),
        ("other", Some("Errata & other"), &["Errata"]),
    ];
    for (slug, label, kinds) in groups {
        for format in ["pdf", "original", "both"] {
            let chosen: Vec<&Expected> = documents.iter().filter(|d| kinds.contains(&d.kind)).collect();
            let mut tests: Vec<i64> = chosen.iter().map(|d| d.test).collect();
            tests.dedup();
            let expected: Vec<(String, Vec<u8>)> = chosen
                .iter()
                .flat_map(|d| {
                    let files = match format {
                        "pdf" => &d.pdf,
                        "original" => &d.original,
                        _ => &d.both,
                    };
                    files.iter().map(move |(extension, bytes)| {
                        (
                            format!("{}/T{} D{} {} - {}.{extension}", d.folder, d.test, d.id, d.base, d.kind),
                            bytes.clone(),
                        )
                    })
                })
                .collect();

            // Either Test order in the request gives the same zip.
            let path = format!("/tests/zip?sel={old}&sel={new}&group={slug}&format={format}");
            let (name, bytes) = zip(&app, &path).await;
            let n = match tests.len() {
                1 => "1 test".to_owned(),
                n => format!("{n} tests"),
            };
            let expected_name = match label {
                Some(label) => format!("FAMAT Test Bank - {n} - {label}.zip"),
                None => format!("FAMAT Test Bank - {n}.zip"),
            };
            assert_eq!(name, expected_name, "{path}");
            let got: Vec<(String, Vec<u8>)> = entries(&bytes)
                .into_iter()
                .inspect(|entry| assert!(entry.stored, "{path}: {} is Stored", entry.name))
                .map(|entry| (entry.name, entry.bytes))
                .collect();
            assert_eq!(
                got.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                expected.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                "{path}"
            );
            assert!(got == expected, "{path}: the bytes match");
        }
    }

    // The defaults are all documents as PDF, and the order asked for
    // doesn't matter.
    let (name, default) = zip(&app, &format!("/tests/zip?sel={new},{old}")).await;
    assert_eq!(name, "FAMAT Test Bank - 2 tests.zip");
    let (_, explicit) = zip(&app, &format!("/tests/zip?sel={old},{new}&group=all&format=pdf")).await;
    assert_eq!(names(&default), names(&explicit));

    // A group none of them has is a 404.
    let empty = app.insert_test(&NewTest::known()).await;
    let response = app.get(&format!("/tests/zip?sel={empty}")).await;
    assert_eq!(response.status(), 404);
    assert!(response.headers().get("content-disposition").is_none());
}

#[tokio::test]
async fn the_bars_buttons_zip_the_ticks_as_they_are_in_the_chosen_format() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(400);
    let docx = sample_docx(400);
    let tests = app.tests_with_a_pdf(3, "Plant City").await;
    let answers = app
        .insert_document(&[tests[0]], NewDocument::docx("Answers", &docx, Some(&pdf)))
        .await;

    let page = app.get_page(&format!("/?format=both&sel={}", tests[0])).await;
    assert_eq!(
        page.one("#bulk .split > button.all").value().attr("title"),
        Some("Download all (Both)")
    );
    // A box ticked since the page loaded goes along too, even without JS.
    let url = page
        .form("#picks")
        .check("sel", &tests[2].to_string(), true)
        .press("#bulk .menu > button[value=answers]")
        .submit_url();
    assert_eq!(
        url,
        format!("/tests/zip?format=both&group=answers&sel={}&sel={}", tests[0], tests[2])
    );
    let (name, bytes) = zip(&app, &url).await;
    assert_eq!(name, "FAMAT Test Bank - 1 test - Answers.zip");
    let files = names(&bytes);
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files[0].ends_with(&format!(
        "D{answers} 1903 February Invitational - Plant City - Calculus-Mu Individual - Answers.pdf"
    )));
    assert!(files[1].ends_with(" - Answers.docx"));

    let url = page
        .form("#picks")
        .check("sel", &tests[2].to_string(), true)
        .press("#bulk .split > button.all")
        .submit_url();
    let (name, bytes) = zip(&app, &url).await;
    assert_eq!(name, "FAMAT Test Bank - 2 tests.zip");
    // Both: each Test's PDF, plus the Answers' PDF and Original.
    let folders: Vec<String> = names(&bytes)
        .into_iter()
        .map(|name| name.split_once('/').unwrap().0.to_owned())
        .collect();
    assert_eq!(folders.len(), 4);
    assert!(
        folders[..3]
            .iter()
            .all(|f| f.starts_with(&format!("T{} 1903 ", tests[0])))
    );
    assert!(folders[3].starts_with(&format!("T{} 1901 ", tests[2])));
}

#[tokio::test]
async fn crafted_requests_never_zip_hidden_or_trashed_tests_or_documents() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let visible = app.tests_with_a_pdf(1, "Plant City").await[0];
    let hidden_document = app
        .insert_document(
            &[visible],
            NewDocument {
                published: false,
                ..NewDocument::pdf("Answers", &pdf)
            },
        )
        .await;
    let trashed_document = app.insert_document(&[visible], NewDocument::pdf("Errata", &pdf)).await;
    app.trash_document(trashed_document).await;
    let hidden = app.tests_with_a_pdf(1, "FAMAT").await[0];
    app.hide_test(hidden).await;
    let trashed = app.tests_with_a_pdf(1, "MAO").await[0];
    app.trash_test(trashed).await;

    let (name, bytes) = zip(&app, &format!("/tests/zip?sel={hidden},{visible},{trashed},987654")).await;
    assert_eq!(name, "FAMAT Test Bank - 1 test.zip");
    let names = names(&bytes);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with(&format!("T{visible} ")));
    for gone in [hidden_document, trashed_document] {
        assert!(!names[0].contains(&format!("D{gone} ")));
    }
    for group in ["answers", "other"] {
        let response = app.get(&format!("/tests/zip?sel={visible}&group={group}")).await;
        assert_eq!(
            response.status(),
            404,
            "only a Hidden or Trashed Document is in {group}"
        );
    }

    for path in [
        format!("/tests/zip?sel={hidden}"),
        format!("/tests/zip?sel={trashed}"),
        format!("/tests/zip?sel={hidden},{trashed}&group=tests&format=both"),
        "/tests/zip?sel=987654".to_owned(),
    ] {
        let response = app.get(&path).await;
        assert_eq!(response.status(), 404, "{path}");
        assert!(response.headers().get("content-disposition").is_none(), "{path}");
    }
}

#[tokio::test]
async fn a_bulk_zip_holds_at_most_500_tests() {
    let app = TestApp::start().await;
    let visible = app.tests_with_a_pdf(1, "Plant City").await[0];
    let others: Vec<i64> = (1_000_000..1_000_499).collect();

    // 500 different IDs are fine (only one exists).
    let mut five_hundred = vec![visible];
    five_hundred.extend(&others);
    let (name, _) = zip(&app, &format!("/tests/zip?sel={}", ids(&five_hundred))).await;
    assert_eq!(name, "FAMAT Test Bank - 1 test.zip");
    // Repeats count once.
    let (name, _) = zip(
        &app,
        &format!(
            "/tests/zip?sel={}&sel={}&sel={visible}",
            ids(&five_hundred),
            ids(&others)
        ),
    )
    .await;
    assert_eq!(name, "FAMAT Test Bank - 1 test.zip");

    // 501 is too many, whether they exist or not.
    let mut too_many = five_hundred.clone();
    too_many.push(2_000_000);
    let response = app.get(&format!("/tests/zip?sel={}", ids(&too_many))).await;
    assert_eq!(response.status(), 413);
    assert!(response.headers().get("content-disposition").is_none());
    assert_eq!(
        response.text().await.unwrap(),
        "A bulk download holds at most 500 tests. Select fewer."
    );
}

#[tokio::test]
async fn bad_bulk_requests_are_400() {
    let app = TestApp::start().await;
    let test = app.tests_with_a_pdf(1, "Plant City").await[0];

    for path in [
        "/tests/zip".to_owned(),
        "/tests/zip?sel=".to_owned(),
        "/tests/zip?sel=,,".to_owned(),
        "/tests/zip?sel=abc".to_owned(),
        format!("/tests/zip?sel={test},abc"),
        "/tests/zip?sel=-4".to_owned(),
        format!("/tests/zip?sel={test}&group=everything"),
        format!("/tests/zip?sel={test}&format=docx"),
    ] {
        let response = app.get(&path).await;
        assert_eq!(response.status(), 400, "{path}");
        assert!(response.headers().get("content-disposition").is_none(), "{path}");
    }
}
