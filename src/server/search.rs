//! Public search over Published Tests.

use std::collections::HashMap;

use sqlx::{FromRow, PgPool};

use crate::app::{DocumentLink, FacetValue, TestRow};

#[derive(FromRow)]
struct TestRecord {
    id: i64,
    year: Option<i16>,
    month_applicable: bool,
    month: Option<String>,
    competition: Option<String>,
    host: Option<String>,
    division_level: Option<String>,
    test_type: Option<String>,
}

#[derive(FromRow)]
struct DocumentRecord {
    test_id: i64,
    id: i64,
    kind: String,
    label: Option<String>,
    /// NULL when the Original is a PDF.
    original_extension: Option<String>,
}

/// Published, untrashed Tests with their Published, untrashed Documents.
///
/// Order: Year newest first, then Month in season order (month-less last),
/// Competition, Host, Division/Level and Test type, each in Facet display
/// order; unknowns sort last.
pub async fn published_tests(db: &PgPool) -> Result<Vec<TestRow>, sqlx::Error> {
    let tests: Vec<TestRecord> = sqlx::query_as(
        "SELECT t.id, t.year, t.month_applicable,
                m.name AS month, c.name AS competition, h.name AS host,
                dl.name AS division_level, tt.name AS test_type
         FROM tests t
         LEFT JOIN facet_values m  ON m.id  = t.month_id
         LEFT JOIN facet_values c  ON c.id  = t.competition_id
         LEFT JOIN facet_values h  ON h.id  = t.host_id
         LEFT JOIN facet_values dl ON dl.id = t.division_level_id
         LEFT JOIN facet_values tt ON tt.id = t.test_type_id
         WHERE t.published AND t.trashed_at IS NULL
         ORDER BY t.year DESC NULLS LAST,
                  m.display_order NULLS LAST, m.name,
                  c.display_order NULLS LAST, c.name,
                  h.display_order NULLS LAST, h.name,
                  dl.display_order NULLS LAST, dl.name,
                  tt.display_order NULLS LAST, tt.name,
                  t.id",
    )
    .fetch_all(db)
    .await?;

    let ids: Vec<i64> = tests.iter().map(|t| t.id).collect();
    let documents: Vec<DocumentRecord> = sqlx::query_as(
        "SELECT td.test_id, d.id, k.name AS kind, d.label,
                CASE WHEN o.content_type <> 'application/pdf' AND o.extension <> 'pdf'
                     THEN o.extension END AS original_extension
         FROM test_documents td
         JOIN documents d      ON d.id = td.document_id
         JOIN document_kinds k ON k.id = d.kind_id
         LEFT JOIN document_files o ON o.document_id = d.id AND o.role = 'original'
         WHERE td.test_id = ANY($1) AND d.published AND d.trashed_at IS NULL
         ORDER BY k.display_order, k.name, d.label NULLS FIRST, d.id",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;

    let mut by_test: HashMap<i64, Vec<DocumentLink>> = HashMap::new();
    for d in documents {
        by_test.entry(d.test_id).or_default().push(DocumentLink {
            id: d.id,
            kind: d.kind,
            label: d.label,
            original_extension: d.original_extension,
        });
    }

    Ok(tests
        .into_iter()
        .map(|t| TestRow {
            id: t.id,
            year: t.year,
            month: if t.month_applicable {
                facet_value(t.month)
            } else {
                FacetValue::NotApplicable
            },
            competition: facet_value(t.competition),
            host: facet_value(t.host),
            division_level: facet_value(t.division_level),
            test_type: facet_value(t.test_type),
            documents: by_test.remove(&t.id).unwrap_or_default(),
        })
        .collect())
}

/// A Facet value from a nullable column: NULL means unknown.
fn facet_value(name: Option<String>) -> FacetValue {
    name.map_or(FacetValue::Unknown, FacetValue::Known)
}
