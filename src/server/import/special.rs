//! The import's special cases (spec, "Migration"; issues #10, #13, #31):
//! passes [`super::plan::Plan::build`] runs between its core steps.
//!
//! - [`prepare_files`], before the Documents are made:
//!   - `.lnk` shortcuts are dropped.
//!   - The bundle zips named in unpacked.csv are dropped, and the Documents
//!     unpacked from them join the docs export, with any PDF that came with
//!     them as their supplied PDF.
//!   - An attach decision's "duplicate of" a Word or PowerPoint file that is
//!     a PDF becomes that Original's supplied PDF: one Document, not two.
//! - [`hide`]: duplicates (`Duplicate Of`, and the attach decisions'),
//!   broken files and non-test files are imported Hidden, with a note.
//! - [`attach`]: an `attach` decision puts its Document on the Test holding
//!   the Documents it names.
//! - [`needs_review`]: the AI's flags (review-core.csv), unpacked.csv's
//!   "needs review" notes, and the attach-checks' mislabels from #10.
//! - [`keep_new_tests_apart`], after grouping: a `new test` decision keeps
//!   its Document off the old Test its labels match.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{Context, bail};

use super::{
    plan::{Groups, PlannedDocument, SkipOutcome, Skipped, SuppliedPdf},
    sources::{AiColumns, Decision, DecisionKind, LabelRow, ReviewFlag, SourceFile, UnpackedDocument},
    tidy::TestLabels,
};
use crate::server::pdf;

/// Drop the shortcuts and bundle zips, add the unpacked Documents, and pair
/// PDFs with their Word Originals. Returns the supplied PDFs by the `tid`
/// of their Original.
pub fn prepare_files(
    labels: &mut BTreeMap<i64, Vec<LabelRow>>,
    files: &mut BTreeMap<i64, SourceFile>,
    unpacked: Vec<UnpackedDocument>,
    unpacked_files: Vec<SourceFile>,
    decisions: &[Decision],
    skipped: &mut Vec<Skipped>,
) -> anyhow::Result<BTreeMap<i64, SuppliedPdf>> {
    let shortcuts: Vec<i64> = files
        .iter()
        .filter(|(_, file)| file.extension == "lnk")
        .map(|(tid, _)| *tid)
        .collect();
    for tid in shortcuts {
        let file = files.remove(&tid).expect("listed just now");
        labels.remove(&tid);
        skipped.push(Skipped {
            what: file.shown,
            outcome: SkipOutcome::Dropped,
            reason: "a Windows shortcut (.lnk), not a Document".to_owned(),
        });
    }

    let mut bundles: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for document in &unpacked {
        bundles.entry(document.bundle).or_default().push(document.tid);
    }
    for (bundle, tids) in &bundles {
        labels.remove(bundle);
        let what = files
            .remove(bundle)
            .map_or_else(|| format!("File Number {bundle}"), |f| f.shown);
        skipped.push(match what.starts_with("File Number") {
            false => Skipped {
                what,
                outcome: SkipOutcome::Dropped,
                reason: format!(
                    "a bundle zip, not imported itself: the {} in it are imported as {}",
                    plural(tids.len(), "Document"),
                    ids(tids)
                ),
            },
            true => Skipped {
                what,
                outcome: SkipOutcome::Missing,
                reason: "unpacked.csv names it as a bundle zip, but there is no such file in the docs export"
                    .to_owned(),
            },
        });
    }

    let mut pdfs = BTreeMap::new();
    let mut used: HashSet<PathBuf> = HashSet::new();
    for document in unpacked {
        if let Some(other) = files.get(&document.tid) {
            bail!(
                "D{} is both {:?} in the docs export and {:?} in unpacked.csv",
                document.tid,
                other.shown,
                document.original.shown
            );
        }
        used.insert(document.original.path.clone());
        if let Some(pdf) = document.pdf {
            used.insert(pdf.path.clone());
            pdfs.insert(
                document.tid,
                SuppliedPdf {
                    file: pdf,
                    former_tid: None,
                },
            );
        }
        files.insert(document.tid, document.original);
    }
    for file in unpacked_files {
        if !used.contains(&file.path) {
            skipped.push(Skipped {
                what: file.shown,
                outcome: SkipOutcome::Failed,
                reason: "in the unpacked directory, but unpacked.csv does not list it".to_owned(),
            });
        }
    }

    for decision in decisions {
        let DecisionKind::DuplicateOf(original) = decision.kind else {
            continue;
        };
        let (Some(copy), Some(word)) = (files.get(&decision.file_number), files.get(&original)) else {
            continue;
        };
        if extension(copy)? == "pdf" && pdf::office(&extension(word)?) && !pdfs.contains_key(&original) {
            let file = files.remove(&decision.file_number).expect("found just now");
            labels.remove(&decision.file_number);
            pdfs.insert(
                original,
                SuppliedPdf {
                    file,
                    former_tid: Some(decision.file_number),
                },
            );
        }
    }
    Ok(pdfs)
}

/// A file's extension, from its contents when its name has none.
fn extension(file: &SourceFile) -> anyhow::Result<String> {
    if !file.extension.is_empty() {
        return Ok(file.extension.clone());
    }
    Ok(pdf::sniff::sniff(&file.path)
        .with_context(|| format!("reading {}", file.path.display()))?
        .unwrap_or_default()
        .to_owned())
}

fn ai_columns(document: &PlannedDocument) -> impl Iterator<Item = &AiColumns> {
    document.rows.iter().filter_map(|r| r.ai.as_ref())
}

/// What a Hidden Document's note starts with, by why it is Hidden.
pub const DUPLICATE_NOTE: &str = "duplicate of ";
pub const BROKEN_NOTE: &str = "broken";
pub const NOT_A_TEST_NOTE: &str = "not a test: ";

/// Hide the duplicates, the broken files and the files that are not tests.
pub fn hide(documents: &mut [PlannedDocument], decisions: &[Decision], warnings: &mut Vec<String>) {
    let imported: HashSet<i64> = documents.iter().map(|d| d.id).collect();
    let decided: HashMap<i64, i64> = decisions
        .iter()
        .filter_map(|d| match d.kind {
            DecisionKind::DuplicateOf(of) => Some((d.file_number, of)),
            _ => None,
        })
        .collect();
    for document in documents.iter_mut() {
        let mut duplicate_of = decided.get(&document.id).copied();
        for ai in ai_columns(document).filter(|ai| !ai.duplicate_of.is_empty()) {
            match ai.duplicate_of.parse::<i64>() {
                Ok(of) => duplicate_of = duplicate_of.or(Some(of)),
                Err(_) => warnings.push(format!(
                    "D{} has Duplicate Of {:?}, which is not a tid; it is not Hidden as a duplicate",
                    document.id, ai.duplicate_of
                )),
            }
        }
        let broken = ai_columns(document)
            .map(|ai| ai.note.trim())
            .find(|note| note.to_lowercase().starts_with(BROKEN_NOTE))
            .map(str::to_owned);
        let not_a_test = ai_columns(document)
            .any(|ai| ai.hidden.eq_ignore_ascii_case("yes"))
            .then(|| {
                let note = ai_columns(document)
                    .map(|ai| ai.note.trim())
                    .find(|n| !n.is_empty())
                    .unwrap_or("the AI labels say to hide it");
                format!("{NOT_A_TEST_NOTE}{note}")
            });

        if let Some(of) = duplicate_of {
            if !imported.contains(&of) {
                warnings.push(format!(
                    "D{} is a duplicate of D{of}, which is not imported",
                    document.id
                ));
            }
            document.hide(format!("{DUPLICATE_NOTE}{of}"));
        }
        if let Some(note) = broken {
            document.hide(note);
        }
        if let Some(note) = not_a_test {
            document.hide(note);
        }
    }
}

/// Apply the `attach` decisions. Returns what each decision did (empty for
/// the other kinds, which other passes apply); one that cannot be applied
/// is also a warning.
pub fn attach(documents: &mut [PlannedDocument], decisions: &[Decision], warnings: &mut Vec<String>) -> Vec<String> {
    let index: HashMap<i64, usize> = documents.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
    decisions
        .iter()
        .map(|decision| {
            let DecisionKind::Attach(tids) = &decision.kind else {
                return String::new();
            };
            let id = decision.file_number;
            let applied = (|| {
                let &i = index.get(&id).ok_or(format!("D{id} is not imported"))?;
                let mut shared: Option<Vec<TestLabels>> = None;
                for tid in tids {
                    let &j = index.get(tid).ok_or(format!("D{tid} is not imported"))?;
                    let labels = &documents[j].test_labels;
                    shared = Some(match shared {
                        None => labels.clone(),
                        Some(shared) => shared.into_iter().filter(|l| labels.contains(l)).collect(),
                    });
                }
                let shared = shared.unwrap_or_default();
                let target = shared.first().ok_or(format!("{} share no Test", ids(tids)))?.clone();
                let document = &mut documents[i];
                Ok(if shared.iter().any(|l| document.test_labels.contains(l)) {
                    format!("already on the Test of {} (same labels)", ids(tids))
                } else if let [was] = &document.test_labels[..] {
                    let was = was.describe();
                    document.test_labels = vec![target];
                    format!("moved onto the Test of {}; its own labels said {was}", ids(tids))
                } else {
                    document.test_labels.push(target);
                    format!("added to the Test of {}, as well as its own", ids(tids))
                })
            })();
            applied.unwrap_or_else(|why: String| {
                warnings.push(format!("The attach decision for D{id} was not applied: {why}"));
                format!("not applied: {why}")
            })
        })
        .collect()
}

/// The attach-checks' mislabels (#10, "Flagged Needs review on import"): a
/// Document whose labels are known to be wrong, and why.
fn mislabel(document: &PlannedDocument) -> Option<&'static str> {
    Some(match document.id {
        3974 | 3976 | 3978 => {
            "Attach-check: mainly Theta Individual Solutions, with a combined Individual and Team answer key, \
             not Team Answers."
        }
        3915 | 3916 => "Attach-check: a 1998 \"Mu Test\" pair (3915 Solutions, 3916 Test), not Applications.",
        6510 => "Attach-check: page 1 is a 1997-titled grid inside a 1998 file.",
        5651..=5678 if document.kind == "Condensed Test" => {
            "Attach-check: labeled Condensed Test, but byte-identical to a file labeled Test."
        }
        5991 | 5992 => {
            "Attach-check: the 2005 Open Hustle Test mixes the Algebra II (5991) and Calculus (5992) Hustles; \
             it should be split per subject."
        }
        3657 => "Attach-check: this answer key also covers another Test, the 1992 Calculus Individual.",
        3688 => "Attach-check: this answer key also covers the other 1996 Calculus-division Tests (its other columns).",
        _ => return None,
    })
}

/// Flag Needs review: the AI's flags, the AI's own "needs review" notes,
/// and the mislabels.
pub fn needs_review(documents: &mut [PlannedDocument], review: &[ReviewFlag], warnings: &mut Vec<String>) {
    let index: HashMap<i64, usize> = documents.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
    for flag in review {
        match index.get(&flag.file_number) {
            Some(&i) => documents[i].flag(&format!("AI labels flagged: {}", flag.flags)),
            None => warnings.push(format!(
                "review-core.csv flags D{} ({}), which is not imported",
                flag.file_number, flag.flags
            )),
        }
    }
    for document in documents.iter_mut() {
        let notes: Vec<String> = ai_columns(document)
            .map(|ai| ai.note.trim())
            .filter(|note| note.to_lowercase().starts_with("needs review"))
            .map(|note| format!("AI labels: {note}"))
            .collect();
        for note in notes {
            document.flag(&note);
        }
        if let Some(reason) = mislabel(document) {
            document.flag(reason);
        }
    }
}

/// The Documents the attach-checks decided are a `new test`.
pub fn new_tests(decisions: &[Decision]) -> HashSet<i64> {
    decisions
        .iter()
        .filter(|d| d.kind == DecisionKind::NewTest)
        .map(|d| d.file_number)
        .collect()
}

/// A Document decided to be a `new test` does not join the old Test its
/// labels happen to match (one holding Documents of the old site). It
/// leaves that Test; one that is on no other Test becomes, with any other
/// `new test` Documents of the same labels, a Test of their own. Returns
/// what was done, by Document ID.
pub fn keep_new_tests_apart(
    groups: &mut Groups,
    documents: &[PlannedDocument],
    new_tests: &HashSet<i64>,
    legacy_tids: &HashSet<i64>,
) -> HashMap<i64, String> {
    let mut on_tests: HashMap<usize, usize> = HashMap::new();
    for (_, members) in groups.iter() {
        for &i in members {
            *on_tests.entry(i).or_default() += 1;
        }
    }
    let mut done = HashMap::new();
    let mut apart = Vec::new();
    for (labels, members) in groups.iter_mut() {
        let (new, rest): (BTreeSet<usize>, BTreeSet<usize>) =
            members.iter().partition(|&&i| new_tests.contains(&documents[i].id));
        let old: Vec<i64> = rest
            .iter()
            .map(|&i| documents[i].id)
            .filter(|id| legacy_tids.contains(id))
            .collect();
        if new.is_empty() || old.is_empty() {
            continue;
        }
        let (leave, own): (BTreeSet<usize>, BTreeSet<usize>) = new.into_iter().partition(|i| on_tests[i] > 1);
        for &i in &leave {
            done.insert(
                documents[i].id,
                format!("kept off the old Test of {} ({})", ids(&old), labels.describe()),
            );
        }
        for &i in &own {
            done.insert(
                documents[i].id,
                format!(
                    "a Test of its own, apart from the old Test of {} with the same labels",
                    ids(&old)
                ),
            );
        }
        *members = rest;
        if !own.is_empty() {
            apart.push((labels.clone(), own));
        }
    }
    groups.extend(apart);
    // Stable, so each split-off Test comes right after the one it left.
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    done
}

/// `noun`, with an s unless there is one.
pub fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `D1, D2 and D3`.
pub fn ids(tids: &[i64]) -> String {
    let names: Vec<String> = tids.iter().map(|t| format!("D{t}")).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}
