//! **Document kinds**: an Admin renames, adds, reorders and deletes kinds and
//! ticks what each contains, and the search's Includes filters, the download
//! menus and Add test follow at once. Every action lands in the change log.

use scraper::{ElementRef, Selector};

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest, sample_pdf},
    html::{Page, links_in, text},
};

const PAGE: &str = "/admin/document-kinds";

/// The kinds the first migration seeds, in display order.
const SEEDED: [&str; 7] = [
    "Test",
    "Condensed Test",
    "Answers",
    "Solutions",
    "Combined Test+Solutions",
    "Errata",
    "Other",
];

/// A change log entry: `(entity, entity id, field, before, after)`.
type Entry = (String, i64, String, Option<String>, Option<String>);

fn entry(id: i64, field: &str, before: Option<&str>, after: Option<&str>) -> Entry {
    (
        "document_kind".to_owned(),
        id,
        field.to_owned(),
        before.map(str::to_owned),
        after.map(str::to_owned),
    )
}

/// What a kind contains: questions, answers, solutions.
type Ticks = (bool, bool, bool);

/// One row of the page, as a browser with JavaScript off shows it.
#[derive(Debug, PartialEq)]
struct Row {
    id: i64,
    name: String,
    /// Questions, answers, solutions.
    ticks: Ticks,
    documents: String,
    deletable: bool,
}

fn rows(page: &Page) -> Vec<Row> {
    page.select("#kinds tbody tr.kind")
        .into_iter()
        .map(|tr| {
            let id: i64 = tr
                .value()
                .attr("id")
                .unwrap()
                .strip_prefix('K')
                .unwrap()
                .parse()
                .unwrap();
            let one = |css: &str| {
                tr.select(&Selector::parse(css).unwrap())
                    .next()
                    .unwrap_or_else(|| panic!("K{id} has no {css}"))
            };
            let ticked = |field: &str| {
                one(&format!("input[type=checkbox][name={field}]"))
                    .value()
                    .attr("checked")
                    .is_some()
            };
            Row {
                id,
                name: one("input[name=name]").value().attr("value").unwrap().to_owned(),
                ticks: (ticked("questions"), ticked("answers"), ticked("solutions")),
                documents: text(one("td.count")),
                deletable: tr.select(&Selector::parse("form.delete").unwrap()).next().is_some(),
            }
        })
        .collect()
}

fn names(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|r| r.name.as_str()).collect()
}

/// The Test IDs a search lists, top to bottom.
async fn search(app: &TestApp, path: &str) -> Vec<i64> {
    app.get_page(path)
        .await
        .select("#results tbody tr")
        .into_iter()
        .map(|tr| {
            tr.value()
                .attr("id")
                .unwrap()
                .strip_prefix('T')
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect()
}

/// The search with one Includes box ticked, submitted from the form.
async fn includes(app: &TestApp, part: &str) -> Vec<i64> {
    let page = app.get_page("/").await;
    let url = page.form("form#filters").check("includes", part, true).submit_url();
    let mut ids = search(app, &url).await;
    ids.sort();
    ids
}

/// The kind groups a Test's download menu offers, as `label count`.
async fn groups(app: &TestApp, test: i64) -> Vec<String> {
    let count = Selector::parse(".n").unwrap();
    app.get_page("/")
        .await
        .select(&format!("tr#T{test} .split .menu > a.group"))
        .into_iter()
        .map(|a| {
            let n = text(a.select(&count).next().expect("a count"));
            let label = text(a);
            format!("{} {n}", label.strip_suffix(&n).unwrap().trim())
        })
        .collect()
}

/// The `name` and value of every control a form would post, wherever the
/// control sits: inside the form or tied to it with `form="…"`. Unticked
/// checkboxes are left out, as a browser leaves them out.
fn form_fields(page: &Page, form_id: &str) -> Vec<(String, String)> {
    let css = format!("form#{form_id} input[name], input[name][form={form_id}]");
    page.select(&css)
        .into_iter()
        .filter(|input| input.value().attr("type") != Some("checkbox") || input.value().attr("checked").is_some())
        .map(|input| {
            let value = input.value();
            (
                value.attr("name").unwrap().to_owned(),
                value.attr("value").unwrap_or("").to_owned(),
            )
        })
        .collect()
}

impl TestApp {
    async fn kinds_page(&self, cookie: &str) -> Page {
        self.get_page_as(PAGE, Some(cookie)).await
    }

    async fn kinds(&self, cookie: &str) -> Vec<Row> {
        rows(&self.kinds_page(cookie).await)
    }

    async fn kind_id(&self, cookie: &str, name: &str) -> i64 {
        self.kinds(cookie)
            .await
            .into_iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no kind {name:?}"))
            .id
    }

    /// POST one of the page's actions with the page's CSRF token.
    async fn kind_action(&self, cookie: &str, action: &str, fields: &[(&str, &str)]) -> reqwest::Response {
        let page = self.kinds_page(cookie).await;
        let csrf = page
            .one("#add-kind input[name=csrf]")
            .value()
            .attr("value")
            .unwrap()
            .to_owned();
        let mut form = vec![("csrf", csrf.as_str())];
        form.extend_from_slice(fields);
        self.post_form(
            &format!("/api/admin/document-kinds/{action}"),
            &form,
            &[("cookie", cookie)],
        )
        .await
    }

    /// The same, asserting it was done.
    async fn kind_done(&self, cookie: &str, action: &str, fields: &[(&str, &str)]) {
        let response = self.kind_action(cookie, action, fields).await;
        let status = response.status();
        let location = response
            .headers()
            .get("location")
            .map(|l| l.to_str().unwrap().to_owned());
        assert_eq!(
            (status.as_u16(), location.as_deref()),
            (303, Some(PAGE)),
            "{action}: {}",
            response.text().await.unwrap_or_default()
        );
    }

    /// Submit a row's Save form as a browser would, after `edit` changes the
    /// fields it posts.
    async fn save_kind(&self, cookie: &str, id: i64, edit: impl FnOnce(&mut Vec<(String, String)>)) {
        let page = self.kinds_page(cookie).await;
        let form = page.one(&format!("form#kind-{id}"));
        assert_eq!(form.value().attr("method"), Some("post"));
        let action = form.value().attr("action").unwrap().to_owned();
        let mut fields = form_fields(&page, &format!("kind-{id}"));
        edit(&mut fields);
        let fields: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        let response = self.post_form(&action, &fields, &[("cookie", cookie)]).await;
        assert_eq!(response.status(), 303, "{}", response.text().await.unwrap_or_default());
    }

    /// Tick or untick what a kind contains, through its row's Save form.
    async fn set_tick(&self, cookie: &str, kind: &str, field: &str, ticked: bool) {
        let id = self.kind_id(cookie, kind).await;
        self.save_kind(cookie, id, |fields| {
            fields.retain(|(name, _)| name != field);
            if ticked {
                fields.push((field.to_owned(), "true".to_owned()));
            }
        })
        .await;
    }

    /// The kind names Add test's dropdowns offer, from the data the page
    /// hydrates with (the dropdowns appear once files are chosen, so only
    /// with JavaScript).
    async fn add_test_kinds(&self, cookie: &str) -> Vec<String> {
        let response = self.get_as("/api/admin/add-test?added=&pending=", Some(cookie)).await;
        assert_eq!(response.status(), 200);
        let page: serde_json::Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        page["kinds"]
            .as_array()
            .expect("a list of kinds")
            .iter()
            .map(|k| k.as_str().unwrap().to_owned())
            .collect()
    }

    async fn kind_log(&self) -> Vec<Entry> {
        self.change_log()
            .await
            .into_iter()
            .filter(|(entity, ..)| entity == "document_kind")
            .collect()
    }
}

fn attr<'a>(element: ElementRef<'a>, name: &str) -> Option<&'a str> {
    element.value().attr(name)
}

// --------------------------------------------------------------- the page

#[tokio::test]
async fn the_page_is_in_the_admin_menu_and_only_for_admins() {
    let app = TestApp::start().await;

    let anonymous = app.get_page(PAGE).await;
    assert!(anonymous.select("#kinds").is_empty());
    assert!(text(anonymous.one("#not-logged-in")).contains("not logged in"));
    assert_eq!(app.get("/api/admin/document-kinds").await.status(), 401);

    let cookie = app.admin().await;
    let menu = links_in(app.get_page_as("/", Some(&cookie)).await.one("#admin-menu"));
    assert!(
        menu.contains(&("Document kinds".to_owned(), PAGE.to_owned())),
        "{menu:?}"
    );
    let page = app.kinds_page(&cookie).await;
    assert!(page.select("#admin-menu").len() == 1, "the Admin menu is on the page");
    assert_eq!(names(&rows(&page)), SEEDED);
}

#[tokio::test]
async fn kinds_are_listed_in_display_order_with_what_they_contain_and_how_many_documents_use_them() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    // Hidden and trashed Documents still use their kind.
    app.insert_document(
        &[test],
        NewDocument {
            published: false,
            ..NewDocument::pdf("Solutions", &pdf)
        },
    )
    .await;
    let trashed = app.insert_document(&[test], NewDocument::pdf("Errata", &pdf)).await;
    app.trash_document(trashed).await;
    let cookie = app.admin().await;

    let found: Vec<(String, Ticks, String, bool)> = app
        .kinds(&cookie)
        .await
        .into_iter()
        .map(|r| (r.name, r.ticks, r.documents, r.deletable))
        .collect();
    let expected = [
        ("Test", (true, false, false), "2", false),
        ("Condensed Test", (true, false, false), "0", true),
        ("Answers", (false, true, false), "0", true),
        ("Solutions", (false, true, true), "1", false),
        ("Combined Test+Solutions", (true, true, true), "0", true),
        ("Errata", (false, false, false), "1", false),
        ("Other", (false, false, false), "0", true),
    ]
    .map(|(name, ticks, count, deletable)| (name.to_owned(), ticks, count.to_owned(), deletable));
    assert_eq!(found, expected);
    let page = app.kinds_page(&cookie).await;
    let in_use: Vec<String> = page.select("#kinds tr.kind .in-use").into_iter().map(text).collect();
    assert_eq!(in_use, ["In use"; 3]);
}

#[tokio::test]
async fn every_action_is_a_native_form_post_tied_to_its_row() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let page = app.kinds_page(&cookie).await;
    let id = rows(&page)[1].id;

    let posts = |css: &str| {
        let form = page.one(css);
        (attr(form, "method"), attr(form, "action"))
    };
    assert_eq!(
        posts(&format!("form#kind-{id}")),
        (Some("post"), Some("/api/admin/document-kinds/save"))
    );
    assert_eq!(
        posts(&format!("tr#K{id} form.move")),
        (Some("post"), Some("/api/admin/document-kinds/move"))
    );
    assert_eq!(
        posts(&format!("tr#K{id} form.delete")),
        (Some("post"), Some("/api/admin/document-kinds/delete"))
    );
    assert_eq!(
        posts("form#add-kind"),
        (Some("post"), Some("/api/admin/document-kinds/add"))
    );
    assert_eq!(
        posts("form#reorder"),
        (Some("post"), Some("/api/admin/document-kinds/order"))
    );

    // The row's name and ticks post with its Save form; Condensed Test
    // contains questions only.
    let mut fields = form_fields(&page, &format!("kind-{id}"));
    fields.sort();
    let csrf = page.one("#add-kind input[name=csrf]").value().attr("value").unwrap();
    assert_eq!(
        fields,
        [
            ("csrf", csrf),
            ("id", id.to_string().as_str()),
            ("name", "Condensed Test"),
            ("questions", "true"),
        ]
        .map(|(n, v)| (n.to_owned(), v.to_owned()))
    );

    // ↑ is off on the first row and ↓ on the last.
    let all = rows(&page);
    let disabled = |row: i64, direction: &str| {
        attr(
            page.one(&format!("tr#K{row} form.move button[value={direction}]")),
            "disabled",
        )
        .is_some()
    };
    assert!(disabled(all[0].id, "up") && !disabled(all[0].id, "down"));
    assert!(!disabled(all[1].id, "up") && !disabled(all[1].id, "down"));
    assert!(!disabled(all[6].id, "up") && disabled(all[6].id, "down"));
    // The reorder form starts with the saved order.
    let order: Vec<String> = all.iter().map(|r| r.id.to_string()).collect();
    assert_eq!(
        attr(page.one("form#reorder input[name=order]"), "value"),
        Some(order.join(",").as_str())
    );
}

#[tokio::test]
async fn every_action_needs_an_admin_session_and_a_valid_csrf_token() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let before = app.kinds(&cookie).await;
    let id = app.kind_id(&cookie, "Other").await.to_string();
    let order: Vec<String> = before.iter().rev().map(|r| r.id.to_string()).collect();
    let order = order.join(",");
    let actions: [(&str, Vec<(&str, &str)>); 5] = [
        ("add", vec![("name", "Homework")]),
        ("save", vec![("id", &id), ("name", "Misc")]),
        ("move", vec![("id", &id), ("direction", "up")]),
        ("order", vec![("order", &order)]),
        ("delete", vec![("id", &id)]),
    ];
    let csrf = app
        .kinds_page(&cookie)
        .await
        .one("#add-kind input[name=csrf]")
        .value()
        .attr("value")
        .unwrap()
        .to_owned();

    for (action, fields) in &actions {
        let path = format!("/api/admin/document-kinds/{action}");
        let mut form = vec![("csrf", csrf.as_str())];
        form.extend_from_slice(fields);

        let anonymous = app.post_form(&path, &form, &[]).await;
        assert_eq!(anonymous.status(), 401, "{action} without a session");

        let mut forged = vec![("csrf", "1.deadbeef")];
        forged.extend_from_slice(fields);
        let response = app.post_form(&path, &forged, &[("cookie", &cookie)]).await;
        assert_eq!(response.status(), 403, "{action} with a bad token");

        let cross_site = app
            .post_form(&path, &form, &[("cookie", &cookie), ("origin", "https://evil.example")])
            .await;
        assert_eq!(cross_site.status(), 403, "{action} from another site");
    }
    assert_eq!(app.kinds(&cookie).await, before, "nothing changed");
    assert!(app.kind_log().await.is_empty());
}

// ------------------------------------------------------------ contains flags

#[tokio::test]
async fn ticking_what_a_kind_contains_changes_which_tests_match_includes() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let with_test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[with_test], NewDocument::pdf("Test", &pdf)).await;
    let with_errata = app
        .insert_test(&NewTest {
            year: Some(2001),
            ..NewTest::known()
        })
        .await;
    app.insert_document(&[with_errata], NewDocument::pdf("Errata", &pdf))
        .await;
    let cookie = app.admin().await;

    assert_eq!(includes(&app, "test").await, [with_test]);
    assert!(includes(&app, "answers").await.is_empty());
    assert!(includes(&app, "solutions").await.is_empty());

    // Errata now contains questions, answers and solutions, one tick at a time.
    app.set_tick(&cookie, "Errata", "questions", true).await;
    assert_eq!(includes(&app, "test").await, [with_test, with_errata]);
    assert!(includes(&app, "answers").await.is_empty());
    app.set_tick(&cookie, "Errata", "answers", true).await;
    assert_eq!(includes(&app, "answers").await, [with_errata]);
    app.set_tick(&cookie, "Errata", "solutions", true).await;
    assert_eq!(includes(&app, "solutions").await, [with_errata]);

    // Untick Test's questions: its Test no longer matches Includes: Test.
    app.set_tick(&cookie, "Test", "questions", false).await;
    assert_eq!(includes(&app, "test").await, [with_errata]);
    let found: Vec<(String, Ticks)> = app
        .kinds(&cookie)
        .await
        .into_iter()
        .filter(|r| r.name == "Test" || r.name == "Errata")
        .map(|r| (r.name, r.ticks))
        .collect();
    assert_eq!(
        found,
        [
            ("Test".to_owned(), (false, false, false)),
            ("Errata".to_owned(), (true, true, true))
        ]
    );
}

#[tokio::test]
async fn ticking_what_a_kind_contains_changes_the_download_menus_kind_groups_and_zips() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    app.insert_document(&[test], NewDocument::pdf("Other", &pdf)).await;
    let cookie = app.admin().await;

    assert_eq!(
        groups(&app, test).await,
        ["All documents 2", "Tests 1", "Errata & other 1"]
    );
    assert_eq!(app.get(&format!("/tests/{test}/zip?group=answers")).await.status(), 404);

    // "Other" now holds answers only: it moves from Errata & other to Answers.
    app.set_tick(&cookie, "Other", "answers", true).await;
    assert_eq!(groups(&app, test).await, ["All documents 2", "Tests 1", "Answers 1"]);
    assert_eq!(app.get(&format!("/tests/{test}/zip?group=answers")).await.status(), 200);
    assert_eq!(app.get(&format!("/tests/{test}/zip?group=other")).await.status(), 404);

    // With solutions too, it is a Solutions Document, no longer Answers.
    app.set_tick(&cookie, "Other", "solutions", true).await;
    assert_eq!(groups(&app, test).await, ["All documents 2", "Tests 1", "Solutions 1"]);

    // With questions too, it is in both Tests and Solutions.
    app.set_tick(&cookie, "Other", "questions", true).await;
    assert_eq!(groups(&app, test).await, ["All documents 2", "Tests 2", "Solutions 1"]);
}

// ------------------------------------------------------------------ rename

#[tokio::test]
async fn renaming_a_kind_renames_it_everywhere_and_names_must_be_unique() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Answers", &sample_pdf(500)))
        .await;
    let cookie = app.admin().await;
    let id = app.kind_id(&cookie, "Answers").await;

    app.save_kind(&cookie, id, |fields| {
        fields.iter_mut().find(|(n, _)| n == "name").unwrap().1 = "  Answer   key ".to_owned();
    })
    .await;

    let row = app.kinds(&cookie).await.into_iter().find(|r| r.id == id).unwrap();
    assert_eq!((row.name.as_str(), row.ticks), ("Answer key", (false, true, false)));
    let links: Vec<String> = links_in(app.get_page("/").await.one(&format!("tr#T{test} td.documents")))
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(links, ["Answer key"]);
    assert!(app.add_test_kinds(&cookie).await.contains(&"Answer key".to_owned()));
    assert_eq!(
        app.kind_log().await,
        [entry(id, "name", Some("Answers"), Some("Answer key"))]
    );

    // Another kind's name, in any case, or no name, is refused.
    let too_long = "x".repeat(61);
    for (name, status, message) in [
        ("solutions", 409, "There is already a Document kind “solutions”."),
        ("   ", 400, "Give the Document kind a name."),
        (too_long.as_str(), 400, "at most 60 characters"),
    ] {
        let id = id.to_string();
        let response = app
            .kind_action(&cookie, "save", &[("id", &id), ("name", name), ("answers", "true")])
            .await;
        assert_eq!(response.status(), status, "{name:?}");
        assert!(response.text().await.unwrap().contains(message), "{name:?}");
    }
    let response = app.kind_action(&cookie, "add", &[("name", "TEST")]).await;
    assert_eq!(response.status(), 409);
    assert_eq!(app.kinds(&cookie).await.len(), 7);
    assert_eq!(app.kind_log().await.len(), 1, "nothing more was logged");
}

// --------------------------------------------------------------------- add

#[tokio::test]
async fn adding_a_kind_puts_it_last_with_its_ticks_and_add_test_offers_it() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;

    app.kind_done(
        &cookie,
        "add",
        &[("name", " Answer sheet "), ("answers", "true"), ("solutions", "true")],
    )
    .await;

    let kinds = app.kinds(&cookie).await;
    let added = kinds.last().unwrap();
    assert_eq!(
        (
            added.name.as_str(),
            added.ticks,
            added.documents.as_str(),
            added.deletable
        ),
        ("Answer sheet", (false, true, true), "0", true)
    );
    let mut expected: Vec<&str> = SEEDED.to_vec();
    expected.push("Answer sheet");
    assert_eq!(app.add_test_kinds(&cookie).await, expected);

    // A Document of the new kind counts toward Includes: Solutions.
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Answer sheet", &sample_pdf(500)))
        .await;
    assert_eq!(includes(&app, "solutions").await, [test]);
    assert_eq!(includes(&app, "answers").await, [test]);
    assert!(includes(&app, "test").await.is_empty());

    let id = added.id;
    assert_eq!(
        app.kind_log().await,
        [
            entry(id, "id", None, Some(&id.to_string())),
            entry(id, "name", None, Some("Answer sheet")),
            entry(id, "display_order", None, Some("8")),
            entry(id, "contains_questions", None, Some("false")),
            entry(id, "contains_answers", None, Some("true")),
            entry(id, "contains_solutions", None, Some("true")),
        ]
    );
}

// ----------------------------------------------------------------- reorder

#[tokio::test]
async fn up_and_down_reorder_kinds_and_add_test_and_the_results_follow_the_order() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    app.insert_document(&[test], NewDocument::pdf("Solutions", &pdf)).await;
    let cookie = app.admin().await;
    let solutions = app.kind_id(&cookie, "Solutions").await;
    let test_kind = app.kind_id(&cookie, "Test").await;

    for _ in 0..3 {
        app.kind_done(&cookie, "move", &[("id", &solutions.to_string()), ("direction", "up")])
            .await;
    }
    // Already first: it stays.
    app.kind_done(&cookie, "move", &[("id", &solutions.to_string()), ("direction", "up")])
        .await;
    let expected = [
        "Solutions",
        "Test",
        "Condensed Test",
        "Answers",
        "Combined Test+Solutions",
        "Errata",
        "Other",
    ];
    assert_eq!(names(&app.kinds(&cookie).await), expected);
    assert_eq!(app.add_test_kinds(&cookie).await, expected);
    let links: Vec<String> = links_in(app.get_page("/").await.one(&format!("tr#T{test} td.documents")))
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(links, ["Solutions", "Test"], "Documents follow the kind order");

    app.kind_done(
        &cookie,
        "move",
        &[("id", &test_kind.to_string()), ("direction", "down")],
    )
    .await;
    assert_eq!(
        names(&app.kinds(&cookie).await),
        [
            "Solutions",
            "Condensed Test",
            "Test",
            "Answers",
            "Combined Test+Solutions",
            "Errata",
            "Other"
        ]
    );
    let response = app
        .kind_action(
            &cookie,
            "move",
            &[("id", &test_kind.to_string()), ("direction", "sideways")],
        )
        .await;
    assert_eq!(response.status(), 400);

    // Each move logs the two display orders it swapped.
    let log = app.kind_log().await;
    let condensed = app.kind_id(&cookie, "Condensed Test").await;
    let answers = app.kind_id(&cookie, "Answers").await;
    assert_eq!(
        log,
        [
            entry(solutions, "display_order", Some("4"), Some("3")),
            entry(answers, "display_order", Some("3"), Some("4")),
            entry(solutions, "display_order", Some("3"), Some("2")),
            entry(condensed, "display_order", Some("2"), Some("3")),
            entry(solutions, "display_order", Some("2"), Some("1")),
            entry(test_kind, "display_order", Some("1"), Some("2")),
            entry(condensed, "display_order", Some("3"), Some("2")),
            entry(test_kind, "display_order", Some("2"), Some("3")),
        ]
    );
}

#[tokio::test]
async fn a_drag_posts_the_whole_new_order() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let kinds = app.kinds(&cookie).await;
    let id = |name: &str| kinds.iter().find(|r| r.name == name).unwrap().id;
    let new_order = [
        "Other",
        "Test",
        "Condensed Test",
        "Answers",
        "Solutions",
        "Combined Test+Solutions",
        "Errata",
    ];
    let order: Vec<String> = new_order.iter().map(|n| id(n).to_string()).collect();

    app.kind_done(&cookie, "order", &[("order", &order.join(","))]).await;

    assert_eq!(names(&app.kinds(&cookie).await), new_order);
    assert_eq!(app.add_test_kinds(&cookie).await, new_order);
    // Every kind moved down one but Other, which moved to the top.
    let log = app.kind_log().await;
    assert_eq!(log.len(), 7);
    assert_eq!(log[0], entry(id("Other"), "display_order", Some("7"), Some("1")));
    assert_eq!(log[1], entry(id("Test"), "display_order", Some("1"), Some("2")));

    // An order that leaves out a kind, repeats one, or names a stranger is
    // stale: refused, and nothing moves.
    let partial = order[..6].join(",");
    let repeated = format!("{},{}", order[0], order[..6].join(","));
    let stranger = format!("{},999999", order[..6].join(","));
    for bad in [partial.as_str(), &repeated, &stranger, "Other,Test"] {
        let response = app.kind_action(&cookie, "order", &[("order", bad)]).await;
        assert_eq!(response.status(), 409, "{bad}");
        assert!(response.text().await.unwrap().contains("Reload it and drag again"));
    }
    assert_eq!(names(&app.kinds(&cookie).await), new_order);
    assert_eq!(app.kind_log().await.len(), 7);
}

// ------------------------------------------------------------------ delete

#[tokio::test]
async fn a_kind_in_use_cannot_be_deleted_and_an_unused_one_can() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(500);
    let test = app.insert_test(&NewTest::known()).await;
    app.insert_document(
        &[test],
        NewDocument {
            published: false,
            ..NewDocument::pdf("Errata", &pdf)
        },
    )
    .await;
    let trashed = app.insert_document(&[test], NewDocument::pdf("Answers", &pdf)).await;
    app.trash_document(trashed).await;
    let cookie = app.admin().await;

    // Used by a Hidden Document, or by one in Trash: refused.
    for (name, documents) in [("Errata", "1 Document uses it"), ("Answers", "1 Document uses it")] {
        let id = app.kind_id(&cookie, name).await.to_string();
        let response = app.kind_action(&cookie, "delete", &[("id", &id)]).await;
        assert_eq!(response.status(), 409, "{name}");
        let body = response.text().await.unwrap();
        assert!(
            body.contains(&format!("“{name}” can’t be deleted: {documents}")),
            "{body}"
        );
    }
    assert_eq!(names(&app.kinds(&cookie).await), SEEDED);

    let other = app.kind_id(&cookie, "Other").await;
    app.kind_done(&cookie, "delete", &[("id", &other.to_string())]).await;
    assert_eq!(names(&app.kinds(&cookie).await), &SEEDED[..6]);
    assert_eq!(app.add_test_kinds(&cookie).await, &SEEDED[..6]);
    assert_eq!(
        app.kind_log().await,
        [
            entry(other, "id", Some(&other.to_string()), None),
            entry(other, "name", Some("Other"), None),
            entry(other, "display_order", Some("7"), None),
            entry(other, "contains_questions", Some("false"), None),
            entry(other, "contains_answers", Some("false"), None),
            entry(other, "contains_solutions", Some("false"), None),
        ]
    );

    // Deleting it again: it is gone.
    let response = app.kind_action(&cookie, "delete", &[("id", &other.to_string())]).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn a_kind_held_by_a_duplicate_warning_cannot_be_deleted() {
    let app = TestApp::start().await;
    app.insert_test(&NewTest::known()).await;
    let cookie = app.admin().await;
    let new_kind = "Answer sheet";
    app.kind_done(&cookie, "add", &[("name", new_kind), ("answers", "true")])
        .await;
    // The same six Facet values as the Test above: held for the warning.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("Key.pdf", &sample_pdf(500))
        .field("kind_0", new_kind)
        .field("label_0", "");
    let response = app.submit_add_test(&cookie, form, "hidden").await;
    assert_eq!(response.status(), 303);
    assert!(response.headers()["location"].to_str().unwrap().contains("pending="));

    let row = app
        .kinds(&cookie)
        .await
        .into_iter()
        .find(|r| r.name == new_kind)
        .unwrap();
    assert_eq!((row.documents.as_str(), row.deletable), ("0 + 1 waiting", false));
    let response = app.kind_action(&cookie, "delete", &[("id", &row.id.to_string())]).await;
    assert_eq!(response.status(), 409);
    assert!(response.text().await.unwrap().contains("duplicate warning"));
}
