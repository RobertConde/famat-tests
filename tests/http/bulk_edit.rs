//! Admin bulk actions on the search page's selection: Hidden Tests in an
//! Admin's search, the Needs review filter, **Edit selected** and **Merge
//! into…** (`/admin/tests/bulk`, and Merge into… on the Test edit page).
//! All as a browser with JavaScript off uses them, and every write is
//! checked over HTTP and in the change log it wrote.

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Form, Page, links_in, text, text_in},
};

// ------------------------------------------------------------------ helpers

/// The Tests the results list, top to bottom.
fn rows(page: &Page) -> Vec<i64> {
    page.select("#results tbody tr")
        .into_iter()
        .map(|row| row.value().attr("id").unwrap().trim_start_matches('T').parse().unwrap())
        .collect()
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("location")
        .map(|l| l.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

/// Send a filled-in POST form as a browser on this site would, and follow
/// its 303 (if any) to the page it lands on.
async fn submit(app: &TestApp, cookie: &str, form: Form<'_>) -> reqwest::Response {
    let fields = form.fields();
    let pairs: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    app.post_form(&form.action(), &pairs, &[("cookie", cookie)]).await
}

/// Submit and follow the 303 to the page it lands on.
async fn submit_and_follow(app: &TestApp, cookie: &str, form: Form<'_>) -> (String, Page) {
    let response = submit(app, cookie, form).await;
    let status = response.status();
    let location = location(&response);
    assert_eq!(status, 303, "{}", response.text().await.unwrap_or_default());
    (location.clone(), app.get_page_as(&location, Some(cookie)).await)
}

/// The search page as an Admin with `selected` selected when it loads,
/// then `ticked` ticked too, and a bulk bar button (such as **Edit
/// selected**) pressed: the bulk edit page it opens.
async fn open_bulk(app: &TestApp, cookie: &str, selected: &[i64], ticked: &[i64], button: &str) -> Page {
    let search = match selected {
        [] => "/".to_owned(),
        ids => format!("/?sel={}", ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")),
    };
    let page = app.get_page_as(&search, Some(cookie)).await;
    let mut form = page.form("#picks");
    for id in ticked {
        form = form.check("sel", &id.to_string(), true);
    }
    let url = form.press(button).submit_url();
    assert!(url.starts_with("/admin/tests/bulk?"), "{url}");
    app.get_page_as(&url, Some(cookie)).await
}

/// The Tests the bulk page lists as selected.
fn bulk_tests(page: &Page) -> Vec<i64> {
    page.select("#selected-tests tbody tr")
        .into_iter()
        .map(|row| row.value().attr("id").unwrap().trim_start_matches('T').parse().unwrap())
        .collect()
}

type Entry = (String, Option<String>, Option<String>);

/// The change log entries for one record, as `(field, before, after)`.
async fn logged(app: &TestApp, entity: &str, id: i64) -> Vec<Entry> {
    app.change_log()
        .await
        .into_iter()
        .filter(|(e, i, _, _, _)| e == entity && *i == id)
        .map(|(_, _, field, before, after)| (field, before, after))
        .collect()
}

fn entry(field: &str, before: Option<&str>, after: Option<&str>) -> Entry {
    (field.to_owned(), before.map(str::to_owned), after.map(str::to_owned))
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

/// The text of the Test's row on the bulk page.
fn bulk_row(page: &Page, test: i64) -> String {
    page.text(&format!("#selected-tests tr#T{test} .description"))
}

impl TestApp {
    async fn flag_test(&self, test: i64, comment: &str) {
        sqlx::query("UPDATE tests SET needs_review = true, needs_review_comment = $2 WHERE id = $1")
            .bind(test)
            .bind(comment)
            .execute(&self.db)
            .await
            .expect("flag Test");
    }

    async fn flag_document(&self, document: i64, comment: &str) {
        sqlx::query("UPDATE documents SET needs_review = true, needs_review_comment = $2 WHERE id = $1")
            .bind(document)
            .bind(comment)
            .execute(&self.db)
            .await
            .expect("flag Document");
    }
}

fn hidden(test: NewTest) -> NewTest {
    NewTest {
        published: false,
        ..test
    }
}

fn host(host: &'static str, year: i16) -> NewTest {
    NewTest {
        year: Some(year),
        host: Some(host),
        ..NewTest::known()
    }
}

// ------------------------------------------------------------ Hidden Tests

#[tokio::test]
async fn hidden_tests_show_only_to_admins_marked_hidden_and_selectable() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let published = app.insert_test(&host("Plant City", 1993)).await;
    app.insert_document(&[published], NewDocument::pdf("Test", &pdf)).await;
    let hidden_test = app.insert_test(&hidden(host("Chiles", 1987))).await;
    let document = app
        .insert_document(&[hidden_test], NewDocument::pdf("Answers", &pdf))
        .await;

    // Anonymous: never found, never selectable, never in the Years.
    let public = app.get_page("/").await;
    assert_eq!(rows(&public), [published]);
    assert_eq!(public.text("#count"), "1 test");
    assert!(public.select("option[value='1987']").is_empty());
    assert!(public.select(".tag").is_empty());
    let public = app.get_page(&format!("/?sel={hidden_test}&year=1987")).await;
    assert!(rows(&public).is_empty());
    assert!(
        public.select("#bulk #selected").is_empty(),
        "a Hidden Test is never selected"
    );
    assert!(public.select("#edit-selected").is_empty());

    // An Admin: found, marked Hidden, its Documents listed without links
    // or a download button (they can't be downloaded while it's Hidden).
    let cookie = app.admin().await;
    let page = app.get_page_as("/", Some(&cookie)).await;
    assert_eq!(rows(&page), [published, hidden_test]);
    assert_eq!(page.text("#count"), "2 tests");
    let row = page.one(&format!("tr#T{hidden_test}"));
    assert_eq!(text_in(row, ".tag.hidden"), "Hidden");
    assert_eq!(text_in(row, ".document"), "Answers");
    assert!(
        links_in(row).iter().all(|(_, href)| href.starts_with("/admin/")),
        "{:?}",
        links_in(row)
    );
    assert!(
        row.select(&scraper::Selector::parse(".split").unwrap())
            .next()
            .is_none()
    );
    let row = page.one(&format!("tr#T{published}"));
    assert!(row.select(&scraper::Selector::parse(".tag").unwrap()).next().is_none());
    assert_eq!(page.select("#f-year option[value='1987']").len(), 1);
    let found = app.get_page_as("/?year=1987", Some(&cookie)).await;
    assert_eq!(rows(&found), [hidden_test]);

    // An Admin can select it, and Edit selected is offered.
    let page = app.get_page_as(&format!("/?sel={hidden_test}"), Some(&cookie)).await;
    assert_eq!(page.text("#bulk #selected"), "1 selected");
    assert_eq!(page.text("#edit-selected"), "Edit selected");
    // Still not downloadable by anyone.
    let response = app.get(&format!("/tests/{hidden_test}/documents/{document}/pdf")).await;
    assert_eq!(response.status(), 404);
}

// ------------------------------------------------------ Needs review filter

#[tokio::test]
async fn the_needs_review_filter_finds_flagged_tests_and_documents_for_admins_only() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let flagged = app.insert_test(&host("A", 2001)).await;
    app.flag_test(flagged, "Is this Theta?").await;
    let with_flagged_document = app.insert_test(&host("B", 2002)).await;
    let d = app
        .insert_document(&[with_flagged_document], NewDocument::pdf("Test", &pdf))
        .await;
    app.flag_document(d, "AI unsure of the kind").await;
    let with_failed_pdf = app.insert_test(&host("C", 2003)).await;
    let docx = sample_docx(1_000);
    app.insert_document(
        &[with_failed_pdf],
        NewDocument {
            pdf_status: "failed",
            ..NewDocument::docx("Test", &docx, None)
        },
    )
    .await;
    let clean = app.insert_test(&host("D", 2004)).await;
    app.insert_document(&[clean], NewDocument::pdf("Test", &pdf)).await;
    let hidden_flagged = app.insert_test(&hidden(host("E", 2005))).await;
    app.flag_test(hidden_flagged, "Hidden until checked").await;
    let trashed_flag = app.insert_test(&host("F", 2006)).await;
    let t = app
        .insert_document(&[trashed_flag], NewDocument::pdf("Test", &pdf))
        .await;
    app.flag_document(t, "in the Trash").await;
    app.trash_document(t).await;
    let everything_public = [trashed_flag, clean, with_failed_pdf, with_flagged_document, flagged];

    // Anonymous visitors: no checkbox, and the parameter changes nothing.
    let public = app.get_page("/").await;
    assert!(public.select("#f-needs-review").is_empty());
    assert_eq!(rows(&public), everything_public);
    let crafted = app.get_page("/?needs_review=on").await;
    assert_eq!(rows(&crafted), everything_public);
    assert!(crafted.select("#needs-review-note, .tag").is_empty());
    assert!(
        crafted.select("input[name=needs_review]").is_empty(),
        "nothing carries the parameter on"
    );
    let next = crafted.form("#picks").submit_url();
    assert!(!next.contains("needs_review"), "{next}");

    // An Admin: the menu's Needs review link and the form's checkbox.
    let cookie = app.admin().await;
    let page = app.get_page_as("/", Some(&cookie)).await;
    let menu = links_in(page.one("#admin-menu"));
    assert!(
        menu.contains(&("Needs review".to_owned(), "/?needs_review=on".to_owned())),
        "{menu:?}"
    );
    assert!(page.one("#f-needs-review").value().attr("checked").is_none());
    assert_eq!(rows(&page).len(), 6, "Hidden ones too");
    let url = page.form("#filters").check("needs_review", "on", true).submit_url();
    assert!(url.ends_with("&needs_review=on"), "{url}");

    let filtered = app.get_page_as(&url, Some(&cookie)).await;
    assert_eq!(
        rows(&filtered),
        [hidden_flagged, with_failed_pdf, with_flagged_document, flagged]
    );
    assert_eq!(filtered.text("#count"), "4 tests");
    assert!(filtered.one("#f-needs-review").value().attr("checked").is_some());
    for test in rows(&filtered) {
        assert_eq!(
            text_in(filtered.one(&format!("tr#T{test}")), ".tag.review"),
            "Needs review"
        );
    }
    // The PDF problems page stays reachable from the filtered view.
    let note = links_in(filtered.one("#needs-review-note"));
    assert_eq!(note, [("PDF problems".to_owned(), "/admin/needs-review".to_owned())]);
    assert_eq!(
        app.get_page_as("/admin/needs-review", Some(&cookie)).await.text("h2"),
        "PDF problems"
    );
    // It combines with the other filters, and the selection form keeps it.
    let narrowed = app.get_page_as("/?needs_review=on&host=B", Some(&cookie)).await;
    assert_eq!(rows(&narrowed), [with_flagged_document]);
    let kept = filtered.form("#picks").submit_url();
    assert!(kept.contains("needs_review=on"), "{kept}");
}

// ------------------------------------------------------------ Edit selected

#[tokio::test]
async fn edit_selected_sets_one_facet_on_exactly_the_selected_tests_and_logs_each() {
    let app = TestApp::start().await;
    let a = app.insert_test(&host("Plant City", 2001)).await;
    let b = app.insert_test(&hidden(host("Plant City", 2002))).await;
    let already = app.insert_test(&host("Lawton Chiles", 2003)).await;
    let untouched = app.insert_test(&host("Plant City", 2004)).await;
    let cookie = app.admin().await;

    // Only Admins get the button; it opens the page with the ticks as they are.
    assert!(
        app.get_page(&format!("/?sel={a}"))
            .await
            .select("#edit-selected")
            .is_empty()
    );
    // Ticks made since the page loaded go along too.
    let page = open_bulk(&app, &cookie, &[a], &[b, already], "#edit-selected").await;
    assert_eq!(page.text("h2"), "Edit selected");
    assert_eq!(bulk_tests(&page), [a, b, already]);
    assert_eq!(page.text("#selected-heading"), "Selected (3 tests)");
    assert_eq!(
        bulk_row(&page, a),
        "2001 February Invitational · Plant City · Calculus/Mu Individual"
    );
    assert_eq!(page.text(&format!("#selected-tests tr#T{b} .visibility")), "Hidden");
    let back = page.one("#back").value().attr("href").unwrap().to_owned();
    // In results order: newest first.
    assert_eq!(back, format!("/?sel={already}%2C{b}%2C{a}"));

    let chiles = option_value(&page, "#set-host select[name=host]", "Lawton Chiles");
    let form = page.post_form("#set-host").select("host", &chiles);
    let (location, page) = submit_and_follow(&app, &cookie, form).await;
    assert!(location.starts_with("/admin/tests/bulk?"), "{location}");
    assert_eq!(
        page.text("#outcome"),
        "Host set to Lawton Chiles: 2 changed, 1 unchanged."
    );
    assert!(page.select("#refusals").is_empty());
    for test in [a, b, already] {
        assert!(bulk_row(&page, test).contains("· Lawton Chiles ·"), "T{test}");
    }
    assert_eq!(page.one("#back").value().attr("href"), Some(back.as_str()));

    // Exactly the selected Tests changed, and each change is logged.
    let search = app.get_page_as("/?host=Plant%20City", Some(&cookie)).await;
    assert_eq!(rows(&search), [untouched]);
    for test in [a, b] {
        assert_eq!(
            logged(&app, "test", test).await,
            [entry("host", Some("Plant City"), Some("Lawton Chiles"))]
        );
    }
    assert!(logged(&app, "test", already).await.is_empty());
    assert!(logged(&app, "test", untouched).await.is_empty());

    // + New value creates the value once, and logs it.
    let page = open_bulk(&app, &cookie, &[a, b], &[], "#edit-selected").await;
    let form = page
        .post_form("#set-test_type")
        .select("test_type", "new")
        .fill("test_type_new", "Ciphering");
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(
        page.text("#outcome"),
        "Test type set to Ciphering: 2 changed, 0 unchanged."
    );
    let created: Vec<_> = app
        .change_log()
        .await
        .into_iter()
        .filter(|(e, _, f, _, _)| e == "facet_value" && f == "name")
        .map(|(_, _, _, _, after)| after.unwrap())
        .collect();
    assert_eq!(created, ["Ciphering"]);
    assert_eq!(
        logged(&app, "test", a).await[1..],
        [entry("test_type", Some("Individual"), Some("Ciphering"))]
    );
    let public = app.get_page("/?test_type=Ciphering").await;
    assert_eq!(rows(&public), [a], "the Hidden one stays Hidden");

    // The Year, typed, and the Month as Not applicable.
    let page = open_bulk(&app, &cookie, &[a], &[], "#edit-selected").await;
    let form = page
        .post_form("#set-year")
        .select("year", "new")
        .fill("year_new", "1999");
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(page.text("#outcome"), "Year set to 1999: 1 changed, 0 unchanged.");
    let form = page.post_form("#set-month").select("month", "not_applicable");
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(
        page.text("#outcome"),
        "Month set to Not applicable: 1 changed, 0 unchanged."
    );
    assert_eq!(
        bulk_row(&page, a),
        "1999 Invitational · Lawton Chiles · Calculus/Mu Ciphering"
    );
    assert_eq!(
        logged(&app, "test", a).await[2..],
        [
            entry("year", Some("2001"), Some("1999")),
            entry("month", Some("February"), Some("Not applicable")),
        ]
    );
}

#[tokio::test]
async fn edit_selected_applies_the_publish_rule_per_test_and_reports_refusals() {
    let app = TestApp::start().await;
    let year_known = app.insert_test(&host("A", 2001)).await;
    let no_year = NewTest {
        year: None,
        ..NewTest::known()
    };
    let published_no_year = app.insert_test(&no_year).await;
    let hidden_no_year = app.insert_test(&hidden(no_year.clone())).await;
    let trashed = app.insert_test(&host("B", 2002)).await;
    let cookie = app.admin().await;

    let selected = [year_known, published_no_year, hidden_no_year];
    let page = open_bulk(&app, &cookie, &selected, &[], "#edit-selected").await;
    app.trash_test(trashed).await;
    // A Test trashed since, as a stale page would still post it.
    let selection = format!("{year_known},{published_no_year},{hidden_no_year},{trashed}");
    let form = page
        .post_form("#set-competition")
        .select("competition", "unknown")
        .fill("sel", &selection);
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(
        page.text("#outcome p"),
        "Competition set to Unknown: 2 changed, 0 unchanged, 2 refused."
    );
    assert!(
        page.text(&format!("#refused-T{published_no_year}"))
            .contains("Year and Competition both unknown"),
    );
    assert!(page.text(&format!("#refused-T{trashed}")).contains("in the Trash"));
    assert_eq!(page.select("#refusals li").len(), 2);

    // The refused Test kept its Competition and stays Published.
    assert!(logged(&app, "test", published_no_year).await.is_empty());
    let public = app.get_page("/?competition=Invitational").await;
    assert_eq!(rows(&public), [published_no_year]);
    for test in [year_known, hidden_no_year] {
        assert_eq!(
            logged(&app, "test", test).await,
            [entry("competition", Some("Invitational"), None)]
        );
    }
    assert!(logged(&app, "test", trashed).await.is_empty());
}

#[tokio::test]
async fn edit_selected_ticks_and_clears_needs_review_on_the_selected_tests() {
    let app = TestApp::start().await;
    let a = app.insert_test(&host("A", 2001)).await;
    let b = app.insert_test(&host("B", 2002)).await;
    app.flag_test(b, "Wrong host?").await;
    let other = app.insert_test(&host("C", 2003)).await;
    let cookie = app.admin().await;

    let page = open_bulk(&app, &cookie, &[a, b], &[], "#edit-selected").await;
    let form = page
        .post_form("#set-review")
        .fill("needs_review_comment", "AI labels unchecked")
        .press("button[value=tick]");
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(page.text("#outcome"), "Needs review ticked: 2 changed, 0 unchanged.");
    assert_eq!(page.text(&format!("#selected-tests tr#T{a} .needs-review")), "Yes");
    assert_eq!(
        logged(&app, "test", a).await,
        [
            entry("needs_review", Some("false"), Some("true")),
            entry("needs_review_comment", None, Some("AI labels unchecked")),
        ]
    );
    // An existing reason is kept, with the new one on its own line.
    assert_eq!(
        logged(&app, "test", b).await,
        [entry(
            "needs_review_comment",
            Some("Wrong host?"),
            Some("Wrong host?\nAI labels unchecked")
        )]
    );
    let edit = app.get_page_as(&format!("/admin/tests/{b}/edit"), Some(&cookie)).await;
    assert_eq!(
        edit.one("form#edit-test textarea").text().collect::<String>(),
        "Wrong host?\nAI labels unchecked"
    );

    // Ticking again changes nothing.
    let form = page
        .post_form("#set-review")
        .fill("needs_review_comment", "AI labels unchecked")
        .press("button[value=tick]");
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(page.text("#outcome"), "Needs review ticked: 0 changed, 2 unchanged.");

    // The filter lists them, and clearing takes them off it.
    let filtered = app.get_page_as("/?needs_review=on", Some(&cookie)).await;
    assert_eq!(rows(&filtered), [b, a]);
    let form = page.post_form("#set-review").press("button[value=clear]");
    let (_, page) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(page.text("#outcome"), "Needs review cleared: 2 changed, 0 unchanged.");
    assert!(rows(&app.get_page_as("/?needs_review=on", Some(&cookie)).await).is_empty());
    assert_eq!(
        logged(&app, "test", a).await[2..],
        [
            entry("needs_review", Some("true"), Some("false")),
            entry("needs_review_comment", Some("AI labels unchecked"), None),
        ]
    );
    assert!(logged(&app, "test", other).await.is_empty());
}

// ---------------------------------------------------------------- Merge

#[tokio::test]
async fn merge_moves_every_document_keeps_multi_test_attachments_and_trashes_the_empty_test() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let from = app.insert_test(&host("Chiles", 2001)).await;
    let into = app.insert_test(&host("Lawton Chiles", 2001)).await;
    let elsewhere = app.insert_test(&host("Plant City", 2002)).await;
    let only_from = app.insert_document(&[from], NewDocument::pdf("Test", &pdf)).await;
    let on_both = app
        .insert_document(&[from, into], NewDocument::pdf("Answers", &pdf))
        .await;
    let master_key = app
        .insert_document(&[from, elsewhere], NewDocument::pdf("Solutions", &pdf))
        .await;
    let trashed = app.insert_document(&[from], NewDocument::pdf("Errata", &pdf)).await;
    app.trash_document(trashed).await;
    let own = app.insert_document(&[into], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;

    // Merge… shows in the bar only with exactly two selected.
    let one = app.get_page_as(&format!("/?sel={from}"), Some(&cookie)).await;
    assert!(one.select("#merge-selected").is_empty());
    let three = open_bulk(&app, &cookie, &[from, into, elsewhere], &[], "#edit-selected").await;
    assert_eq!(three.select("#merge-needs-two").len(), 1);
    assert!(three.select("form.merge").is_empty());

    let page = open_bulk(&app, &cookie, &[from, into], &[], "#merge-selected").await;
    let buttons: Vec<String> = page.select("form.merge button").into_iter().map(text).collect();
    assert_eq!(
        buttons,
        [
            format!("Merge T{from} into T{into}"),
            format!("Merge T{into} into T{from}")
        ]
    );
    let form = page.post_form(&format!("#merge-T{from}-into-T{into}"));
    let (location, edit) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(location, format!("/admin/tests/{into}/edit"));

    // Every untrashed Document is now on the survivor, each once.
    let sections: Vec<String> = edit
        .select("section.document")
        .into_iter()
        .map(|s| s.value().attr("id").unwrap().to_owned())
        .collect();
    assert_eq!(sections, [only_from, on_both, master_key, own].map(|d| format!("D{d}")));
    let on = |document: i64| -> Vec<String> {
        edit.select(&format!("section#D{document} .on-test"))
            .into_iter()
            .map(|e| e.value().attr("id").unwrap().to_owned())
            .collect()
    };
    assert_eq!(on(on_both), [format!("D{on_both}-on-T{into}")]);
    assert_eq!(
        on(master_key),
        [
            format!("D{master_key}-on-T{into}"),
            format!("D{master_key}-on-T{elsewhere}")
        ]
    );
    let public = app.get_page("/").await;
    assert_eq!(rows(&public), [elsewhere, into]);
    let documents: Vec<String> = public
        .select(&format!("tr#T{into} .document > a:first-child"))
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(documents, ["Test", "Test", "Answers", "Solutions"]);
    let response = app.get(&format!("/tests/{into}/documents/{only_from}/pdf")).await;
    assert_eq!(response.status(), 200);
    let response = app.get(&format!("/tests/{elsewhere}/documents/{master_key}/pdf")).await;
    assert_eq!(response.status(), 200);

    // The empty Test is in the Trash: gone for everyone, Admins included.
    let admin_search = app.get_page_as("/", Some(&cookie)).await;
    assert!(!rows(&admin_search).contains(&from));
    let gone = app
        .get_page_as(&format!("/admin/tests/{from}/edit"), Some(&cookie))
        .await;
    assert_eq!(gone.select("#no-test").len(), 1);

    // Each moved Document's Tests, and the merged Test, are logged.
    let (f, i, e) = (format!("T{from}"), format!("T{into}"), format!("T{elsewhere}"));
    assert_eq!(
        logged(&app, "document", only_from).await,
        [entry("tests", Some(&f), Some(&i))]
    );
    assert_eq!(
        logged(&app, "document", on_both).await,
        [entry("tests", Some(&format!("{f}, {i}")), Some(&i))]
    );
    assert_eq!(
        logged(&app, "document", master_key).await,
        [entry("tests", Some(&format!("{f}, {e}")), Some(&format!("{i}, {e}")))]
    );
    assert_eq!(
        logged(&app, "document", trashed).await,
        [entry("tests", Some(&f), Some(&i))]
    );
    assert!(logged(&app, "document", own).await.is_empty());
    assert_eq!(
        logged(&app, "test", from).await,
        [
            entry("merged_into", None, Some(&i)),
            entry("published", Some("true"), Some("false")),
            entry("trashed", Some("false"), Some("true")),
        ]
    );
    assert!(logged(&app, "test", into).await.is_empty());
}

#[tokio::test]
async fn merge_into_on_the_edit_page_merges_into_a_test_typed_by_id() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let from = app.insert_test(&host("A", 2001)).await;
    let into = app.insert_test(&hidden(host("B", 2001))).await;
    let trashed = app.insert_test(&host("C", 2001)).await;
    app.trash_test(trashed).await;
    let document = app.insert_document(&[from], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let edit_path = format!("/admin/tests/{from}/edit");
    let page = app.get_page_as(&edit_path, Some(&cookie)).await;

    // Refused, with nothing changed: itself, a missing Test, a trashed one,
    // nothing typed.
    for (typed, status, message) in [
        (format!("T{from}"), 400, "merged into itself"),
        ("T999999".to_owned(), 404, "T999999 does not exist"),
        (format!("{trashed}"), 404, "in the Trash"),
        ("abc".to_owned(), 400, "Type the ID"),
    ] {
        let form = page.post_form("form#merge-test").fill("into", &typed);
        let response = submit(&app, &cookie, form).await;
        assert_eq!(response.status(), status, "{typed}");
        let refusal = Page::parse(&response.text().await.unwrap());
        assert!(
            refusal.text("#refusal").contains(message),
            "{typed}: {}",
            refusal.text("#refusal")
        );
        assert_eq!(links_in(refusal.one("main"))[0].1, edit_path);
    }
    assert!(app.change_log().await.is_empty());

    let form = page.post_form("form#merge-test").fill("into", &format!("T{into}"));
    let (location, edit) = submit_and_follow(&app, &cookie, form).await;
    assert_eq!(location, format!("/admin/tests/{into}/edit"));
    assert_eq!(edit.select(&format!("section#D{document}")).len(), 1);
    let gone = app.get_page_as(&edit_path, Some(&cookie)).await;
    assert_eq!(gone.select("#no-test").len(), 1);
    // The survivor is Hidden, so nothing is public any more.
    assert!(rows(&app.get_page("/").await).is_empty());
}

// ------------------------------------------------------------------- auth

#[tokio::test]
async fn bulk_writes_need_an_admin_session_and_a_valid_csrf_token() {
    let app = TestApp::start().await;
    let a = app.insert_test(&host("A", 2001)).await;
    let b = app.insert_test(&host("B", 2002)).await;
    let pdf = sample_pdf(1_000);
    app.insert_document(&[a], NewDocument::pdf("Test", &pdf)).await;

    // The page itself: anonymous visitors are told to log in.
    let anonymous = app.get_page(&format!("/admin/tests/bulk?sel={a},{b}")).await;
    assert_eq!(anonymous.select("#not-logged-in").len(), 1);
    assert!(anonymous.select("form").is_empty());
    assert_eq!(app.get(&format!("/api/admin/tests/bulk?sel={a}")).await.status(), 401);

    let cookie = app.admin().await;
    let page = open_bulk(&app, &cookie, &[a, b], &[], "#edit-selected").await;
    let csrf = |token: &str, fields: Vec<(String, String)>| -> Vec<(String, String)> {
        fields
            .into_iter()
            .map(|(name, value)| {
                let value = if name == "csrf" { token.to_owned() } else { value };
                (name, value)
            })
            .collect()
    };
    let forms: Vec<(String, Vec<(String, String)>)> = [
        page.post_form("#set-host").select("host", "unknown"),
        page.post_form("#set-review").press("button[value=tick]"),
        page.post_form(&format!("#merge-T{a}-into-T{b}")),
    ]
    .into_iter()
    .map(|form| (form.action(), form.fields()))
    .collect();
    let edit = app.get_page_as(&format!("/admin/tests/{a}/edit"), Some(&cookie)).await;
    let merge = edit.post_form("form#merge-test").fill("into", &b.to_string());
    let forms: Vec<_> = forms.into_iter().chain([(merge.action(), merge.fields())]).collect();

    let post = async |path: &str, fields: &[(String, String)], headers: &[(&str, &str)]| {
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        app.post_form(path, &pairs, headers).await.status()
    };
    for (path, fields) in forms {
        assert_eq!(post(&path, &fields, &[]).await, 401, "anonymous POST {path}");
        let bad = csrf("1.bad", fields.clone());
        assert_eq!(
            post(&path, &bad, &[("cookie", &cookie)]).await,
            403,
            "bad CSRF POST {path}"
        );
        let cross_site = [("cookie", cookie.as_str()), ("origin", "https://evil.example")];
        assert_eq!(post(&path, &fields, &cross_site).await, 403, "cross-site POST {path}");
    }

    // Nothing changed.
    assert!(app.change_log().await.is_empty());
    let public = app.get_page("/").await;
    assert_eq!(rows(&public), [b, a]);
    let filtered = app.get_page_as("/?needs_review=on", Some(&cookie)).await;
    assert!(rows(&filtered).is_empty());
}
