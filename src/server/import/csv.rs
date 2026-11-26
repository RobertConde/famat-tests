//! Just enough CSV (RFC 4180) for the import's inputs, as Google Sheets and
//! the labelling scripts write it: comma-separated, fields optionally in
//! double quotes, `""` for a quote inside one, and line breaks allowed inside
//! quotes.

/// One record: its fields and the line it starts on (1-based).
pub struct Record {
    pub line: usize,
    pub fields: Vec<String>,
}

impl Record {
    /// The field at `index`, or `""` when the record is shorter.
    pub fn get(&self, index: usize) -> &str {
        self.fields.get(index).map_or("", String::as_str)
    }

    /// Whether every field is empty or blank.
    pub fn is_blank(&self) -> bool {
        self.fields.iter().all(|f| f.trim().is_empty())
    }
}

/// Every record in `text`. A byte order mark is skipped, and a final line
/// break does not make an extra record.
pub fn parse(text: &str) -> Result<Vec<Record>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut records = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let (mut line, mut start_line) = (1, 1);
    let mut in_quotes = false;
    // Whether the current record has anything in it yet.
    let mut started = false;

    while let Some(c) = chars.next() {
        started = true;
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                c => {
                    if c == '\n' {
                        line += 1;
                    }
                    field.push(c);
                }
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            ',' => fields.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                fields.push(std::mem::take(&mut field));
                records.push(Record {
                    line: start_line,
                    fields: std::mem::take(&mut fields),
                });
                line += 1;
                start_line = line;
                started = false;
            }
            c => field.push(c),
        }
    }
    if in_quotes {
        return Err(format!("line {start_line}: a quoted field is never closed"));
    }
    if started {
        fields.push(field);
        records.push(Record {
            line: start_line,
            fields,
        });
    }
    Ok(records)
}
