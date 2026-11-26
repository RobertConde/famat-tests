//! The import plan: what the archive will hold, worked out from the
//! [`Sources`] before anything is written. [`Plan::build`] runs these steps
//! in order; each is a function of its own, so #31 can add its passes
//! (duplicates and Hidden, Needs review, attach decisions, Word+PDF pairs,
//! unpacked bundles, dropping `.lnk` files) between them.
//!
//! 1. [`labels_by_document`]: each Document's label rows. The AI's rows
//!    replace the sheet's row for the same File Number.
//! 2. [`documents`]: each labelled Document with its file and tidied rows.
//!    Labelled numbers with no file, and files with no labels, are
//!    [`Skipped`].
//! 3. [`split_applications`]: Calculus/Mu "Applications" becomes `Mu
//!    Applications` or `Calculus Applications`.
//! 4. [`group`]: Documents with identical labels form one Test; a Document
//!    with rows for several Tests belongs to each.
//! 5. [`assign_test_ids`]: a Test keeps the legacy `test_id` of the old Test
//!    it shares the most Documents with (on a tie, the one with the same
//!    labels); the others are numbered after [`LAST_LEGACY_TEST_ID`].
//! 6. [`facet_values`]: every Facet value used, in its seeded display order.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::Read,
};

use anyhow::Context;

use super::{
    sources::{LabelRow, LabelSource, SourceFile, Sources},
    tidy::{self, Month, TestLabels},
};
use crate::server::add_test::content_type;

/// The highest old `test_id`: new Tests are numbered after it (spec, "IDs").
pub const LAST_LEGACY_TEST_ID: i64 = 3706;

/// The highest legacy `tid`, counting the Documents unpacked from bundles
/// (10364–10394, #31): new Documents are numbered after it.
pub const LAST_LEGACY_DOCUMENT_ID: i64 = 10394;

pub const CALCULUS_MU: &str = "Calculus/Mu";
pub const APPLICATIONS: &str = "Applications";
pub const MU_APPLICATIONS: &str = "Mu Applications";
pub const CALCULUS_APPLICATIONS: &str = "Calculus Applications";

/// A Facet with a curated value list, as the `facet` enum in Postgres names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facet {
    Month,
    Competition,
    DivisionLevel,
    TestType,
    Host,
}

impl Facet {
    pub const ALL: [Facet; 5] = [
        Facet::Month,
        Facet::Competition,
        Facet::DivisionLevel,
        Facet::TestType,
        Facet::Host,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Competition => "competition",
            Self::DivisionLevel => "division_level",
            Self::TestType => "test_type",
            Self::Host => "host",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Month => "Month",
            Self::Competition => "Competition",
            Self::DivisionLevel => "Division/Level",
            Self::TestType => "Test type",
            Self::Host => "Host",
        }
    }

    /// The values seeded first, in this order; every other value follows
    /// alphabetically (spec, "Initial display orders").
    fn seeded_order(self) -> &'static [&'static str] {
        match self {
            Self::Month => &["November", "January", "February", "March", "April"],
            Self::Competition => &["Invitational", "Regional", "State", "National", "Mail-in"],
            Self::DivisionLevel => &[
                "Algebra I",
                "Geometry",
                "Algebra II",
                "Precalculus/Alpha",
                "Calculus/Mu",
                "Statistics",
                "Theta",
                "Open",
                "Euclidean",
                "Dostal",
                "Nunn",
                "Hiller",
            ],
            Self::TestType => &["Individual", "Team"],
            Self::Host => &["FAMAT", "MAO"],
        }
    }

    /// A Test's value of this Facet; `None` when unknown or not applicable.
    pub fn of(self, test: &TestLabels) -> Option<&str> {
        match self {
            Self::Month => match &test.month {
                Month::Known(month) => Some(month),
                Month::Unknown | Month::NotApplicable => None,
            },
            Self::Competition => test.competition.as_deref(),
            Self::DivisionLevel => test.division_level.as_deref(),
            Self::TestType => test.test_type.as_deref(),
            Self::Host => test.host.as_deref(),
        }
    }
}

/// What the archive will hold.
pub struct Plan {
    /// Each Facet's values in display order.
    pub facet_values: Vec<(Facet, Vec<String>)>,
    /// By ID.
    pub tests: Vec<PlannedTest>,
    /// By ID.
    pub documents: Vec<PlannedDocument>,
    /// What was not imported, and why. #31's reconciliation report starts here.
    pub skipped: Vec<Skipped>,
    /// How the Calculus/Mu Applications Documents were split.
    pub applications: Vec<(i64, Applications)>,
    /// Old Tests whose `test_id` no Test kept: every one of their Documents
    /// went to a Test that kept another old Test's ID, or none was imported.
    pub unclaimed_legacy_ids: Vec<i64>,
}

pub struct PlannedTest {
    pub id: i64,
    pub labels: TestLabels,
    /// Whether it keeps an old Test's `test_id`.
    pub legacy: bool,
    /// The publish rule: a Document kind is known, and the Year or the
    /// Competition is.
    pub published: bool,
}

pub struct PlannedDocument {
    /// Its legacy `tid`.
    pub id: i64,
    pub file: SourceFile,
    /// The Original's extension (sniffed from its contents when the file
    /// name has none) and content type.
    pub extension: String,
    pub content_type: &'static str,
    pub pdf_status: &'static str,
    /// A `document_kinds` name. An unknown kind is imported as Other.
    pub kind: &'static str,
    pub kind_known: bool,
    pub label_source: LabelSource,
    /// Its label rows, as read (#31 reads the AI's Hidden and Duplicate Of).
    pub rows: Vec<LabelRow>,
    /// The Tests it belongs to: their labels, then (from step 5) their IDs.
    pub test_labels: Vec<TestLabels>,
    pub tests: Vec<i64>,
    pub published: bool,
    /// Needs review, with its comment.
    pub needs_review: Option<String>,
}

/// Something in the inputs that was not imported.
#[derive(Clone, Debug)]
pub struct Skipped {
    /// The file name, or `File Number N` for labels with no file.
    pub what: String,
    pub reason: &'static str,
}

/// How a Calculus/Mu "Applications" Document was split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applications {
    /// Its file name, its AI evidence or its contents say "Mu App…".
    Mu(&'static str),
    /// They say "Calc App…" or "Calculus App…".
    Calculus(&'static str),
    /// They say neither, but the other Documents with the same labels agree
    /// on this Test type.
    LikeSiblings(&'static str),
    /// Nothing says which: imported as Calculus Applications, Needs review.
    Undecided,
}

impl Applications {
    fn test_type(&self) -> &'static str {
        match self {
            Self::Mu(_) => MU_APPLICATIONS,
            Self::Calculus(_) | Self::Undecided => CALCULUS_APPLICATIONS,
            Self::LikeSiblings(test_type) => test_type,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Mu(why) => format!("{MU_APPLICATIONS} ({why})"),
            Self::Calculus(why) => format!("{CALCULUS_APPLICATIONS} ({why})"),
            Self::LikeSiblings(test_type) => format!("{test_type} (like the other Documents of its Test)"),
            Self::Undecided => format!("{CALCULUS_APPLICATIONS} by default, Needs review"),
        }
    }
}

pub const APPLICATIONS_REVIEW: &str = "Imported as Calculus Applications: neither the file name, the AI evidence \
     nor the file says whether this is Calculus Applications or Mu Applications.";

impl Plan {
    pub fn build(sources: Sources) -> anyhow::Result<Self> {
        let Sources {
            rows,
            files,
            stray_names,
            legacy_tests,
        } = sources;
        let mut skipped = Vec::new();
        let labels = labels_by_document(rows);
        let mut documents = documents(labels, files, stray_names, &mut skipped)?;
        let applications = split_applications(&mut documents)?;
        let groups = group(&documents);
        let tests = assign_test_ids(groups, &mut documents, &legacy_tests);
        let kept: HashSet<i64> = tests.iter().filter(|t| t.legacy).map(|t| t.id).collect();
        let unclaimed_legacy_ids = legacy_tests
            .iter()
            .map(|t| t.id)
            .filter(|id| !kept.contains(id))
            .collect();
        let facet_values = facet_values(&tests);
        Ok(Self {
            facet_values,
            tests,
            documents,
            skipped,
            applications,
            unclaimed_legacy_ids,
        })
    }
}

/// Step 1: each Document's rows. The AI's rows win over the sheet's row.
fn labels_by_document(rows: Vec<LabelRow>) -> BTreeMap<i64, Vec<LabelRow>> {
    let by_ai: HashSet<i64> = rows
        .iter()
        .filter(|r| r.source == LabelSource::Ai)
        .map(|r| r.file_number)
        .collect();
    let mut by_document: BTreeMap<i64, Vec<LabelRow>> = BTreeMap::new();
    for row in rows {
        if row.source == LabelSource::Sheet && by_ai.contains(&row.file_number) {
            continue;
        }
        by_document.entry(row.file_number).or_default().push(row);
    }
    by_document
}

/// Step 2: the labelled Documents that have a file.
fn documents(
    labels: BTreeMap<i64, Vec<LabelRow>>,
    mut files: BTreeMap<i64, SourceFile>,
    stray_names: Vec<String>,
    skipped: &mut Vec<Skipped>,
) -> anyhow::Result<Vec<PlannedDocument>> {
    let mut documents = Vec::new();
    for (tid, rows) in labels {
        let Some(file) = files.remove(&tid) else {
            skipped.push(Skipped {
                what: format!("File Number {tid}"),
                reason: "labelled, but there is no such file in the docs export",
            });
            continue;
        };
        let tidied = rows.iter().map(tidy::tidy).collect::<anyhow::Result<Vec<_>>>()?;
        // A Document has one kind. When the AI gave it different ones on
        // different Tests (a Master Key that is Solutions for its own Test
        // and Answers for the others), its first row's kind is kept.
        let kind = tidied[0].kind;
        let mut test_labels: Vec<TestLabels> = Vec::new();
        for row in tidied {
            if !test_labels.contains(&row.test) {
                test_labels.push(row.test);
            }
        }
        let extension = if file.extension.is_empty() {
            sniff_extension(&file).with_context(|| format!("reading {}", file.path.display()))?
        } else {
            file.extension.clone()
        };
        documents.push(PlannedDocument {
            id: tid,
            content_type: content_type(&extension),
            pdf_status: pdf_status(&extension),
            extension,
            kind: kind.unwrap_or("Other"),
            kind_known: kind.is_some(),
            label_source: rows[0].source,
            file,
            rows,
            test_labels,
            tests: Vec::new(),
            published: true,
            needs_review: None,
        });
    }
    for file in files.into_values() {
        skipped.push(Skipped {
            what: file.file_name,
            reason: "no labels in the sheet or the AI labels",
        });
    }
    for name in stray_names {
        skipped.push(Skipped {
            what: name,
            reason: "not a Document: the name does not start with a tid",
        });
    }
    Ok(documents)
}

/// What a Document's PDF will be: a PDF Original is its own; a ZIP has none;
/// the office formats are for the conversion queue; anything else cannot be
/// converted.
fn pdf_status(extension: &str) -> &'static str {
    match extension {
        "pdf" => "ready",
        "doc" | "docx" | "ppt" | "pptx" | "rtf" | "odt" => "preparing",
        _ => "not_convertible",
    }
}

/// The extension a file with none should have, from its first bytes; empty
/// when they are not recognised.
fn sniff_extension(file: &SourceFile) -> anyhow::Result<String> {
    let mut head = Vec::with_capacity(4096);
    std::fs::File::open(&file.path)?.take(4096).read_to_end(&mut head)?;
    let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
    let extension = if head.starts_with(b"%PDF") {
        "pdf"
    } else if head.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        // An OLE compound file; in this archive, Word 97–2003.
        if has(b"P\0o\0w\0e\0r\0P\0o\0i\0n\0t") {
            "ppt"
        } else {
            "doc"
        }
    } else if head.starts_with(b"PK\x03\x04") {
        if has(b"word/") {
            "docx"
        } else if has(b"ppt/") {
            "pptx"
        } else {
            "zip"
        }
    } else if head.starts_with(b"{\\rtf") {
        "rtf"
    } else {
        ""
    };
    Ok(extension.to_owned())
}

/// Step 3: Calculus/Mu "Applications" was two tests at State, so each such
/// Document becomes `Mu Applications` when its file name, its AI evidence
/// or its contents (in that order; the first that says either decides) say
/// "Mu App…", and `Calculus Applications` when they say "Calc App…" or
/// "Calculus App…". A Document none of them decide takes the type its
/// decided siblings (same labels) agree on, or else is `Calculus
/// Applications` and Needs review.
///
/// Contents are searched as raw bytes, which finds the text of Word 97–2003
/// files and uncompressed PDFs but not of compressed PDF streams.
fn split_applications(documents: &mut [PlannedDocument]) -> anyhow::Result<Vec<(i64, Applications)>> {
    let is_applications = |t: &TestLabels| {
        t.division_level.as_deref() == Some(CALCULUS_MU) && t.test_type.as_deref() == Some(APPLICATIONS)
    };

    let mut decisions: BTreeMap<usize, Applications> = BTreeMap::new();
    for (i, document) in documents.iter().enumerate() {
        if !document.test_labels.iter().any(is_applications) {
            continue;
        }
        let evidence: String = document
            .rows
            .iter()
            .filter_map(|r| r.ai.as_ref())
            .flat_map(|ai| [ai.evidence.as_str(), " ", ai.note.as_str(), " "])
            .collect();
        let decision = match applications_in(document.file.file_name.as_bytes(), "file name") {
            Some(decided) => decided,
            None => match applications_in(evidence.as_bytes(), "AI evidence") {
                Some(decided) => decided,
                None => {
                    let contents = std::fs::read(&document.file.path)
                        .with_context(|| format!("reading {}", document.file.path.display()))?;
                    applications_in(&contents, "file contents").unwrap_or(Applications::Undecided)
                }
            },
        };
        decisions.insert(i, decision);
    }

    // Undecided Documents follow their decided siblings when those agree.
    let mut siblings: HashMap<&TestLabels, BTreeSet<&'static str>> = HashMap::new();
    for (&i, decision) in &decisions {
        if *decision != Applications::Undecided {
            for labels in documents[i].test_labels.iter().filter(|t| is_applications(t)) {
                siblings.entry(labels).or_default().insert(decision.test_type());
            }
        }
    }
    let mut adopted = Vec::new();
    for (&i, decision) in &decisions {
        if *decision != Applications::Undecided {
            continue;
        }
        let agreed: BTreeSet<&'static str> = documents[i]
            .test_labels
            .iter()
            .filter(|t| is_applications(t))
            .flat_map(|t| siblings.get(t).into_iter().flatten().copied())
            .collect();
        if let [only] = agreed.into_iter().collect::<Vec<_>>()[..] {
            adopted.push((i, Applications::LikeSiblings(only)));
        }
    }
    decisions.extend(adopted);

    let mut summary = Vec::new();
    for (i, decision) in decisions {
        let document = &mut documents[i];
        let mut split: Vec<TestLabels> = Vec::new();
        for mut labels in std::mem::take(&mut document.test_labels) {
            if is_applications(&labels) {
                labels.test_type = Some(decision.test_type().to_owned());
            }
            if !split.contains(&labels) {
                split.push(labels);
            }
        }
        document.test_labels = split;
        if decision == Applications::Undecided {
            document.needs_review = Some(APPLICATIONS_REVIEW.to_owned());
        }
        summary.push((document.id, decision));
    }
    Ok(summary)
}

/// Whether `text` says Mu Applications or Calculus Applications. Mu wins
/// when it says both. NUL bytes are dropped first, so UTF-16 text matches.
fn applications_in(text: &[u8], source: &'static str) -> Option<Applications> {
    let cleaned: String = text
        .iter()
        .filter(|b| **b != 0)
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                b.to_ascii_lowercase() as char
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let says = |first: &[&str]| {
        words
            .windows(2)
            .any(|w| first.contains(&w[0]) && w[1].starts_with("app"))
    };
    if says(&["mu"]) {
        Some(Applications::Mu(source))
    } else if says(&["calc", "calculus"]) {
        Some(Applications::Calculus(source))
    } else {
        None
    }
}

/// Step 4: each Test's labels and its Documents (indexes into `documents`).
fn group(documents: &[PlannedDocument]) -> BTreeMap<TestLabels, BTreeSet<usize>> {
    let mut groups: BTreeMap<TestLabels, BTreeSet<usize>> = BTreeMap::new();
    for (i, document) in documents.iter().enumerate() {
        for labels in &document.test_labels {
            groups.entry(labels.clone()).or_default().insert(i);
        }
    }
    groups
}

/// Step 5: Test IDs. Pairs of (new Test, old Test) sharing Documents are
/// taken most-shared first, so each old `test_id` goes to the Test holding
/// most of its old Documents. On a tie, a Test with the old Test's labels
/// (tidied) wins, then the lower IDs. The remaining Tests
/// are numbered after the highest old `test_id`, in order of their lowest
/// Document ID. Also fills in each Document's `tests`, and each Test's
/// Published from the publish rule.
fn assign_test_ids(
    groups: BTreeMap<TestLabels, BTreeSet<usize>>,
    documents: &mut [PlannedDocument],
    legacy_tests: &[super::sources::LegacyTest],
) -> Vec<PlannedTest> {
    let groups: Vec<(TestLabels, BTreeSet<usize>)> = groups.into_iter().collect();
    let mut legacy_of_tid: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut legacy_labels: HashMap<i64, TestLabels> = HashMap::new();
    for test in legacy_tests {
        for tid in &test.tids {
            legacy_of_tid.entry(*tid).or_default().push(test.id);
        }
        // Labels the tidying refuses simply never match.
        if let Ok(tidied) = tidy::tidy_labels(&test.labels) {
            legacy_labels.insert(test.id, tidied.test);
        }
    }

    // (shared Documents, labels differ, old test_id, group's lowest Document
    // ID, group), sorted so the best pair comes first.
    let mut pairs = Vec::new();
    for (g, (labels, members)) in groups.iter().enumerate() {
        let mut shared: BTreeMap<i64, usize> = BTreeMap::new();
        for &i in members {
            for &old in legacy_of_tid.get(&documents[i].id).into_iter().flatten() {
                *shared.entry(old).or_default() += 1;
            }
        }
        let lowest = members.iter().map(|&i| documents[i].id).min().unwrap_or_default();
        pairs.extend(shared.into_iter().map(|(old, n)| {
            let differ = legacy_labels.get(&old) != Some(labels);
            (Reverse(n), differ, old, lowest, g)
        }));
    }
    pairs.sort();
    let mut ids: Vec<Option<i64>> = vec![None; groups.len()];
    let mut taken = HashSet::new();
    for (_, _, old, _, g) in pairs {
        if ids[g].is_none() && taken.insert(old) {
            ids[g] = Some(old);
        }
    }
    let legacy: Vec<bool> = ids.iter().map(Option::is_some).collect();

    let highest_legacy = legacy_tests.iter().map(|t| t.id).max().unwrap_or(0);
    let first_new = highest_legacy.max(LAST_LEGACY_TEST_ID) + 1;
    let mut unmatched: Vec<(i64, usize)> = (0..groups.len())
        .filter(|&g| ids[g].is_none())
        .map(|g| {
            (
                groups[g].1.iter().map(|&i| documents[i].id).min().unwrap_or_default(),
                g,
            )
        })
        .collect();
    unmatched.sort();
    for (id, (_, g)) in (first_new..).zip(unmatched) {
        ids[g] = Some(id);
    }

    let mut tests = Vec::new();
    for (((labels, members), id), legacy) in groups.into_iter().zip(ids).zip(legacy) {
        let id = id.expect("every Test has an ID by now");
        for &i in &members {
            documents[i].tests.push(id);
        }
        let published =
            members.iter().any(|&i| documents[i].kind_known) && (labels.year.is_some() || labels.competition.is_some());
        tests.push(PlannedTest {
            id,
            legacy,
            labels,
            published,
        });
    }
    tests.sort_by_key(|t| t.id);
    for document in documents.iter_mut() {
        document.tests.sort();
    }
    tests
}

/// Step 6: every Facet value the Tests use, in display order: the seeded
/// values first in their order, then the rest alphabetically.
fn facet_values(tests: &[PlannedTest]) -> Vec<(Facet, Vec<String>)> {
    Facet::ALL
        .into_iter()
        .map(|facet| {
            let used: BTreeSet<&str> = tests.iter().filter_map(|t| facet.of(&t.labels)).collect();
            let seeded = facet.seeded_order();
            let mut values: Vec<&str> = seeded.iter().copied().filter(|v| used.contains(v)).collect();
            let mut rest: Vec<&str> = used.iter().copied().filter(|v| !seeded.contains(v)).collect();
            rest.sort_by_key(|v| (v.to_lowercase(), v.to_string()));
            values.extend(rest);
            (facet, values.into_iter().map(str::to_owned).collect())
        })
        .collect()
}
