//! **Facet values**: an Admin curates each Facet's values (reorder, rename,
//! merge, delete, add), searches keep finding Tests by old names, and every
//! change lands in the change log.

use crate::support::{
    TestApp,
    add_test::added_test,
    fixtures::{Month, NewTest},
    html::{Page, text, text_in},
};

/// A change log entry: `(entity, entity id, field, before, after)`.
type Entry = (String, i64, String, Option<String>, Option<String>);

fn entry(entity: &str, id: i64, field: &str, before: Option<&str>, after: Option<&str>) -> Entry {
    (
        entity.to_owned(),
        id,
        field.to_owned(),
        before.map(str::to_owned),
        after.map(str::to_owned),
    )
}

/// One value as the Facet values page lists it.
#[derive(Debug, PartialEq)]
struct Row {
    id: i64,
    name: String,
    /// The usage count, e.g. "2 tests (1 in Trash)".
    uses: String,
    aliases: Vec<String>,
}

fn rows(page: &Page) -> Vec<Row> {
    page.select("#values > li")
        .into_iter()
        .map(|li| {
            let id = li.value().attr("id").expect("rows have an id");
            let alias = scraper::Selector::parse(".alias").unwrap();
            Row {
                id: id.strip_prefix('v').unwrap().parse().unwrap(),
                name: text_in(li, ".name"),
                uses: text_in(li, ".uses").trim_start_matches("· ").to_owned(),
                aliases: li.select(&alias).map(text).collect(),
            }
        })
        .collect()
}

fn names(page: &Page) -> Vec<String> {
    rows(page).into_iter().map(|r| r.name).collect()
}

/// The texts of a search page dropdown's values, without Any and Unknown.
fn dropdown(page: &Page, name: &str) -> Vec<String> {
    page.select(&format!("#filters select[name={name:?}] option"))
        .into_iter()
        .filter(|o| !matches!(o.value().attr("value"), Some("" | "unknown")))
        .map(text)
        .collect()
}

/// The text of a search page dropdown's chosen option.
fn chosen(page: &Page, name: &str) -> String {
    page.text(&format!("#filters select[name={name:?}] option[selected]"))
}

fn result_ids(page: &Page) -> Vec<i64> {
    page.select("#results tbody tr")
        .into_iter()
        .map(|row| row.value().attr("id").unwrap()[1..].parse().unwrap())
        .collect()
}

fn location(response: &reqwest::Response) -> String {
    response.headers()["location"].to_str().unwrap().to_owned()
}

impl TestApp {
    async fn facets_page(&self, cookie: &str, facet: &str) -> Page {
        self.get_page_as(&format!("/admin/facets?facet={facet}"), Some(cookie))
            .await
    }

    /// The ID of the value the Facet values page lists under `name`.
    async fn value_id(&self, cookie: &str, facet: &str, name: &str) -> i64 {
        rows(&self.facets_page(cookie, facet).await)
            .into_iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no {facet} value {name:?}"))
            .id
    }

    /// Post one of the page's forms as the logged-in Admin, with the CSRF
    /// token the page carries.
    async fn post_facet_action(&self, cookie: &str, path: &str, fields: &[(&str, &str)]) -> reqwest::Response {
        let page = self.facets_page(cookie, "month").await;
        let csrf = page
            .one("#add-value input[name=csrf]")
            .value()
            .attr("value")
            .unwrap()
            .to_owned();
        let mut form = vec![("csrf", csrf.as_str())];
        form.extend_from_slice(fields);
        self.post_form(path, &form, &[("cookie", cookie)]).await
    }

    /// Like [`Self::post_facet_action`], expecting the 303 back to the page.
    async fn facet_action(&self, cookie: &str, path: &str, fields: &[(&str, &str)]) -> String {
        let response = self.post_facet_action(cookie, path, fields).await;
        let status = response.status();
        let location = response
            .headers()
            .get("location")
            .map(|l| l.to_str().unwrap().to_owned())
            .unwrap_or_default();
        assert_eq!(
            status,
            303,
            "POST {path}: {}",
            response.text().await.unwrap_or_default()
        );
        assert!(location.starts_with("/admin/facets?facet="), "{location}");
        location
    }

    async fn rename(&self, cookie: &str, id: i64, name: &str) {
        let id = id.to_string();
        self.facet_action(cookie, "/api/admin/facet-values/rename", &[("id", &id), ("name", name)])
            .await;
    }

    async fn merge(&self, cookie: &str, id: i64, into: &str) {
        let id = id.to_string();
        self.facet_action(cookie, "/api/admin/facet-values/merge", &[("id", &id), ("into", into)])
            .await;
    }

    async fn move_value(&self, cookie: &str, id: i64, direction: &str) -> String {
        let id = id.to_string();
        self.facet_action(
            cookie,
            "/api/admin/facet-values/move",
            &[("id", &id), ("direction", direction)],
        )
        .await
    }

    /// A Published Test with these Host and Division/Level values and every
    /// other Facet as [`NewTest::known`].
    async fn test_at(&self, host: &'static str, division_level: &'static str) -> i64 {
        self.insert_test(&NewTest {
            host: Some(host),
            division_level: Some(division_level),
            ..NewTest::known()
        })
        .await
    }

    /// The Test IDs a search URL lists.
    async fn found(&self, path: &str) -> Vec<i64> {
        result_ids(&self.get_page(path).await)
    }
}

// --------------------------------------------------------------- the page

#[tokio::test]
async fn the_page_lists_each_facets_values_in_display_order_with_usage_counts() {
    let app = TestApp::start().await;
    app.test_at("Plant City", "Geometry").await;
    app.test_at("Plant City", "Algebra I").await;
    app.insert_test(&NewTest {
        host: Some("Chiles"),
        published: false,
        ..NewTest::known()
    })
    .await;
    let trashed = app.test_at("Chiles", "Geometry").await;
    app.trash_test(trashed).await;
    app.facet_value("host", "Lawton Chiles").await;
    let cookie = app.admin().await;

    let search = app.get_page_as("/", Some(&cookie)).await;
    assert_eq!(
        search
            .one("#admin-menu a[href='/admin/facets']")
            .text()
            .collect::<String>(),
        "Facet values"
    );

    let page = app.facets_page(&cookie, "host").await;
    let tabs: Vec<(String, String)> = page
        .select(".facet-tabs a")
        .into_iter()
        .map(|a| (text(a), a.value().attr("href").unwrap().to_owned()))
        .collect();
    assert_eq!(
        tabs,
        [
            ("Month", "/admin/facets?facet=month"),
            ("Competition", "/admin/facets?facet=competition"),
            ("Division/Level", "/admin/facets?facet=division_level"),
            ("Test type", "/admin/facets?facet=test_type"),
            ("Host", "/admin/facets?facet=host"),
        ]
        .map(|(l, h)| (l.to_owned(), h.to_owned()))
    );
    assert_eq!(page.text(".facet-tabs a[aria-current=page]"), "Host");
    let listed: Vec<(String, String)> = rows(&page).into_iter().map(|r| (r.name, r.uses)).collect();
    assert_eq!(
        listed,
        [
            ("Plant City", "2 tests"),
            ("Chiles", "2 tests (1 in Trash)"),
            ("Lawton Chiles", "0 tests"),
        ]
        .map(|(n, u)| (n.to_owned(), u.to_owned()))
    );
    // The non-JS controls: the first can't move up, the last can't move down,
    // and only the unused value offers Delete.
    let up = page.select("#values form.move button[value=up]");
    let down = page.select("#values form.move button[value=down]");
    assert!(up[0].value().attr("disabled").is_some());
    assert!(up[1].value().attr("disabled").is_none());
    assert!(down[2].value().attr("disabled").is_some());
    let deletable: Vec<String> = page
        .select("#values > li")
        .into_iter()
        .filter(|li| {
            li.select(&scraper::Selector::parse("form.delete").unwrap())
                .next()
                .is_some()
        })
        .map(|li| text_in(li, ".name"))
        .collect();
    assert_eq!(deletable, ["Lawton Chiles"]);
    assert_eq!(page.select("#values li form.rename input[name=name]").len(), 3);
    assert_eq!(page.select("#values li form.merge input[name=into]").len(), 3);

    // Month is the default view, and each Facet has its own.
    let month = app.get_page_as("/admin/facets", Some(&cookie)).await;
    assert_eq!(month.text(".facet-tabs a[aria-current=page]"), "Month");
    assert_eq!(names(&month), ["February"]);
    assert_eq!(
        names(&app.facets_page(&cookie, "division_level").await),
        ["Geometry", "Algebra I", "Calculus/Mu"]
    );
}

#[tokio::test]
async fn the_page_and_its_data_are_for_admins_only() {
    let app = TestApp::start().await;
    app.insert_test(&NewTest::known()).await;

    let page = app.get_page("/admin/facets?facet=host").await;
    assert!(page.select("#values").is_empty());
    assert_eq!(page.text("#not-logged-in"), "You are not logged in. Log in");
    assert_eq!(app.get("/api/admin/facet-values?facet=host").await.status(), 401);
}

// ---------------------------------------------------------------- reorder

#[tokio::test]
async fn moving_a_value_changes_the_dropdown_and_result_order() {
    let app = TestApp::start().await;
    let plant_city = app.test_at("Plant City", "Calculus/Mu").await;
    let chiles = app.test_at("Chiles", "Calculus/Mu").await;
    let cookie = app.admin().await;
    let search = app.get_page("/").await;
    assert_eq!(dropdown(&search, "host"), ["Plant City", "Chiles"]);
    assert_eq!(result_ids(&search), [plant_city, chiles]);
    let id = app.value_id(&cookie, "host", "Plant City").await;
    let before = app.change_log().await.len();

    let location = app.move_value(&cookie, id, "down").await;

    assert_eq!(location, format!("/admin/facets?facet=host#v{id}"));
    assert_eq!(names(&app.facets_page(&cookie, "host").await), ["Chiles", "Plant City"]);
    let search = app.get_page("/").await;
    assert_eq!(dropdown(&search, "host"), ["Chiles", "Plant City"]);
    assert_eq!(result_ids(&search), [chiles, plant_city]);
    let chiles_id = app.value_id(&cookie, "host", "Chiles").await;
    assert_eq!(
        app.change_log().await[before..],
        [
            entry("facet_value", chiles_id, "display_order", Some("2"), Some("1")),
            entry("facet_value", id, "display_order", Some("1"), Some("2")),
        ]
    );

    // Back up again; moving past either end changes nothing.
    app.move_value(&cookie, id, "up").await;
    app.move_value(&cookie, id, "up").await;
    let search = app.get_page("/").await;
    assert_eq!(dropdown(&search, "host"), ["Plant City", "Chiles"]);
    assert_eq!(result_ids(&search), [plant_city, chiles]);
    assert_eq!(app.change_log().await.len(), before + 4);
}

#[tokio::test]
async fn dragging_saves_the_whole_new_order() {
    let app = TestApp::start().await;
    app.facet_value("division_level", "Calculus/Mu").await;
    let geometry = app.test_at("Plant City", "Geometry").await;
    let algebra = app.test_at("Plant City", "Algebra I").await;
    let theta = app.test_at("Plant City", "Theta").await;
    let cookie = app.admin().await;
    let ids = [
        app.value_id(&cookie, "division_level", "Calculus/Mu").await,
        app.value_id(&cookie, "division_level", "Geometry").await,
        app.value_id(&cookie, "division_level", "Algebra I").await,
        app.value_id(&cookie, "division_level", "Theta").await,
    ];
    let [calculus, geometry_id, algebra_id, theta_id] = ids;
    let before = app.change_log().await.len();

    // The page posts the order after a drop. An ID that isn't the Facet's is
    // ignored, and a value the order leaves out (say, added meanwhile) keeps
    // its place after the listed ones.
    let page = app.facets_page(&cookie, "division_level").await;
    let reorder = page.one("form#reorder");
    assert_eq!(reorder.value().attr("action"), Some("/api/admin/facet-values/reorder"));
    let order = format!("{algebra_id},{geometry_id},999999,{theta_id}");
    app.facet_action(
        &cookie,
        "/api/admin/facet-values/reorder",
        &[("facet", "division_level"), ("order", &order)],
    )
    .await;

    let expected = ["Algebra I", "Geometry", "Theta", "Calculus/Mu"];
    assert_eq!(names(&app.facets_page(&cookie, "division_level").await), expected);
    let search = app.get_page("/").await;
    assert_eq!(dropdown(&search, "division_level"), expected);
    assert_eq!(result_ids(&search), [algebra, geometry, theta]);
    assert_eq!(
        app.change_log().await[before..],
        [
            entry("facet_value", algebra_id, "display_order", Some("3"), Some("1")),
            entry("facet_value", theta_id, "display_order", Some("4"), Some("3")),
            entry("facet_value", calculus, "display_order", Some("1"), Some("4")),
        ]
    );
}

// ----------------------------------------------------------------- rename

#[tokio::test]
async fn after_a_rename_a_search_url_with_the_old_name_still_finds_the_same_tests() {
    let app = TestApp::start().await;
    let first = app.test_at("Chiles", "Geometry").await;
    let second = app.test_at("Chiles", "Algebra I").await;
    app.test_at("Plant City", "Geometry").await;
    let cookie = app.admin().await;
    let old_url = "/?host=Chiles";
    let found = app.found(old_url).await;
    assert_eq!(found, [first, second]);
    let id = app.value_id(&cookie, "host", "Chiles").await;
    let before = app.change_log().await.len();

    app.rename(&cookie, id, "Lawton Chiles").await;

    let page = app.get_page(old_url).await;
    assert_eq!(result_ids(&page), found);
    // The page shows the value under its new name.
    assert_eq!(chosen(&page, "host"), "Lawton Chiles");
    assert_eq!(dropdown(&page, "host"), ["Lawton Chiles", "Plant City"]);
    assert_eq!(page.text(&format!("tr#T{first} td.host")), "Lawton Chiles");
    assert_eq!(app.found("/?host=Lawton+Chiles").await, found);
    assert_eq!(app.found("/?host=lawton+chiles").await, found, "case doesn't matter");
    let row = rows(&app.facets_page(&cookie, "host").await).remove(0);
    assert_eq!(
        (row.name.as_str(), row.aliases),
        ("Lawton Chiles", vec!["Chiles".to_owned()])
    );
    assert_eq!(
        app.change_log().await[before..],
        [
            entry("facet_value", id, "name", Some("Chiles"), Some("Lawton Chiles")),
            entry("facet_value", id, "aliases", None, Some("Chiles")),
        ]
    );

    // Every former name keeps working.
    app.rename(&cookie, id, "Lawton Chiles High").await;
    assert_eq!(app.found(old_url).await, found);
    assert_eq!(app.found("/?host=Lawton+Chiles").await, found);
    let row = rows(&app.facets_page(&cookie, "host").await).remove(0);
    assert_eq!(row.aliases, ["Chiles", "Lawton Chiles"]);

    // Renaming back to a former name takes it back from the aliases.
    app.rename(&cookie, id, "Chiles").await;
    let row = rows(&app.facets_page(&cookie, "host").await).remove(0);
    assert_eq!(row.name, "Chiles");
    assert_eq!(row.aliases, ["Lawton Chiles", "Lawton Chiles High"]);
    assert_eq!(app.found("/?host=Lawton+Chiles+High").await, found);
}

#[tokio::test]
async fn a_rename_to_another_values_name_is_refused() {
    let app = TestApp::start().await;
    app.test_at("Chiles", "Geometry").await;
    app.test_at("Lawton Chiles", "Geometry").await;
    let cookie = app.admin().await;
    let chiles = app.value_id(&cookie, "host", "Chiles").await;
    let lawton = app.value_id(&cookie, "host", "Lawton Chiles").await;
    app.rename(&cookie, lawton, "Lawton Chiles HS").await;
    let before = app.change_log().await.len();
    let id = chiles.to_string();

    for (name, status, message) in [
        (
            "lawton chiles hs",
            409,
            "There is already a Host “Lawton Chiles HS”. To make them one, merge “Chiles” into it.",
        ),
        (
            "Lawton Chiles",
            409,
            "“Lawton Chiles” is a former name of the Host “Lawton Chiles HS”. \
             To make them one, merge “Chiles” into “Lawton Chiles HS”.",
        ),
        ("  ", 400, "Type a name for the Host."),
        (
            "Unknown",
            400,
            "“Unknown” is how the search says a value is not known; choose another name.",
        ),
    ] {
        let response = app
            .post_facet_action(
                &cookie,
                "/api/admin/facet-values/rename",
                &[("id", &id), ("name", name)],
            )
            .await;
        assert_eq!(response.status(), status, "{name}");
        let page = Page::parse(&response.text().await.unwrap());
        assert_eq!(page.text("#refusal"), message);
        assert_eq!(
            page.one("a").value().attr("href"),
            Some("/admin/facets?facet=host"),
            "it links back"
        );
    }
    assert_eq!(
        names(&app.facets_page(&cookie, "host").await),
        ["Chiles", "Lawton Chiles HS"]
    );
    assert_eq!(app.change_log().await.len(), before);

    // A change of case alone is a rename.
    app.rename(&cookie, chiles, "CHILES").await;
    assert_eq!(
        names(&app.facets_page(&cookie, "host").await),
        ["CHILES", "Lawton Chiles HS"]
    );
}

// ------------------------------------------------------------------ merge

#[tokio::test]
async fn a_merge_moves_the_tests_and_old_names_still_find_them() {
    let app = TestApp::start().await;
    let chiles = app.test_at("Chiles HS", "Geometry").await;
    let trashed = app.test_at("Chiles HS", "Algebra I").await;
    app.trash_test(trashed).await;
    let lawton = app.test_at("Lawton Chiles", "Algebra I").await;
    let other = app.test_at("Plant City", "Geometry").await;
    let cookie = app.admin().await;
    let old = app.value_id(&cookie, "host", "Chiles HS").await;
    app.rename(&cookie, old, "Chiles").await;
    let survivor = app.value_id(&cookie, "host", "Lawton Chiles").await;
    assert_eq!(app.found("/?host=Chiles").await, [chiles]);
    let before = app.change_log().await.len();

    app.merge(&cookie, old, "Lawton Chiles").await;

    // In Division/Level order: Geometry, then Algebra I.
    let both = [chiles, lawton];
    for url in ["/?host=Chiles", "/?host=Chiles+HS", "/?host=Lawton+Chiles"] {
        let page = app.get_page(url).await;
        assert_eq!(result_ids(&page), both, "{url}");
        assert_eq!(chosen(&page, "host"), "Lawton Chiles", "{url}");
    }
    let search = app.get_page("/").await;
    assert_eq!(dropdown(&search, "host"), ["Lawton Chiles", "Plant City"]);
    assert_eq!(search.text(&format!("tr#T{chiles} td.host")), "Lawton Chiles");
    assert_eq!(app.found("/?host=Plant+City").await, [other]);
    let page = app.facets_page(&cookie, "host").await;
    assert_eq!(
        rows(&page)[0],
        Row {
            id: survivor,
            name: "Lawton Chiles".to_owned(),
            uses: "3 tests (1 in Trash)".to_owned(),
            aliases: vec!["Chiles".to_owned(), "Chiles HS".to_owned()],
        }
    );
    assert_eq!(names(&page), ["Lawton Chiles", "Plant City"]);
    assert_eq!(
        app.change_log().await[before..],
        [
            entry("test", chiles, "host", Some("Chiles"), Some("Lawton Chiles")),
            entry("test", trashed, "host", Some("Chiles"), Some("Lawton Chiles")),
            entry("facet_value", old, "id", Some(&old.to_string()), None),
            entry("facet_value", old, "facet", Some("host"), None),
            entry("facet_value", old, "name", Some("Chiles"), None),
            entry("facet_value", old, "display_order", Some("1"), None),
            entry("facet_value", old, "aliases", Some("Chiles HS"), None),
            entry("facet_value", old, "merged_into", None, Some("Lawton Chiles")),
            entry("facet_value", survivor, "aliases", None, Some("Chiles, Chiles HS")),
        ]
    );
}

#[tokio::test]
async fn a_merge_needs_another_value_of_the_same_facet() {
    let app = TestApp::start().await;
    app.test_at("Chiles", "Geometry").await;
    let cookie = app.admin().await;
    let id = app.value_id(&cookie, "host", "Chiles").await.to_string();
    let before = app.change_log().await.len();

    for (into, message) in [
        ("Chiles", "“Chiles” can’t be merged into itself. Choose another Host."),
        // A Division/Level, not a Host.
        ("Geometry", "There is no Host “Geometry”."),
        ("", "Choose the Host to merge “Chiles” into."),
    ] {
        let response = app
            .post_facet_action(&cookie, "/api/admin/facet-values/merge", &[("id", &id), ("into", into)])
            .await;
        assert_eq!(response.status(), 400, "{into}");
        assert_eq!(Page::parse(&response.text().await.unwrap()).text("#refusal"), message);
    }
    assert_eq!(app.change_log().await.len(), before);
}

#[tokio::test]
async fn files_held_for_the_duplicate_warning_follow_a_merge() {
    let app = TestApp::start().await;
    app.insert_test(&NewTest::known()).await;
    app.facet_value("host", "Plant City High").await;
    let cookie = app.admin().await;
    let pdf = crate::support::fixtures::sample_pdf(2000);
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_Calculus.pdf", &pdf);
    let response = app.submit_add_test(&cookie, form, "publish").await;
    let held = location(&response);
    assert!(held.starts_with("/admin/tests/new?pending="), "{held}");
    let id = app.value_id(&cookie, "host", "Plant City").await;

    app.merge(&cookie, id, "Plant City High").await;

    let csrf = app.csrf_on(&held, Some(&cookie)).await;
    let pending = held.rsplit('=').next().unwrap();
    let response = app
        .post_form(
            "/api/admin/uploads/finish",
            &[("csrf", &csrf), ("pending", pending), ("action", "new")],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(response.status(), 303);
    let test = added_test(&location(&response));
    let page = app.get_page("/?host=Plant+City").await;
    assert_eq!(page.text(&format!("tr#T{test} td.host")), "Plant City High");
}

// ----------------------------------------------------------------- delete

#[tokio::test]
async fn deleting_a_value_in_use_is_refused() {
    let app = TestApp::start().await;
    // Only a Trashed Test uses it, which still counts: it can be restored.
    let trashed = app.test_at("Chiles", "Geometry").await;
    app.trash_test(trashed).await;
    let cookie = app.admin().await;
    let id = app.value_id(&cookie, "host", "Chiles").await;
    let before = app.change_log().await.len();

    let response = app
        .post_facet_action(&cookie, "/api/admin/facet-values/delete", &[("id", &id.to_string())])
        .await;

    assert_eq!(response.status(), 409);
    assert_eq!(
        Page::parse(&response.text().await.unwrap()).text("#refusal"),
        "“Chiles” can’t be deleted: 1 Test uses it. Merge it into another Host instead."
    );
    assert_eq!(names(&app.facets_page(&cookie, "host").await), ["Chiles"]);
    assert_eq!(app.change_log().await.len(), before);
}

#[tokio::test]
async fn an_unused_value_can_be_deleted() {
    let app = TestApp::start().await;
    app.test_at("Plant City", "Geometry").await;
    let cookie = app.admin().await;
    let unused = app.facet_value("host", "Chiles").await;
    let page = app.facets_page(&cookie, "host").await;
    let delete = page.one(&format!("#v{unused} form.delete"));
    assert_eq!(delete.value().attr("action"), Some("/api/admin/facet-values/delete"));
    let before = app.change_log().await.len();

    app.facet_action(
        &cookie,
        "/api/admin/facet-values/delete",
        &[("id", &unused.to_string())],
    )
    .await;

    assert_eq!(names(&app.facets_page(&cookie, "host").await), ["Plant City"]);
    assert_eq!(dropdown(&app.get_page("/").await, "host"), ["Plant City"]);
    assert_eq!(app.found("/?host=Chiles").await, Vec::<i64>::new());
    assert_eq!(
        app.change_log().await[before..],
        [
            entry("facet_value", unused, "id", Some(&unused.to_string()), None),
            entry("facet_value", unused, "facet", Some("host"), None),
            entry("facet_value", unused, "name", Some("Chiles"), None),
            entry("facet_value", unused, "display_order", Some("2"), None),
        ]
    );

    // Deleting it again finds nothing.
    let response = app
        .post_facet_action(
            &cookie,
            "/api/admin/facet-values/delete",
            &[("id", &unused.to_string())],
        )
        .await;
    assert_eq!(response.status(), 404);
}

// -------------------------------------------------------------------- add

#[tokio::test]
async fn an_added_value_goes_last_and_is_offered_everywhere() {
    let app = TestApp::start().await;
    app.insert_test(&NewTest {
        month: Month::Known("January"),
        ..NewTest::known()
    })
    .await;
    let cookie = app.admin().await;
    let before = app.change_log().await.len();

    let location = app
        .facet_action(
            &cookie,
            "/api/admin/facet-values",
            &[("facet", "month"), ("name", "  November ")],
        )
        .await;

    let id = app.value_id(&cookie, "month", "November").await;
    assert_eq!(location, format!("/admin/facets?facet=month#v{id}"));
    let page = app.facets_page(&cookie, "month").await;
    assert_eq!(rows(&page)[1].uses, "0 tests");
    assert_eq!(dropdown(&app.get_page("/").await, "month"), ["January", "November"]);
    let add_test = app.add_test_form(&cookie).await;
    let months: Vec<&str> = add_test.options("month").iter().map(|(t, _)| t.as_str()).collect();
    assert!(months.contains(&"November"), "{months:?}");
    assert_eq!(
        app.change_log().await[before..],
        [
            entry("facet_value", id, "id", None, Some(&id.to_string())),
            entry("facet_value", id, "facet", None, Some("month")),
            entry("facet_value", id, "name", None, Some("November")),
            entry("facet_value", id, "display_order", None, Some("2")),
        ]
    );
}

#[tokio::test]
async fn adding_a_name_the_facet_has_or_had_is_refused() {
    let app = TestApp::start().await;
    app.test_at("Chiles", "Geometry").await;
    let cookie = app.admin().await;
    let id = app.value_id(&cookie, "host", "Chiles").await;
    app.rename(&cookie, id, "Lawton Chiles").await;
    let before = app.change_log().await.len();

    for (facet, name, status, message) in [
        ("host", "lawton chiles", 409, "There is already a Host “Lawton Chiles”."),
        (
            "host",
            "Chiles",
            409,
            "“Chiles” is a former name of the Host “Lawton Chiles”.",
        ),
        ("host", "", 400, "Type a name for the Host."),
        ("year", "1999", 400, "Choose a Facet."),
    ] {
        let response = app
            .post_facet_action(&cookie, "/api/admin/facet-values", &[("facet", facet), ("name", name)])
            .await;
        assert_eq!(response.status(), status, "{name}");
        assert_eq!(Page::parse(&response.text().await.unwrap()).text("#refusal"), message);
    }
    // The same name is fine in another Facet.
    app.facet_action(
        &cookie,
        "/api/admin/facet-values",
        &[("facet", "test_type"), ("name", "Chiles")],
    )
    .await;
    assert_eq!(app.change_log().await.len(), before + 4);
}

// ------------------------------------------------------------ authorization

#[tokio::test]
async fn every_action_needs_an_admin_session_and_its_csrf_token() {
    let app = TestApp::start().await;
    app.test_at("Chiles", "Geometry").await;
    app.test_at("Plant City", "Geometry").await;
    let unused = app.facet_value("host", "Lawton Chiles").await.to_string();
    let cookie = app.admin().await;
    let id = app.value_id(&cookie, "host", "Chiles").await.to_string();
    let other = app.value_id(&cookie, "host", "Plant City").await.to_string();
    let page = app.facets_page(&cookie, "host").await;
    let csrf = page
        .one("#add-value input[name=csrf]")
        .value()
        .attr("value")
        .unwrap()
        .to_owned();
    let order = format!("{other},{id}");
    let actions: [(&str, Vec<(&str, &str)>); 6] = [
        ("/api/admin/facet-values", vec![("facet", "host"), ("name", "Wharton")]),
        (
            "/api/admin/facet-values/rename",
            vec![("id", &id), ("name", "Chiles HS")],
        ),
        (
            "/api/admin/facet-values/merge",
            vec![("id", &id), ("into", "Plant City")],
        ),
        ("/api/admin/facet-values/delete", vec![("id", &unused)]),
        ("/api/admin/facet-values/move", vec![("id", &id), ("direction", "up")]),
        (
            "/api/admin/facet-values/reorder",
            vec![("facet", "host"), ("order", &order)],
        ),
    ];

    for (path, fields) in &actions {
        // Every page form posts to one of these.
        assert!(
            !page.select(&format!("form[action='{path}']")).is_empty(),
            "the page has a form for {path}"
        );
        let with = |token: &'static str| -> Vec<(&str, &str)> {
            let mut form = vec![("csrf", if token.is_empty() { csrf.as_str() } else { token })];
            form.extend(fields.iter().copied());
            form
        };
        let anonymous = app.post_form(path, &with(""), &[]).await;
        assert_eq!(anonymous.status(), 401, "{path} without a session");
        let forged = app.post_form(path, &with("forged"), &[("cookie", &cookie)]).await;
        assert_eq!(forged.status(), 403, "{path} with a bad token");
        let cross_site = app
            .post_form(
                path,
                &with(""),
                &[("cookie", &cookie), ("origin", "https://evil.example")],
            )
            .await;
        assert_eq!(cross_site.status(), 403, "{path} from another site");
    }
    assert_eq!(
        names(&app.facets_page(&cookie, "host").await),
        ["Chiles", "Plant City", "Lawton Chiles"]
    );
    assert!(app.change_log().await.is_empty(), "nothing changed");
}
