//! Opening a Document's PDF, streamed from Garage.

use crate::support::{
    TestApp,
    fixtures::{NewDocument, NewTest, sample_pdf},
    html::links_in,
};

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
    let page = app.get_page("/").await;
    let row = page.one(&format!("tr#T{test}"));
    let (_, href) = links_in(row).into_iter().next().expect("a Document link");
    let response = app.get(&href).await;

    assert_eq!(response.status(), 200);
    let headers = response.headers();
    assert_eq!(headers["content-type"], "application/pdf");
    assert_eq!(headers["content-length"], pdf.len().to_string().as_str());
    assert!(headers["content-disposition"].to_str().unwrap().starts_with("inline"));
    let body = response.bytes().await.unwrap();
    assert!(
        body == stored,
        "downloaded {} bytes that differ from the {} stored",
        body.len(),
        stored.len()
    );
}

#[tokio::test]
async fn a_document_whose_object_is_missing_from_garage_is_a_404() {
    let app = TestApp::start().await;
    let test = app.insert_test(&NewTest::known()).await;
    let pdf = sample_pdf(1_000);
    let document = app
        .insert_document(
            &[test],
            NewDocument {
                stored: false,
                ..NewDocument::pdf("Test", &pdf)
            },
        )
        .await;

    let response = app.get(&format!("/documents/{document}/pdf")).await;

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn hidden_documents_and_documents_of_hidden_tests_are_a_404() {
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
    let hidden_test = app
        .insert_test(&NewTest {
            published: false,
            ..NewTest::known()
        })
        .await;
    let on_hidden_test = app
        .insert_document(&[hidden_test], NewDocument::pdf("Test", &pdf))
        .await;

    for document in [hidden_document, on_hidden_test, 999_999] {
        let response = app.get(&format!("/documents/{document}/pdf")).await;
        assert_eq!(response.status(), 404, "Document {document}");
    }
}
