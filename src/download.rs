//! What a Test's download menu offers, shared by the search page and the
//! zip route: kind groups and formats, and the zip URL they make.
//!
//! - **Kind groups** gather Documents by what their kind contains, so they
//!   follow Admin renames and new kinds: Tests (questions), Answers (answers
//!   only), Solutions (solutions), Errata & other (nothing searchable).
//!   Combined Test+Solutions is in both Tests and Solutions.
//! - **Formats:** PDF (falling back to the Original when there is no ready
//!   PDF, e.g. for a ZIP), Original, or Both.

use serde::{Deserialize, Serialize};

/// What a Document's kind contains, as the Document kinds table states it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contents {
    pub questions: bool,
    pub answers: bool,
    pub solutions: bool,
}

/// A part of a Test's Documents to download, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindGroup {
    #[default]
    All,
    Tests,
    Answers,
    Solutions,
    /// Errata & other.
    Other,
}

impl KindGroup {
    /// In menu order.
    pub const MENU: [Self; 5] = [Self::All, Self::Tests, Self::Answers, Self::Solutions, Self::Other];

    /// The query-string value.
    pub fn slug(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Tests => "tests",
            Self::Answers => "answers",
            Self::Solutions => "solutions",
            Self::Other => "other",
        }
    }

    /// The menu label, also the zip name's suffix.
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All documents",
            Self::Tests => "Tests",
            Self::Answers => "Answers",
            Self::Solutions => "Solutions",
            Self::Other => "Errata & other",
        }
    }

    /// Whether a Document whose kind contains `contents` is in this group.
    pub fn includes(self, contents: Contents) -> bool {
        let Contents {
            questions,
            answers,
            solutions,
        } = contents;
        match self {
            Self::All => true,
            Self::Tests => questions,
            Self::Answers => answers && !questions && !solutions,
            Self::Solutions => solutions,
            Self::Other => !questions && !answers && !solutions,
        }
    }
}

/// Which of each Document's files to download.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// The PDF, or the Original when there is no ready PDF.
    #[default]
    Pdf,
    Original,
    /// The Original and, when it is not itself a PDF, the PDF.
    Both,
}

// The search reads `format` from the query string on the server only.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
impl Format {
    /// In switch order.
    pub const ALL: [Self; 3] = [Self::Pdf, Self::Original, Self::Both];

    /// The query-string value.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Original => "original",
            Self::Both => "both",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.slug() == slug)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Original => "Original",
            Self::Both => "Both",
        }
    }
}

/// The zip of one Test's Documents in `group`, as `format`. Defaults are left
/// out, so "all as PDF" is just `/tests/{id}/zip`.
pub fn zip_href(test_id: i64, group: KindGroup, format: Format) -> String {
    let mut params = Vec::new();
    if group != KindGroup::All {
        params.push(format!("group={}", group.slug()));
    }
    if format != Format::Pdf {
        params.push(format!("format={}", format.slug()));
    }
    if params.is_empty() {
        format!("/tests/{test_id}/zip")
    } else {
        format!("/tests/{test_id}/zip?{}", params.join("&"))
    }
}
