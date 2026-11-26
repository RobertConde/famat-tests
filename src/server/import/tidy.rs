//! Value tidying: one row's raw labels become a Document kind and the Test
//! they label (spec, "Migration"):
//!
//! - `?`, `-` and blank are unknown, and so is year `0`.
//! - Month is not applicable for State, National and Mail-in.
//! - Plural event names become singular (Invitationals → Invitational,
//!   Regionals → Regional, States → State, Nationals and the sheet's
//!   "Nationalls" → National).
//! - Test type parentheses are dropped: `(Individual)` → `Individual`,
//!   `State Bowl (Nationals)` → `State Bowl Nationals`.
//! - The sheet's "Washington State" (or "Washington") event is Competition
//!   State with Host Washington Mu Alpha Theta.
//! - Whitespace is trimmed and runs of it collapse to one space.
//!
//! Splitting Calculus/Mu "Applications" needs the files, so the plan does it
//! ([`super::plan`]).

use anyhow::{Context, bail};

use super::sources::{LabelRow, RawLabels};

pub const STATE: &str = "State";
pub const NATIONAL: &str = "National";
pub const MAIL_IN: &str = "Mail-in";
pub const WASHINGTON_HOST: &str = "Washington Mu Alpha Theta";

/// The Document kinds the sheet and the AI use, as named in
/// `document_kinds` (migration 0001).
pub const KINDS: [&str; 7] = [
    "Test",
    "Condensed Test",
    "Answers",
    "Solutions",
    "Combined Test+Solutions",
    "Errata",
    "Other",
];

/// A Test's Month.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Month {
    Known(String),
    Unknown,
    /// State, National and Mail-in events have no Month.
    NotApplicable,
}

/// The six Facets of a Test: Documents with the same ones form one Test.
/// `None` is unknown.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TestLabels {
    pub year: Option<i16>,
    pub month: Month,
    pub competition: Option<String>,
    pub division_level: Option<String>,
    pub test_type: Option<String>,
    pub host: Option<String>,
}

/// One row, tidied.
#[derive(Clone, Debug)]
pub struct Tidied {
    /// One of [`KINDS`]; `None` when unknown.
    pub kind: Option<&'static str>,
    pub test: TestLabels,
}

pub fn tidy(row: &LabelRow) -> anyhow::Result<Tidied> {
    tidy_labels(&row.labels).with_context(|| {
        format!(
            "File Number {} ({} line {})",
            row.file_number,
            row.source.as_str(),
            row.line
        )
    })
}

pub fn tidy_labels(raw: &RawLabels) -> anyhow::Result<Tidied> {
    let kind = match known(&raw.file_type) {
        None => None,
        Some(kind) => match KINDS.iter().find(|k| k.eq_ignore_ascii_case(&kind)) {
            Some(kind) => Some(*kind),
            None => bail!("File Type {kind:?} is not a Document kind"),
        },
    };

    let year = match known(&raw.year).as_deref() {
        None | Some("0") => None,
        Some(year) => match year.parse::<i16>() {
            Ok(y) if (1900..=2100).contains(&y) => Some(y),
            _ => bail!("Year {year:?} is not a year"),
        },
    };

    let mut host = known(&raw.host);
    let competition = known(&raw.comp_type).map(|comp| match comp.to_lowercase().as_str() {
        "invitational" | "invitationals" => "Invitational".to_owned(),
        "regional" | "regionals" => "Regional".to_owned(),
        "state" | "states" => STATE.to_owned(),
        "national" | "nationals" | "nationalls" => NATIONAL.to_owned(),
        "mail-in" | "mail in" | "mailin" => MAIL_IN.to_owned(),
        "washington" | "washington state" => {
            host = Some(WASHINGTON_HOST.to_owned());
            STATE.to_owned()
        }
        _ => comp,
    });

    let month = match competition.as_deref() {
        Some(STATE | NATIONAL | MAIL_IN) => Month::NotApplicable,
        _ => known(&raw.comp_month).map_or(Month::Unknown, Month::Known),
    };

    let test_type = known(&raw.test.replace(['(', ')'], " "));

    Ok(Tidied {
        kind,
        test: TestLabels {
            year,
            month,
            competition,
            division_level: known(&raw.division),
            test_type,
            host,
        },
    })
}

/// A label with its whitespace tidied, or `None` when it says unknown.
pub fn known(raw: &str) -> Option<String> {
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    match value.as_str() {
        "" | "?" | "-" => None,
        _ => Some(value),
    }
}
