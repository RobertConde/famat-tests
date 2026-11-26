//! **Add test**: an Admin adds a Test by choosing its six Facet values and
//! dropping in its files, and every write lands in the change log.

use crate::support::{
    TestApp,
    add_test::{AddTestForm, SavedDocument, added_test},
    admin::session_cookie,
    download::disposition,
    fixtures::{Month, NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Page, links_in, text},
};

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const ALLOWED: &str = "only PDF, DOC, DOCX, PPT, PPTX, RTF, ODT and ZIP files are allowed";
const MIB: u64 = 1024 * 1024;

/// A change log entry's `(field, before, after)`.
type Entry = (String, Option<String>, Option<String>);

/// The `(text, href)` of every Document link in a Test's row on the search page.
async fn row_links(app: &TestApp, test: i64) -> Vec<(String, String)> {
    let page = app.get_page("/").await;
    links_in(page.one(&format!("tr#T{test} td.documents")))
}

/// The bytes of a download, asserting a 200.
async fn download(app: &TestApp, path: &str) -> Vec<u8> {
    let response = app.get(path).await;
    assert_eq!(response.status(), 200, "GET {path}");
    response.bytes().await.unwrap().to_vec()
}

fn ids(page: &Page, css: &str) -> Vec<String> {
    page.select(css)
        .into_iter()
        .map(|e| e.value().attr("id").unwrap_or_default().to_owned())
        .collect()
}

// --------------------------------------------------------------- the page

#[tokio::test]
async fn the_add_test_page_is_in_the_admin_menu_and_only_for_admins() {
    let app = TestApp::start().await;

    let anonymous = app.get_page("/admin/tests/new").await;
    assert!(anonymous.select("form#add-test").is_empty());
    assert!(text(anonymous.one("#not-logged-in")).contains("not logged in"));

    let cookie = app.admin().await;
    let search = app.get_page_as("/", Some(&cookie)).await;
    let menu = links_in(search.one("#admin-menu"));
    assert!(
        menu.contains(&("Add test".to_owned(), "/admin/tests/new".to_owned())),
        "{menu:?}"
    );

    let page = app.get_page_as("/admin/tests/new", Some(&cookie)).await;
    let form = page.one("form#add-test");
    assert_eq!(form.value().attr("method"), Some("post"));
    assert_eq!(form.value().attr("action"), Some("/api/admin/tests"));
    assert_eq!(form.value().attr("enctype"), Some("multipart/form-data"));
    let file_input = page.one("form#add-test input[type=file]");
    assert_eq!(file_input.value().attr("name"), Some("files"));
    assert!(file_input.value().attr("multiple").is_some(), "several files at once");
    let buttons: Vec<(String, String)> = page
        .select("form#add-test button[type=submit]")
        .into_iter()
        .map(|b| (text(b), b.value().attr("value").unwrap().to_owned()))
        .collect();
    assert_eq!(
        buttons,
        [("Save hidden", "hidden"), ("Publish", "publish")].map(|(t, v)| (t.to_owned(), v.to_owned()))
    );
}

#[tokio::test]
async fn the_six_dropdowns_list_their_values_in_display_order_with_unknown_and_new_value() {
    let app = TestApp::start().await;
    // Created out of order: display order, not insertion or the alphabet, rules.
    for (name, order) in [("Lawton Chiles", 3), ("FAMAT", 1), ("MAO", 2)] {
        let id = app.facet_value("host", name).await;
        sqlx::query("UPDATE facet_values SET display_order = $2 WHERE id = $1")
            .bind(id)
            .bind(order)
            .execute(&app.db)
            .await
            .unwrap();
    }
    app.insert_test(&NewTest {
        year: Some(1999),
        ..NewTest::known()
    })
    .await;
    app.insert_test(&NewTest {
        year: Some(2004),
        published: false,
        ..NewTest::known()
    })
    .await;
    let cookie = app.admin().await;

    let form = app.add_test_form(&cookie).await;
    let page = app.get_page_as("/admin/tests/new", Some(&cookie)).await;
    let names: Vec<String> = page
        .select("form#add-test select")
        .into_iter()
        .map(|s| s.value().attr("name").unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        ["year", "month", "competition", "division_level", "test_type", "host"]
    );
    let labels: Vec<String> = page
        .select("form#add-test .facet > label")
        .into_iter()
        .map(text)
        .collect();
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

    let texts = |name: &str| -> Vec<String> { form.options(name).iter().map(|(t, _)| t.clone()).collect() };
    assert_eq!(
        texts("host"),
        [
            "Choose…",
            "FAMAT",
            "MAO",
            "Lawton Chiles",
            "Plant City",
            "Unknown",
            "+ New value…"
        ]
    );
    assert_eq!(
        texts("month"),
        ["Choose…", "February", "Unknown", "Not applicable", "+ New value…"]
    );
    // Every year in use, Hidden Tests' too, newest first.
    assert_eq!(texts("year"), ["Choose…", "2004", "1999", "Unknown", "+ New value…"]);
    for name in ["year", "month", "competition", "division_level", "test_type", "host"] {
        assert_eq!(
            page.select(&format!("form#add-test input[name={name}_new]")).len(),
            1,
            "{name}"
        );
    }
}

#[tokio::test]
async fn uploads_need_an_admin_session_and_a_valid_csrf_token() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;

    let anonymous = app
        .http
        .post(app.url("/api/admin/tests"))
        .header("origin", &app.base_url)
        .multipart(reqwest::multipart::Form::new().text("csrf", "x"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    // A token from another session is refused, before any file is stored.
    let other = session_cookie(&app.submit_login(crate::support::admin::PASSCODE, &[]).await).unwrap();
    let foreign = app
        .add_test_form(&other)
        .await
        .test(&NewTest::known())
        .file("T_x.pdf", &sample_pdf(2000));
    let response = app.submit_add_test(&cookie, foreign, "publish").await;
    assert_eq!(response.status(), 403);

    let response = app
        .http
        .post(app.url("/api/admin/tests"))
        .header("origin", "https://evil.example")
        .header("cookie", &cookie)
        .multipart(reqwest::multipart::Form::new().text("csrf", "x"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403, "another site's Origin");

    assert!(app.stored_keys().await.is_empty());
    assert!(app.change_log().await.is_empty());
    // The real thing works.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_x.pdf", &sample_pdf(2000));
    app.add_test(&cookie, form, "publish").await;
}

// ---------------------------------------------------------- adding a Test

#[tokio::test]
async fn adding_a_test_with_several_files_creates_it_with_guessed_kinds_and_the_files_download_intact() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    // Over one 8 MiB part, so it is stored as a multipart upload.
    let test_pdf = sample_pdf(9 * MIB as usize + 12_345);
    let solutions_pdf = sample_pdf(40_000);
    let answers_docx = sample_docx(25_000);
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_Calculus Individual.pdf", &test_pdf)
        .file("S_Calculus Individual.pdf", &solutions_pdf)
        .file("Calculus Individual answers.docx", &answers_docx);

    let response = app.submit_add_test(&cookie, form, "publish").await;

    assert_eq!(response.status(), 303);
    let test = added_test(response.headers()["location"].to_str().unwrap());
    let saved = app.saved_documents(&cookie, test).await;
    let summary: Vec<(&str, &str, &str, &str)> = saved
        .iter()
        .map(|d| {
            (
                d.kind.as_str(),
                d.pdf_status.as_str(),
                d.visibility.as_str(),
                d.label_source.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Test", "PDF ready", "Published", "labels by an Admin"),
            ("Solutions", "PDF ready", "Published", "labels by an Admin"),
            ("Answers", "PDF being prepared", "Published", "labels by an Admin"),
        ]
    );
    let page = app
        .get_page_as(&format!("/admin/tests/new?added={test}"), Some(&cookie))
        .await;
    assert_eq!(
        page.text("#added .test"),
        "1993 February Invitational · Plant City · Calculus/Mu Individual · Published"
    );

    // On the public page, in kind order, each downloading what was uploaded.
    let [t, s, a] = [saved[0].id, saved[1].id, saved[2].id];
    let links = row_links(&app, test).await;
    assert_eq!(
        links,
        [
            ("Test", format!("/tests/{test}/documents/{t}/pdf")),
            ("Answers", format!("/tests/{test}/documents/{a}/pdf")),
            (".docx", format!("/tests/{test}/documents/{a}/original")),
            ("Solutions", format!("/tests/{test}/documents/{s}/pdf")),
        ]
        .map(|(text, href)| (text.to_owned(), href))
    );
    assert!(
        download(&app, &links[0].1).await == test_pdf,
        "the multipart-uploaded PDF"
    );
    assert!(download(&app, &links[3].1).await == solutions_pdf);
    let original = app.get(&links[2].1).await;
    assert_eq!(original.status(), 200);
    assert_eq!(original.headers()["content-type"], DOCX);
    assert_eq!(
        disposition(&original).name,
        format!("T{test} D{a} 1993 February Invitational - Plant City - Calculus-Mu Individual - Answers.docx")
    );
    assert!(original.bytes().await.unwrap() == answers_docx);
    // Its PDF waits for the conversion queue.
    let preparing = app.get_page(&links[1].1).await;
    assert!(preparing.text("#pdf-status").contains("being prepared"));
}

#[tokio::test]
async fn kinds_are_guessed_from_name_prefixes_and_keywords() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let cases = [
        // Prefixes.
        ("T_Geometry.pdf", "Test"),
        ("S_Geometry.pdf", "Solutions"),
        ("A_Geometry.pdf", "Answers"),
        ("C_Geometry.pdf", "Condensed Test"),
        ("E_Geometry.pdf", "Errata"),
        ("s_geometry answers.pdf", "Solutions"),
        // Keywords, as whole words in any case or style.
        ("Geometry Solutions.pdf", "Solutions"),
        ("geometry_sol.pdf", "Solutions"),
        ("GeometrySol2.pdf", "Solutions"),
        ("Geometry answers.pdf", "Answers"),
        ("Geometry-ANS.pdf", "Answers"),
        ("GeometryKey.pdf", "Answers"),
        ("Geometry Answer Key.pdf", "Answers"),
        ("Condensed Geometry.pdf", "Condensed Test"),
        ("Geometry errata.pdf", "Errata"),
        // Nothing to go on: a Test. Words that merely contain a keyword don't count.
        ("Geometry Individual.pdf", "Test"),
        ("Turkey Keystone Solo Answ.pdf", "Test"),
    ];
    let mut form = app.add_test_form(&cookie).await.test(&NewTest::known());
    for (i, (name, _)) in cases.iter().enumerate() {
        form = form.file(name, &sample_pdf(1000 + i));
    }

    let test = app.add_test(&cookie, form, "hidden").await;

    let kinds = app.saved_kinds(&cookie, test).await;
    let expected: Vec<&str> = cases.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(kinds, expected, "kinds in upload order");
}

#[tokio::test]
async fn a_guessed_kind_can_be_changed_and_each_file_given_a_label() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_Algebra.pdf", &sample_pdf(1000))
        .file("T_Algebra 2up.pdf", &sample_pdf(1100))
        .file("S_Algebra.pdf", &sample_pdf(1200))
        // What the hydrated page posts for the file list.
        .field("kind_0", "Combined Test+Solutions")
        .field("label_0", "")
        .field("kind_1", "Test")
        .field("label_1", "  2 per page ")
        .field("kind_2", "")
        .field("label_2", "Part 2");

    let test = app.add_test(&cookie, form, "publish").await;

    let saved: Vec<(String, Option<String>)> = app
        .saved_documents(&cookie, test)
        .await
        .into_iter()
        .map(|d| (d.kind, d.label))
        .collect();
    assert_eq!(
        saved,
        [
            ("Combined Test+Solutions", None),
            ("Test", Some("2 per page")),
            ("Solutions", Some("Part 2")),
        ]
        .map(|(kind, label)| (kind.to_owned(), label.map(str::to_owned)))
    );
    let link_texts: Vec<String> = row_links(&app, test).await.into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        link_texts,
        ["Test (2 per page)", "Solutions (Part 2)", "Combined Test+Solutions"]
    );

    // A kind that does not exist is refused, and nothing is kept.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            year: Some(2001),
            ..NewTest::known()
        })
        .file("T_x.pdf", &sample_pdf(1000))
        .field("kind_0", "Homework");
    let before = app.stored_keys().await;
    let response = app.submit_add_test(&cookie, form, "publish").await;
    assert_eq!(response.status(), 400);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("There is no Document kind “Homework”.")
    );
    assert_eq!(app.stored_keys().await, before);
}

#[tokio::test]
async fn a_word_file_and_its_pdf_with_the_same_name_become_one_document() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let (team_pdf, team_docx) = (sample_pdf(30_000), sample_docx(31_000));
    let (sol_docx, sol_pdf) = (sample_docx(32_000), sample_pdf(33_000));
    let notes_doc = sample_docx(5_000);
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        // The PDF first: the Word file still becomes the Original, and its
        // fields (index 1), not the PDF's, apply.
        .file("Geometry Team.pdf", &team_pdf)
        .file("Geometry Team.docx", &team_docx)
        .field("kind_0", "Errata")
        .field("label_1", "author's PDF")
        // The Word file first, and the names differing in case.
        .file("S_Geometry Team.docx", &sol_docx)
        .file("s_geometry team.PDF", &sol_pdf)
        // A Word file on its own waits for a conversion.
        .file("Geometry Notes.doc", &notes_doc);

    let test = app.add_test(&cookie, form, "publish").await;

    let saved = app.saved_documents(&cookie, test).await;
    let summary: Vec<(&str, Option<&str>, &str)> = saved
        .iter()
        .map(|d| (d.kind.as_str(), d.label.as_deref(), d.pdf_status.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("Test", Some("author's PDF"), "PDF supplied"),
            ("Solutions", None, "PDF supplied"),
            ("Test", None, "PDF being prepared"),
        ]
    );
    let [team, sol, notes] = [saved[0].id, saved[1].id, saved[2].id];
    for (document, pdf, docx) in [(team, &team_pdf, &team_docx), (sol, &sol_pdf, &sol_docx)] {
        let pdf_response = app.get(&format!("/tests/{test}/documents/{document}/pdf")).await;
        assert_eq!(pdf_response.status(), 200);
        assert_eq!(pdf_response.headers()["content-type"], "application/pdf");
        assert!(
            pdf_response.bytes().await.unwrap() == *pdf,
            "D{document}'s PDF is the supplied one"
        );
        let original = app.get(&format!("/tests/{test}/documents/{document}/original")).await;
        assert_eq!(original.headers()["content-type"], DOCX);
        assert!(disposition(&original).name.ends_with(".docx"));
        assert!(
            original.bytes().await.unwrap() == *docx,
            "D{document}'s Original is the Word file"
        );
    }
    assert!(download(&app, &format!("/tests/{test}/documents/{notes}/original")).await == notes_doc);
    // Stored under the Document IDs, with nothing left over.
    assert_eq!(
        app.stored_keys().await,
        [
            format!("documents/{notes}/original"),
            format!("documents/{sol}/original"),
            format!("documents/{sol}/pdf"),
            format!("documents/{team}/original"),
            format!("documents/{team}/pdf"),
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
}

// ----------------------------------------------------------------- limits

#[tokio::test]
async fn every_allowed_type_can_be_added_and_keeps_its_own_content_type() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let types = [
        ("pdf", "application/pdf"),
        ("doc", "application/msword"),
        ("docx", DOCX),
        ("ppt", "application/vnd.ms-powerpoint"),
        (
            "pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
        ("rtf", "application/rtf"),
        ("odt", "application/vnd.oasis.opendocument.text"),
        ("zip", "application/zip"),
    ];
    let mut form = app.add_test_form(&cookie).await.test(&NewTest::known());
    for (i, (extension, _)) in types.iter().enumerate() {
        // Upper case, as Windows names often are.
        form = form.file(
            &format!("T_File{i}.{}", extension.to_uppercase()),
            &sample_docx(2000 + i),
        );
    }

    let test = app.add_test(&cookie, form, "publish").await;

    let saved = app.saved_documents(&cookie, test).await;
    assert_eq!(saved.len(), types.len());
    for (document, (extension, content_type)) in saved.iter().zip(types) {
        let response = app
            .get(&format!("/tests/{test}/documents/{}/original", document.id))
            .await;
        assert_eq!(response.status(), 200, "{extension}");
        assert_eq!(response.headers()["content-type"], content_type, "{extension}");
        assert!(disposition(&response).name.ends_with(&format!(".{extension}")));
    }
    let statuses: Vec<&str> = saved.iter().map(|d| d.pdf_status.as_str()).collect();
    assert_eq!(statuses[0], "PDF ready", "a PDF is its own PDF");
    assert!(
        statuses[1..7].iter().all(|s| *s == "PDF being prepared"),
        "{statuses:?}"
    );
    assert_eq!(statuses[7], "no PDF (not convertible)", "a ZIP is not converted");
}

#[tokio::test]
async fn other_file_types_are_refused_with_a_message_naming_the_allowed_types() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;

    for name in ["setup.exe", "notes.txt", "README", "figure.png", "test.pdf.lnk"] {
        // A good file first, so something was already stored when the bad one arrives.
        let form = app
            .add_test_form(&cookie)
            .await
            .test(&NewTest::known())
            .file("T_Geometry.pdf", &sample_pdf(9 * MIB as usize))
            .file(name, b"not a test")
            .file("S_Geometry.pdf", &sample_pdf(2000));

        let response = app.submit_add_test(&cookie, form, "publish").await;

        assert_eq!(response.status(), 415, "{name}");
        let page = Page::parse(&response.text().await.unwrap());
        assert_eq!(page.text("#refusal"), format!("“{name}” can’t be added: {ALLOWED}."));
    }
    assert!(app.stored_keys().await.is_empty(), "nothing stored is left behind");
    assert!(app.change_log().await.is_empty(), "nothing was saved");
    assert_eq!(app.get_page("/").await.text("#count"), "0 tests");
}

#[tokio::test]
async fn a_file_over_100_mb_is_refused_and_one_of_exactly_100_mb_is_not() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;

    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("S_Calculus.pdf", &sample_pdf(2000))
        .generated_file("T_Calculus.pdf", 100 * MIB + 1);
    let response = app.submit_add_test(&cookie, form, "publish").await;

    assert_eq!(response.status(), 413);
    let page = Page::parse(&response.text().await.unwrap());
    assert_eq!(
        page.text("#refusal"),
        "“T_Calculus.pdf” can’t be added: files can be at most 100 MB."
    );
    assert!(app.stored_keys().await.is_empty(), "nothing stored is left behind");
    assert!(app.change_log().await.is_empty());

    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .generated_file("T_Calculus.pdf", 100 * MIB);
    let test = app.add_test(&cookie, form, "publish").await;
    let saved = app.saved_documents(&cookie, test).await;
    let response = app.get(&format!("/tests/{test}/documents/{}/pdf", saved[0].id)).await;
    assert_eq!(response.headers()["content-length"], (100 * MIB).to_string().as_str());
}

// ---------------------------------------------------------------- saving

#[tokio::test]
async fn save_hidden_keeps_the_test_off_the_public_page_and_publish_shows_it() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let hidden = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            year: Some(1994),
            ..NewTest::known()
        })
        .file("T_x.pdf", &sample_pdf(1000));
    let hidden = app.add_test(&cookie, hidden, "hidden").await;
    let published = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            year: Some(1995),
            ..NewTest::known()
        })
        .file("T_x.pdf", &sample_pdf(1000));
    let published = app.add_test(&cookie, published, "publish").await;

    let page = app.get_page("/").await;
    assert_eq!(ids(&page, "#results tbody tr"), [format!("T{published}")]);
    let added = app
        .get_page_as(&format!("/admin/tests/new?added={hidden}"), Some(&cookie))
        .await;
    assert!(added.text("#added .test").ends_with("· Hidden"));
    assert!(added.select("#notice").is_empty());
}

#[tokio::test]
async fn publishing_a_test_with_neither_year_nor_competition_known_saves_it_hidden() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let unknown = NewTest {
        year: None,
        month: Month::Unknown,
        competition: None,
        ..NewTest::known()
    };
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&unknown)
        .file("T_x.pdf", &sample_pdf(1000));

    let response = app.submit_add_test(&cookie, form, "publish").await;

    assert_eq!(response.status(), 303);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.ends_with("&notice=hidden"), "{location}");
    let page = app.get_page_as(&location, Some(&cookie)).await;
    assert_eq!(
        page.text("#notice"),
        "Saved hidden: a Test needs its Year or Competition known to be published."
    );
    assert_eq!(
        page.text("#added .test"),
        "— — — · Plant City · Calculus/Mu Individual · Hidden"
    );
    assert_eq!(app.get_page("/").await.text("#count"), "0 tests");
}

#[tokio::test]
async fn the_dropdowns_must_each_be_chosen_and_a_new_value_typed() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    type Fill = fn(AddTestForm) -> AddTestForm;
    let cases: [(&str, Fill); 4] = [
        ("Choose the Host, or Unknown.", |f| {
            f.test(&NewTest::known()).unchoose("host")
        }),
        ("Type the new Host.", |f| {
            f.test(&NewTest::known()).new_value("host", "  ")
        }),
        ("The Year must be a number from 1900 to 2100.", |f| {
            f.test(&NewTest::known()).new_value("year", "93")
        }),
        ("Choose at least one file to add.", |f| f.test(&NewTest::known())),
    ];
    for (message, fill) in cases {
        let mut form = fill(app.add_test_form(&cookie).await);
        if message != "Choose at least one file to add." {
            form = form.file("T_x.pdf", &sample_pdf(1000));
        }
        let response = app.submit_add_test(&cookie, form, "publish").await;
        assert_eq!(response.status(), 400, "{message}");
        let page = Page::parse(&response.text().await.unwrap());
        assert_eq!(page.text("#refusal"), message);
    }
    assert!(app.stored_keys().await.is_empty());
    assert!(app.change_log().await.is_empty());
}

#[tokio::test]
async fn new_value_creates_the_facet_value_at_the_end_of_its_order() {
    let app = TestApp::start().await;
    app.insert_test(&NewTest::known()).await;
    let cookie = app.admin().await;
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .new_value("year", "2023")
        .new_value("host", "Lawton Chiles")
        // Typing an existing value, in another case, picks it rather than
        // making a second one.
        .new_value("competition", "invitational")
        .file("T_x.pdf", &sample_pdf(1000));

    let test = app.add_test(&cookie, form, "publish").await;

    let page = app
        .get_page_as(&format!("/admin/tests/new?added={test}"), Some(&cookie))
        .await;
    assert_eq!(
        page.text("#added .test"),
        "2023 February Invitational · Lawton Chiles · Calculus/Mu Individual · Published"
    );
    let form = app.add_test_form(&cookie).await;
    let hosts: Vec<&str> = form.options("host").iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        hosts,
        ["Choose…", "Plant City", "Lawton Chiles", "Unknown", "+ New value…"]
    );
    let competitions: Vec<&str> = form.options("competition").iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(competitions, ["Choose…", "Invitational", "Unknown", "+ New value…"]);
    let years: Vec<&str> = form.options("year").iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(years, ["Choose…", "2023", "1993", "Unknown", "+ New value…"]);
    // And the public search offers it too.
    let search = app.get_page("/").await;
    assert!(
        search
            .select("#f-host option")
            .into_iter()
            .map(text)
            .any(|t| t == "Lawton Chiles")
    );
}

// ------------------------------------------------------ duplicate guard

#[tokio::test]
async fn a_duplicate_test_is_flagged_and_its_files_can_be_added_to_that_test_instead() {
    let app = TestApp::start().await;
    let existing = app.insert_test(&NewTest::known()).await;
    let test_pdf = sample_pdf(5000);
    let first = app
        .insert_document(&[existing], NewDocument::pdf("Test", &test_pdf))
        .await;
    let cookie = app.admin().await;
    let solutions = sample_pdf(6000);
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("S_Calculus.pdf", &solutions)
        .field("label_0", "Key only");

    let response = app.submit_add_test(&cookie, form, "publish").await;

    assert_eq!(response.status(), 303);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.starts_with("/admin/tests/new?pending="), "{location}");
    let warning = app.get_page_as(&location, Some(&cookie)).await;
    assert!(
        warning.select("form#add-test").is_empty(),
        "the warning replaces the form"
    );
    assert_eq!(
        warning.text("#duplicate-intro"),
        "A Test already has the Facet values 1993 February Invitational · Plant City · Calculus/Mu Individual:"
    );
    assert_eq!(ids(&warning, "#duplicate .match"), [format!("match-T{existing}")]);
    assert_eq!(
        warning.text("#duplicate .held"),
        "S_Calculus.pdf → Solutions (Key only)"
    );
    let add = warning.one("#finish button[value^='add:']");
    assert_eq!(text(add), "Add these files to that Test instead");
    // Nothing is saved until the Admin chooses.
    assert_eq!(row_links(&app, existing).await.len(), 1);

    let csrf = app.csrf_on(&location, Some(&cookie)).await;
    let pending = location.rsplit('=').next().unwrap();
    let action = add.value().attr("value").unwrap();
    let response = app
        .post_form(
            "/api/admin/uploads/finish",
            &[("csrf", &csrf), ("pending", pending), ("action", action)],
            &[("cookie", &cookie)],
        )
        .await;

    assert_eq!(response.status(), 303);
    assert_eq!(added_test(response.headers()["location"].to_str().unwrap()), existing);
    let page = app.get_page("/").await;
    assert_eq!(page.text("#count"), "1 test", "no second Test");
    let links = row_links(&app, existing).await;
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].1, format!("/tests/{existing}/documents/{first}/pdf"));
    assert_eq!(links[1].0, "Solutions (Key only)");
    assert!(download(&app, &links[1].1).await == solutions);

    // A second click (or the back button) does nothing more.
    let again = app
        .post_form(
            "/api/admin/uploads/finish",
            &[("csrf", &csrf), ("pending", pending), ("action", action)],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(again.status(), 409);
    assert_eq!(row_links(&app, existing).await.len(), 2);
}

#[tokio::test]
async fn a_duplicate_can_still_be_saved_as_a_new_test_or_discarded() {
    let app = TestApp::start().await;
    let existing = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    let cookie = app.admin().await;
    let finish = |location: String, action: &'static str| {
        let (app, cookie) = (&app, &cookie);
        async move {
            let csrf = app.csrf_on(&location, Some(cookie)).await;
            let pending = location.rsplit('=').next().unwrap().to_owned();
            app.post_form(
                "/api/admin/uploads/finish",
                &[("csrf", &csrf), ("pending", &pending), ("action", action)],
                &[("cookie", cookie)],
            )
            .await
        }
    };
    let upload = || async {
        let form = app
            .add_test_form(&cookie)
            .await
            .test(&NewTest::known())
            .file("T_x.pdf", &sample_pdf(4000));
        let response = app.submit_add_test(&cookie, form, "publish").await;
        assert_eq!(response.status(), 303);
        response.headers()["location"].to_str().unwrap().to_owned()
    };

    // A Hidden Test counts as a duplicate too.
    let location = upload().await;
    assert!(location.contains("pending="), "{location}");
    let response = finish(location, "discard").await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/admin/tests/new");
    assert!(app.stored_keys().await.is_empty(), "the held files are deleted");

    let location = upload().await;
    let response = finish(location, "new").await;
    assert_eq!(response.status(), 303);
    let new = added_test(response.headers()["location"].to_str().unwrap());
    assert_ne!(new, existing);
    assert_eq!(ids(&app.get_page("/").await, "#results tbody tr"), [format!("T{new}")]);

    // A Test in the Trash is no duplicate.
    app.trash_test(existing).await;
    app.trash_test(new).await;
    let location = upload().await;
    assert!(location.contains("added="), "{location}");
}

// ------------------------------------------------------------ change log

#[tokio::test]
async fn adding_a_test_records_every_write_in_the_change_log() {
    let app = TestApp::start().await;
    let cookie = app.admin().await;
    let (docx, pdf) = (sample_docx(3000), sample_pdf(2000));
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            month: Month::NotApplicable,
            competition: Some("State"),
            ..NewTest::known()
        })
        .file("S_State.docx", &docx)
        .file("S_State.pdf", &pdf)
        .field("label_0", "Part 2");

    let test = app.add_test(&cookie, form, "publish").await;

    let document = app.saved_documents(&cookie, test).await[0].id;
    let log = app.change_log().await;
    let entries_of = |entity: &str| -> Vec<Entry> {
        log.iter()
            .filter(|(e, _, _, _, _)| e == entity)
            .map(|(_, _, field, before, after)| (field.clone(), before.clone(), after.clone()))
            .collect()
    };
    let created = |pairs: &[(&str, &str)]| -> Vec<Entry> {
        pairs
            .iter()
            .map(|(field, after)| (field.to_string(), None, Some(after.to_string())))
            .collect()
    };

    // Every Facet value was new, so each was created and logged.
    let values: Vec<(i64, Vec<Entry>)> = {
        let mut ids: Vec<i64> = log
            .iter()
            .filter(|(e, _, _, _, _)| e == "facet_value")
            .map(|(_, id, _, _, _)| *id)
            .collect();
        ids.dedup();
        ids.into_iter()
            .map(|id| {
                let fields = log
                    .iter()
                    .filter(|(e, i, _, _, _)| e == "facet_value" && *i == id)
                    .map(|(_, _, field, before, after)| (field.clone(), before.clone(), after.clone()))
                    .collect();
                (id, fields)
            })
            .collect()
    };
    let names: Vec<(String, String)> = values
        .iter()
        .map(|(id, fields)| {
            assert_eq!(fields[0], ("id".to_owned(), None, Some(id.to_string())));
            (fields[1].2.clone().unwrap(), fields[2].2.clone().unwrap())
        })
        .collect();
    assert_eq!(
        names,
        [
            ("competition", "State"),
            ("division_level", "Calculus/Mu"),
            ("test_type", "Individual"),
            ("host", "Plant City"),
        ]
        .map(|(f, n)| (f.to_owned(), n.to_owned()))
    );

    assert_eq!(
        entries_of("test"),
        created(&[
            ("id", &format!("T{test}")),
            ("year", "1993"),
            ("month", "Not applicable"),
            ("competition", "State"),
            ("division_level", "Calculus/Mu"),
            ("test_type", "Individual"),
            ("host", "Plant City"),
            ("published", "true"),
        ])
    );
    let hash = |bytes: &[u8]| {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        entries_of("document"),
        created(&[
            ("id", &format!("D{document}")),
            ("tests", &format!("T{test}")),
            ("kind", "Solutions"),
            ("label", "Part 2"),
            ("published", "true"),
            ("label_source", "admin"),
            ("pdf_status", "ready"),
            (
                "original",
                &format!("S_State.docx (3000 bytes, sha256 {})", hash(&docx))
            ),
            ("pdf", &format!("S_State.pdf (2000 bytes, sha256 {})", hash(&pdf))),
        ])
    );
    assert!(
        log.iter()
            .all(|(e, ..)| ["test", "document", "facet_value"].contains(&e.as_str()))
    );

    // Adding files to an existing Test logs the new Documents, attached to
    // it, and leaves the Test's own entries alone.
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest {
            month: Month::NotApplicable,
            competition: Some("State"),
            ..NewTest::known()
        })
        .file("E_State.pdf", &sample_pdf(1000));
    let response = app.submit_add_test(&cookie, form, "hidden").await;
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let csrf = app.csrf_on(&location, Some(&cookie)).await;
    let pending = location.rsplit('=').next().unwrap();
    let response = app
        .post_form(
            "/api/admin/uploads/finish",
            &[
                ("csrf", &csrf),
                ("pending", pending),
                ("action", &format!("add:{test}")),
            ],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(response.status(), 303);
    let errata = app.saved_documents(&cookie, test).await[1].clone();
    assert_eq!(
        errata,
        SavedDocument {
            id: errata.id,
            kind: "Errata".to_owned(),
            label: None,
            pdf_status: "PDF ready".to_owned(),
            visibility: "Hidden".to_owned(),
            label_source: "labels by an Admin".to_owned(),
        },
        "Save hidden hides the added files"
    );
    let added: Vec<(String, Option<String>)> = app.change_log().await[log.len()..]
        .iter()
        .map(|(entity, id, field, before, after)| {
            assert_eq!((entity.as_str(), *id, before), ("document", errata.id, &None));
            (field.clone(), after.clone())
        })
        .collect();
    assert_eq!(
        &added[..5],
        [
            ("id", format!("D{}", errata.id)),
            ("tests", format!("T{test}")),
            ("kind", "Errata".to_owned()),
            ("published", "false".to_owned()),
            ("label_source", "admin".to_owned()),
        ]
        .map(|(f, a)| (f.to_owned(), Some(a)))
    );
}
