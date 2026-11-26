//! The rules for files added to a Test, shared by the server (which enforces
//! them) and the browser (which previews them as soon as files are chosen):
//! allowed types and size, Document kind guessing, and Word+PDF pairing.

/// The extensions an Original may have, lower case, without the dot.
pub const ALLOWED_EXTENSIONS: [&str; 8] = ["pdf", "doc", "docx", "ppt", "pptx", "rtf", "odt", "zip"];

/// For the file picker's `accept`.
pub const ACCEPT: &str = ".pdf,.doc,.docx,.ppt,.pptx,.rtf,.odt,.zip";

/// The allowed types, as a person reads them.
pub const ALLOWED_TYPES: &str = "PDF, DOC, DOCX, PPT, PPTX, RTF, ODT and ZIP";

/// The most one file may hold: 100 MB (MiB).
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// The most one upload (every file of one form post) may hold. axum's own
/// default would be 2 MB.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub const MAX_UPLOAD_BYTES: usize = 1024 * 1024 * 1024;

/// Word files, which pair with a PDF of the same name.
const WORD_EXTENSIONS: [&str; 2] = ["doc", "docx"];

/// A file name's stem and lower-case extension (empty when it has none).
/// Any directory part, as old browsers sent, is dropped.
pub fn split_name(file_name: &str) -> (&str, String) {
    let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, extension.to_ascii_lowercase()),
        _ => (name, String::new()),
    }
}

/// Why a file cannot be added, as a message naming it, or `None` if it can.
pub fn refusal(file_name: &str, byte_size: Option<u64>) -> Option<String> {
    let (_, extension) = split_name(file_name);
    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Some(type_refusal(file_name));
    }
    if byte_size.is_some_and(|size| size > MAX_FILE_BYTES) {
        return Some(size_refusal(file_name));
    }
    None
}

pub fn type_refusal(file_name: &str) -> String {
    format!("“{file_name}” can’t be added: only {ALLOWED_TYPES} files are allowed.")
}

pub fn size_refusal(file_name: &str) -> String {
    format!("“{file_name}” can’t be added: files can be at most 100 MB.")
}

/// The Document kind a file's name suggests, by kind name.
///
/// - A `T_`, `S_`, `A_`, `C_` or `E_` prefix: Test, Solutions, Answers,
///   Condensed Test, Errata. The prefix wins over any keyword.
/// - Otherwise keywords, as whole words ("Calc_Sol.pdf", "MuKey.docx",
///   "geometry answers.pdf"): "errata"; "condensed"; "combined"; "solutions",
///   "sol"…; "answers", "ans", "key"…
/// - Otherwise Test.
pub fn guess_kind(file_name: &str) -> &'static str {
    let (stem, _) = split_name(file_name);
    let mut chars = stem.chars();
    if let (Some(prefix), Some('_')) = (chars.next(), chars.next()) {
        match prefix.to_ascii_uppercase() {
            'T' => return "Test",
            'S' => return "Solutions",
            'A' => return "Answers",
            'C' => return "Condensed Test",
            'E' => return "Errata",
            _ => {}
        }
    }
    let words = words(stem);
    let has = |keywords: &[&str]| words.iter().any(|w| keywords.contains(&w.as_str()));
    if has(&["errata", "erratum"]) {
        "Errata"
    } else if has(&["condensed"]) {
        "Condensed Test"
    } else if has(&["combined"]) {
        "Combined Test+Solutions"
    } else if has(&["solutions", "solution", "sol", "sols", "soln", "solns"]) {
        "Solutions"
    } else if has(&["answers", "answer", "ans", "key", "keys", "answerkey"]) {
        "Answers"
    } else {
        "Test"
    }
}

/// The lower-case words of a name, split at anything not a letter or digit,
/// at camelCase humps ("MuKey" → mu, key; "IISolutions" → ii, solutions) and
/// between letters and digits ("Sol2" → sol, 2).
fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        if let Some(&previous) = i.checked_sub(1).and_then(|p| chars.get(p))
            && previous.is_alphanumeric()
        {
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            let hump = (previous.is_lowercase() && c.is_uppercase())
                || (previous.is_uppercase() && c.is_uppercase() && next_is_lower)
                || (previous.is_numeric() != c.is_numeric());
            if hump && !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        }
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Where a file goes, decided as files arrive in order: a Word file and a
/// PDF with the same stem (ignoring case) become one Document, with the Word
/// file as its Original and the PDF as its PDF, whichever comes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// The Original of a new Document, `document` (an index into the
    /// Documents placed so far).
    New { document: usize },
    /// The PDF for `document`, whose Original is a Word file.
    PdfFor { document: usize },
    /// A Word Original for `document`, which so far had only a PDF: that PDF
    /// becomes its PDF.
    WordFor { document: usize },
}

/// Places files into Documents, see [`Placement`].
#[derive(Default)]
pub struct Pairer {
    documents: Vec<Slot>,
}

struct Slot {
    stem: String,
    has_word: bool,
    has_pdf: bool,
}

impl Pairer {
    pub fn place(&mut self, file_name: &str) -> Placement {
        let (stem, extension) = split_name(file_name);
        let stem = stem.to_lowercase();
        let is_word = WORD_EXTENSIONS.contains(&extension.as_str());
        let is_pdf = extension == "pdf";
        if is_pdf
            && let Some(document) = self
                .documents
                .iter()
                .position(|d| d.stem == stem && d.has_word && !d.has_pdf)
        {
            self.documents[document].has_pdf = true;
            return Placement::PdfFor { document };
        }
        if is_word
            && let Some(document) = self
                .documents
                .iter()
                .position(|d| d.stem == stem && d.has_pdf && !d.has_word)
        {
            self.documents[document].has_word = true;
            return Placement::WordFor { document };
        }
        self.documents.push(Slot {
            stem,
            has_word: is_word,
            has_pdf: is_pdf,
        });
        Placement::New {
            document: self.documents.len() - 1,
        }
    }
}
