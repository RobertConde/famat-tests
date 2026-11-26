//! Download names, built from metadata at download time and never stored,
//! so Facet renames and merges apply at once (issue #6):
//!
//! - **Document:** `T{test id} D{doc id} {Year} {Month} {Competition} - {Host} - {Division/Level} {Test type} - {Document kind}[ ({label})].{ext}`
//! - **Per-Test zip:** `T{test id} {Year} {Month} {Competition} - {Host} - {Division/Level} {Test type}[ - {Kind group}].zip`
//! - **Bulk zip:** `FAMAT Test Bank - {N} tests[ - {Kind group}].zip`, with
//!   each Test's files in a folder named like its own zip of all Documents.
//!
//! - Unknown and not-applicable parts are left out, with their separators.
//! - Test type parentheses are dropped, and `/` becomes `-`.
//! - Characters Windows rejects are removed.
//! - Names are capped at about 150 characters by shortening the middle
//!   Facets (Month, Competition, Host, Division/Level, Test type), longest
//!   first, then the label. The IDs, Year, kind (or kind group) and
//!   extension are never shortened.
//!
//! A PDF and its Original share the base name and differ only by extension,
//! so the cap applies to the base name: [`MAX_BASE_CHARS`] leaves room for a
//! dot and a four-letter extension within 150.

/// The cap on a name without its extension, in characters.
const MAX_BASE_CHARS: usize = 150 - ".docx".len();

/// Shortening stops at this many characters per part.
const MIN_PART_CHARS: usize = 3;

/// What a Test contributes to a download name. Facets are `None` when
/// unknown or, for Month, not applicable.
pub struct TestParts<'a> {
    /// The Test it is downloaded from: a Document can belong to several.
    pub test_id: i64,
    pub year: Option<i16>,
    pub month: Option<&'a str>,
    pub competition: Option<&'a str>,
    pub host: Option<&'a str>,
    pub division_level: Option<&'a str>,
    pub test_type: Option<&'a str>,
}

/// What a Document's download name is made of.
pub struct NameParts<'a> {
    pub test: TestParts<'a>,
    pub document_id: i64,
    pub kind: &'a str,
    pub label: Option<&'a str>,
}

/// The name of one of a Document's files. `extension` has no dot and may be
/// empty, in which case the name has none.
pub fn file_name(parts: &NameParts<'_>, extension: &str) -> String {
    let ids = format!("T{} D{}", parts.test.test_id, parts.document_id);
    capped(Name::new(ids, &parts.test, Some(parts.kind), parts.label), extension)
}

/// The name of a zip of one Test's Documents. `group` is the kind group's
/// label, or `None` for all of them.
pub fn zip_name(test: &TestParts<'_>, group: Option<&str>) -> String {
    capped(Name::new(format!("T{}", test.test_id), test, group, None), "zip")
}

/// The folder a Test's files go in inside a bulk zip: its zip's name for all
/// Documents, without `.zip`.
pub fn folder_name(test: &TestParts<'_>) -> String {
    capped(Name::new(format!("T{}", test.test_id), test, None, None), "")
}

/// The name of a bulk zip of `tests` Tests. `group` is as for [`zip_name`].
pub fn bulk_zip_name(tests: usize, group: Option<&str>) -> String {
    let tests = if tests == 1 {
        "1 test".to_owned()
    } else {
        format!("{tests} tests")
    };
    match group {
        Some(group) => format!("FAMAT Test Bank - {tests} - {group}.zip"),
        None => format!("FAMAT Test Bank - {tests}.zip"),
    }
}

/// The name shortened to the cap, with its extension.
fn capped(mut name: Name, extension: &str) -> String {
    let mut base = name.assemble();
    while base.chars().count() > MAX_BASE_CHARS {
        let longest_facet = name
            .middle
            .iter_mut()
            .flatten()
            .filter(|part| part.chars().count() > MIN_PART_CHARS)
            .max_by_key(|part| part.chars().count());
        let part = match longest_facet {
            Some(part) => part,
            None => match name.label.as_mut().filter(|l| l.chars().count() > MIN_PART_CHARS) {
                Some(label) => label,
                // Only the parts that are never shortened are left.
                None => break,
            },
        };
        shorten_by_one(part);
        base = name.assemble();
    }

    match clean(Some(extension)) {
        Some(extension) => format!("{base}.{extension}"),
        // Windows drops trailing dots and spaces; keep the name as shown.
        None => base.trim_end_matches(['.', ' ']).to_owned(),
    }
}

struct Name {
    ids: String,
    year: Option<String>,
    /// Month, Competition, Host, Division/Level, Test type.
    middle: [Option<String>; 5],
    /// The Document kind, or a zip's kind group.
    kind: Option<String>,
    label: Option<String>,
}

impl Name {
    fn new(ids: String, test: &TestParts<'_>, kind: Option<&str>, label: Option<&str>) -> Self {
        Self {
            ids,
            year: test.year.map(|year| year.to_string()),
            middle: [
                clean(test.month),
                clean(test.competition),
                clean(test.host),
                clean(test.division_level),
                clean(test.test_type.map(|t| t.replace(['(', ')'], " ")).as_deref()),
            ],
            kind: clean(kind),
            label: clean(label),
        }
    }

    /// The name without its extension.
    fn assemble(&self) -> String {
        let [month, competition, host, division_level, test_type] = &self.middle;
        let head = join(
            " ",
            [
                Some(&self.ids),
                self.year.as_ref(),
                month.as_ref(),
                competition.as_ref(),
            ],
        );
        let level = join(" ", [division_level.as_ref(), test_type.as_ref()]);
        let kind = match (&self.kind, &self.label) {
            (Some(kind), Some(label)) => Some(format!("{kind} ({label})")),
            (kind, _) => kind.clone(),
        };
        join(" - ", [Some(&head), host.as_ref(), Some(&level), kind.as_ref()])
    }
}

/// The non-empty parts, joined.
fn join<'a>(separator: &str, parts: impl IntoIterator<Item = Option<&'a String>>) -> String {
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(separator)
}

/// A part made safe for a file name on every common system: `/` becomes
/// `-`, characters Windows rejects are removed, and whitespace collapses.
/// `None` when nothing is left.
fn clean(part: Option<&str>) -> Option<String> {
    let cleaned: String = part?
        .chars()
        .filter_map(|c| match c {
            '/' => Some('-'),
            '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' => None,
            c if c.is_whitespace() => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Drop the last character, and any separator it leaves dangling.
fn shorten_by_one(part: &mut String) {
    part.pop();
    part.truncate(part.trim_end_matches([' ', '-', ',', '.', '&', '+']).len());
}

/// A `Content-Disposition` value carrying `name`: a plain ASCII `filename`
/// for every client, plus an RFC 5987 `filename*` when the name is not
/// ASCII (RFC 6266).
pub fn content_disposition(disposition: &str, name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| match c {
            '"' | '\\' => '_',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '_',
        })
        .collect();
    if fallback == name {
        format!("{disposition}; filename=\"{name}\"")
    } else {
        format!(
            "{disposition}; filename=\"{fallback}\"; filename*=UTF-8''{}",
            percent_encode(name)
        )
    }
}

/// RFC 5987 `value-chars`: `attr-char`s as they are, every other byte of the
/// UTF-8 encoding as `%XX`.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        let is_attr_char = byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte);
        if is_attr_char {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
