//! Each result row's split download button and its menu, as served before
//! any JavaScript runs: a link for the icon, and a `<details>` disclosure of
//! plain links for ▾, so the menu works with the keyboard and without JS.

use scraper::ElementRef;

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Page, links_in, text},
};

/// The split button of a Test's row.
fn split<'a>(page: &'a Page, test: i64) -> ElementRef<'a> {
    page.one(&format!("tr#T{test} td.download .split"))
}

/// `(label, count, href)` of each kind group in a Test's menu.
fn groups(page: &Page, test: i64) -> Vec<(String, String, String)> {
    page.select(&format!("tr#T{test} .split details .menu > a"))
        .into_iter()
        .map(|a| {
            let count = a
                .select(&scraper::Selector::parse(".n").unwrap())
                .next()
                .map(text)
                .expect("a count");
            let label = text(a).strip_suffix(&count).unwrap().trim().to_owned();
            (label, count, a.value().attr("href").unwrap().to_owned())
        })
        .collect()
}

/// `(label, href, aria-current)` of each Format switch link.
fn formats(page: &Page, test: i64) -> Vec<(String, String, String)> {
    page.select(&format!("tr#T{test} .split .menu .fmt a"))
        .into_iter()
        .map(|a| {
            (
                text(a),
                a.value().attr("href").unwrap().to_owned(),
                a.value().attr("aria-current").unwrap_or("").to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_icon_downloads_all_documents_as_pdf() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(1_000)))
        .await;

    let page = app.get_page("/").await;
    let icon = split(&page, test)
        .select(&scraper::Selector::parse(":scope > a").unwrap())
        .next()
        .expect("the icon is a link");
    assert_eq!(icon.value().attr("href"), Some(format!("/tests/{test}/zip").as_str()));
    assert_eq!(icon.value().attr("aria-label"), Some("Download all documents"));
    assert_eq!(icon.value().attr("title"), Some("Download all (PDF)"));
    assert!(
        icon.select(&scraper::Selector::parse("svg").unwrap()).next().is_some(),
        "a download icon"
    );

    let response = app.get(icon.value().attr("href").unwrap()).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/zip");
}

#[tokio::test]
async fn the_menu_lists_only_non_empty_kind_groups_with_counts_and_combined_under_both() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    app.insert_document(&[test], NewDocument::pdf("Combined Test+Solutions", &pdf))
        .await;
    app.insert_document(&[test], NewDocument::docx("Answers", &docx, Some(&pdf)))
        .await;
    app.insert_document(
        &[test],
        NewDocument {
            label: Some("Part 2"),
            ..NewDocument::pdf("Answers", &pdf)
        },
    )
    .await;
    // Neither counts.
    app.insert_document(
        &[test],
        NewDocument {
            published: false,
            ..NewDocument::pdf("Errata", &pdf)
        },
    )
    .await;
    let trashed = app.insert_document(&[test], NewDocument::pdf("Solutions", &pdf)).await;
    app.trash_document(trashed).await;

    let page = app.get_page("/").await;
    let row = |label: &str, count: &str, query: &str| {
        (label.to_owned(), count.to_owned(), format!("/tests/{test}/zip{query}"))
    };
    assert_eq!(
        groups(&page, test),
        [
            row("All documents", "4", ""),
            row("Tests", "2", "?group=tests"),
            row("Answers", "2", "?group=answers"),
            row("Solutions", "1", "?group=solutions"),
        ],
        "no Errata & other: the only Errata is Hidden"
    );

    // Every choice is a real zip.
    for (_, _, href) in groups(&page, test) {
        let response = app.get(&href).await;
        assert_eq!(response.status(), 200, "GET {href}");
        assert_eq!(response.headers()["content-type"], "application/zip", "GET {href}");
    }

    let only_errata = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[only_errata], NewDocument::pdf("Errata", &pdf))
        .await;
    app.insert_document(&[only_errata], NewDocument::zip("Other", &docx))
        .await;
    let page = app.get_page("/").await;
    assert_eq!(
        groups(&page, only_errata),
        [
            (
                "All documents".to_owned(),
                "2".to_owned(),
                format!("/tests/{only_errata}/zip")
            ),
            (
                "Errata & other".to_owned(),
                "2".to_owned(),
                format!("/tests/{only_errata}/zip?group=other")
            ),
        ]
    );
}

#[tokio::test]
async fn the_menu_is_a_closed_disclosure_of_plain_links_so_it_works_by_keyboard_and_without_js() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(1_000)))
        .await;

    let page = app.get_page("/").await;
    let details = page.one(&format!("tr#T{test} .split details"));
    assert!(
        details.value().attr("open").is_none(),
        "closed until clicked: it never opens on hover"
    );
    // The toggle is the details' own summary, which browsers make focusable
    // and open with Enter or Space, with or without JS.
    let summary = page.one(&format!("tr#T{test} .split details > summary"));
    assert_eq!(text(summary), "▾");
    assert_eq!(summary.value().attr("aria-label"), Some("Choose what to download"));
    // Choices are links: reachable with Tab, chosen with Enter.
    let menu = page.one(&format!("tr#T{test} .split details > .menu"));
    let links = links_in(menu);
    assert_eq!(links.len(), 2 + 3, "two kind groups and three Formats: {links:?}");
    assert!(
        page.select(&format!("tr#T{test} .split [role=menu], tr#T{test} .split button"))
            .is_empty(),
        "a disclosure of links, not an ARIA menu that would need arrow keys"
    );
}

#[tokio::test]
async fn the_format_switch_reloads_the_page_and_every_download_link_follows_it() {
    let app = TestApp::start().await;
    let docx = sample_docx(1_000);
    let pdf = sample_pdf(1_000);
    let test = app.insert_test(&NewTest::known()).await;
    let document = app
        .insert_document(&[test], NewDocument::docx("Solutions", &docx, Some(&pdf)))
        .await;

    let page = app.get_page("/?host=Plant+City").await;
    assert_eq!(
        formats(&page, test),
        [
            ("PDF".to_owned(), "/?host=Plant+City".to_owned(), "true".to_owned()),
            (
                "Original".to_owned(),
                "/?host=Plant+City&format=original".to_owned(),
                "false".to_owned()
            ),
            (
                "Both".to_owned(),
                "/?host=Plant+City&format=both".to_owned(),
                "false".to_owned()
            ),
        ]
    );

    // Choosing Original reloads the page with it.
    let page = app.get_page("/?host=Plant+City&format=original").await;
    let current: Vec<String> = formats(&page, test)
        .into_iter()
        .filter(|(_, _, current)| current == "true")
        .map(|(label, _, _)| label)
        .collect();
    assert_eq!(current, ["Original"]);
    let icon = page.one(&format!("tr#T{test} .split > a"));
    assert_eq!(
        icon.value().attr("href"),
        Some(format!("/tests/{test}/zip?format=original").as_str())
    );
    assert_eq!(icon.value().attr("title"), Some("Download all (Original)"));
    let hrefs: Vec<String> = groups(&page, test).into_iter().map(|(_, _, href)| href).collect();
    assert_eq!(
        hrefs,
        [
            format!("/tests/{test}/zip?format=original"),
            format!("/tests/{test}/zip?group=solutions&format=original"),
        ]
    );
    let response = app.get(&hrefs[1]).await;
    assert_eq!(response.status(), 200);
    let bytes = response.bytes().await.unwrap();
    assert_eq!(
        crate::support::zip::names(&bytes),
        [format!(
            "T{test} D{document} 1993 February Invitational - Plant City - Calculus-Mu Individual - Solutions.docx"
        )]
    );

    // Searching again keeps it, and a nonsense Format is the default.
    let url = page.form("#filters").select("host", "").submit_url();
    assert!(url.ends_with("format=original"), "{url}");
    let page = app.get_page("/?format=docx").await;
    assert_eq!(
        page.one(&format!("tr#T{test} .split > a")).value().attr("href"),
        Some(format!("/tests/{test}/zip").as_str())
    );
}

#[tokio::test]
async fn the_format_is_kept_across_pages() {
    let app = TestApp::start().await;
    for _ in 0..51 {
        let test = app.insert_test(&NewTest::known()).await;
        app.insert_document(&[test], NewDocument::pdf("Test", &sample_pdf(100)))
            .await;
    }

    let page = app.get_page("/?format=both").await;
    let next = page.form("#picks").press("#next").submit_url();
    assert_eq!(next, "/?format=both&page=2");
    let page = app.get_page(&next).await;
    assert_eq!(page.form("#picks").press("#prev").submit_url(), "/?format=both&page=1");
    let original = page
        .select("#results tbody tr .split .fmt a")
        .into_iter()
        .find(|a| text(*a) == "Original")
        .unwrap();
    assert_eq!(
        original.value().attr("href"),
        Some("/?format=original&page=2"),
        "switching Format stays on this page"
    );
}

#[tokio::test]
async fn a_test_without_visible_documents_has_no_download_button() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(
        &[test],
        NewDocument {
            published: false,
            ..NewDocument::pdf("Test", &sample_pdf(100))
        },
    )
    .await;

    let page = app.get_page("/").await;
    assert!(page.select(&format!("tr#T{test} .split")).is_empty());
}
