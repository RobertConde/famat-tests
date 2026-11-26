//! `famat import`: build the archive from the old site's data (spec,
//! "Migration"; issues #10, #13, #30).
//!
//! ```text
//! famat import --sheet "test list - 000final.csv" --ai-labels data/labels/ai-labels.csv \
//!              --docs <docs export> --legacy-sql doralmao_famat.sql [--dry-run]
//! ```
//!
//! 1. [`sources`] reads the inputs as they are.
//! 2. [`plan::Plan::build`] tidies the values ([`tidy`]), groups Documents
//!    into Tests, keeps the legacy IDs and seeds the display orders. Nothing
//!    is written yet; `--dry-run` stops here and prints the summary.
//! 3. [`load`] refuses if an Admin has changed anything, streams every file
//!    into Garage and checks Garage's copy against it, then replaces the
//!    archive tables in one transaction and removes stale Garage objects.
//!
//! #31 adds its passes to the plan (see [`plan`]) and its reconciliation
//! report to the summary; [`plan::Plan::skipped`] lists what was not
//! imported.

mod csv;
mod load;
mod plan;
mod sources;
// Old search URLs are translated with the import's tidying (`super::legacy`).
pub(crate) mod tidy;

pub(crate) use sources::RawLabels;

use std::{collections::BTreeMap, fmt::Write, path::PathBuf};

use anyhow::{Context, bail};

use super::{config::Config, db, storage::Storage};
use plan::Plan;
use sources::{Inputs, LabelSource, Sources};

pub const USAGE: &str = "\
  import         Build the archive from the old site's data, replacing it,
                 until an Admin has changed anything. Options:
                   --sheet FILE        the students' sheet (test list - 000final.csv)
                   --ai-labels FILE    data/labels/ai-labels.csv
                   --docs DIR          the docs export (files named \"<tid> <name>\")
                   --legacy-sql FILE   doralmao_famat.sql
                   --dry-run           only read the inputs and print the summary";

/// `famat import …`: the options after `import`.
pub fn main(args: &[&str]) -> anyhow::Result<String> {
    let (inputs, dry_run) = parse_args(args)?;
    let sources = Sources::read(&inputs)?;
    let read = describe_sources(&sources);
    let plan = Plan::build(sources)?;
    let mut summary = read + &describe_plan(&plan);
    if dry_run {
        summary.push_str("Dry run: nothing was written.\n");
        return Ok(summary);
    }

    let config = Config::from_env()?;
    let stored = super::cli::runtime().block_on(async {
        let db = db::connect(&config.database).await?;
        load::ensure_not_frozen(&db).await?;
        let storage = Storage::new(&config.storage);
        let stored = load::store_files(&storage, &plan).await?;
        load::write(&db, &plan, &stored).await?;
        let swept = load::sweep(&storage, &plan).await?;
        anyhow::Ok((stored, swept))
    })?;
    let (stored, swept) = stored;
    let bytes: i64 = stored.values().map(|s| s.byte_size).sum();
    writeln!(
        summary,
        "Stored {} files ({:.1} MB) in Garage, each checked against its source; removed {swept} stale {}.",
        stored.len(),
        bytes as f64 / 1e6,
        plural(swept, "object"),
    )?;
    Ok(summary)
}

fn parse_args(args: &[&str]) -> anyhow::Result<(Inputs, bool)> {
    let (mut sheet, mut ai_labels, mut docs, mut legacy_sql, mut dry_run) = (None, None, None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .map(PathBuf::from)
                .with_context(|| format!("{name} needs a value"))
        };
        match *arg {
            "--sheet" => sheet = Some(value(arg)?),
            "--ai-labels" => ai_labels = Some(value(arg)?),
            "--docs" => docs = Some(value(arg)?),
            "--legacy-sql" => legacy_sql = Some(value(arg)?),
            "--dry-run" => dry_run = true,
            other => bail!("unexpected argument {other:?}"),
        }
    }
    let required = |value: Option<PathBuf>, name: &str| value.with_context(|| format!("{name} is required"));
    Ok((
        Inputs {
            sheet: required(sheet, "--sheet")?,
            ai_labels: required(ai_labels, "--ai-labels")?,
            docs: required(docs, "--docs")?,
            legacy_sql: required(legacy_sql, "--legacy-sql")?,
        },
        dry_run,
    ))
}

// ------------------------------------------------------------------ summary

fn describe_sources(sources: &Sources) -> String {
    let rows = |source| sources.rows.iter().filter(move |r| r.source == source);
    let ai_documents = rows(LabelSource::Ai)
        .map(|r| r.file_number)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    format!(
        "Read {} sheet rows, {} AI label rows ({ai_documents} Documents), {} files in the docs export \
         and {} old Tests.\n",
        rows(LabelSource::Sheet).count(),
        rows(LabelSource::Ai).count(),
        sources.files.len(),
        sources.legacy_tests.len(),
    )
}

fn describe_plan(plan: &Plan) -> String {
    let mut out = String::new();
    let by_source = |source| plan.documents.iter().filter(|d| d.label_source == source).count();
    let _ = writeln!(
        out,
        "Documents: {} ({} labelled by the sheet, {} by the AI); {} of unknown kind, imported as Other.",
        plan.documents.len(),
        by_source(LabelSource::Sheet),
        by_source(LabelSource::Ai),
        plan.documents.iter().filter(|d| !d.kind_known).count(),
    );
    let legacy = plan.tests.iter().filter(|t| t.legacy).count();
    let new: Vec<i64> = plan.tests.iter().filter(|t| !t.legacy).map(|t| t.id).collect();
    let _ = writeln!(
        out,
        "Tests: {} ({legacy} keep their old test_id, {} new{}); {} Hidden by the publish rule.",
        plan.tests.len(),
        new.len(),
        match (new.first(), new.last()) {
            (Some(first), Some(last)) => format!(", T{first}–T{last}"),
            _ => String::new(),
        },
        plan.tests.iter().filter(|t| !t.published).count(),
    );
    if !plan.unclaimed_legacy_ids.is_empty() {
        let ids: Vec<String> = plan.unclaimed_legacy_ids.iter().map(|id| format!("T{id}")).collect();
        let _ = writeln!(
            out,
            "Old Tests whose test_id no Test kept ({}): {}.",
            ids.len(),
            ids.join(", ")
        );
    }
    let attached = plan.documents.iter().filter(|d| d.tests.len() > 1).count();
    let _ = writeln!(out, "Documents on more than one Test: {attached}.");

    let _ = writeln!(out, "Facet values, in display order:");
    for (facet, values) in &plan.facet_values {
        let _ = writeln!(out, "  {} ({}): {}", facet.label(), values.len(), values.join(", "));
    }

    let mut decisions: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (id, decision) in &plan.applications {
        decisions.entry(decision.describe()).or_default().push(*id);
    }
    if !decisions.is_empty() {
        let _ = writeln!(out, "Calculus/Mu Applications:");
        for (decision, ids) in &decisions {
            let _ = writeln!(out, "  {decision}: {}", id_list(ids));
        }
    }
    let review = plan.documents.iter().filter(|d| d.needs_review.is_some()).count();
    let _ = writeln!(out, "Needs review: {review} {}.", plural(review, "Document"));

    // What #31 will act on, so a dry run shows its size.
    let ai_flag = |pick: fn(&sources::AiColumns) -> bool| {
        plan.documents
            .iter()
            .filter(|d| d.rows.iter().filter_map(|r| r.ai.as_ref()).any(pick))
            .count()
    };
    let _ = writeln!(
        out,
        "Left for #31: the AI marks {} Documents Hidden and {} as duplicates; {} have a low-confidence AI row.",
        ai_flag(|ai| ai.hidden.eq_ignore_ascii_case("yes")),
        ai_flag(|ai| !ai.duplicate_of.is_empty()),
        ai_flag(|ai| ai.confidence.eq_ignore_ascii_case("low")),
    );

    if !plan.skipped.is_empty() {
        let _ = writeln!(out, "Not imported ({}):", plan.skipped.len());
        for skipped in &plan.skipped {
            let _ = writeln!(out, "  {}: {}", skipped.what, skipped.reason);
        }
    }
    out
}

/// `noun`, with an s unless there is one.
fn plural(n: usize, noun: &str) -> String {
    if n == 1 { noun.to_owned() } else { format!("{noun}s") }
}

/// `D1, D2, …`, shortened past 30.
fn id_list(ids: &[i64]) -> String {
    let mut list: Vec<String> = ids.iter().take(30).map(|id| format!("D{id}")).collect();
    if ids.len() > 30 {
        list.push(format!("… ({} in all)", ids.len()));
    }
    list.join(", ")
}
