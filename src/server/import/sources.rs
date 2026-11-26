//! Reading the import's inputs, exactly as they are: the students' sheet,
//! the AI labels, the docs export and the legacy SQL dump, plus the
//! Documents unpacked from bundle zips, the attach decisions and the AI's
//! review flags. Nothing is tidied here ([`super::tidy`] does that).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use super::csv;

/// Where the inputs are.
#[derive(Clone, Debug)]
pub struct Inputs {
    /// `test list - 000final.csv`
    pub sheet: PathBuf,
    /// `data/labels/ai-labels.csv`
    pub ai_labels: PathBuf,
    /// The docs export: files named `<tid> <name>`.
    pub docs: PathBuf,
    /// `doralmao_famat.sql`, for each old Test's `test_id`.
    pub legacy_sql: PathBuf,
    /// `data/labels/unpacked.csv`: the Documents unpacked from bundle zips.
    pub unpacked: PathBuf,
    /// `data/unpacked/`, which unpacked.csv's paths are relative to.
    pub unpacked_dir: PathBuf,
    /// `data/labels/attach-{1,2}.out.csv`: the attach decisions.
    pub attach: Vec<PathBuf>,
    /// `data/labels/review-core.csv`: the Documents the AI flagged.
    pub review: PathBuf,
}

/// Everything read from the inputs.
pub struct Sources {
    /// Every labelled row, sheet rows first, each in file order.
    pub rows: Vec<LabelRow>,
    /// The docs export's files, by `tid`.
    pub files: BTreeMap<i64, SourceFile>,
    /// Names in the docs export that are not `<tid> <name>` files.
    pub stray_names: Vec<String>,
    /// The old site's Tests.
    pub legacy_tests: Vec<LegacyTest>,
    /// The Documents unpacked from bundle zips (their labels are in `rows`).
    pub unpacked: Vec<UnpackedDocument>,
    /// Every file under the unpacked directory, listed or not.
    pub unpacked_files: Vec<SourceFile>,
    /// The attach decisions, in file order.
    pub decisions: Vec<Decision>,
    /// The Documents the AI flagged for review, with their flags.
    pub review: Vec<ReviewFlag>,
}

impl Sources {
    pub fn read(inputs: &Inputs) -> anyhow::Result<Self> {
        let mut rows = read_sheet(&inputs.sheet).with_context(|| format!("reading {}", inputs.sheet.display()))?;
        rows.extend(
            read_ai_labels(&inputs.ai_labels).with_context(|| format!("reading {}", inputs.ai_labels.display()))?,
        );
        let (files, stray_names) =
            read_docs(&inputs.docs).with_context(|| format!("reading {}", inputs.docs.display()))?;
        let legacy_tests =
            read_legacy_sql(&inputs.legacy_sql).with_context(|| format!("reading {}", inputs.legacy_sql.display()))?;
        let (unpacked_rows, unpacked) = read_unpacked(&inputs.unpacked, &inputs.unpacked_dir)
            .with_context(|| format!("reading {}", inputs.unpacked.display()))?;
        rows.extend(unpacked_rows);
        let unpacked_files = list_unpacked(&inputs.unpacked_dir)
            .with_context(|| format!("reading {}", inputs.unpacked_dir.display()))?;
        let mut decisions = Vec::new();
        for path in &inputs.attach {
            decisions.extend(read_decisions(path).with_context(|| format!("reading {}", path.display()))?);
        }
        let review = read_review(&inputs.review).with_context(|| format!("reading {}", inputs.review.display()))?;
        Ok(Self {
            rows,
            files,
            stray_names,
            legacy_tests,
            unpacked,
            unpacked_files,
            decisions,
            review,
        })
    }
}

// ------------------------------------------------------------------ labels

/// Where a Document's labels came from (`label_source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LabelSource {
    Sheet,
    Ai,
}

impl LabelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sheet => "sheet",
            Self::Ai => "ai",
        }
    }
}

/// A row's label columns, untouched.
#[derive(Clone, Debug, Default)]
pub struct RawLabels {
    pub file_type: String,
    pub year: String,
    pub comp_type: String,
    pub comp_month: String,
    pub division: String,
    pub test: String,
    pub host: String,
}

/// One row of the sheet or of the AI labels: one Document's labels for one
/// Test. A Document the AI found on several Tests has a row for each.
#[derive(Clone, Debug)]
pub struct LabelRow {
    pub file_number: i64,
    pub source: LabelSource,
    /// The line of its file the row starts on, for messages.
    pub line: usize,
    pub labels: RawLabels,
    /// The AI's columns; `None` for a sheet row.
    pub ai: Option<AiColumns>,
}

/// What the AI labels add to a row.
#[derive(Clone, Debug, Default)]
pub struct AiColumns {
    pub confidence: String,
    /// What the AI read in the file, e.g. its title.
    pub evidence: String,
    pub note: String,
    /// The `tid` this Document duplicates, if any (#31 hides it).
    pub duplicate_of: String,
    /// `yes` when the AI said to import it Hidden (#31).
    pub hidden: String,
}

const LABEL_COLUMNS: [&str; 8] = [
    "File Number",
    "File Type",
    "Year",
    "Comp Type",
    "Comp Month",
    "Division",
    "Test",
    "Host",
];

/// Column positions by header name.
struct Columns(Vec<(String, usize)>);

impl Columns {
    fn new(header: &csv::Record, required: &[&str]) -> anyhow::Result<Self> {
        let columns = Self(
            header
                .fields
                .iter()
                .enumerate()
                .map(|(i, name)| (name.trim().to_owned(), i))
                .collect(),
        );
        for name in required {
            if columns.index(name).is_none() {
                bail!("line {}: there is no {name:?} column", header.line);
            }
        }
        Ok(columns)
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, i)| *i)
    }

    fn get<'a>(&self, record: &'a csv::Record, name: &str) -> &'a str {
        self.index(name).map_or("", |i| record.get(i))
    }

    fn labels(&self, record: &csv::Record) -> RawLabels {
        RawLabels {
            file_type: self.get(record, "File Type").to_owned(),
            year: self.get(record, "Year").to_owned(),
            comp_type: self.get(record, "Comp Type").to_owned(),
            comp_month: self.get(record, "Comp Month").to_owned(),
            division: self.get(record, "Division").to_owned(),
            test: self.get(record, "Test").to_owned(),
            host: self.get(record, "Host").to_owned(),
        }
    }
}

fn read_csv(path: &Path) -> anyhow::Result<Vec<csv::Record>> {
    let text = std::fs::read_to_string(path)?;
    csv::parse(&text).map_err(anyhow::Error::msg)
}

/// The students' sheet. It starts with a directions row, then the header
/// row, then the sheet's dropdown lists and junk (rows whose File Number is
/// blank or not a number), which are skipped.
fn read_sheet(path: &Path) -> anyhow::Result<Vec<LabelRow>> {
    let records = read_csv(path)?;
    let Some(header) = records.iter().position(|r| r.get(0).trim() == "File Number") else {
        bail!("there is no header row starting with \"File Number\"");
    };
    let columns = Columns::new(&records[header], &LABEL_COLUMNS)?;
    Ok(records[header + 1..]
        .iter()
        .filter_map(|record| {
            let file_number = columns.get(record, "File Number").trim().parse().ok()?;
            Some(LabelRow {
                file_number,
                source: LabelSource::Sheet,
                line: record.line,
                labels: columns.labels(record),
                ai: None,
            })
        })
        .collect())
}

/// The AI labels: a header row, then one row per Document per Test.
fn read_ai_labels(path: &Path) -> anyhow::Result<Vec<LabelRow>> {
    let records = read_csv(path)?;
    let Some((header, rows)) = records.split_first() else {
        bail!("the file is empty");
    };
    let columns = Columns::new(header, &LABEL_COLUMNS)?;
    rows.iter()
        .filter(|record| !record.is_blank())
        .map(|record| {
            let raw = columns.get(record, "File Number").trim();
            let file_number = raw
                .parse()
                .with_context(|| format!("line {}: File Number {raw:?} is not a number", record.line))?;
            Ok(LabelRow {
                file_number,
                source: LabelSource::Ai,
                line: record.line,
                labels: columns.labels(record),
                ai: Some(AiColumns {
                    confidence: columns.get(record, "Confidence").to_owned(),
                    evidence: columns.get(record, "Evidence").to_owned(),
                    note: columns.get(record, "Note").to_owned(),
                    duplicate_of: columns.get(record, "Duplicate Of").trim().to_owned(),
                    hidden: columns.get(record, "Hidden").trim().to_owned(),
                }),
            })
        })
        .collect()
}

// -------------------------------------------------------------------- files

/// A file in the docs export, named `<tid> <name>`.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    /// The whole file name, e.g. `766 1999 Test.pdf`.
    pub file_name: String,
    /// How the reconciliation report names it: its name in the docs export,
    /// or `unpacked/<path>` for a file unpacked from a bundle.
    pub shown: String,
    /// The extension, lower case without the dot; empty when there is none.
    pub extension: String,
    pub byte_size: u64,
}

/// The docs export's `<tid> <name>` files, and every other name in it.
fn read_docs(dir: &Path) -> anyhow::Result<(BTreeMap<i64, SourceFile>, Vec<String>)> {
    let mut files = BTreeMap::new();
    let mut stray = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let metadata = entry.metadata()?;
        let tid = file_name
            .split_once(' ')
            .and_then(|(tid, _)| tid.parse::<i64>().ok())
            .filter(|_| metadata.is_file());
        let Some(tid) = tid else {
            stray.push(file_name);
            continue;
        };
        let file = SourceFile {
            path: entry.path(),
            extension: extension_of(&file_name),
            file_name: file_name.clone(),
            shown: file_name.clone(),
            byte_size: metadata.len(),
        };
        if let Some(other) = files.insert(tid, file) {
            bail!("two files have tid {tid}: {:?} and {file_name:?}", other.file_name);
        }
    }
    stray.sort();
    Ok((files, stray))
}

/// A file name's extension, lower case without the dot; empty when there is
/// none.
fn extension_of(file_name: &str) -> String {
    Path::new(file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        // "5544 Matrices & Vectors Solutions" has no extension, not a
        // " Solutions" one.
        .filter(|e| !e.contains(' '))
        .unwrap_or_default()
}

// ----------------------------------------------------------------- unpacked

/// A Document unpacked from a bundle zip of the docs export.
#[derive(Clone, Debug)]
pub struct UnpackedDocument {
    /// Its new `tid` (10364–10394 in the real data).
    pub tid: i64,
    /// The `tid` of the bundle zip it came out of.
    pub bundle: i64,
    pub original: SourceFile,
    /// The PDF that came with it in the bundle, if any.
    pub pdf: Option<SourceFile>,
}

/// `unpacked.csv`: like the AI labels (one row per Document per Test), plus
/// the bundle zip, and the paths of the Original and of any PDF that came
/// with it, relative to `dir`.
fn read_unpacked(path: &Path, dir: &Path) -> anyhow::Result<(Vec<LabelRow>, Vec<UnpackedDocument>)> {
    let records = read_csv(path)?;
    let Some((header, records)) = records.split_first() else {
        bail!("the file is empty");
    };
    let mut required = LABEL_COLUMNS.to_vec();
    required.extend(["Source Zip", "Original Path", "PDF Path"]);
    let columns = Columns::new(header, &required)?;
    let (mut rows, mut documents) = (Vec::new(), Vec::<UnpackedDocument>::new());
    for record in records.iter().filter(|r| !r.is_blank()) {
        let line = record.line;
        let raw = columns.get(record, "File Number").trim();
        let tid: i64 = raw
            .parse()
            .with_context(|| format!("line {line}: File Number {raw:?} is not a number"))?;
        let zip = columns.get(record, "Source Zip").trim();
        let bundle = zip
            .split_once(' ')
            .and_then(|(tid, _)| tid.parse().ok())
            .with_context(|| format!("line {line}: Source Zip {zip:?} does not start with a tid"))?;
        let file = |column: &str| -> anyhow::Result<Option<SourceFile>> {
            let relative = columns.get(record, column).trim();
            if relative.is_empty() {
                return Ok(None);
            }
            let path = dir.join(relative);
            let metadata =
                std::fs::metadata(&path).with_context(|| format!("line {line}: {column} {}", path.display()))?;
            let file_name = Path::new(relative)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(Some(SourceFile {
                extension: extension_of(&file_name),
                file_name,
                shown: format!("unpacked/{relative}"),
                path,
                byte_size: metadata.len(),
            }))
        };
        let original =
            file("Original Path")?.with_context(|| format!("line {line}: File Number {tid} has no Original Path"))?;
        let pdf = file("PDF Path")?;
        match documents.iter().find(|d| d.tid == tid) {
            Some(seen) if seen.original.path != original.path || seen.bundle != bundle => {
                bail!("line {line}: File Number {tid} has another file or bundle than on an earlier row")
            }
            Some(_) => {}
            None => documents.push(UnpackedDocument {
                tid,
                bundle,
                original,
                pdf,
            }),
        }
        rows.push(LabelRow {
            file_number: tid,
            source: LabelSource::Ai,
            line,
            labels: columns.labels(record),
            ai: Some(AiColumns {
                confidence: columns.get(record, "Confidence").to_owned(),
                evidence: columns.get(record, "Evidence").to_owned(),
                note: columns.get(record, "Note").to_owned(),
                duplicate_of: String::new(),
                hidden: String::new(),
            }),
        });
    }
    Ok((rows, documents))
}

/// Every file under the unpacked directory, by path.
fn list_unpacked(dir: &Path) -> anyhow::Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name == ".DS_Store" {
                continue;
            }
            let path = entry.path();
            let relative = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().into_owned();
            files.push(SourceFile {
                extension: extension_of(&file_name),
                file_name,
                shown: format!("unpacked/{relative}"),
                path,
                byte_size: metadata.len(),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

// -------------------------------------------------------- attach decisions

/// What the attach-checks decided about an uncatalogued file whose labels
/// matched an existing Test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionKind {
    /// It belongs on the Test that holds these Documents.
    Attach(Vec<i64>),
    /// It is not the Test its labels matched, but a Test of its own.
    NewTest,
    /// It duplicates this Document.
    DuplicateOf(i64),
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub file_number: i64,
    pub kind: DecisionKind,
    pub reason: String,
}

/// `attach-N.out.csv`: `File Number, Decision, Test Documents, Confidence,
/// Reason`, where Decision is `attach`, `new test` or `duplicate of <tid>`.
fn read_decisions(path: &Path) -> anyhow::Result<Vec<Decision>> {
    let records = read_csv(path)?;
    let Some((header, records)) = records.split_first() else {
        bail!("the file is empty");
    };
    let columns = Columns::new(header, &["File Number", "Decision", "Test Documents", "Reason"])?;
    records
        .iter()
        .filter(|r| !r.is_blank())
        .map(|record| {
            let line = record.line;
            let raw = columns.get(record, "File Number").trim();
            let file_number = raw
                .parse()
                .with_context(|| format!("line {line}: File Number {raw:?} is not a number"))?;
            let decision = columns.get(record, "Decision").trim();
            let kind = match decision.to_lowercase().as_str() {
                "attach" => {
                    let tids = columns
                        .get(record, "Test Documents")
                        .split_whitespace()
                        .map(|t| {
                            t.parse()
                                .with_context(|| format!("line {line}: Test Documents has {t:?}"))
                        })
                        .collect::<anyhow::Result<Vec<i64>>>()?;
                    if tids.is_empty() {
                        bail!("line {line}: attach, but no Test Documents");
                    }
                    DecisionKind::Attach(tids)
                }
                "new test" => DecisionKind::NewTest,
                other => match other.strip_prefix("duplicate of").map(str::trim) {
                    Some(tid) => DecisionKind::DuplicateOf(
                        tid.parse()
                            .with_context(|| format!("line {line}: {decision:?} names no tid"))?,
                    ),
                    None => bail!("line {line}: Decision {decision:?} is not attach, new test or duplicate of <tid>"),
                },
            };
            Ok(Decision {
                file_number,
                kind,
                reason: columns.get(record, "Reason").trim().to_owned(),
            })
        })
        .collect()
}

// ------------------------------------------------------------- review flags

/// A Document the AI labelling pass flagged for an Admin to check.
#[derive(Clone, Debug)]
pub struct ReviewFlag {
    pub file_number: i64,
    /// e.g. `low confidence | AI changed sheet: File Type 'Test'→'Solutions'`
    pub flags: String,
}

/// `review-core.csv`: `File Number, Flags, …`.
fn read_review(path: &Path) -> anyhow::Result<Vec<ReviewFlag>> {
    let records = read_csv(path)?;
    let Some((header, records)) = records.split_first() else {
        bail!("the file is empty");
    };
    let columns = Columns::new(header, &["File Number", "Flags"])?;
    records
        .iter()
        .filter(|r| !r.is_blank())
        .map(|record| {
            let raw = columns.get(record, "File Number").trim();
            Ok(ReviewFlag {
                file_number: raw
                    .parse()
                    .with_context(|| format!("line {}: File Number {raw:?} is not a number", record.line))?,
                flags: columns.get(record, "Flags").trim().to_owned(),
            })
        })
        .collect()
}

// --------------------------------------------------------------- legacy SQL

/// A Test of the old site: its `test_id`, its labels and the `tid`s of its
/// Documents.
#[derive(Clone, Debug)]
pub struct LegacyTest {
    pub id: i64,
    /// From `category` ("Invitationals - January" is Comp Type and Comp
    /// Month), `level`, `style`, `year` and `host`. No File Type.
    pub labels: RawLabels,
    pub tids: Vec<i64>,
}

/// The old site's `test_bank` rows, from its `INSERT INTO \`test_bank\``
/// statements: `test_id`, the label columns, and `documents` (a JSON list
/// of `{"tid": …, "doc_title": …}`).
fn read_legacy_sql(path: &Path) -> anyhow::Result<Vec<LegacyTest>> {
    let sql = std::fs::read_to_string(path)?;
    let mut tests = Vec::new();
    let mut rest = sql.as_str();
    while let Some(start) = rest.find("INSERT INTO `test_bank`") {
        rest = &rest[start..];
        let (columns, after) = column_list(rest)?;
        let at = |name: &str| columns.iter().position(|c| c == name);
        let (Some(id_at), Some(documents_at)) = (at("test_id"), at("documents")) else {
            bail!("an INSERT INTO `test_bank` has no test_id or documents column");
        };
        let Some(values) = after.trim_start().strip_prefix("VALUES") else {
            bail!("an INSERT INTO `test_bank` has no VALUES");
        };
        let mut parser = SqlValues { rest: values };
        loop {
            let row = parser.row()?;
            let id = match row.get(id_at) {
                Some(SqlValue::Number(n)) => *n,
                other => bail!("a test_bank row has test_id {other:?}"),
            };
            let tids = match row.get(documents_at) {
                Some(SqlValue::Text(json)) => document_tids(json).with_context(|| format!("test_id {id}"))?,
                Some(SqlValue::Null) => Vec::new(),
                other => bail!("test_id {id} has documents {other:?}"),
            };
            let text = |name: &str| match at(name).and_then(|i| row.get(i)) {
                Some(SqlValue::Text(text)) => text.clone(),
                Some(SqlValue::Number(n)) => n.to_string(),
                Some(SqlValue::Null) | None => String::new(),
            };
            let category = text("category");
            let (comp_type, comp_month) = category.split_once(" - ").unwrap_or((&category, ""));
            let labels = RawLabels {
                file_type: String::new(),
                year: text("year"),
                comp_type: comp_type.to_owned(),
                comp_month: comp_month.to_owned(),
                division: text("level"),
                test: text("style"),
                host: text("host"),
            };
            tests.push(LegacyTest { id, labels, tids });
            if !parser.more_rows()? {
                break;
            }
        }
        rest = parser.rest;
    }
    if tests.is_empty() {
        bail!("there are no INSERT INTO `test_bank` statements");
    }
    Ok(tests)
}

/// The column names of `INSERT INTO \`t\` (\`a\`, \`b\`) …`, and what follows.
fn column_list(insert: &str) -> anyhow::Result<(Vec<String>, &str)> {
    let open = insert.find('(').context("an INSERT has no column list")?;
    let close = insert[open..]
        .find(')')
        .context("an INSERT's column list is never closed")?
        + open;
    let columns = insert[open + 1..close]
        .split(',')
        .map(|c| c.trim().trim_matches('`').to_owned())
        .collect();
    Ok((columns, &insert[close + 1..]))
}

fn document_tids(json: &str) -> anyhow::Result<Vec<i64>> {
    #[derive(serde::Deserialize)]
    struct Entry {
        tid: i64,
    }
    let entries: Vec<Entry> = serde_json::from_str(json).context("reading its documents JSON")?;
    Ok(entries.into_iter().map(|e| e.tid).collect())
}

#[derive(Debug)]
enum SqlValue {
    Number(i64),
    Text(String),
    Null,
}

/// The `(…), (…);` after `VALUES`, as mysqldump writes it.
struct SqlValues<'a> {
    rest: &'a str,
}

impl<'a> SqlValues<'a> {
    fn row(&mut self) -> anyhow::Result<Vec<SqlValue>> {
        self.rest = self.rest.trim_start();
        self.rest = self.rest.strip_prefix('(').context("expected ( to start a row")?;
        let mut values = Vec::new();
        loop {
            values.push(self.value()?);
            self.rest = self.rest.trim_start();
            if let Some(rest) = self.rest.strip_prefix(',') {
                self.rest = rest;
            } else if let Some(rest) = self.rest.strip_prefix(')') {
                self.rest = rest;
                return Ok(values);
            } else {
                bail!("expected , or ) in a row");
            }
        }
    }

    /// After a row: `true` on `,` (another row follows), `false` on `;`.
    fn more_rows(&mut self) -> anyhow::Result<bool> {
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix(',') {
            self.rest = rest;
            Ok(true)
        } else if let Some(rest) = self.rest.strip_prefix(';') {
            self.rest = rest;
            Ok(false)
        } else {
            bail!("expected , or ; after a row")
        }
    }

    fn value(&mut self) -> anyhow::Result<SqlValue> {
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix('\'') {
            let mut text = String::new();
            let mut chars = rest.char_indices();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        let (_, escaped) = chars.next().context("a string ends in a backslash")?;
                        text.push(match escaped {
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            '0' => '\0',
                            c => c,
                        });
                    }
                    '\'' if rest[i + 1..].starts_with('\'') => {
                        chars.next();
                        text.push('\'');
                    }
                    '\'' => {
                        self.rest = &rest[i + 1..];
                        return Ok(SqlValue::Text(text));
                    }
                    c => text.push(c),
                }
            }
            bail!("a string is never closed");
        }
        let end = self
            .rest
            .find(|c: char| c == ',' || c == ')' || c.is_whitespace())
            .unwrap_or(self.rest.len());
        let (token, rest) = self.rest.split_at(end);
        self.rest = rest;
        if token.eq_ignore_ascii_case("NULL") {
            return Ok(SqlValue::Null);
        }
        match token.parse() {
            Ok(n) => Ok(SqlValue::Number(n)),
            // Other columns (e.g. decimals) are not needed; keep the text.
            Err(_) => Ok(SqlValue::Text(token.to_owned())),
        }
    }
}
