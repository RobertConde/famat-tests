//! Fixtures: write Tests and Documents straight into this test's database and
//! Garage prefix, as the import or an Admin eventually will.
//!
//! The object key scheme (`documents/{id}/{role}` under the prefix) is part
//! of the storage contract, since objects never move, so the fixtures spell
//! it out rather than borrow it from the app.

use sha2::{Digest, Sha256};

use super::TestApp;

/// A Test's Month: unlike the other Facets it can also be not applicable.
#[derive(Clone, Copy, Debug)]
pub enum Month {
    Known(&'static str),
    Unknown,
    NotApplicable,
}

/// A Test to insert. Facets left `None` are unknown.
#[derive(Clone, Debug)]
pub struct NewTest {
    pub year: Option<i16>,
    pub month: Month,
    pub competition: Option<&'static str>,
    pub division_level: Option<&'static str>,
    pub test_type: Option<&'static str>,
    pub host: Option<&'static str>,
    pub published: bool,
}

impl NewTest {
    /// A Published Test with every Facet known:
    /// 1993 February Invitational, Plant City, Calculus/Mu Individual.
    pub fn known() -> Self {
        Self {
            year: Some(1993),
            month: Month::Known("February"),
            competition: Some("Invitational"),
            division_level: Some("Calculus/Mu"),
            test_type: Some("Individual"),
            host: Some("Plant City"),
            published: true,
        }
    }
}

/// A Document to insert.
#[derive(Clone, Debug)]
pub struct NewDocument<'a> {
    /// A Document kind name, e.g. "Test" or "Solutions".
    pub kind: &'static str,
    /// The optional short label, e.g. "2 per page".
    pub label: Option<&'static str>,
    pub published: bool,
    /// The Original's bytes, content type and extension (no dot, lower case).
    pub original: &'a [u8],
    pub content_type: &'static str,
    pub extension: &'static str,
    /// A PDF made from the Original, when the Original isn't one.
    pub pdf: Option<&'a [u8]>,
    /// "preparing", "ready", "failed" or "not_convertible".
    pub pdf_status: &'static str,
    /// Whether to put the files in Garage. `false` leaves the database
    /// pointing at missing objects.
    pub stored: bool,
}

impl<'a> NewDocument<'a> {
    /// A Published Document of `kind` whose Original is `pdf`, stored in Garage.
    pub fn pdf(kind: &'static str, pdf: &'a [u8]) -> Self {
        Self {
            kind,
            label: None,
            published: true,
            original: pdf,
            content_type: "application/pdf",
            extension: "pdf",
            pdf: None,
            pdf_status: "ready",
            stored: true,
        }
    }

    /// A Published Document of `kind` whose Original is a Word file, with the
    /// PDF made from it once one is given (until then it is being prepared).
    pub fn docx(kind: &'static str, docx: &'a [u8], pdf: Option<&'a [u8]>) -> Self {
        Self {
            content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            extension: "docx",
            pdf,
            pdf_status: if pdf.is_some() { "ready" } else { "preparing" },
            ..Self::pdf(kind, docx)
        }
    }

    /// A Published Document of `kind` whose Original is a ZIP, which has no PDF.
    pub fn zip(kind: &'static str, zip: &'a [u8]) -> Self {
        Self {
            content_type: "application/zip",
            extension: "zip",
            pdf: None,
            pdf_status: "not_convertible",
            ..Self::pdf(kind, zip)
        }
    }
}

impl TestApp {
    pub async fn insert_test(&self, test: &NewTest) -> i64 {
        let month = match test.month {
            Month::Known(name) => Some(self.facet_value("month", name).await),
            Month::Unknown | Month::NotApplicable => None,
        };
        let competition = self.optional_facet_value("competition", test.competition).await;
        let division_level = self.optional_facet_value("division_level", test.division_level).await;
        let test_type = self.optional_facet_value("test_type", test.test_type).await;
        let host = self.optional_facet_value("host", test.host).await;
        sqlx::query_scalar(
            "INSERT INTO tests (year, month_id, month_applicable, competition_id,
                                division_level_id, test_type_id, host_id, published)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(test.year)
        .bind(month)
        .bind(!matches!(test.month, Month::NotApplicable))
        .bind(competition)
        .bind(division_level)
        .bind(test_type)
        .bind(host)
        .bind(test.published)
        .fetch_one(&self.db)
        .await
        .expect("insert Test")
    }

    /// Insert a Document belonging to `tests`. Returns its ID.
    pub async fn insert_document(&self, tests: &[i64], document: NewDocument<'_>) -> i64 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO documents (kind_id, label, published, pdf_status)
             VALUES ((SELECT id FROM document_kinds WHERE name = $1), $2, $3, $4::pdf_status)
             RETURNING id",
        )
        .bind(document.kind)
        .bind(document.label)
        .bind(document.published)
        .bind(document.pdf_status)
        .fetch_one(&self.db)
        .await
        .expect("insert Document (is the kind name right?)");
        for test in tests {
            sqlx::query("INSERT INTO test_documents (test_id, document_id) VALUES ($1, $2)")
                .bind(test)
                .bind(id)
                .execute(&self.db)
                .await
                .expect("attach Document to Test");
        }
        let mut files = vec![("original", document.content_type, document.extension, document.original)];
        if let Some(pdf) = document.pdf {
            files.push(("pdf", "application/pdf", "pdf", pdf));
        }
        for (role, content_type, extension, bytes) in files {
            sqlx::query(
                "INSERT INTO document_files (document_id, role, content_type, extension, byte_size, sha256)
                 VALUES ($1, $2::file_role, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(role)
            .bind(content_type)
            .bind(extension)
            .bind(bytes.len() as i64)
            .bind(Sha256::digest(bytes).to_vec())
            .execute(&self.db)
            .await
            .expect("insert Document file");
            if document.stored {
                self.s3
                    .put_object()
                    .bucket(&self.bucket)
                    .key(self.document_key(id, role))
                    .content_type(content_type)
                    .body(bytes.to_vec().into())
                    .send()
                    .await
                    .expect("put the file into Garage");
            }
        }
        id
    }

    /// Move a Test to Trash. It stays marked Published, so the Trash alone must
    /// keep it out of sight.
    pub async fn trash_test(&self, id: i64) {
        sqlx::query("UPDATE tests SET trashed_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.db)
            .await
            .expect("trash Test");
    }

    /// Move a Document to Trash. It stays marked Published, so the Trash alone
    /// must keep it out of sight.
    pub async fn trash_document(&self, id: i64) {
        sqlx::query("UPDATE documents SET trashed_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.db)
            .await
            .expect("trash Document");
    }

    /// Say when one of a Document's files was stored, e.g.
    /// `"2023-02-11 15:04:06+00"`.
    pub async fn set_stored_at(&self, document: i64, role: &str, at: &str) {
        sqlx::query(
            "UPDATE document_files SET created_at = $3::timestamptz
             WHERE document_id = $1 AND role = $2::file_role",
        )
        .bind(document)
        .bind(role)
        .bind(at)
        .execute(&self.db)
        .await
        .expect("set when a Document file was stored");
    }

    /// Rename a Facet value, as an Admin's rename does.
    pub async fn rename_facet_value(&self, facet: &str, from: &str, to: &str) {
        sqlx::query("UPDATE facet_values SET name = $3 WHERE facet = $1::facet AND name = $2")
            .bind(facet)
            .bind(from)
            .bind(to)
            .execute(&self.db)
            .await
            .expect("rename Facet value");
    }

    /// The Garage key of one of this test's Document files.
    pub fn document_key(&self, document_id: i64, role: &str) -> String {
        format!("{}documents/{document_id}/{role}", self.key_prefix)
    }

    /// The bytes of an object as Garage holds them.
    pub async fn stored_object(&self, key: &str) -> Vec<u8> {
        let object = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .expect("get object");
        object.body.collect().await.unwrap().into_bytes().to_vec()
    }

    /// The ID of a Facet value, created at the end of the display order if new.
    pub async fn facet_value(&self, facet: &str, name: &str) -> i64 {
        sqlx::query_scalar(
            "WITH existing AS (
                 SELECT id FROM facet_values WHERE facet = $1::facet AND name = $2
             ), inserted AS (
                 INSERT INTO facet_values (facet, name, display_order)
                 SELECT $1::facet, $2,
                        (SELECT coalesce(max(display_order), 0) + 1 FROM facet_values WHERE facet = $1::facet)
                 WHERE NOT EXISTS (SELECT 1 FROM existing)
                 RETURNING id
             )
             SELECT id FROM existing UNION ALL SELECT id FROM inserted",
        )
        .bind(facet)
        .bind(name)
        .fetch_one(&self.db)
        .await
        .expect("get or create Facet value")
    }

    async fn optional_facet_value(&self, facet: &str, name: Option<&str>) -> Option<i64> {
        match name {
            Some(name) => Some(self.facet_value(facet, name).await),
            None => None,
        }
    }
}

/// A PDF-looking file of about `len` bytes with varied content, so a
/// truncated or reordered download cannot match by accident.
pub fn sample_pdf(len: usize) -> Vec<u8> {
    sample_file(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n", len, 0x9e37_79b9_7f4a_7c15)
}

/// A Word-looking file of about `len` bytes, different from any `sample_pdf`.
pub fn sample_docx(len: usize) -> Vec<u8> {
    sample_file(b"PK\x03\x04", len, 0x2545_f491_4f6c_dd1d)
}

fn sample_file(magic: &[u8], len: usize, seed: u64) -> Vec<u8> {
    let mut bytes = magic.to_vec();
    let mut state = seed;
    while bytes.len() + 6 < len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        bytes.push((state >> 33) as u8);
    }
    bytes.extend_from_slice(b"\n%%EOF");
    bytes
}
