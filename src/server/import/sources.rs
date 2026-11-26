//! Reading the import's inputs, exactly as they are: the students' sheet,
//! the AI labels, the docs export and the legacy SQL dump. Nothing is tidied
//! here ([`super::tidy`] does that).

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
        Ok(Self {
            rows,
            files,
            stray_names,
            legacy_tests,
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
        let extension = Path::new(&file_name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            // "5544 Matrices & Vectors Solutions" has no extension, not a
            // " Solutions" one.
            .filter(|e| !e.contains(' '))
            .unwrap_or_default();
        let file = SourceFile {
            path: entry.path(),
            file_name: file_name.clone(),
            extension,
            byte_size: metadata.len(),
        };
        if let Some(other) = files.insert(tid, file) {
            bail!("two files have tid {tid}: {:?} and {file_name:?}", other.file_name);
        }
    }
    stray.sort();
    Ok((files, stray))
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
