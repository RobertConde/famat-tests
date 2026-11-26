//! One conversion: fetch the Original into a throwaway job directory, pick
//! a converter by its type, run it under the hard timeout, and check that
//! the result is a non-empty PDF that parses.
//!
//! LibreOffice gets its own profile per job, copied from the template
//! (`PDF_PROFILE_TEMPLATE`) with `MathTypeToMath` forced off: two `soffice`
//! processes sharing a profile silently produce nothing. It runs in its own
//! process group, and the whole group is killed when it finishes or times
//! out, since `soffice` leaves `soffice.bin` behind when only it is killed.
//! The Original is copied to a generated name, so its own name never
//! reaches a command line.

use std::{
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use tempfile::TempDir;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use super::{image::image_to_pdf, sniff::sniff};
use crate::server::{
    config::PdfConfig,
    storage::{FileRole, Storage},
};

/// Originals LibreOffice converts: Writer and Impress formats.
const LIBREOFFICE: &[&str] = &[
    "doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "ott", "wpd", "wps", "ppt", "pptx", "pptm", "pps", "ppsx",
    "pot", "potx", "odp",
];
/// Originals [`image_to_pdf`] wraps.
const IMAGES: &[&str] = &["jpg", "jpeg", "png", "gif", "tif", "tiff", "bmp", "webp"];

/// Whether a PDF can be made of an Original with this extension (lower
/// case, no dot): LibreOffice's formats and images. A PDF is its own PDF.
pub fn convertible(extension: &str) -> bool {
    LIBREOFFICE.contains(&extension) || IMAGES.contains(&extension)
}

/// Whether LibreOffice converts this extension: a Word or PowerPoint
/// Original, or one of their open relatives.
pub fn office(extension: &str) -> bool {
    LIBREOFFICE.contains(&extension)
}

/// What the `document_files` row says about an Original.
pub(super) struct Original {
    pub extension: String,
    pub content_type: String,
}

pub(super) enum Outcome {
    /// A PDF to store. It lives in the job's directory, deleted on drop.
    Made { pdf: PathBuf, _dir: TempDir },
    /// The Original is a PDF already, and so is its own PDF.
    OriginalIsPdf,
    /// A ZIP: there is nothing to make a PDF of.
    NotConvertible,
}

/// Make a PDF of a Document's Original. The error is a sentence for the
/// Needs review comment.
pub(super) async fn convert(
    config: &PdfConfig,
    storage: &Storage,
    document_id: i64,
    original: &Original,
) -> Result<Outcome, String> {
    if original.extension == "pdf" || original.content_type == "application/pdf" {
        return Ok(Outcome::OriginalIsPdf);
    }
    if original.extension == "zip" {
        return Ok(Outcome::NotConvertible);
    }

    let dir = tempfile::Builder::new()
        .prefix(&format!("famat-pdf-D{document_id}-"))
        .tempdir_in(&config.work_dir)
        .map_err(|e| format!("could not make a working directory: {e}"))?;
    let download = dir.path().join("download");
    fetch(storage, document_id, &download).await?;

    let extension = if original.extension.is_empty() {
        let path = download.clone();
        let sniffed = tokio::task::spawn_blocking(move || sniff(&path))
            .await
            .map_err(|e| format!("reading the Original: {e}"))?
            .map_err(|e| format!("reading the Original: {e}"))?;
        sniffed
            .ok_or("the Original has no extension, and its contents don't say what kind of file it is")?
            .to_owned()
    } else {
        original.extension.clone()
    };
    let input = dir.path().join(format!("original.{extension}"));
    tokio::fs::rename(&download, &input)
        .await
        .map_err(|e| format!("preparing the Original: {e}"))?;

    let ext = extension.as_str();
    let pdf = if ext == "pdf" {
        // An extensionless PDF passes through unchanged, as its own PDF file.
        return Ok(Outcome::Made { pdf: input, _dir: dir });
    } else if ext == "zip" {
        return Ok(Outcome::NotConvertible);
    } else if LIBREOFFICE.contains(&ext) {
        let pdf = libreoffice(config, dir.path(), &input).await?;
        verify(&pdf, "LibreOffice").await?;
        pdf
    } else if IMAGES.contains(&ext) {
        let pdf = dir.path().join("image.pdf");
        let (from, to) = (input.clone(), pdf.clone());
        let made = tokio::time::timeout(
            config.timeout,
            tokio::task::spawn_blocking(move || image_to_pdf(&from, &to)),
        )
        .await
        .map_err(|_| format!("making a PDF of the image took longer than {}", seconds(config.timeout)))?
        .map_err(|e| format!("making a PDF of the image: {e}"))?;
        made?;
        verify(&pdf, "the image converter").await?;
        pdf
    } else {
        return Err(format!("a PDF can't be made from a .{ext} file"));
    };
    Ok(Outcome::Made { pdf, _dir: dir })
}

/// Stream the Original from Garage into `path`.
async fn fetch(storage: &Storage, document_id: i64, path: &Path) -> Result<(), String> {
    let key = storage.document_key(document_id, FileRole::Original);
    let object = storage
        .get(&key)
        .await
        .map_err(|e| format!("fetching the Original: {e:#}"))?
        .ok_or("the Original is missing from storage")?;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("saving the Original for conversion: {e}"))?;
    tokio::io::copy(&mut object.body.into_async_read(), &mut file)
        .await
        .map_err(|e| format!("fetching the Original: {e}"))?;
    Ok(())
}

// ------------------------------------------------------------- LibreOffice

/// Run `soffice --convert-to pdf` on `input` with a fresh profile in `dir`.
/// Returns the PDF's path, which exists.
async fn libreoffice(config: &PdfConfig, dir: &Path, input: &Path) -> Result<PathBuf, String> {
    let profile = dir.join("profile");
    prepare_profile(config.profile_template.clone(), profile.clone()).await?;
    let out = dir.join("out");
    tokio::fs::create_dir(&out)
        .await
        .map_err(|e| format!("could not make an output directory: {e}"))?;

    let mut command = Command::new(&config.soffice);
    command
        .args([
            "--headless",
            "--invisible",
            "--norestore",
            "--nologo",
            "--nolockcheck",
            "--nodefault",
        ])
        .arg(format!("-env:UserInstallation={}", file_url(&profile)))
        .args(["--convert-to", "pdf", "--outdir"])
        .arg(&out)
        .arg(input)
        .current_dir(dir);
    let (status, output) = run(command, config.timeout, "LibreOffice").await?;

    let stem = input.file_stem().unwrap_or_default();
    let pdf = out.join(stem).with_extension("pdf");
    if tokio::fs::try_exists(&pdf).await.unwrap_or(false) {
        return Ok(pdf);
    }
    let said = if output.is_empty() {
        String::new()
    } else {
        format!(": {output}")
    };
    Err(format!("LibreOffice made no PDF ({status}){said}"))
}

/// The profile LibreOffice runs with: a copy of the template (or empty),
/// with `MathTypeToMath` off, so legacy equations keep the picture Word
/// saved instead of being re-typeset.
async fn prepare_profile(template: Option<PathBuf>, profile: PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        match &template {
            Some(template) => copy_dir(template, &profile)
                .map_err(|e| format!("copying the LibreOffice profile template {}: {e}", template.display()))?,
            None => std::fs::create_dir_all(&profile).map_err(|e| format!("making a LibreOffice profile: {e}"))?,
        }
        math_type_off(&profile).map_err(|e| format!("configuring the LibreOffice profile: {e}"))
    })
    .await
    .map_err(|e| format!("preparing the LibreOffice profile: {e}"))?
}

const MATH_TYPE_OFF: &str = r#"<item oor:path="/org.openoffice.Office.Common/Filter/Microsoft/Import"><prop oor:name="MathTypeToMath" oor:op="fuse"><value>false</value></prop></item>"#;

/// Set `MathTypeToMath` to false in the profile's
/// `user/registrymodifications.xcu`, replacing any setting there.
fn math_type_off(profile: &Path) -> std::io::Result<()> {
    let user = profile.join("user");
    std::fs::create_dir_all(&user)?;
    let path = user.join("registrymodifications.xcu");
    let existing = std::fs::read_to_string(&path)
        .ok()
        .filter(|x| x.contains("</oor:items>"));
    let xcu = match existing {
        Some(existing) => {
            let kept: Vec<&str> = existing.lines().filter(|l| !l.contains("\"MathTypeToMath\"")).collect();
            kept.join("\n")
                .replacen("</oor:items>", &format!("{MATH_TYPE_OFF}\n</oor:items>"), 1)
        }
        None => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <oor:items xmlns:oor=\"http://openoffice.org/2001/registry\" \
             xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" \
             xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\
             {MATH_TYPE_OFF}\n</oor:items>\n"
        ),
    };
    std::fs::write(path, xcu)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
        // Symlinks and sockets (a lock left behind) are not copied.
    }
    Ok(())
}

/// `file:///…`, percent-encoding anything but unreserved characters and `/`.
fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => url.push(char::from(byte)),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

// ---------------------------------------------------------------- running

/// Run a command in its own process group with a hard timeout. Whether it
/// finishes or not, the whole group is then killed, so nothing it started
/// outlives the job. Returns its exit status and the tail of its output.
async fn run(mut command: Command, timeout: Duration, what: &str) -> Result<(ExitStatus, String), String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("{what} could not be started: {e}"))?;
    let pid = child.id();
    let stdout = tokio::spawn(tail(child.stdout.take()));
    let stderr = tokio::spawn(tail(child.stderr.take()));

    let waited = tokio::time::timeout(timeout, child.wait()).await;
    if let Some(pid) = pid {
        kill_group(pid);
    }
    let status = match waited {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("waiting for {what}: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!("{what} took longer than {} and was stopped", seconds(timeout)));
        }
    };
    // With the group gone, the pipes close; don't wait on a process that
    // left the group anyway.
    let mut output = Vec::new();
    for reader in [stdout, stderr] {
        if let Ok(Ok(text)) = tokio::time::timeout(Duration::from_secs(5), reader).await {
            output.push(text);
        }
    }
    let output = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    Ok((status, last_chars(&output, 300)))
}

/// SIGKILL every process in the group. A group that is already gone is fine.
fn kill_group(pgid: u32) {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return;
    };
    // SAFETY: killpg only sends a signal; it touches no memory of ours.
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

/// Read a pipe to its end, keeping the last few KiB.
async fn tail(pipe: Option<impl AsyncRead + Unpin>) -> String {
    const KEEP: usize = 4096;
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut buffer = [0u8; 4096];
    while let Ok(n) = pipe.read(&mut buffer).await {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buffer[..n]);
        if kept.len() > 2 * KEEP {
            kept.drain(..kept.len() - KEEP);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

fn last_chars(text: &str, n: usize) -> String {
    let count = text.chars().count();
    if count <= n {
        return text.to_owned();
    }
    let rest: String = text.chars().skip(count - n).collect();
    format!("…{rest}")
}

fn seconds(duration: Duration) -> String {
    format!("{} s", duration.as_secs())
}

// --------------------------------------------------------------- checking

/// Success means a non-empty PDF that parses and has pages.
async fn verify(pdf: &Path, what: &'static str) -> Result<(), String> {
    let pdf = pdf.to_owned();
    tokio::task::spawn_blocking(move || {
        let len = std::fs::metadata(&pdf).map(|m| m.len()).unwrap_or(0);
        if len == 0 {
            return Err(format!("{what} made an empty PDF"));
        }
        let document = lopdf::Document::load(&pdf).map_err(|e| format!("the PDF {what} made does not parse: {e}"))?;
        if document.get_pages().is_empty() {
            return Err(format!("the PDF {what} made has no pages"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("checking the PDF: {e}"))?
}
