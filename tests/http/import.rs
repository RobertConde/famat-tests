//! `famat import`: the real command, run on the committed fixture set
//! (`tests/fixtures/import/`), then checked through what visitors and
//! Admins see over HTTP.
//!
//! The fixture set is a synthetic students' sheet (with its directions row,
//! dropdown lists and junk rows), AI labels that override some of its rows
//! and add previously uncatalogued files and a Master Key, a docs export of
//! tiny made-up files named like the real one, and a legacy SQL dump. For
//! the special cases (#31, mostly checked in `import_special.rs`) it also
//! has a bundle zip unpacked into `unpacked/` with its `unpacked.csv`,
//! attach decisions, and the AI's review flags.

use std::collections::BTreeMap;

use crate::support::{
    TestApp,
    download::disposition,
    fixtures::{NewDocument, NewTest, sample_pdf},
    html::{Page, links_in, text, text_in},
    import::fixture,
};

/// The Published Tests, in search order: Year newest first, then Month in
/// season order (month-less last), Competition, Host, Division/Level and
/// Test type, each in its seeded display order; unknown Year last.
pub const PUBLISHED: [i64; 32] = [
    3600, // 2021 January Regional · FAMAT · Geometry Individual
    3650, // 2019 State · FAMAT · Calculus/Mu Team
    3714, // 2013 State · FAMAT · Geometry Hustle (unpacked from the bundle 8053)
    3713, // 2013 State · FAMAT · Algebra II Hustle (unpacked from the bundle 8053)
    3000, // 2012 February Regional · FAMAT · Algebra II Team, as the old T3000 was
    3711, // 2012 March Regional · FAMAT · Algebra II Team (new: the AI moved 7631 here)
    3712, // 2011 November Invitational · Hagerty · Statistics Individual (AI only)
    3700, // 2010 Mail-in · FAMAT · Theta Individual
    2100, // 2008 State · FAMAT · Calculus/Mu Calculus Applications
    2000, // 2008 National · MAO · Calculus/Mu Mu Applications
    2500, // 2007 State · Washington Mu Alpha Theta · Open Relays
    2400, // 2005 State · FAMAT · Precalculus/Alpha Applications
    2600, // 2005 State · FAMAT · Calculus/Mu BC Calculus (a Word+PDF pair)
    1950, // 2004 National · MAO · Theta Sequences and Series
    3100, // 2001 State · FAMAT · Open Analytic Geometry (and a Hidden duplicate)
    2800, // 2001 State · FAMAT · Open Gemini (with D3415, attached)
    3707, // 2000 January Invitational · Berkeley Prep · Algebra II Individual (the AI said 2000)
    2200, // 2000 State · FAMAT · Calculus/Mu Calculus Applications
    3710, // 2000 State · FAMAT · Calculus/Mu Mu Applications (split from old T2200)
    1,    // 1999 January Invitational · Berkeley Prep · Algebra I Individual
    2,    // 1999 January Invitational · Berkeley Prep · Algebra I Team
    3,    // 1999 January Invitational · Berkeley Prep · Algebra II Individual
    2900, // 1998 State · FAMAT · Calculus/Mu Mu Applications
    3709, // 1998 State · FAMAT · Calculus/Mu Mu Applications: the same labels, but a new test
    2302, // 1997 State · FAMAT · Calculus/Mu Mu Applications (from the AI evidence)
    3500, // 1997 National · MAO · Open State Bowl Nationals
    2301, // 1996 State · FAMAT · Calculus/Mu Mu Applications (from the file contents)
    2300, // 1995 State · FAMAT · Calculus/Mu Calculus Applications (undecided)
    1501, // 1989 National · MAO · Precalculus/Alpha Individual
    3708, // 1989 National · MAO · Calculus/Mu Individual (Master Key D1162 only)
    1500, // 1989 National · MAO · Theta Individual
    3160, // State · FAMAT · Open Computer Programming (a zip)
];

/// Every imported Document: its Tests, and its file under
/// `tests/fixtures/import/`.
pub const DOCUMENTS: [(i64, &[i64], &str); 46] = [
    (766, &[1], "docs/766 1999 Test.pdf"),
    (767, &[1], "docs/767 1999 Solutions.pdf"),
    (768, &[2], "docs/768 1999 Test.pdf"),
    (769, &[2], "docs/769 1999 Solutions.pdf"),
    (770, &[3707], "docs/770 2000 Solutions.pdf"),
    (771, &[3], "docs/771 1999 Test.pdf"),
    (1160, &[1500], "docs/1160 1989 Test.pdf"),
    (1161, &[1501], "docs/1161 1989 Test.pdf"),
    (1162, &[1500, 1501, 3708], "docs/1162 1989 Answers.pdf"),
    (1176, &[1500], "docs/1176 1989 Answers.pdf"),
    (1600, &[3500], "docs/1600 1997 Test.pdf"),
    (2118, &[3706], "docs/2118 2002 Test2.pdf"),
    (3414, &[2800], "docs/3414 2001 Solutions.pdf"),
    (3415, &[2800], "docs/3415 2001 Test.pdf"),
    (3455, &[3100], "docs/3455 2001 Test.pdf"),
    (3907, &[2300], "docs/3907 1995 Solutions.pdf"),
    (3909, &[2301], "docs/3909 1996 Solutions.pdf"),
    (3910, &[2301], "docs/3910 1996 Test.doc"),
    (3911, &[2302], "docs/3911 1997 Solutions.pdf"),
    (3915, &[3709], "docs/3915 1998 Solutions.pdf"),
    (3916, &[3709], "docs/3916 1998 Test.pdf"),
    (4811, &[2200], "docs/4811 T_State 2000 - Calculus Applications.pdf"),
    (4833, &[3710], "docs/4833 T_State 2000 - Theta Mu Applications.pdf"),
    (4925, &[2100], "docs/4925 T_Calc Apps.pdf"),
    (5147, &[2000], "docs/5147 A_Mu_Apps_answers.pdf"),
    (5149, &[2000], "docs/5149 T_Mu_Applications_2008_nationals[1].pdf"),
    (5585, &[1950], "docs/5585 Sequences and Series"),
    (5617, &[1888], "docs/5617 Calculus Applications ANSWERS.doc"),
    (5642, &[2600], "docs/5642 BC Calculus SOLUTIONS.doc"),
    (5700, &[2400], "docs/5700 Alpha Applications.pdf"),
    (5937, &[3100], "docs/5937 State '01 - Analytic Geometry.pdf"),
    (6051, &[2500], "docs/6051 Relays Answers.pdf"),
    (6052, &[2500], "docs/6052 Relays.pdf"),
    (6293, &[3160], "docs/6293 S_Solutions.zip"),
    (6510, &[2900], "docs/6510 S_98_Mu_Apps_Sol.pdf"),
    (6606, &[3150], "docs/6606 T_State calculus region bowl Blank Form.pdf"),
    (7631, &[3711], "docs/7631 T_Alg 2 Team.pdf"),
    (9000, &[3000], "docs/9000 T_Algebra II Team.pdf"),
    (9800, &[3650], "docs/9800 T_Calc Team.docx"),
    (9801, &[3650], "docs/9801 A_Calc Team Answers.PDF"),
    (9900, &[3700], "docs/9900 T_Mail-in Theta.pdf"),
    (10005, &[3600], "docs/10005 T_ Geom Indiv JAN 2021.pdf"),
    (10300, &[3712], "docs/10300 T_Hagerty Fall Stats Individual.pdf"),
    (
        10364,
        &[3713, 3714],
        "unpacked/8053/Hustle/2013 State Hustle Answers.doc",
    ),
    (
        10365,
        &[3713],
        "unpacked/8053/Hustle/Algebra/Algebra 2 Hustle MAO States 2013.docx",
    ),
    (10372, &[3714], "unpacked/8053/Hustle/Geo/Hustle Geometry 2013.doc"),
];

/// The Documents imported Hidden: two duplicates, a broken file and one
/// that is not a test.
pub const HIDDEN_DOCUMENTS: [i64; 4] = [1176, 3455, 5617, 6606];

/// The Documents whose PDF came with them, and that PDF's file.
pub const SUPPLIED_PDFS: [(i64, &str); 2] = [
    (5642, "docs/3623 2005 Solutions.pdf"),
    (
        10365,
        "unpacked/8053/Hustle/Algebra/Algebra 2 Hustle MAO States 2013.pdf",
    ),
];

/// Tests the publish rule keeps Hidden: T1888 has no Published Document of
/// a known kind, nor a Year or Competition; T3706 has neither a Year nor a
/// Competition; T3150's only Document is Hidden (not a test).
pub const HIDDEN: [i64; 3] = [1888, 3150, 3706];

/// The imported Documents a visitor can download, from each of their
/// Published Tests.
pub fn downloadable() -> impl Iterator<Item = (i64, i64, &'static str)> {
    DOCUMENTS
        .into_iter()
        .filter(|(id, _, _)| !HIDDEN_DOCUMENTS.contains(id))
        .flat_map(|(id, tests, file)| {
            tests
                .iter()
                .filter(|t| !HIDDEN.contains(t))
                .map(move |&test| (test, id, file))
        })
}

// ------------------------------------------------------------------ helpers

/// The Test IDs in the results table, top to bottom.
pub fn result_ids(page: &Page) -> Vec<i64> {
    page.select("#results tbody tr")
        .iter()
        .map(|row| row.value().attr("id").unwrap().trim_start_matches('T').parse().unwrap())
        .collect()
}

/// The texts of a dropdown's options.
fn options(page: &Page, name: &str) -> Vec<String> {
    page.select(&format!("#filters select[name={name:?}] option"))
        .into_iter()
        .map(text)
        .collect()
}

/// A result row's `(year, event, host, division/level, test type)`.
pub fn row(page: &Page, test: i64) -> [String; 5] {
    let row = page.one(&format!("#results tbody tr#T{test}"));
    [".year", ".event", ".host", ".division-level", ".test-type"].map(|css| text_in(row, css))
}

impl TestApp {
    /// The Test IDs a search lists.
    pub async fn search_ids(&self, path: &str) -> Vec<i64> {
        result_ids(&self.get_page(path).await)
    }

    /// A Document's Original as downloaded from a Test: its name and bytes.
    pub async fn download_original(&self, test: i64, document: i64) -> (String, Vec<u8>) {
        let response = self.get(&format!("/tests/{test}/documents/{document}/original")).await;
        assert_eq!(response.status(), 200, "the Original of D{document} on T{test}");
        let name = disposition(&response).name;
        (name, response.bytes().await.unwrap().to_vec())
    }

    /// What a visitor can see of the archive: the search form and every
    /// result, and each Published Document's downloads; plus every object in
    /// Garage. Two equal snapshots are the same archive.
    async fn snapshot(&self) -> Snapshot {
        let page = self.get_page("/").await;
        let mut downloads = BTreeMap::new();
        for (test, document, _) in downloadable() {
            downloads.insert((test, document), self.download_original(test, document).await);
        }
        let mut objects = BTreeMap::new();
        for key in self.stored_keys().await {
            let bytes = self.stored_object(&format!("{}{key}", self.key_prefix)).await;
            objects.insert(key, bytes);
        }
        Snapshot {
            filters: page.one("#filters").html(),
            results: page.one("#results").html(),
            downloads,
            objects,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    filters: String,
    results: String,
    downloads: BTreeMap<(i64, i64), (String, Vec<u8>)>,
    objects: BTreeMap<String, Vec<u8>>,
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn the_imported_tests_are_listed_in_search_order() {
    let app = TestApp::start().await;
    app.import_fixtures();

    let page = app.get_page("/").await;

    assert_eq!(page.text("#count"), "32 tests");
    assert_eq!(result_ids(&page), PUBLISHED);
    for hidden in HIDDEN {
        assert!(page.select(&format!("tr#T{hidden}")).is_empty(), "T{hidden} is Hidden");
    }

    // Tidied values: singular events, no parentheses, Month only where it
    // applies, Washington as a State event with its own Host.
    let expect = |values: [&str; 5]| values.map(str::to_owned);
    assert_eq!(
        row(&page, 1),
        expect([
            "1999",
            "January Invitational",
            "Berkeley Prep",
            "Algebra I",
            "Individual"
        ])
    );
    assert_eq!(
        row(&page, 3711),
        expect(["2012", "March Regional", "FAMAT", "Algebra II", "Team"])
    );
    assert_eq!(
        row(&page, 3650),
        expect(["2019", "State", "FAMAT", "Calculus/Mu", "Team"])
    );
    assert_eq!(
        row(&page, 1500),
        expect(["1989", "National", "MAO", "Theta", "Individual"])
    );
    assert_eq!(
        row(&page, 3700),
        expect(["2010", "Mail-in", "FAMAT", "Theta", "Individual"])
    );
    assert_eq!(
        row(&page, 3712),
        expect(["2011", "November Invitational", "Hagerty", "Statistics", "Individual"])
    );
    assert_eq!(
        row(&page, 2500),
        expect(["2007", "State", "Washington Mu Alpha Theta", "Open", "Relays"])
    );
    assert_eq!(
        row(&page, 3500),
        expect(["1997", "National", "MAO", "Open", "State Bowl Nationals"])
    );

    // Documents with identical labels form one Test, kinds from the AI where
    // it relabelled them (768 is the Test, not Solutions).
    let documents = |test: i64| links_in(page.one(&format!("tr#T{test} td.documents")));
    assert_eq!(
        documents(2),
        [
            ("Test".to_owned(), "/tests/2/documents/768/pdf".to_owned()),
            ("Solutions".to_owned(), "/tests/2/documents/769/pdf".to_owned()),
        ]
    );
    // A Word Original links its PDF (being prepared) and the .docx.
    assert_eq!(
        documents(3650),
        [
            ("Test".to_owned(), "/tests/3650/documents/9800/pdf".to_owned()),
            (".docx".to_owned(), "/tests/3650/documents/9800/original".to_owned()),
            ("Answers".to_owned(), "/tests/3650/documents/9801/pdf".to_owned()),
        ]
    );
    // The Master Key is on every Test it covers.
    for test in [1500, 1501, 3708] {
        assert!(
            documents(test).contains(&("Answers".to_owned(), format!("/tests/{test}/documents/1162/pdf"))),
            "D1162 on T{test}"
        );
    }
}

#[tokio::test]
async fn the_dropdowns_offer_the_imported_values_in_their_seeded_order() {
    let app = TestApp::start().await;
    app.import_fixtures();

    let page = app.get_page("/").await;

    assert_eq!(
        options(&page, "year"),
        [
            "Any", "2021", "2019", "2013", "2012", "2011", "2010", "2008", "2007", "2005", "2004", "2001", "2000",
            "1999", "1998", "1997", "1996", "1995", "1989", "Unknown"
        ]
    );
    assert_eq!(
        options(&page, "month"),
        ["Any", "November", "January", "February", "March", "Unknown"]
    );
    assert_eq!(
        options(&page, "competition"),
        [
            "Any",
            "Invitational",
            "Regional",
            "State",
            "National",
            "Mail-in",
            "Unknown"
        ]
    );
    assert_eq!(
        options(&page, "division_level"),
        [
            "Any",
            "Algebra I",
            "Geometry",
            "Algebra II",
            "Precalculus/Alpha",
            "Calculus/Mu",
            "Statistics",
            "Theta",
            "Open",
            "Unknown"
        ]
    );
    // Individual and Team first, then alphabetical; Integration and Region
    // Bowl are only on Hidden Tests but are values all the same.
    assert_eq!(
        options(&page, "test_type"),
        [
            "Any",
            "Individual",
            "Team",
            "Analytic Geometry",
            "Applications",
            "BC Calculus",
            "Calculus Applications",
            "Computer Programming",
            "Gemini",
            "Hustle",
            "Integration",
            "Mu Applications",
            "Region Bowl",
            "Relays",
            "Sequences and Series",
            "State Bowl Nationals",
            "Unknown"
        ]
    );
    assert_eq!(
        options(&page, "host"),
        [
            "Any",
            "FAMAT",
            "MAO",
            "Berkeley Prep",
            "Hagerty",
            "Washington Mu Alpha Theta",
            "Unknown"
        ]
    );
}

#[tokio::test]
async fn calculus_mu_applications_splits_into_calculus_and_mu_applications() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // "Mu App…" in the file name (5147, 5149, 4833, 6510), the AI evidence
    // (3911, and the new test 3915/3916) or the file itself (3910, with 3909
    // following it as its sibling).
    assert_eq!(
        app.search_ids("/?test_type=Mu+Applications").await,
        [2000, 3710, 2900, 3709, 2302, 2301]
    );
    // "Calc App…" in the file name (4925, 4811); 3907 says neither, so it
    // defaults to Calculus Applications.
    assert_eq!(
        app.search_ids("/?test_type=Calculus+Applications").await,
        [2100, 2200, 2300]
    );
    // Other Divisions keep plain Applications.
    assert_eq!(app.search_ids("/?test_type=Applications").await, [2400]);
    let page = app.get_page("/?test_type=Applications").await;
    assert_eq!(row(&page, 2400)[3], "Precalculus/Alpha");
}

#[tokio::test]
async fn tidied_values_are_what_searches_match() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // "States" and "Washington State" are both State, with no Month.
    assert_eq!(
        app.search_ids("/?competition=State").await,
        [
            3650, 3714, 3713, 2100, 2500, 2400, 2600, 3100, 2800, 2200, 3710, 2900, 3709, 2302, 2301, 2300, 3160
        ]
    );
    assert_eq!(
        app.search_ids("/?competition=National").await,
        [2000, 1950, 3500, 1501, 3708, 1500]
    );
    assert_eq!(app.search_ids("/?host=Washington+Mu+Alpha+Theta").await, [2500]);
    // Month is not applicable to State, National and Mail-in, so none of
    // them is a Test of unknown Month.
    assert!(app.search_ids("/?month=unknown").await.is_empty());
    assert_eq!(app.search_ids("/?month=November").await, [3712]);
}

#[tokio::test]
async fn downloads_carry_the_legacy_ids_and_the_source_bytes() {
    let app = TestApp::start().await;
    app.import_fixtures();

    for (test, document, file) in downloadable() {
        let (name, bytes) = app.download_original(test, document).await;
        assert!(
            name.starts_with(&format!("T{test} D{document} ")),
            "D{document} on T{test} is named {name:?}"
        );
        assert!(bytes == fixture(file), "D{document} downloads as {file:?}");
    }

    let app = &app;
    let name = |test, document| async move { app.download_original(test, document).await.0 };
    assert_eq!(
        name(1, 766).await,
        "T1 D766 1999 January Invitational - Berkeley Prep - Algebra I Individual - Test.pdf"
    );
    // A Word Original keeps its extension; ".PDF" is lower-cased.
    assert_eq!(
        name(3650, 9800).await,
        "T3650 D9800 2019 State - FAMAT - Calculus-Mu Team - Test.docx"
    );
    assert_eq!(
        name(3650, 9801).await,
        "T3650 D9801 2019 State - FAMAT - Calculus-Mu Team - Answers.pdf"
    );
    // A file with no extension gets one from its contents.
    assert_eq!(
        name(1950, 5585).await,
        "T1950 D5585 2004 National - MAO - Theta Sequences and Series - Test.pdf"
    );
    // A new Test numbered after the old maximum (3706), and the Master Key
    // named for each Test it is downloaded from.
    assert_eq!(
        name(3708, 1162).await,
        "T3708 D1162 1989 National - MAO - Calculus-Mu Individual - Answers.pdf"
    );
    assert_eq!(
        name(1501, 1162).await,
        "T1501 D1162 1989 National - MAO - Precalculus-Alpha Individual - Answers.pdf"
    );
}

#[tokio::test]
async fn every_garage_object_matches_its_source_file() {
    let app = TestApp::start().await;
    app.import_fixtures();

    let objects: Vec<(i64, &str, &str)> = DOCUMENTS
        .iter()
        .map(|&(id, _, file)| (id, "original", file))
        .chain(SUPPLIED_PDFS.iter().map(|&(id, file)| (id, "pdf", file)))
        .collect();
    let mut expected: Vec<String> = objects
        .iter()
        .map(|(id, role, _)| format!("documents/{id}/{role}"))
        .collect();
    expected.sort();
    // Nothing else: the shortcut, the stray .htaccess, the bundle zip, the
    // Word lock file in the bundle and the labelled row with no file (5618)
    // are not imported, and the PDF of a Word+PDF pair is no Document of its
    // own.
    assert_eq!(app.stored_keys().await, expected);
    for (document, role, file) in objects {
        let stored = app.stored_object(&app.document_key(document, role)).await;
        assert!(stored == fixture(file), "Garage's {role} of D{document} is {file:?}");
    }
}

#[tokio::test]
async fn admins_see_the_label_source_and_the_hidden_tests() {
    let app = TestApp::start().await;
    app.import_fixtures();
    let cookie = app.admin().await;

    // The Add test summary lists any Test with its Documents.
    let sources = |test| {
        let app = &app;
        let cookie = &cookie;
        async move {
            app.saved_documents(cookie, test)
                .await
                .into_iter()
                .map(|d| (d.id, d.label_source))
                .collect::<Vec<_>>()
        }
    };
    let (sheet, ai) = ("labels from the sheet".to_owned(), "labels from AI".to_owned());
    assert_eq!(sources(2).await, [(768, ai.clone()), (769, sheet.clone())]);
    assert_eq!(
        sources(1500).await,
        [(1160, sheet.clone()), (1162, ai.clone()), (1176, ai.clone())]
    );
    assert_eq!(sources(3712).await, [(10300, ai)]);

    for (test, description) in [
        (1888, "— — — · — · — —"),
        (3150, "2010 State · FAMAT · Calculus/Mu Region Bowl"),
        (3706, "— — — · MAO · Calculus/Mu Integration"),
    ] {
        let page = app
            .get_page_as(&format!("/admin/tests/new?added={test}"), Some(&cookie))
            .await;
        assert_eq!(page.text("#added .test .description"), description);
        assert_eq!(page.text("#added .test .visibility"), "Hidden");
    }
    // A Document of unknown kind is imported as Other.
    let unknown = app.saved_documents(&cookie, 1888).await;
    assert_eq!(unknown.len(), 1);
    assert_eq!((unknown[0].id, unknown[0].kind.as_str()), (5617, "Other"));
    assert_eq!(unknown[0].pdf_status, "PDF being prepared");
}

#[tokio::test]
async fn an_undecided_applications_document_needs_review() {
    let app = TestApp::start().await;
    app.import_fixtures();

    // There is no Needs review page yet (#25), so this reads the Documents
    // table, which is that page's contract.
    let comment: Option<String> =
        sqlx::query_scalar("SELECT needs_review_comment FROM documents WHERE id = 3907 AND needs_review")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(
        comment
            .as_deref()
            .unwrap_or_default()
            .contains("Calculus Applications or Mu Applications"),
        "{comment:?}"
    );
}

#[tokio::test]
async fn a_rerun_rebuilds_the_same_archive_and_removes_anything_else() {
    let app = TestApp::start().await;
    app.import_fixtures();
    let first = app.snapshot().await;

    // Something not from the import, written without the change log (an
    // Admin's write would freeze the import instead).
    let stray = app.insert_test(&NewTest::known()).await;
    app.insert_document(&[stray], NewDocument::pdf("Test", &sample_pdf(300)))
        .await;
    assert!(app.search_ids("/").await.contains(&stray));

    let summary = app.import_fixtures();

    assert!(summary.contains("removed 1 stale object."), "{summary}");
    assert!(!app.search_ids("/").await.contains(&stray));
    assert_eq!(app.snapshot().await, first);
}

#[tokio::test]
async fn after_an_admin_change_the_import_refuses_and_changes_nothing() {
    let app = TestApp::start().await;
    app.import_fixtures();
    let cookie = app.admin().await;
    let form = app
        .add_test_form(&cookie)
        .await
        .test(&NewTest::known())
        .file("T_Calculus.pdf", &sample_pdf(800));
    let added = app.add_test(&cookie, form, "publish").await;

    // New records are numbered after the imported ones (and the legacy ones).
    assert_eq!(added, 3715);
    let documents = app.saved_documents(&cookie, added).await;
    assert_eq!(documents.iter().map(|d| d.id).collect::<Vec<_>>(), [10395]);
    let before = app.snapshot().await;

    let run = app.run_import(&[]);

    assert!(!run.success, "the import ran:\n{}", run.stdout);
    assert!(
        run.stderr
            .contains("refusing to import: Admins have started editing the archive"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("Importing again would wipe their changes."),
        "{}",
        run.stderr
    );
    assert_eq!(app.snapshot().await, before);
    assert!(app.search_ids("/").await.contains(&added));
}

#[tokio::test]
async fn a_dry_run_reports_what_it_would_import_and_writes_nothing() {
    let app = TestApp::start().await;

    let run = app.run_import(&["--dry-run"]);

    assert!(run.success, "{}{}", run.stdout, run.stderr);
    for line in [
        "Read 40 sheet rows, 20 AI label rows (17 Documents), 46 files in the docs export and 28 old Tests.",
        "Read 3 unpacked Documents from 1 bundle zip (5 files in the unpacked directory), 5 attach decisions \
         and 3 review flags.",
        "Documents: 46 (30 labelled by the sheet, 16 by the AI); 1 of unknown kind, imported as Other.",
        "Tests: 35 (27 keep their old test_id, 8 new, T3707–T3714); 3 Hidden by the publish rule.",
        "  File Number 5618: labelled, but there is no such file in the docs export",
        "  1769 Open Tests.lnk: a Windows shortcut (.lnk), not a Document",
        "  Source files: 52 — 42 imported, 4 imported Hidden, 2 imported as supplied PDFs, 3 dropped, \
         1 failed, 0 not accounted for.",
        "  Garage: not checked (dry run); 48 objects would be stored (46 Originals, 2 supplied PDFs).",
        "  PDF conversions: 5 would be queued.",
        "Dry run: nothing was written to Postgres or Garage.",
    ] {
        assert!(
            run.stdout.lines().any(|l| l == line),
            "no line {line:?} in:\n{}",
            run.stdout
        );
    }
    assert_eq!(app.get_page("/").await.text("#count"), "0 tests");
    assert!(app.stored_keys().await.is_empty());
}
