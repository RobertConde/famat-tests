//! `famat import`: build the archive from the old site's data (spec,
//! "Migration"; issues #10, #13, #30).
//!
//! ```text
//! famat import --sheet "test list - 000final.csv" --ai-labels data/labels/ai-labels.csv \
//!              --docs <docs export> --legacy-sql doralmao_famat.sql \
//!              --unpacked data/labels/unpacked.csv --unpacked-dir data/unpacked \
//!              --attach data/labels/attach-1.out.csv --attach data/labels/attach-2.out.csv \
//!              --review data/labels/review-core.csv [--report import-report.txt] [--dry-run]
//! ```
//!
//! 1. [`sources`] reads the inputs as they are.
//! 2. [`plan::Plan::build`] tidies the values ([`tidy`]), applies the
//!    special cases ([`special`]), groups Documents into Tests, keeps the
//!    legacy IDs and seeds the display orders. Nothing is written yet;
//!    `--dry-run` stops here, with the summary and the reconciliation report.
//! 3. [`load`] refuses if an Admin has changed anything, streams every file
//!    into Garage and checks Garage's copy against it, then replaces the
//!    archive tables in one transaction and removes stale Garage objects.
//! 4. Backfill: every Original being prepared gets a PDF conversion job;
//!    a running server's workers pick them up.
//! 5. [`report`]: the reconciliation report, to stdout (its summary) and
//!    to the report file (all of it).

mod csv;
mod load;
mod plan;
mod report;
mod sources;
mod special;
// Old search URLs are translated with the import's tidying (`super::legacy`).
pub(crate) mod tidy;

pub(crate) use sources::RawLabels;

use std::{collections::BTreeMap, fmt::Write, path::PathBuf};

use anyhow::{Context, bail};

use super::{config::Config, db, pdf, storage::Storage};
use plan::Plan;
use report::Loaded;
use sources::{Inputs, LabelSource, Sources};

pub const USAGE: &str = "\
  import         Build the archive from the old site's data, replacing it,
                 until an Admin has changed anything. Options:
                   --sheet FILE        the students' sheet (test list - 000final.csv)
                   --ai-labels FILE    data/labels/ai-labels.csv
                   --docs DIR          the docs export (files named \"<tid> <name>\")
                   --legacy-sql FILE   doralmao_famat.sql
                   --unpacked FILE     data/labels/unpacked.csv
                   --unpacked-dir DIR  data/unpacked (unpacked.csv's paths start here)
                   --attach FILE       an attach decisions file (attach-N.out.csv);
                                       give it once per file
                   --review FILE       data/labels/review-core.csv
                   --report FILE       where to write the reconciliation report
                                       (default import-report.txt)
                   --dry-run           only read the inputs; print the summary and
                                       write the report, but not the archive";

/// The default `--report`.
const DEFAULT_REPORT: &str = "import-report.txt";

/// `famat import …`: the options after `import`.
pub fn main(args: &[&str]) -> anyhow::Result<String> {
    let options = parse_args(args)?;
    let sources = Sources::read(&options.inputs)?;
    let read = describe_sources(&sources);
    let plan = Plan::build(sources)?;
    let mut summary = read + &describe_plan(&plan);
    if options.dry_run {
        let report = report::reconcile(&plan, &Loaded::DryRun);
        write_report(&options.report, &report.full)?;
        summary.push_str(&report.summary);
        writeln!(summary, "Full report: {}", options.report.display())?;
        summary.push_str("Dry run: nothing was written to Postgres or Garage.\n");
        return Ok(summary);
    }

    let config = Config::from_env()?;
    let (stored, swept, held, extra, queued) = super::cli::runtime().block_on(async {
        let db = db::connect(&config.database).await?;
        load::ensure_not_frozen(&db).await?;
        let storage = Storage::new(&config.storage);
        let stored = load::store_files(&storage, &plan).await?;
        load::write(&db, &plan, &stored).await?;
        let swept = load::sweep(&storage, &plan).await?;
        let (held, extra) = load::check_garage(&storage, &plan).await?;
        let queued = pdf::enqueue_preparing(&db).await.context("queuing PDF conversions")?;
        anyhow::Ok((stored, swept, held, extra, queued))
    })?;
    let bytes: i64 = stored.values().map(|s| s.byte_size).sum();
    writeln!(
        summary,
        "Stored {} files ({:.1} MB) in Garage, each checked against its source; removed {swept} stale {}.",
        stored.len(),
        bytes as f64 / 1e6,
        plural(swept, "object"),
    )?;
    let report = report::reconcile(
        &plan,
        &Loaded::Written {
            stored: &stored,
            held,
            extra,
            queued,
        },
    );
    write_report(&options.report, &report.full)?;
    summary.push_str(&report.summary);
    writeln!(summary, "Full report: {}", options.report.display())?;
    Ok(summary)
}

fn write_report(path: &std::path::Path, report: &str) -> anyhow::Result<()> {
    std::fs::write(path, report).with_context(|| format!("writing the report to {}", path.display()))
}

struct Options {
    inputs: Inputs,
    report: PathBuf,
    dry_run: bool,
}

fn parse_args(args: &[&str]) -> anyhow::Result<Options> {
    let (mut sheet, mut ai_labels, mut docs, mut legacy_sql) = (None, None, None, None);
    let (mut unpacked, mut unpacked_dir, mut attach, mut review) = (None, None, Vec::new(), None);
    let (mut report, mut dry_run) = (None, false);
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
            "--unpacked" => unpacked = Some(value(arg)?),
            "--unpacked-dir" => unpacked_dir = Some(value(arg)?),
            "--attach" => attach.push(value(arg)?),
            "--review" => review = Some(value(arg)?),
            "--report" => report = Some(value(arg)?),
            "--dry-run" => dry_run = true,
            other => bail!("unexpected argument {other:?}"),
        }
    }
    let required = |value: Option<PathBuf>, name: &str| value.with_context(|| format!("{name} is required"));
    if attach.is_empty() {
        bail!("--attach is required (once per attach decisions file)");
    }
    Ok(Options {
        inputs: Inputs {
            sheet: required(sheet, "--sheet")?,
            ai_labels: required(ai_labels, "--ai-labels")?,
            docs: required(docs, "--docs")?,
            legacy_sql: required(legacy_sql, "--legacy-sql")?,
            unpacked: required(unpacked, "--unpacked")?,
            unpacked_dir: required(unpacked_dir, "--unpacked-dir")?,
            attach,
            review: required(review, "--review")?,
        },
        report: report.unwrap_or_else(|| PathBuf::from(DEFAULT_REPORT)),
        dry_run,
    })
}

// ------------------------------------------------------------------ summary

fn describe_sources(sources: &Sources) -> String {
    let rows = |source| sources.rows.iter().filter(move |r| r.source == source);
    let ai_documents = rows(LabelSource::Ai)
        .map(|r| r.file_number)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let bundles = sources
        .unpacked
        .iter()
        .map(|d| d.bundle)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    format!(
        "Read {} sheet rows, {} AI label rows ({ai_documents} Documents), {} files in the docs export \
         and {} old Tests.\n\
         Read {} unpacked Documents from {bundles} {} ({} files in the unpacked directory), {} attach \
         decisions and {} review flags.\n",
        rows(LabelSource::Sheet).count(),
        rows(LabelSource::Ai).count(),
        sources.files.len(),
        sources.legacy_tests.len(),
        sources.unpacked.len(),
        plural(bundles, "bundle zip"),
        sources.unpacked_files.len(),
        sources.decisions.len(),
        sources.review.len(),
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
    let low = plan
        .documents
        .iter()
        .filter(|d| {
            d.rows
                .iter()
                .filter_map(|r| r.ai.as_ref())
                .any(|ai| ai.confidence.eq_ignore_ascii_case("low"))
        })
        .count();
    let _ = writeln!(
        out,
        "Needs review: {review} {}; {low} have a low-confidence AI row.",
        plural(review, "Document")
    );
    let supplied = plan.documents.iter().filter(|d| d.pdf.is_some()).count();
    let _ = writeln!(
        out,
        "PDFs: {} Originals are PDFs, {supplied} have a supplied PDF, {} are to be converted, {} cannot be.",
        plan.documents.iter().filter(|d| d.extension == "pdf").count(),
        plan.documents.iter().filter(|d| d.pdf_status == "preparing").count(),
        plan.documents
            .iter()
            .filter(|d| d.pdf_status == "not_convertible")
            .count(),
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
