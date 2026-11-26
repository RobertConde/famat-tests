//! Reading a downloaded zip the way an unzip tool does: from its central
//! directory, with the `zip` crate rather than the server's async_zip.

use std::io::{Cursor, Read};

/// One file in a zip.
#[derive(Debug)]
pub struct Entry {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Stored, not compressed.
    pub stored: bool,
    /// The DOS date and time: year, month, day, hour, minute, second.
    pub modified: (u16, u8, u8, u8, u8, u8),
}

/// Every entry of a zip, in order, with its contents.
pub fn entries(zip: &[u8]) -> Vec<Entry> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).expect("a readable zip");
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).expect("a readable zip entry");
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).expect("entry contents (CRC checked)");
            let date = file.last_modified().expect("a modification date");
            Entry {
                name: file.name().to_owned(),
                stored: file.compression() == zip::CompressionMethod::Stored,
                modified: (
                    date.year(),
                    date.month(),
                    date.day(),
                    date.hour(),
                    date.minute(),
                    date.second(),
                ),
                bytes,
            }
        })
        .collect()
}

/// The names of a zip's entries, in order.
pub fn names(zip: &[u8]) -> Vec<String> {
    entries(zip).into_iter().map(|entry| entry.name).collect()
}
