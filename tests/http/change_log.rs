//! The **Change log** page (`/admin/change-log`): the entries the other
//! Admin pages write, newest first and grouped by Admin action, with each
//! record linked while it exists, a filter and a pager.

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest, sample_pdf},
    html::{Form, Page, text, text_in},
};

/// One entry as the page shows it: `(entry id, record, link, field,
/// before, after)`.
type Shown = (i64, String, Option<String>, String, String, String);

fn edit_path(test: i64) -> String {
    format!("/admin/tests/{test}/edit")
}

async fn submit(app: &TestApp, cookie: &str, form: Form<'_>) -> reqwest::Response {
    let fields = form.fields();
    let pairs: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    app.post_form(&form.action(), &pairs, &[("cookie", cookie)]).await
}

async fn assert_303(response: reqwest::Response) {
    let status = response.status();
    assert_eq!(status, 303, "{}", response.text().await.unwrap_or_default());
}

/// The page's actions, each as its entries.
fn actions(page: &Page) -> Vec<Vec<Shown>> {
    let entry = scraper::Selector::parse("tr.entry").unwrap();
    let link = scraper::Selector::parse("td.record a").unwrap();
    page.select(".change-log section.action")
        .into_iter()
        .map(|action| {
            action
                .select(&entry)
                .map(|row| {
                    let id = row.value().attr("id").unwrap().trim_start_matches('C').parse().unwrap();
                    let href = row
                        .select(&link)
                        .next()
                        .map(|a| a.value().attr("href").unwrap().to_owned());
                    (
                        id,
                        text_in(row, "td.record"),
                        href,
                        text_in(row, "td.field"),
                        text_in(row, ".before"),
                        text_in(row, ".after"),
                    )
                })
                .collect()
        })
        .collect()
}

/// An entry as compared: `(entry id, field, before, after)`.
type Compared = (i64, String, String, String);

/// Every change log entry, grouped as the page should group them: newest
/// action first, entries in the order they were written.
async fn expected_groups(app: &TestApp) -> Vec<Vec<Compared>> {
    // `—` is how the page shows an unknown or absent value.
    let rows: Vec<(String, i64, String, String, String)> = sqlx::query_as(
        "SELECT changed_at::text, id, replace(field, '_', ' '), coalesce(before, '—'), coalesce(after, '—')
         FROM change_log ORDER BY changed_at DESC, id",
    )
    .fetch_all(&app.db)
    .await
    .unwrap();
    let mut groups: Vec<(String, Vec<Compared>)> = Vec::new();
    for (at, id, field, before, after) in rows {
        let entry = (id, field, before, after);
        match groups.last_mut() {
            Some((last, entries)) if *last == at => entries.push(entry),
            _ => groups.push((at, vec![entry])),
        }
    }
    groups.into_iter().map(|(_, entries)| entries).collect()
}

#[tokio::test]
async fn the_change_log_shows_every_admin_write_newest_first_with_linked_ids() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let pdf = sample_pdf(1_200);

    // Add test (#23), with new Facet values.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_calculus.pdf", &pdf);
    let test = app.add_test(&cookie, form, "publish").await;
    let page = app.get_page_as(&edit_path(test), Some(&cookie)).await;
    let section = page.select("section.document")[0];
    let document: i64 = section.value().attr("id").unwrap()[1..].parse().unwrap();

    // The Test edit page (#25): the Test, then the Document.
    let form = page
        .post_form("form#edit-test")
        .select("host", "new")
        .fill("host_new", "Lawton Chiles");
    assert_303(submit(&app, &cookie, form).await).await;
    let page = app.get_page_as(&edit_path(test), Some(&cookie)).await;
    let form = page
        .post_form(&format!("section#D{document} form.save-document"))
        .fill("label", "Part 1");
    assert_303(submit(&app, &cookie, form).await).await;

    // Facet values (#28) and Document kinds (#29).
    let plant_city = app.facet_value("host", "Plant City").await.to_string();
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let response = app
        .post_form(
            "/api/admin/facet-values/rename",
            &[("csrf", &csrf), ("id", &plant_city), ("name", "Plant City HS")],
            &[("cookie", &cookie)],
        )
        .await;
    assert_303(response).await;
    let answers: i64 = sqlx::query_scalar("SELECT id FROM document_kinds WHERE name = 'Answers'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let answers_id = answers.to_string();
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let response = app
        .post_form(
            "/api/admin/document-kinds/save",
            &[
                ("csrf", &csrf),
                ("id", &answers_id),
                ("name", "Answer key"),
                ("answers", "true"),
            ],
            &[("cookie", &cookie)],
        )
        .await;
    assert_303(response).await;

    // Trash (#26).
    let page = app.get_page_as(&edit_path(test), Some(&cookie)).await;
    assert_303(
        submit(
            &app,
            &cookie,
            page.post_form(&format!("section#D{document} form.remove-document")),
        )
        .await,
    )
    .await;

    let page = app.get_page_as("/admin/change-log", Some(&cookie)).await;
    assert!(page.select("#admin-menu a[href='/admin/change-log']").len() == 1);
    let shown = actions(&page);
    let expected = expected_groups(&app).await;
    assert!(expected.len() >= 6, "{expected:?}");
    assert_eq!(shown.len(), expected.len(), "one block per Admin action");
    for (shown, expected) in shown.iter().zip(&expected) {
        let shown: Vec<_> = shown
            .iter()
            .map(|(id, _, _, field, before, after)| (*id, field.clone(), before.clone(), after.clone()))
            .collect();
        assert_eq!(&shown, expected);
    }

    // Newest first: the Remove, the kind, the Facet value, the Document,
    // the Test, then Add test's own entries.
    let record = |action: &[Shown]| -> (String, Option<String>) {
        let (_, record, link, ..) = &action[0];
        (record.clone(), link.clone())
    };
    let trash_link = Some(format!("/admin/trash#D{document}"));
    assert_eq!(record(&shown[0]), (format!("D{document}"), trash_link));
    assert_eq!(
        shown[0]
            .iter()
            .map(|e| (e.3.as_str(), e.4.as_str(), e.5.as_str()))
            .collect::<Vec<_>>(),
        [("trashed", "false", "true")]
    );
    assert_eq!(
        record(&shown[1]),
        (
            format!("Document kind {answers}"),
            Some(format!("/admin/document-kinds#K{answers}"))
        )
    );
    assert!(
        shown[1]
            .iter()
            .any(|e| e.3 == "name" && e.4 == "Answers" && e.5 == "Answer key")
    );
    assert_eq!(
        record(&shown[2]),
        (
            format!("Facet value {plant_city}"),
            Some(format!("/admin/facets?facet=host#v{plant_city}"))
        )
    );
    assert!(
        shown[2]
            .iter()
            .any(|e| e.3 == "name" && e.4 == "Plant City" && e.5 == "Plant City HS")
    );
    assert_eq!(record(&shown[3]).0, format!("D{document}"));
    assert!(shown[3].iter().any(|e| e.3 == "label" && e.4 == "—" && e.5 == "Part 1"));
    // The Test's entries, and the new Host value created with them.
    assert!(
        shown[4].iter().any(|e| e.1 == format!("T{test}")
            && e.2.as_deref() == Some(edit_path(test).as_str())
            && e.3 == "host"
            && e.4 == "Plant City"
            && e.5 == "Lawton Chiles"),
        "{:?}",
        shown[4]
    );
    assert!(
        shown[4]
            .iter()
            .any(|e| e.1.starts_with("Facet value ") && e.3 == "name" && e.5 == "Lawton Chiles")
    );
    // Add test created the Test and its Document in one action.
    let created = shown.last().unwrap();
    assert!(
        created
            .iter()
            .any(|e| e.1 == format!("T{test}") && e.3 == "id" && e.5 == format!("T{test}"))
    );
    assert!(
        created
            .iter()
            .any(|e| e.1 == format!("D{document}") && e.3 == "id" && e.4 == "—")
    );
    // Each action is headed by when it happened.
    for heading in page.select(".change-log section.action h3.when") {
        let when = text(heading);
        assert_eq!(when.len(), "2026-11-25 14:03:12".len(), "{when}");
    }
}

/// Write a change log entry `seconds_ago`, as a past Admin action.
async fn log(app: &TestApp, seconds_ago: i64, entity: &str, entity_id: i64, field: &str) {
    sqlx::query(
        "INSERT INTO change_log (changed_at, entity, entity_id, field, before, after)
         VALUES (timestamptz '2026-01-01 12:00:00+00' - make_interval(secs => $1), $2, $3, $4, 'old', 'new')",
    )
    .bind(seconds_ago as f64)
    .bind(entity)
    .bind(entity_id)
    .bind(field)
    .execute(&app.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_change_log_is_paged_by_action_and_filtered_by_entity_and_id() {
    let app = TestApp::start().await;
    let live = app.insert_test(&NewTest::known()).await;
    let trashed = app
        .insert_test(&NewTest {
            division_level: Some("Geometry"),
            ..NewTest::known()
        })
        .await;
    app.trash_test(trashed).await;
    let pdf = sample_pdf(500);
    let document = app.insert_document(&[live], NewDocument::pdf("Test", &pdf)).await;
    let gone = 987_654;
    let cookie = app.admin().await;

    // 120 actions of two entries each on the live Test, oldest last.
    for i in 0..120 {
        log(&app, 1_000 + i, "test", live, "year").await;
        log(&app, 1_000 + i, "test", live, "host").await;
    }
    log(&app, 10, "test", trashed, "published").await;
    log(&app, 9, "test", gone, "id").await;
    log(&app, 8, "document", document, "label").await;
    log(&app, 7, "document", gone, "id").await;

    let page = app.get_page_as("/admin/change-log", Some(&cookie)).await;
    let shown = actions(&page);
    assert_eq!(shown.len(), 50, "50 actions a page");
    assert_eq!(page.text("#page"), "1 / 3");
    assert!(page.select("#newer").is_empty());
    let older = page.one("#older").value().attr("href").unwrap().to_owned();
    assert_eq!(older, "/admin/change-log?page=2");
    // Links while the record exists: the Trash for one in it, none once gone.
    let first: Vec<(String, Option<String>)> = shown[..4].iter().map(|a| (a[0].1.clone(), a[0].2.clone())).collect();
    assert_eq!(
        first,
        [
            (format!("D{gone}"), None),
            (format!("D{document}"), Some(format!("{}#D{document}", edit_path(live)))),
            (format!("T{gone}"), None),
            (format!("T{trashed}"), Some(format!("/admin/trash#T{trashed}"))),
        ]
    );
    assert_eq!(shown[4].len(), 2, "an action's entries stay together");

    let last = app.get_page_as("/admin/change-log?page=3", Some(&cookie)).await;
    assert_eq!(actions(&last).len(), 24);
    assert!(last.select("#older").is_empty());
    assert_eq!(
        last.one("#newer").value().attr("href"),
        Some("/admin/change-log?page=2")
    );
    // Past the end shows the last page.
    let beyond = app.get_page_as("/admin/change-log?page=99", Some(&cookie)).await;
    assert_eq!(beyond.text("#page"), "3 / 3");

    // The filter: by type, by ID (T and D pick the type), and both.
    let records = |page: &Page| -> Vec<String> {
        actions(page)
            .into_iter()
            .flatten()
            .map(|(_, record, ..)| record)
            .collect()
    };
    let documents = app
        .get_page_as("/admin/change-log?entity=document", Some(&cookie))
        .await;
    assert_eq!(records(&documents), [format!("D{gone}"), format!("D{document}")]);
    let selected = documents.one("#change-log-filter select[name=entity] option[selected]");
    assert_eq!(text(selected), "Documents");
    let one = app
        .get_page_as(&format!("/admin/change-log?id=T{trashed}"), Some(&cookie))
        .await;
    assert_eq!(records(&one), [format!("T{trashed}")]);
    let page = app
        .get_page_as(&format!("/admin/change-log?entity=test&id={gone}"), Some(&cookie))
        .await;
    assert_eq!(records(&page), [format!("T{gone}")]);
    let page = app
        .get_page_as(
            &format!("/admin/change-log?entity=document&id={trashed}"),
            Some(&cookie),
        )
        .await;
    assert!(records(&page).is_empty());
    assert_eq!(page.select("#no-changes").len(), 1);
    // The filtered pages page too, keeping the filter.
    let page = app
        .get_page_as(&format!("/admin/change-log?id=T{live}"), Some(&cookie))
        .await;
    assert_eq!(page.text("#page"), "1 / 3");
    assert_eq!(
        page.one("#older").value().attr("href"),
        Some(format!("/admin/change-log?entity=test&id=T{live}&page=2").as_str())
    );
    // The filter form is a plain GET form.
    let form = page
        .form("#change-log-filter")
        .select("entity", "document")
        .fill("id", &format!("D{document}"));
    assert_eq!(
        form.submit_url(),
        format!("/admin/change-log?entity=document&id=D{document}")
    );
    // An ID that isn't one is ignored, and said so.
    let page = app.get_page_as("/admin/change-log?id=banana", Some(&cookie)).await;
    assert_eq!(page.select("#bad-id").len(), 1);
    assert_eq!(page.text("#page"), "1 / 3");
}
