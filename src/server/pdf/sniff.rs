//! Telling a file's type from its contents, for Originals with no
//! extension (spec: "Files with no extension get their type detected from
//! their contents"). The import can use it too.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

/// The extension (lower case, no dot) a file's contents say it should have,
/// or `None` when they don't say.
///
/// - PDF, RTF, and the common image formats by their magic bytes.
/// - ZIP containers by what is inside: `docx`, `pptx`, `xlsx`, `odt`, `odp`,
///   `ods`, otherwise `zip`.
/// - Old Office compound files by their streams: `doc`, `ppt`, `xls`.
pub fn sniff(path: &Path) -> io::Result<Option<&'static str>> {
    let mut head = [0u8; 16];
    let n = read_head(path, &mut head)?;
    let head = &head[..n];
    let starts = |magic: &[u8]| head.starts_with(magic);
    Ok(if starts(b"%PDF-") {
        Some("pdf")
    } else if starts(b"{\\rtf") {
        Some("rtf")
    } else if starts(b"\xFF\xD8\xFF") {
        Some("jpg")
    } else if starts(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("gif")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some("tif")
    } else if starts(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if starts(b"BM") && head.len() >= 14 {
        Some("bmp")
    } else if starts(b"PK\x03\x04") {
        Some(zip_type(path))
    } else if starts(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        compound_type(path)
    } else {
        None
    })
}

fn read_head(path: &Path, head: &mut [u8]) -> io::Result<usize> {
    let mut file = File::open(path)?;
    let mut n = 0;
    while n < head.len() {
        match file.read(&mut head[n..])? {
            0 => break,
            read => n += read,
        }
    }
    Ok(n)
}

/// Office Open XML and OpenDocument files are ZIPs; anything else is a
/// plain `zip`.
fn zip_type(path: &Path) -> &'static str {
    let Some(mut archive) = File::open(path).ok().and_then(|f| zip::ZipArchive::new(f).ok()) else {
        return "zip";
    };
    let has = |archive: &zip::ZipArchive<File>, name: &str| archive.file_names().any(|n| n == name);
    if has(&archive, "word/document.xml") {
        return "docx";
    }
    if has(&archive, "ppt/presentation.xml") {
        return "pptx";
    }
    if has(&archive, "xl/workbook.xml") {
        return "xlsx";
    }
    // OpenDocument stores its type, uncompressed, in `mimetype`.
    let mut mimetype = String::new();
    if let Ok(mut entry) = archive.by_name("mimetype") {
        let _ = entry.read_to_string(&mut mimetype);
    }
    match mimetype.trim() {
        "application/vnd.oasis.opendocument.text" => "odt",
        "application/vnd.oasis.opendocument.presentation" => "odp",
        "application/vnd.oasis.opendocument.spreadsheet" => "ods",
        _ => "zip",
    }
}

/// Word 97–2003, PowerPoint 97–2003 and Excel files are OLE compound files,
/// told apart by their main stream.
fn compound_type(path: &Path) -> Option<&'static str> {
    let file = cfb::open(path).ok()?;
    if file.exists("/WordDocument") {
        Some("doc")
    } else if file.exists("/PowerPoint Document") {
        Some("ppt")
    } else if file.exists("/Workbook") || file.exists("/Book") {
        Some("xls")
    } else {
        None
    }
}
