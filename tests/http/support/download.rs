//! Reading a download's `Content-Disposition` the way a browser does.

/// What a response's `Content-Disposition` says.
#[derive(Debug)]
pub struct Disposition {
    /// `inline` or `attachment`.
    pub kind: String,
    /// The name a browser saves the file as: `filename*` (RFC 5987) when
    /// present, otherwise `filename`.
    pub name: String,
    /// The plain `filename`, for clients without RFC 5987 support.
    pub fallback: String,
}

/// The `Content-Disposition` of a response, which must have one.
pub fn disposition(response: &reqwest::Response) -> Disposition {
    let header = response
        .headers()
        .get("content-disposition")
        .expect("a Content-Disposition header")
        .to_str()
        .expect("an ASCII Content-Disposition");
    parse(header)
}

fn parse(header: &str) -> Disposition {
    let (kind, mut rest) = header.split_once(';').unwrap_or((header, ""));
    let mut fallback = None;
    let mut extended = None;
    while !rest.trim().is_empty() {
        let (name, after) = rest
            .split_once('=')
            .unwrap_or_else(|| panic!("bad parameter in {header:?}"));
        let after = after.trim_start();
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            let end = quoted
                .find('"')
                .unwrap_or_else(|| panic!("unterminated quote in {header:?}"));
            assert!(!quoted[..end].contains('\\'), "escapes in {header:?}");
            (quoted[..end].to_owned(), quoted[end + 1..].trim_start())
        } else {
            let (value, next) = after.split_once(';').map_or((after, ""), |(v, n)| (v, n));
            (value.trim().to_owned(), next)
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "filename" => fallback = Some(value),
            "filename*" => extended = Some(decode_rfc5987(&value)),
            other => panic!("unexpected parameter {other:?} in {header:?}"),
        }
        rest = next.strip_prefix(';').unwrap_or(next);
    }
    let fallback = fallback.unwrap_or_else(|| panic!("no filename in {header:?}"));
    Disposition {
        kind: kind.trim().to_owned(),
        name: extended.unwrap_or_else(|| fallback.clone()),
        fallback,
    }
}

/// `UTF-8''T1%20D2.pdf` → `T1 D2.pdf`.
fn decode_rfc5987(value: &str) -> String {
    let encoded = value
        .strip_prefix("UTF-8''")
        .unwrap_or_else(|| panic!("filename* is not UTF-8'': {value:?}"));
    let mut bytes = Vec::new();
    let mut input = encoded.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = [input.next().unwrap(), input.next().unwrap()];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).unwrap(), 16).unwrap());
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).expect("filename* is UTF-8")
}
