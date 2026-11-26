//! The reconciliation report, written at the end of every import (spec,
//! "Verification"):
//!
//! - **Source files:** every file of the docs export and of the unpacked
//!   directory (and every other name in the docs export) was imported as a
//!   Document, imported Hidden, imported as a Document's supplied PDF,
//!   dropped or failed, with the reason.
//! - **Old Tests:** every Test of the old site exists with its Documents,
//!   or the report says where each Document went: another Test, Hidden as
//!   a duplicate, a Document's PDF, unpacked from a bundle.
//! - **Garage:** every object was read back and matched its source's size
//!   and SHA-256, and Garage holds nothing else.
//! - **Problems:** anything missing, failed or unaccounted for, and inputs
//!   that disagree.
//!
//! The summary goes to stdout; the whole report, with a line per source
//! file and per old Test, to the report file.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write,
};

use super::{
    load::StoredFiles,
    plan::{Plan, PlannedDocument, SkipOutcome},
    sources::DecisionKind,
    special::{BROKEN_NOTE, DUPLICATE_NOTE, NOT_A_TEST_NOTE, ids, plural},
};

/// What the load did, for the report's Garage and queue lines.
pub enum Loaded<'a> {
    DryRun,
    Written {
        stored: &'a StoredFiles,
        /// How many of the planned objects Garage holds after the sweep.
        held: usize,
        /// Keys Garage holds besides them.
        extra: Vec<String>,
        /// PDF conversions queued.
        queued: u64,
    },
}

pub struct Report {
    /// For stdout.
    pub summary: String,
    /// For the report file: the summary, then the details.
    pub full: String,
}

/// What happened to one source file.
enum Outcome {
    Imported(String),
    Hidden(String),
    SuppliedPdf(String),
    Dropped(String),
    Failed(String),
}

impl Outcome {
    fn line(&self) -> String {
        match self {
            Self::Imported(how) => format!("imported: {how}"),
            Self::Hidden(how) => format!("imported Hidden: {how}"),
            Self::SuppliedPdf(how) => format!("imported as a PDF: {how}"),
            Self::Dropped(why) => format!("dropped: {why}"),
            Self::Failed(why) => format!("FAILED: {why}"),
        }
    }
}

fn tests_list(tests: &[i64]) -> String {
    let names: Vec<String> = tests.iter().map(|t| format!("T{t}")).collect();
    names.join(", ")
}

pub fn reconcile(plan: &Plan, loaded: &Loaded) -> Report {
    let mut problems: Vec<String> = Vec::new();
    let published_test: HashMap<i64, bool> = plan.tests.iter().map(|t| (t.id, t.published)).collect();
    let on_hidden_tests_only = |d: &PlannedDocument| !d.tests.is_empty() && d.tests.iter().all(|t| !published_test[t]);
    let by_id: HashMap<i64, &PlannedDocument> = plan.documents.iter().map(|d| (d.id, d)).collect();

    // ------------------------------------------------------ source files
    let mut outcomes: BTreeMap<&str, Outcome> = BTreeMap::new();
    for document in &plan.documents {
        let place = format!(
            "D{} on {}{}",
            document.id,
            tests_list(&document.tests),
            if on_hidden_tests_only(document) {
                " (Hidden by the publish rule)"
            } else {
                ""
            }
        );
        outcomes.insert(
            &document.file.shown,
            match &document.note {
                Some(note) if !document.published => Outcome::Hidden(format!("{place}; {note}")),
                _ => Outcome::Imported(place),
            },
        );
        if let Some(pdf) = &document.pdf {
            outcomes.insert(
                &pdf.file.shown,
                Outcome::SuppliedPdf(match pdf.former_tid {
                    Some(tid) => format!(
                        "the supplied PDF of D{}: tid {tid} is its Original as a PDF",
                        document.id
                    ),
                    None => format!("the supplied PDF of D{}, which it came with", document.id),
                }),
            );
        }
    }
    let mut missing_labels = Vec::new();
    for skipped in &plan.skipped {
        let outcome = match skipped.outcome {
            SkipOutcome::Dropped => Outcome::Dropped(skipped.reason.clone()),
            SkipOutcome::Failed => Outcome::Failed(skipped.reason.clone()),
            SkipOutcome::Missing => {
                missing_labels.push(format!("{}: {}", skipped.what, skipped.reason));
                continue;
            }
        };
        outcomes.insert(&skipped.what, outcome);
    }
    let mut lines = Vec::new();
    let mut unaccounted = 0;
    for name in &plan.inventory {
        match outcomes.get(name.as_str()) {
            Some(outcome) => {
                if let Outcome::Failed(why) = outcome {
                    problems.push(format!("{name}: {why}"));
                }
                lines.push(format!("  {name}: {}", outcome.line()));
            }
            None => {
                unaccounted += 1;
                problems.push(format!("{name}: not accounted for by the import"));
                lines.push(format!("  {name}: NOT ACCOUNTED FOR"));
            }
        }
    }
    for line in &missing_labels {
        problems.push(format!("MISSING {line}"));
    }
    let count = |f: fn(&Outcome) -> bool| {
        plan.inventory
            .iter()
            .filter(|n| outcomes.get(n.as_str()).is_some_and(f))
            .count()
    };
    let imported = count(|o| matches!(o, Outcome::Imported(_)));
    let hidden = count(|o| matches!(o, Outcome::Hidden(_)));
    let pdfs = count(|o| matches!(o, Outcome::SuppliedPdf(_)));
    let dropped = count(|o| matches!(o, Outcome::Dropped(_)));
    let failed = count(|o| matches!(o, Outcome::Failed(_)));

    // --------------------------------------------------------- old Tests
    // Where a tid went when it is not a Document, and whether that loses it.
    let mut gone: HashMap<i64, (String, bool)> = HashMap::new();
    for document in &plan.documents {
        if let Some(tid) = document.pdf.as_ref().and_then(|p| p.former_tid) {
            gone.insert(tid, (format!("is now the PDF of D{}", document.id), false));
        }
    }
    for skipped in &plan.skipped {
        let tid: Option<i64> = match skipped.what.strip_prefix("File Number ") {
            Some(tid) => tid.parse().ok(),
            None if skipped.what.starts_with("unpacked/") => None,
            None => skipped.what.split_once(' ').and_then(|(tid, _)| tid.parse().ok()),
        };
        if let Some(tid) = tid {
            let stated = skipped.outcome == SkipOutcome::Dropped;
            gone.entry(tid).or_insert_with(|| {
                let why = if stated {
                    format!("was dropped: {}", skipped.reason)
                } else {
                    format!("is missing: {}", skipped.reason)
                };
                (why, !stated)
            });
        }
    }
    let kept_ids: BTreeSet<i64> = plan.tests.iter().filter(|t| t.legacy).map(|t| t.id).collect();
    let (mut whole, mut moved, mut succeeded, mut incomplete) = (0, 0, 0, 0);
    let mut test_lines = Vec::new();
    for legacy in &plan.legacy_tests {
        let kept = kept_ids.contains(&legacy.id);
        let mut notes = Vec::new();
        let mut lost = Vec::new();
        let mut successors = BTreeSet::new();
        for tid in &legacy.tids {
            match by_id.get(tid) {
                Some(document) if kept && document.tests.contains(&legacy.id) => {
                    if !document.published {
                        notes.push(format!("D{tid} is Hidden ({})", document.note.as_deref().unwrap_or("")));
                    }
                }
                Some(document) => {
                    successors.extend(document.tests.iter().copied());
                    let hidden = match (&document.note, document.published) {
                        (Some(note), false) => format!(", Hidden ({note})"),
                        _ => String::new(),
                    };
                    notes.push(format!("D{tid} is on {}{hidden}", tests_list(&document.tests)));
                }
                None => match gone.get(tid) {
                    Some((why, lose)) => {
                        if *lose {
                            lost.push(*tid);
                        }
                        notes.push(format!("D{tid} {why}"));
                    }
                    None => {
                        lost.push(*tid);
                        notes.push(format!("D{tid} is missing: no labels and no file"));
                    }
                },
            }
        }
        // A bundle's Documents succeed its Test.
        for tid in &legacy.tids {
            for document in plan
                .documents
                .iter()
                .filter(|d| d.file.shown.starts_with(&format!("unpacked/{tid}/")))
            {
                successors.extend(document.tests.iter().copied());
            }
        }
        let status = if !lost.is_empty() {
            incomplete += 1;
            "INCOMPLETE"
        } else if kept && notes.iter().all(|n| n.contains("is Hidden")) {
            whole += 1;
            "kept"
        } else if kept {
            moved += 1;
            "kept"
        } else {
            succeeded += 1;
            "not kept"
        };
        let mut line = format!("  T{}: {status}", legacy.id);
        if !kept {
            if successors.is_empty() {
                line.push_str("; none of its Documents is in the archive");
            } else {
                let _ = write!(
                    line,
                    "; succeeded by {}",
                    tests_list(&successors.into_iter().collect::<Vec<_>>())
                );
            }
        }
        if !notes.is_empty() {
            let _ = write!(line, "; {}", notes.join("; "));
        }
        if status == "INCOMPLETE" || (!kept && line.contains("none of its Documents")) {
            problems.push(line.trim_start().to_owned());
        }
        test_lines.push(line);
    }

    // --------------------------------------------------------- decisions
    let mut decision_lines = Vec::new();
    let mut decision_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (decision, applied) in &plan.decisions {
        let id = decision.file_number;
        let (kind, what) = match &decision.kind {
            DecisionKind::Attach(tids) => ("attach", format!("attach to the Test of {}: {applied}", ids(tids))),
            DecisionKind::NewTest => (
                "new test",
                match by_id.get(&id) {
                    Some(document) => {
                        let others: Vec<String> = document
                            .tests
                            .iter()
                            .map(|t| {
                                let with: Vec<i64> = plan
                                    .documents
                                    .iter()
                                    .filter(|d| d.id != id && d.tests.contains(t))
                                    .map(|d| d.id)
                                    .collect();
                                if with.is_empty() {
                                    format!("T{t} (alone)")
                                } else {
                                    format!("T{t} (with {})", ids(&with))
                                }
                            })
                            .collect();
                        let how = if applied.is_empty() {
                            "no old Test has its labels".to_owned()
                        } else {
                            applied.clone()
                        };
                        format!("new test: on {}; {how}", others.join(", "))
                    }
                    None => format!(
                        "new test: not imported ({})",
                        gone.get(&id).map_or("no file", |(why, _)| why.as_str())
                    ),
                },
            ),
            DecisionKind::DuplicateOf(of) => match by_id.get(&id) {
                Some(document) if !document.published => (
                    "duplicate",
                    format!("duplicate of D{of}: Hidden on {}", tests_list(&document.tests)),
                ),
                Some(_) => ("duplicate", format!("duplicate of D{of}: imported, but not Hidden")),
                None if gone.get(&id).is_some_and(|(why, _)| why.starts_with("is now the PDF")) => {
                    ("Word+PDF pair", format!("duplicate of D{of}: now D{of}'s supplied PDF"))
                }
                None => ("duplicate", format!("duplicate of D{of}: not imported")),
            },
        };
        *decision_counts.entry(kind).or_default() += 1;
        decision_lines.push(format!("  D{id}: {what} (why: {})", decision.reason));
    }

    // -------------------------------------------------------- the summary
    let mut summary = String::new();
    let s = &mut summary;
    let _ = writeln!(s, "Reconciliation:");
    let _ = writeln!(
        s,
        "  Source files: {} — {imported} imported, {hidden} imported Hidden, {pdfs} imported as supplied PDFs, \
         {dropped} dropped, {failed} failed, {unaccounted} not accounted for.",
        plan.inventory.len(),
    );
    let hidden_by = |prefix: &str| {
        plan.documents
            .iter()
            .filter(|d| !d.published && d.note.as_deref().is_some_and(|n| n.to_lowercase().starts_with(prefix)))
            .count()
    };
    let _ = writeln!(
        s,
        "  Hidden Documents: {} — duplicates {}, broken {}, not a test {}.",
        plan.documents.iter().filter(|d| !d.published).count(),
        hidden_by(DUPLICATE_NOTE),
        hidden_by(BROKEN_NOTE),
        hidden_by(NOT_A_TEST_NOTE),
    );
    let _ = writeln!(
        s,
        "  Hidden Tests (publish rule): {}.",
        plan.tests.iter().filter(|t| !t.published).count()
    );
    let _ = writeln!(
        s,
        "  Old Tests: {} — {whole} kept with all their Documents, {moved} kept with some Documents elsewhere, \
         {succeeded} not kept (their Documents' Tests succeed them), {incomplete} with Documents missing.",
        plan.legacy_tests.len(),
    );
    let decisions: Vec<String> = decision_counts.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let _ = writeln!(
        s,
        "  Attach decisions: {} — {}.",
        plan.decisions.len(),
        if decisions.is_empty() {
            "none".to_owned()
        } else {
            decisions.join(", ")
        }
    );
    let supplied = plan.documents.iter().filter(|d| d.pdf.is_some()).count();
    let objects = plan.documents.len() + supplied;
    let preparing = plan.documents.iter().filter(|d| d.pdf_status == "preparing").count();
    match loaded {
        Loaded::DryRun => {
            let _ = writeln!(
                s,
                "  Garage: not checked (dry run); {} would be stored ({} Originals, {}).",
                plural(objects, "object"),
                plan.documents.len(),
                plural(supplied, "supplied PDF"),
            );
            let _ = writeln!(s, "  PDF conversions: {preparing} would be queued.");
        }
        Loaded::Written {
            stored,
            held,
            extra,
            queued,
        } => {
            let _ = writeln!(
                s,
                "  Garage: {} of {objects} objects stored and read back, each matching its source's size and \
                 SHA-256; Garage holds {held} of them and {} else.",
                stored.len(),
                plural(extra.len(), "object"),
            );
            if stored.len() != objects || *held != objects {
                problems.push(format!(
                    "Garage holds {held} of the {objects} planned objects ({} stored)",
                    stored.len()
                ));
            }
            for key in extra {
                problems.push(format!("Garage holds {key}, which the archive does not refer to"));
            }
            let _ = writeln!(
                s,
                "  PDF conversions: {queued} queued, one for each of the {preparing} Originals being prepared."
            );
            if *queued as usize != preparing {
                problems.push(format!(
                    "{queued} PDF conversions queued for {preparing} Originals being prepared"
                ));
            }
        }
    }
    for warning in &plan.warnings {
        problems.push(warning.clone());
    }
    if problems.is_empty() {
        let _ = writeln!(s, "  Problems: none.");
    } else {
        let _ = writeln!(s, "  Problems ({}):", problems.len());
        for problem in &problems {
            let _ = writeln!(s, "    {problem}");
        }
    }

    let mut full = summary.clone();
    let _ = writeln!(full, "\nSource files ({}):", lines.len());
    for line in lines {
        let _ = writeln!(full, "{line}");
    }
    let _ = writeln!(full, "\nOld Tests ({}):", test_lines.len());
    for line in test_lines {
        let _ = writeln!(full, "{line}");
    }
    let _ = writeln!(full, "\nAttach decisions ({}):", decision_lines.len());
    for line in decision_lines {
        let _ = writeln!(full, "{line}");
    }
    let flagged: Vec<&PlannedDocument> = plan.documents.iter().filter(|d| d.needs_review.is_some()).collect();
    let _ = writeln!(full, "\nNeeds review ({}):", flagged.len());
    for document in flagged {
        let comment = document
            .needs_review
            .as_deref()
            .unwrap_or_default()
            .replace('\n', " / ");
        let _ = writeln!(full, "  D{}: {comment}", document.id);
    }
    Report { summary, full }
}
