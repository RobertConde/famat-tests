//! **Remove**, the **Trash** page (`/admin/trash`), **Restore** and
//! **Delete permanently**: removed Tests and Documents vanish from search,
//! downloads and zips, Restore brings them back exactly, and deleting
//! permanently (only from the Trash) removes the rows and the Garage
//! objects. Every write is checked for its auth and its change log.

use sha2::{Digest, Sha256};

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Form, Page, text, text_in},
    zip::entries,
};

fn edit_path(test: i64) -> String {
    format!("/admin/tests/{test}/edit")
}

async fn edit_page(app: &TestApp, cookie: &str, test: i64) -> Page {
    app.get_page_as(&edit_path(test), Some(cookie)).await
}

async fn trash_page(app: &TestApp, cookie: &str) -> Page {
    app.get_page_as("/admin/trash", Some(cookie)).await
}

/// Send a filled-in POST form as a browser on this site would.
async fn submit(app: &TestApp, cookie: &str, form: Form<'_>) -> reqwest::Response {
    let fields = form.fields();
    let pairs: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    app.post_form(&form.action(), &pairs, &[("cookie", cookie)]).await
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("location")
        .map(|l| l.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

/// Assert a 303 to a location starting with `to`.
async fn assert_redirect(response: reqwest::Response, to: &str) {
    let (status, location) = (response.status(), location(&response));
    assert_eq!(status, 303, "{}", response.text().await.unwrap_or_default());
    assert!(location.starts_with(to), "redirected to {location}, not {to}");
}

/// Press Remove test on a Test's edit page.
async fn remove_test(app: &TestApp, cookie: &str, test: i64) {
    let page = edit_page(app, cookie, test).await;
    let response = submit(app, cookie, page.post_form("form#remove-test")).await;
    assert_redirect(response, &format!("/admin/trash#T{test}")).await;
}

/// Press Remove document in a Document's section of a Test's edit page.
async fn remove_document(app: &TestApp, cookie: &str, test: i64, document: i64) {
    let page = edit_page(app, cookie, test).await;
    let form = page.post_form(&format!("section#D{document} form.remove-document"));
    assert_redirect(submit(app, cookie, form).await, &edit_path(test)).await;
}

/// Press Restore or Delete permanently (`button`) on an item's Trash row.
async fn press_in_trash(app: &TestApp, cookie: &str, item: &str, button: &str) -> reqwest::Response {
    let page = trash_page(app, cookie).await;
    let form = page.post_form(&format!("tr#{item} form.{button}"));
    submit(app, cookie, form).await
}

/// Post Restore or Delete permanently for `item` by hand, as a crafted
/// request would, with this session's token.
async fn post_trash(app: &TestApp, cookie: &str, action: &str, item: &str) -> reqwest::Response {
    let csrf = app.csrf_on("/logout", Some(cookie)).await;
    app.post_form(
        &format!("/api/admin/trash/{action}"),
        &[("csrf", &csrf), ("item", item)],
        &[("cookie", cookie)],
    )
    .await
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A line per thing a visitor can see or fetch: each Test's row on the
/// search page, its zip, each Document's downloads under each Test and on
/// their own, and the bulk zip of all of them. Equal views mean equal
/// bytes, names and statuses.
async fn public_view(app: &TestApp, tests: &[i64], documents: &[i64]) -> Vec<String> {
    let mut view = Vec::new();
    let search = app.get_page("/").await;
    for test in tests {
        let row = search.select(&format!("tr#T{test}")).first().map(|row| text(*row));
        view.push(format!("row T{test}: {row:?}"));
    }
    let mut fetch = async |path: String| {
        let response = app.get(&path).await;
        let status = response.status();
        let name = response
            .headers()
            .get("content-disposition")
            .map(|d| d.to_str().unwrap().to_owned());
        let bytes = response.bytes().await.unwrap();
        let body = if path.contains("zip") && status == 200 {
            entries(&bytes)
                .iter()
                .map(|e| format!("{} {}", e.name, sha(&e.bytes)))
                .collect::<Vec<_>>()
                .join(" | ")
        } else {
            sha(&bytes)
        };
        view.push(format!("{path}: {status} {name:?} {body}"));
    };
    for test in tests {
        fetch(format!("/tests/{test}/zip?format=both")).await;
        for document in documents {
            for role in ["pdf", "original"] {
                fetch(format!("/tests/{test}/documents/{document}/{role}")).await;
            }
        }
    }
    for document in documents {
        for role in ["pdf", "original"] {
            fetch(format!("/documents/{document}/{role}")).await;
        }
    }
    let selection: Vec<String> = tests.iter().map(i64::to_string).collect();
    fetch(format!("/tests/zip?sel={}&format=both", selection.join(","))).await;
    view
}

/// What the Test edit page shows of a Test and its Documents.
async fn admin_view(app: &TestApp, cookie: &str, test: i64) -> Vec<String> {
    let page = edit_page(app, cookie, test).await;
    if !page.select("#no-test").is_empty() {
        return vec!["no test".to_owned()];
    }
    let mut view = vec![
        page.text("#description"),
        text(page.one("form#edit-test select[name=visibility] option[selected]")),
    ];
    for section in page.select("section.document") {
        view.push(format!(
            "{} {}",
            section.value().attr("id").unwrap(),
            text_in(section, ".facts")
        ));
    }
    view
}

/// The change log entries for one record, as `(field, before, after)`.
async fn logged(app: &TestApp, entity: &str, id: i64) -> Vec<(String, Option<String>, Option<String>)> {
    app.change_log()
        .await
        .into_iter()
        .filter(|(e, i, _, _, _)| e == entity && *i == id)
        .map(|(_, _, field, before, after)| (field, before, after))
        .collect()
}

fn entry(field: &str, before: Option<&str>, after: Option<&str>) -> (String, Option<String>, Option<String>) {
    (field.to_owned(), before.map(str::to_owned), after.map(str::to_owned))
}

async fn count(app: &TestApp, sql: &str, id: i64) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id)
        .fetch_one(&app.db)
        .await
        .unwrap()
}

/// How many rows anywhere still refer to Document `id`.
async fn document_rows(app: &TestApp, id: i64) -> i64 {
    let mut total = 0;
    for table in [
        "SELECT count(*) FROM documents WHERE id = $1",
        "SELECT count(*) FROM test_documents WHERE document_id = $1",
        "SELECT count(*) FROM document_files WHERE document_id = $1",
        "SELECT count(*) FROM pdf_jobs WHERE document_id = $1",
    ] {
        total += count(app, table, id).await;
    }
    total
}

async fn test_rows(app: &TestApp, id: i64) -> i64 {
    count(app, "SELECT count(*) FROM tests WHERE id = $1", id).await
        + count(app, "SELECT count(*) FROM test_documents WHERE test_id = $1", id).await
}

/// The Trash rows' IDs, in order.
fn trash_items(page: &Page) -> Vec<String> {
    page.select("#trash tbody tr")
        .into_iter()
        .map(|row| row.value().attr("id").unwrap().to_owned())
        .collect()
}

/// A second Test, told apart from [`NewTest::known`] by its Division/Level.
fn other_test() -> NewTest {
    NewTest {
        division_level: Some("Precalculus/Alpha"),
        ..NewTest::known()
    }
}

// --------------------------------------------------------------- auth

#[tokio::test]
async fn every_trash_write_needs_an_admin_session_and_a_valid_csrf_token() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(800);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let trashed = app.insert_test(&other_test()).await;
    app.insert_document(&[trashed], NewDocument::pdf("Answers", &pdf)).await;
    app.trash_test(trashed).await;
    let cookie = app.admin().await;
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let (test_id, document_id, item) = (test.to_string(), document.to_string(), format!("T{trashed}"));

    // Anonymous visitors see no Trash or Change log, and their data is 401.
    for path in ["/admin/trash", "/admin/change-log"] {
        let page = app.get_page(path).await;
        assert_eq!(page.select("#not-logged-in").len(), 1, "{path}");
    }
    for path in ["/api/admin/trash", "/api/admin/change-log?entity=&id=&page="] {
        assert_eq!(app.get(path).await.status(), 401, "GET {path}");
    }

    let writes: [(&str, Vec<(&str, &str)>); 5] = [
        ("/api/admin/tests/remove", vec![("test", &test_id)]),
        (
            "/api/admin/documents/remove",
            vec![("test", &test_id), ("document", &document_id)],
        ),
        ("/api/admin/trash/restore", vec![("item", &item)]),
        ("/api/admin/trash/delete", vec![("item", &item)]),
        ("/api/admin/trash/delete", vec![("item", "D999999")]),
    ];
    for (path, fields) in &writes {
        let with = |token: &str| -> Vec<(String, String)> {
            [("csrf", token)]
                .into_iter()
                .chain(fields.iter().copied())
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect()
        };
        let (good, bad) = (with(&csrf), with("1.bad"));
        let good: Vec<(&str, &str)> = good.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        let bad: Vec<(&str, &str)> = bad.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        assert_eq!(app.post_form(path, &good, &[]).await.status(), 401, "anonymous {path}");
        assert_eq!(
            app.post_form(path, &bad, &[("cookie", &cookie)]).await.status(),
            403,
            "bad CSRF {path}"
        );
        let cross_site = [("cookie", cookie.as_str()), ("origin", "https://evil.example")];
        assert_eq!(
            app.post_form(path, &good, &cross_site).await.status(),
            403,
            "cross-site {path}"
        );
    }

    // Nothing changed.
    assert!(app.change_log().await.is_empty());
    assert_eq!(trash_items(&trash_page(&app, &cookie).await), [format!("T{trashed}")]);
    assert_eq!(
        app.get(&format!("/tests/{test}/documents/{document}/pdf"))
            .await
            .status(),
        200
    );
}

// ------------------------------------------------------ Remove and Restore

#[tokio::test]
async fn a_removed_test_vanishes_everywhere_and_restore_brings_it_back_exactly() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let neighbour = app.insert_test(&other_test()).await;
    let (pdf, docx, docx_pdf) = (sample_pdf(1_500), sample_docx(1_200), sample_pdf(900));
    let questions = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let answers = app
        .insert_document(&[test], NewDocument::docx("Answers", &docx, Some(&docx_pdf)))
        .await;
    let hidden = app
        .insert_document(
            &[test],
            NewDocument {
                published: false,
                ..NewDocument::pdf("Errata", &pdf)
            },
        )
        .await;
    // A Master Key, also on the neighbouring Test.
    let shared = app
        .insert_document(&[test, neighbour], NewDocument::pdf("Solutions", &docx_pdf))
        .await;
    let cookie = app.admin().await;
    let (tests, documents) = ([test, neighbour], [questions, answers, hidden, shared]);
    let public_before = public_view(&app, &tests, &documents).await;
    let admin_before = admin_view(&app, &cookie, test).await;
    assert!(
        public_before[0].contains("Some"),
        "the Test is public: {public_before:?}"
    );

    remove_test(&app, &cookie, test).await;

    let search = app.get_page("/").await;
    assert!(search.select(&format!("tr#T{test}")).is_empty(), "off the search page");
    assert_eq!(search.text("#count"), "1 test");
    for document in [questions, answers] {
        for role in ["pdf", "original"] {
            let path = format!("/tests/{test}/documents/{document}/{role}");
            assert_eq!(app.get(&path).await.status(), 404, "{path}");
            let path = format!("/documents/{document}/{role}");
            assert_eq!(app.get(&path).await.status(), 404, "{path}");
        }
    }
    assert_eq!(app.get(&format!("/tests/{test}/zip")).await.status(), 404);
    assert_eq!(app.get(&format!("/tests/zip?sel={test}")).await.status(), 404);
    // The Master Key stays on the neighbouring Test, and only there.
    let response = app.get(&format!("/tests/zip?sel={test},{neighbour}&format=both")).await;
    let zipped: Vec<String> = entries(&response.bytes().await.unwrap())
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(zipped.len(), 1, "{zipped:?}");
    assert!(zipped[0].starts_with(&format!("T{neighbour} ")) && zipped[0].contains(&format!("D{shared} ")));
    assert_eq!(
        app.get(&format!("/tests/{neighbour}/documents/{shared}/pdf"))
            .await
            .status(),
        200
    );
    assert_eq!(
        app.get(&format!("/tests/{test}/documents/{shared}/pdf")).await.status(),
        404
    );
    // Admins don't see it outside the Trash either.
    assert_eq!(admin_view(&app, &cookie, test).await, ["no test"]);
    assert!(
        app.get_page_as("/", Some(&cookie))
            .await
            .select(&format!("tr#T{test}"))
            .is_empty()
    );

    // The Trash lists it with its Documents.
    let trash = trash_page(&app, &cookie).await;
    assert_eq!(trash_items(&trash), [format!("T{test}")]);
    let row = trash.one(&format!("tr#T{test}"));
    assert!(text(row).contains("1993 February Invitational · Plant City · Calculus/Mu Individual"));
    assert!(text(row).contains("Published"));
    assert!(text_in(row, &format!("#T{test}-D{shared}")).contains("also on another Test"));
    let warning = row
        .select(&scraper::Selector::parse("button.delete").unwrap())
        .next()
        .unwrap()
        .value()
        .attr("title")
        .unwrap()
        .to_owned();
    assert_eq!(
        warning,
        format!("Delete T{test} for good, with D{questions}, D{answers}, D{hidden}")
    );

    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{test}"), "restore").await,
        "/admin/trash",
    )
    .await;

    assert_eq!(public_view(&app, &tests, &documents).await, public_before);
    assert_eq!(admin_view(&app, &cookie, test).await, admin_before);
    assert!(trash_page(&app, &cookie).await.select("#trash-empty").len() == 1);
    assert_eq!(
        logged(&app, "test", test).await,
        [
            entry("trashed", Some("false"), Some("true")),
            entry("trashed", Some("true"), Some("false")),
        ]
    );
}

#[tokio::test]
async fn a_hidden_test_restored_from_the_trash_stays_hidden() {
    let app = TestApp::start().await;
    let test = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    let pdf = sample_pdf(700);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let before = admin_view(&app, &cookie, test).await;
    assert_eq!(before[1], "Hidden");

    remove_test(&app, &cookie, test).await;
    let trash = trash_page(&app, &cookie).await;
    assert!(text(trash.one(&format!("tr#T{test}"))).contains("Hidden"));
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{test}"), "restore").await,
        "/admin/trash",
    )
    .await;

    assert_eq!(admin_view(&app, &cookie, test).await, before);
    assert!(app.get_page("/").await.select(&format!("tr#T{test}")).is_empty());
    assert_eq!(app.get(&format!("/documents/{document}/pdf")).await.status(), 404);
}

#[tokio::test]
async fn a_removed_document_leaves_every_test_and_restore_brings_it_back_exactly() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let neighbour = app.insert_test(&other_test()).await;
    let (pdf, docx, docx_pdf) = (sample_pdf(1_100), sample_docx(1_300), sample_pdf(1_000));
    let questions = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let key = app
        .insert_document(&[test, neighbour], NewDocument::docx("Answers", &docx, Some(&docx_pdf)))
        .await;
    let cookie = app.admin().await;
    let (tests, documents) = ([test, neighbour], [questions, key]);
    let public_before = public_view(&app, &tests, &documents).await;
    let admin_before = (
        admin_view(&app, &cookie, test).await,
        admin_view(&app, &cookie, neighbour).await,
    );

    remove_document(&app, &cookie, test, key).await;

    for on in [test, neighbour] {
        for role in ["pdf", "original"] {
            let path = format!("/tests/{on}/documents/{key}/{role}");
            assert_eq!(app.get(&path).await.status(), 404, "{path}");
        }
        let page = edit_page(&app, &cookie, on).await;
        assert!(
            page.select(&format!("section#D{key}")).is_empty(),
            "off T{on}'s edit page"
        );
    }
    assert_eq!(app.get(&format!("/documents/{key}/pdf")).await.status(), 404);
    let search = app.get_page("/").await;
    let links: Vec<String> = search
        .select(&format!("tr#T{test} .document > a:first-child"))
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(links, ["Test"]);
    assert!(
        search.select(&format!("tr#T{neighbour} .document")).is_empty(),
        "nothing left on the neighbouring Test"
    );
    let response = app.get(&format!("/tests/{test}/zip?format=both")).await;
    let zipped: Vec<String> = entries(&response.bytes().await.unwrap())
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(zipped.len(), 1, "{zipped:?}");
    assert!(zipped[0].contains(&format!("D{questions} ")));
    let response = app.get(&format!("/tests/zip?sel={test},{neighbour}&format=both")).await;
    let zipped: Vec<String> = entries(&response.bytes().await.unwrap())
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(
        zipped.iter().all(|name| !name.contains(&format!("D{key} "))),
        "{zipped:?}"
    );
    assert_eq!(app.get(&format!("/tests/{neighbour}/zip")).await.status(), 404);

    let trash = trash_page(&app, &cookie).await;
    assert_eq!(trash_items(&trash), [format!("D{key}")]);
    let row = trash.one(&format!("tr#D{key}"));
    assert!(text(row).contains("Answers · Published"), "{}", text(row));
    assert_eq!(
        row.select(&scraper::Selector::parse(".details a").unwrap())
            .map(|a| a.value().attr("href").unwrap().to_owned())
            .collect::<Vec<_>>(),
        [edit_path(test), edit_path(neighbour)]
    );

    assert_redirect(
        press_in_trash(&app, &cookie, &format!("D{key}"), "restore").await,
        "/admin/trash",
    )
    .await;

    assert_eq!(public_view(&app, &tests, &documents).await, public_before);
    assert_eq!(
        (
            admin_view(&app, &cookie, test).await,
            admin_view(&app, &cookie, neighbour).await
        ),
        admin_before
    );
    assert_eq!(
        logged(&app, "document", key).await,
        [
            entry("trashed", Some("false"), Some("true")),
            entry("trashed", Some("true"), Some("false")),
        ]
    );
}

#[tokio::test]
async fn the_trash_lists_newest_first_and_a_document_whose_tests_are_all_trashed_waits_for_one() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(600);
    let document = app.insert_document(&[test], NewDocument::pdf("Answers", &pdf)).await;
    let other = app.insert_test(&other_test()).await;
    app.insert_document(&[other], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;

    remove_document(&app, &cookie, test, document).await;
    remove_test(&app, &cookie, test).await;
    remove_test(&app, &cookie, other).await;

    let trash = trash_page(&app, &cookie).await;
    assert_eq!(
        trash_items(&trash),
        [format!("T{other}"), format!("T{test}"), format!("D{document}")],
        "newest first"
    );
    // The Document waits for its Test.
    let row = trash.one(&format!("tr#D{document}"));
    let restore = row
        .select(&scraper::Selector::parse("button.restore").unwrap())
        .next()
        .unwrap();
    assert!(restore.value().attr("disabled").is_some());
    assert_eq!(
        text_in(row, ".blocked"),
        format!("Its Tests are all in the Trash: restore T{test} first.")
    );
    assert!(text_in(trash.one(&format!("tr#T{test}")), ".details").contains("(in the Trash)"));
    let response = post_trash(&app, &cookie, "restore", &format!("D{document}")).await;
    assert_eq!(response.status(), 409);
    assert!(
        Page::parse(&response.text().await.unwrap())
            .text("#refusal")
            .contains(&format!("T{test}"))
    );

    // Restoring the Test brings back neither the Document, which is in the
    // Trash on its own, nor anything else; restoring the Document then does.
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{test}"), "restore").await,
        "/admin/trash",
    )
    .await;
    assert_eq!(app.get(&format!("/documents/{document}/pdf")).await.status(), 404);
    assert_eq!(
        trash_items(&trash_page(&app, &cookie).await),
        [format!("T{other}"), format!("D{document}")]
    );
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("D{document}"), "restore").await,
        "/admin/trash",
    )
    .await;
    assert_eq!(
        app.get(&format!("/tests/{test}/documents/{document}/pdf"))
            .await
            .status(),
        200
    );

    // Restoring what isn't in the Trash changes nothing.
    let entries = app.change_log().await.len();
    assert_eq!(
        post_trash(&app, &cookie, "restore", &format!("T{test}")).await.status(),
        303
    );
    assert_eq!(app.change_log().await.len(), entries);
}

#[tokio::test]
async fn a_document_cant_be_detached_from_its_last_test_outside_the_trash() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let trashed = app.insert_test(&other_test()).await;
    let pdf = sample_pdf(900);
    let key = app
        .insert_document(&[test, trashed], NewDocument::pdf("Answers", &pdf))
        .await;
    let cookie = app.admin().await;
    app.trash_test(trashed).await;

    // The Test in the Trash isn't listed, so Detach is off.
    let page = edit_page(&app, &cookie, test).await;
    assert!(page.select(&format!("#D{key}-on-T{trashed}")).is_empty());
    let detach = page.one(&format!("#D{key}-on-T{test} button"));
    assert!(detach.value().attr("disabled").is_some());
    let csrf = app.csrf_on("/logout", Some(&cookie)).await;
    let (test_id, key_id, action) = (test.to_string(), key.to_string(), format!("detach:{test}"));
    let response = app
        .post_form(
            "/api/admin/documents/tests",
            &[
                ("csrf", &csrf),
                ("test", &test_id),
                ("document", &key_id),
                ("action", &action),
            ],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(response.status(), 409);
    assert_eq!(
        app.get(&format!("/tests/{test}/documents/{key}/pdf")).await.status(),
        200
    );
    assert!(app.change_log().await.is_empty());
}

// ------------------------------------------------------ Delete permanently

#[tokio::test]
async fn only_what_is_in_the_trash_can_be_deleted_permanently() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(800);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let keys = app.stored_keys().await;
    let view = public_view(&app, &[test], &[document]).await;

    for item in [format!("T{test}"), format!("D{document}")] {
        let response = post_trash(&app, &cookie, "delete", &item).await;
        assert_eq!(response.status(), 409, "{item}");
        let refusal = Page::parse(&response.text().await.unwrap());
        assert!(refusal.text("#refusal").contains("is not in the Trash"), "{item}");
    }
    for item in ["T999999", "D999999"] {
        assert_eq!(post_trash(&app, &cookie, "delete", item).await.status(), 404, "{item}");
    }
    assert_eq!(post_trash(&app, &cookie, "delete", "X1").await.status(), 400);

    assert_eq!(app.stored_keys().await, keys);
    assert_eq!(public_view(&app, &[test], &[document]).await, view);
    assert!(app.change_log().await.is_empty());
}

#[tokio::test]
async fn deleting_a_test_permanently_deletes_its_own_documents_and_keeps_shared_ones() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let neighbour = app.insert_test(&other_test()).await;
    let trashed_neighbour = app
        .insert_test(&NewTest {
            division_level: Some("Geometry"),
            ..NewTest::known()
        })
        .await;
    let (pdf, docx, docx_pdf) = (sample_pdf(1_000), sample_docx(1_000), sample_pdf(1_200));
    let own = app
        .insert_document(&[test], NewDocument::docx("Test", &docx, Some(&docx_pdf)))
        .await;
    let own_trashed = app.insert_document(&[test], NewDocument::pdf("Errata", &pdf)).await;
    let shared = app
        .insert_document(&[test, neighbour], NewDocument::pdf("Solutions", &docx_pdf))
        .await;
    let shared_with_trash = app
        .insert_document(&[test, trashed_neighbour], NewDocument::pdf("Answers", &pdf))
        .await;
    let cookie = app.admin().await;
    remove_document(&app, &cookie, test, own_trashed).await;
    app.trash_test(trashed_neighbour).await;
    remove_test(&app, &cookie, test).await;
    let keep = public_view(&app, &[neighbour], &[shared]).await;

    let response = press_in_trash(&app, &cookie, &format!("T{test}"), "delete").await;
    assert_redirect(response, "/admin/trash").await;

    // The Test and the Documents only it had are gone: rows and objects.
    assert_eq!(test_rows(&app, test).await, 0);
    for document in [own, own_trashed] {
        assert_eq!(document_rows(&app, document).await, 0, "D{document}");
        for role in ["pdf", "original"] {
            assert_eq!(app.get(&format!("/documents/{document}/{role}")).await.status(), 404);
        }
    }
    let keys = app.stored_keys().await;
    assert_eq!(
        keys,
        {
            let mut keys = vec![
                format!("documents/{shared}/original"),
                format!("documents/{shared_with_trash}/original"),
            ];
            keys.sort();
            keys
        },
        "only the shared Documents' objects are left"
    );
    // The shared ones live on.
    assert_eq!(public_view(&app, &[neighbour], &[shared]).await, keep);
    let trash = trash_page(&app, &cookie).await;
    assert_eq!(trash_items(&trash), [format!("T{trashed_neighbour}")]);
    assert!(text(trash.one(&format!("tr#T{trashed_neighbour}"))).contains(&format!("D{shared_with_trash}")));
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{trashed_neighbour}"), "restore").await,
        "/admin/trash",
    )
    .await;
    assert_eq!(
        app.get(&format!("/tests/{trashed_neighbour}/documents/{shared_with_trash}/pdf"))
            .await
            .status(),
        200
    );

    // The change log has each deleted record, and the shared ones' Tests.
    let test_entries = logged(&app, "test", test).await;
    assert_eq!(
        test_entries,
        [
            entry("trashed", Some("false"), Some("true")),
            entry("id", Some(&format!("T{test}")), None),
            entry("year", Some("1993"), None),
            entry("month", Some("February"), None),
            entry("competition", Some("Invitational"), None),
            entry("division_level", Some("Calculus/Mu"), None),
            entry("test_type", Some("Individual"), None),
            entry("host", Some("Plant City"), None),
            entry("published", Some("true"), None),
            entry("needs_review", Some("false"), None),
            entry("trashed", Some("true"), None),
        ]
    );
    let own_entries = logged(&app, "document", own).await;
    assert_eq!(own_entries[0], entry("id", Some(&format!("D{own}")), None));
    let field = |entries: &[(String, Option<String>, Option<String>)], name: &str| {
        entries
            .iter()
            .rev()
            .find(|(f, _, _)| f == name)
            .and_then(|(_, before, _)| before.clone())
    };
    assert_eq!(field(&own_entries, "kind").as_deref(), Some("Test"));
    assert_eq!(field(&own_entries, "tests"), Some(format!("T{test}")));
    assert_eq!(
        field(&own_entries, "original"),
        Some(format!("original.docx (1000 bytes, sha256 {})", sha(&docx)))
    );
    assert_eq!(
        field(&own_entries, "pdf"),
        Some(format!("PDF (1200 bytes, sha256 {})", sha(&docx_pdf)))
    );
    assert_eq!(field(&own_entries, "trashed").as_deref(), Some("false"));
    assert_eq!(
        field(&logged(&app, "document", own_trashed).await, "trashed").as_deref(),
        Some("true")
    );
    assert_eq!(
        logged(&app, "document", shared).await,
        [entry(
            "tests",
            Some(&format!("T{test}, T{neighbour}")),
            Some(&format!("T{neighbour}"))
        )]
    );
}

#[tokio::test]
async fn deleting_a_document_permanently_takes_it_off_every_test_with_its_files() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let trashed = app.insert_test(&other_test()).await;
    let (pdf, docx) = (sample_pdf(1_000), sample_docx(1_000));
    let keep = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    // Still being prepared, with its conversion queued.
    let key = app
        .insert_document(&[test, trashed], NewDocument::docx("Answers", &docx, None))
        .await;
    sqlx::query("INSERT INTO pdf_jobs (document_id) VALUES ($1)")
        .bind(key)
        .execute(&app.db)
        .await
        .unwrap();
    let cookie = app.admin().await;
    app.trash_test(trashed).await;
    remove_document(&app, &cookie, test, key).await;
    let view = public_view(&app, &[test], &[keep]).await;

    assert_redirect(
        press_in_trash(&app, &cookie, &format!("D{key}"), "delete").await,
        "/admin/trash",
    )
    .await;

    assert_eq!(document_rows(&app, key).await, 0);
    assert_eq!(app.stored_keys().await, [format!("documents/{keep}/original")]);
    assert_eq!(public_view(&app, &[test], &[keep]).await, view);
    // Its Test in the Trash comes back without it.
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{trashed}"), "restore").await,
        "/admin/trash",
    )
    .await;
    assert!(
        edit_page(&app, &cookie, trashed)
            .await
            .select("section.document")
            .is_empty()
    );
    assert!(trash_page(&app, &cookie).await.select("#trash-empty").len() == 1);

    let entries = logged(&app, "document", key).await;
    assert_eq!(entries[0], entry("trashed", Some("false"), Some("true")));
    assert_eq!(entries[1], entry("id", Some(&format!("D{key}")), None));
    assert!(entries.contains(&entry("tests", Some(&format!("T{test}, T{trashed}")), None)));
    assert!(entries.contains(&entry("pdf_status", Some("preparing"), None)));
    // Deleting it again finds nothing.
    assert_eq!(
        post_trash(&app, &cookie, "delete", &format!("D{key}")).await.status(),
        404
    );
}

#[tokio::test]
async fn an_upload_waiting_on_a_deleted_test_can_still_be_saved_as_a_new_test() {
    let app = TestApp::start().await;
    let existing = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(700);
    app.insert_document(&[existing], NewDocument::pdf("Test", &pdf)).await;
    let cookie = app.admin().await;
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("S_key.pdf", &sample_pdf(900));
    let response = app.submit_add_test(&cookie, form, "publish").await;
    let waiting = location(&response);
    assert!(waiting.contains("pending="), "{waiting}");
    let pending = waiting.rsplit('=').next().unwrap().to_owned();

    remove_test(&app, &cookie, existing).await;
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{existing}"), "delete").await,
        "/admin/trash",
    )
    .await;

    let csrf = app.csrf_on(&waiting, Some(&cookie)).await;
    let finish = async |action: &str| {
        app.post_form(
            "/api/admin/uploads/finish",
            &[("csrf", &csrf), ("pending", &pending), ("action", action)],
            &[("cookie", &cookie)],
        )
        .await
    };
    assert_eq!(finish(&format!("add:{existing}")).await.status(), 409);
    let response = finish("new").await;
    assert_redirect(response, "/admin/tests/new?added=").await;
    assert_eq!(app.get_page("/").await.text("#count"), "1 test");
}

// ------------------------------------------------ Merge into… (#27) and search

/// The IDs of the Test rows a search page shows.
fn result_rows(page: &Page) -> Vec<String> {
    page.select("#results tbody tr")
        .into_iter()
        .filter_map(|row| row.value().attr("id").map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_test_emptied_by_merge_into_is_in_the_trash_and_restores_hidden_and_empty() {
    let app = TestApp::start().await;
    let from = app.insert_test(&NewTest::known()).await;
    let into = app.insert_test(&other_test()).await;
    let (pdf, errata) = (sample_pdf(1_000), sample_pdf(700));
    let moved = app.insert_document(&[from], NewDocument::pdf("Test", &pdf)).await;
    let trashed_document = app.insert_document(&[from], NewDocument::pdf("Errata", &errata)).await;
    let cookie = app.admin().await;
    remove_document(&app, &cookie, from, trashed_document).await;

    let page = edit_page(&app, &cookie, from).await;
    let form = page.post_form("form#merge-test").fill("into", &format!("T{into}"));
    assert_redirect(submit(&app, &cookie, form).await, &edit_path(into)).await;

    // The merged Test is in the Trash, says where its Documents went, and
    // never shows in a search, even an Admin's.
    let trash = trash_page(&app, &cookie).await;
    assert_eq!(
        trash_items(&trash),
        [format!("T{from}"), format!("D{trashed_document}")]
    );
    let row = trash.one(&format!("tr#T{from}"));
    assert!(text(row).contains("Hidden"), "{}", text(row));
    assert_eq!(
        text_in(row, ".merged"),
        format!("Merged into T{into}. Restoring brings it back Hidden and empty: its Documents stay on T{into}.")
    );
    assert!(text_in(row, ".details").contains("No Documents"));
    // Its Trashed Document moved too, so it can be restored onto the survivor.
    let document_row = trash.one(&format!("tr#D{trashed_document}"));
    assert!(text_in(document_row, ".details").contains(&format!("T{into}")));
    for search in ["/", "/?needs_review=on"] {
        let rows = result_rows(&app.get_page_as(search, Some(&cookie)).await);
        assert!(!rows.contains(&format!("T{from}")), "{search}: {rows:?}");
    }

    // The change log page shows the merge.
    let log = app
        .get_page_as(&format!("/admin/change-log?id=T{from}"), Some(&cookie))
        .await;
    let merged: Vec<(String, String)> = log
        .select("tr.entry")
        .into_iter()
        .map(|row| (text_in(row, "td.field"), text_in(row, "td.change")))
        .collect();
    assert!(
        merged.contains(&("merged into".to_owned(), format!("— → T{into}"))),
        "{merged:?}"
    );
    assert!(merged.contains(&("published".to_owned(), "true → false".to_owned())));

    // Restore: Hidden and empty, as the merge left it. The Documents stay.
    let survivor = public_view(&app, &[into], &[moved]).await;
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{from}"), "restore").await,
        "/admin/trash",
    )
    .await;
    assert_eq!(admin_view(&app, &cookie, from).await[1], "Hidden");
    assert!(
        edit_page(&app, &cookie, from)
            .await
            .select("section.document")
            .is_empty()
    );
    assert!(result_rows(&app.get_page_as("/", Some(&cookie)).await).contains(&format!("T{from}")));
    assert!(!result_rows(&app.get_page("/").await).contains(&format!("T{from}")));
    assert_eq!(public_view(&app, &[into], &[moved]).await, survivor);

    // Removed again and deleted for good, it takes nothing with it.
    remove_test(&app, &cookie, from).await;
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("T{from}"), "delete").await,
        "/admin/trash",
    )
    .await;
    assert_eq!(test_rows(&app, from).await, 0);
    assert_eq!(public_view(&app, &[into], &[moved]).await, survivor);
    assert!(app.stored_keys().await.contains(&format!("documents/{moved}/original")));
    assert_redirect(
        press_in_trash(&app, &cookie, &format!("D{trashed_document}"), "restore").await,
        "/admin/trash",
    )
    .await;
    assert_eq!(
        app.get(&format!("/tests/{into}/documents/{trashed_document}/pdf"))
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn trashed_tests_never_show_in_an_admins_search_hidden_or_flagged() {
    let app = TestApp::start().await;
    let hidden = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    let live = app.insert_test(&other_test()).await;
    sqlx::query("UPDATE tests SET needs_review = true, needs_review_comment = 'check' WHERE id = ANY($1)")
        .bind([hidden, live])
        .execute(&app.db)
        .await
        .unwrap();
    let cookie = app.admin().await;
    assert_eq!(
        result_rows(&app.get_page_as("/?needs_review=on", Some(&cookie)).await).len(),
        2
    );

    remove_test(&app, &cookie, hidden).await;
    remove_test(&app, &cookie, live).await;

    for search in ["/", "/?needs_review=on"] {
        let page = app.get_page_as(search, Some(&cookie)).await;
        assert!(result_rows(&page).is_empty(), "{search}: {:?}", result_rows(&page));
    }
    let selected = app.get_page_as(&format!("/?sel={hidden},{live}"), Some(&cookie)).await;
    assert!(selected.select("input.pick[checked]").is_empty());
}
