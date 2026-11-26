//! Opening a Document's PDF and downloading its Original, streamed from
//! Garage, from the links on the search page.

use crate::support::{
    TestApp,
    download::disposition,
    fixtures::{NewDocument, NewTest, sample_docx, sample_pdf},
    html::{Page, links_in},
};

/// The `(text, href)` of every link in a Test's row on the search page.
async fn links_of_test(app: &TestApp, test: i64) -> Vec<(String, String)> {
    let page = app.get_page("/").await;
    links_in(page.one(&format!("tr#T{test} td.documents")))
}

#[tokio::test]
async fn a_documents_pdf_link_downloads_the_exact_bytes_stored_in_garage() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    // Several MiB, so the body arrives in many chunks.
    let pdf = sample_pdf(5 * 1024 * 1024 + 123);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;
    let stored = app.stored_object(&app.document_key(document, "original")).await;
    assert_eq!(stored, pdf, "fixture stored the bytes in Garage");

    // Follow the link the search page offers.
    let links = links_of_test(&app, test).await;
    assert_eq!(
        links,
        vec![("Test".to_owned(), format!("/tests/{test}/documents/{document}/pdf"))],
        "a PDF Original has no separate Original link"
    );
    let response = app.get(&links[0].1).await;

    assert_eq!(response.status(), 200);
    let headers = response.headers();
    assert_eq!(headers["content-type"], "application/pdf");
    assert_eq!(headers["content-length"], pdf.len().to_string().as_str());
    assert_eq!(disposition(&response).kind, "inline");
    let body = response.bytes().await.unwrap();
    assert!(
        body == stored,
        "downloaded {} bytes that differ from the {} stored",
        body.len(),
        stored.len()
    );
}

#[tokio::test]
async fn a_word_document_opens_its_pdf_and_offers_the_original_as_a_small_ext_link() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let docx = sample_docx(40_000);
    let pdf = sample_pdf(30_000);
    let document = app
        .insert_document(&[test], NewDocument::docx("Solutions", &docx, Some(&pdf)))
        .await;

    let links = links_of_test(&app, test).await;
    assert_eq!(
        links,
        vec![
            (
                "Solutions".to_owned(),
                format!("/tests/{test}/documents/{document}/pdf")
            ),
            (
                ".docx".to_owned(),
                format!("/tests/{test}/documents/{document}/original")
            ),
        ]
    );

    let pdf_response = app.get(&links[0].1).await;
    assert_eq!(pdf_response.status(), 200);
    assert_eq!(pdf_response.headers()["content-type"], "application/pdf");
    let pdf_disposition = disposition(&pdf_response);
    assert_eq!(pdf_disposition.kind, "inline");
    assert!(
        pdf_disposition.name.ends_with(" - Solutions.pdf"),
        "{pdf_disposition:?}"
    );
    assert_eq!(
        pdf_response.bytes().await.unwrap(),
        pdf,
        "the PDF made from the Original"
    );

    let original_response = app.get(&links[1].1).await;
    assert_eq!(original_response.status(), 200);
    assert_eq!(
        original_response.headers()["content-type"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    let original_disposition = disposition(&original_response);
    assert_eq!(original_disposition.kind, "attachment");
    assert_eq!(
        original_disposition.name.strip_suffix(".docx"),
        pdf_disposition.name.strip_suffix(".pdf"),
        "the PDF and its Original share the base name"
    );
    assert_eq!(
        original_response.bytes().await.unwrap(),
        docx,
        "the Original exactly as uploaded"
    );
}

#[tokio::test]
async fn a_pdf_original_also_downloads_as_an_attachment_from_its_original_link() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(10_000);
    let document = app.insert_document(&[test], NewDocument::pdf("Test", &pdf)).await;

    let response = app.get(&format!("/tests/{test}/documents/{document}/original")).await;

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/pdf");
    let disposition = disposition(&response);
    assert_eq!(disposition.kind, "attachment");
    assert!(disposition.name.ends_with(" - Test.pdf"), "{disposition:?}");
    assert_eq!(response.bytes().await.unwrap(), pdf);
}

#[tokio::test]
async fn while_the_pdf_is_being_prepared_the_pdf_link_says_so_and_offers_the_original() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let docx = sample_docx(20_000);
    let document = app
        .insert_document(&[test], NewDocument::docx("Test", &docx, None))
        .await;

    // The search page still offers the Original beside the PDF link.
    let links = links_of_test(&app, test).await;
    assert_eq!(
        links,
        vec![
            ("Test".to_owned(), format!("/tests/{test}/documents/{document}/pdf")),
            (
                ".docx".to_owned(),
                format!("/tests/{test}/documents/{document}/original")
            ),
        ]
    );

    let response = app.get(&links[0].1).await;
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html"),
        "a page, not a PDF"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let page = Page::parse(&response.text().await.unwrap());
    assert!(
        page.text("#pdf-status").contains("being prepared"),
        "{}",
        page.text("#pdf-status")
    );
    let original = page.one("a#original");
    assert_eq!(
        original.value().attr("href"),
        Some(format!("/tests/{test}/documents/{document}/original").as_str())
    );
    assert_eq!(
        page.text("a#original"),
        format!("T{test} D{document} 1993 February Invitational - Plant City - Calculus-Mu Individual - Test.docx")
    );

    let download = app.get(original.value().attr("href").unwrap()).await;
    assert_eq!(download.status(), 200);
    assert_eq!(download.bytes().await.unwrap(), docx);
}

#[tokio::test]
async fn a_document_whose_object_is_missing_from_garage_is_a_404() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let pdf_original = app
        .insert_document(
            &[test],
            NewDocument {
                stored: false,
                ..NewDocument::pdf("Test", &pdf)
            },
        )
        .await;
    let word_original = app
        .insert_document(
            &[test],
            NewDocument {
                stored: false,
                ..NewDocument::docx("Solutions", &docx, Some(&pdf))
            },
        )
        .await;

    for path in [
        format!("/tests/{test}/documents/{pdf_original}/pdf"),
        format!("/tests/{test}/documents/{pdf_original}/original"),
        format!("/tests/{test}/documents/{word_original}/pdf"),
        format!("/tests/{test}/documents/{word_original}/original"),
    ] {
        let response = app.get(&path).await;
        assert_eq!(response.status(), 404, "GET {path}");
        assert!(response.headers().get("content-disposition").is_none(), "GET {path}");
    }
}

#[tokio::test]
async fn hidden_or_trashed_documents_and_documents_of_hidden_or_trashed_tests_are_a_404() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let published = app.insert_test(&NewTest::known()).await;
    let hidden_document = app
        .insert_document(
            &[published],
            NewDocument {
                published: false,
                ..NewDocument::pdf("Test", &pdf)
            },
        )
        .await;
    let trashed_document = app.insert_document(&[published], NewDocument::pdf("Test", &pdf)).await;
    app.trash_document(trashed_document).await;

    let hidden_test = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    let on_hidden_test = app
        .insert_document(&[hidden_test], NewDocument::pdf("Test", &pdf))
        .await;
    let trashed_test = app.insert_test(&NewTest::known()).await;
    let on_trashed_test = app
        .insert_document(&[trashed_test], NewDocument::pdf("Test", &pdf))
        .await;
    app.trash_test(trashed_test).await;

    // A visible Document asked for under a Test it doesn't belong to.
    let elsewhere = app.insert_test(&NewTest::known()).await;
    let visible = app.insert_document(&[elsewhere], NewDocument::pdf("Test", &pdf)).await;

    let cases = [
        (published, hidden_document),
        (published, trashed_document),
        (hidden_test, on_hidden_test),
        (trashed_test, on_trashed_test),
        (published, visible),
        (published, 999_999),
        (999_999, visible),
    ];
    for (test, document) in cases {
        for file in ["pdf", "original"] {
            let path = format!("/tests/{test}/documents/{document}/{file}");
            assert_eq!(app.get(&path).await.status(), 404, "GET {path}");
        }
    }
    for document in [
        hidden_document,
        trashed_document,
        on_hidden_test,
        on_trashed_test,
        999_999,
    ] {
        for file in ["pdf", "original"] {
            let path = format!("/documents/{document}/{file}");
            assert_eq!(app.get(&path).await.status(), 404, "GET {path}");
        }
    }

    // None of them are linked from the search page either.
    let page = app.get_page("/").await;
    let hrefs: Vec<String> = page
        .select("#results td.documents a[href]")
        .iter()
        .map(|a| a.value().attr("href").unwrap().to_owned())
        .collect();
    assert_eq!(
        hrefs,
        vec![format!("/tests/{elsewhere}/documents/{visible}/pdf")],
        "only the visible Document is linked"
    );
}

#[tokio::test]
async fn a_document_link_without_a_test_redirects_to_its_first_published_test() {
    let app = TestApp::start().await;
    let pdf = sample_pdf(1_000);
    let docx = sample_docx(1_000);
    let hidden = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    let first = app.insert_test(&NewTest::known()).await;
    let second = app.insert_test(&NewTest::known()).await;
    let document = app
        .insert_document(
            &[second, hidden, first],
            NewDocument::docx("Answers", &docx, Some(&pdf)),
        )
        .await;

    for file in ["pdf", "original"] {
        let response = app.get(&format!("/documents/{document}/{file}")).await;
        assert_eq!(response.status(), 307, "/documents/{document}/{file}");
        assert_eq!(
            response.headers()["location"],
            format!("/tests/{first}/documents/{document}/{file}").as_str()
        );
    }

    let response = app.get(&format!("/tests/{first}/documents/{document}/pdf")).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap(), pdf);
}
