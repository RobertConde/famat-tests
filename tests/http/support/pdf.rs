//! PDF conversion in tests: a stand-in for `soffice`, and waiting for the
//! queue.
//!
//! [`StubConverter`] is a shell script the server runs instead of
//! LibreOffice (`PDF_SOFFICE`). It does what the first line of the Original
//! says (or its `mode` file, once [`StubConverter::set_mode`] writes one):
//!
//! - `ok [seconds]`: wait, then write its PDF where LibreOffice would.
//! - `fail`: say so on stderr and exit 1, writing nothing.
//! - `garbage`: write a "PDF" that isn't one.
//! - `hang`: start a child process and sleep, both far past any timeout.
//!
//! It logs each run: its PID, how many runs were going at once (itself
//! included), whether its profile has `MathTypeToMath` off, the profile and
//! the input's name.

use std::{
    future::Future,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

const STUB: &str = r#"#!/bin/sh
dir=$(cd "$(dirname "$0")" && pwd)
profile=; outdir=; input=
while [ $# -gt 0 ]; do
  case "$1" in
    -env:UserInstallation=file://*) profile=${1#-env:UserInstallation=file://} ;;
    --outdir) outdir=$2; shift ;;
    --convert-to) shift ;;
    -*) ;;
    *) input=$1 ;;
  esac
  shift
done
mkdir -p "$dir/running"
: > "$dir/running/$$"
running=$(ls "$dir/running" | wc -l | tr -d ' ')
if grep -q '<prop oor:name="MathTypeToMath" oor:op="fuse"><value>false</value></prop>' "$profile/user/registrymodifications.xcu" 2>/dev/null
then mathtype=off; else mathtype=on; fi
mode=$(head -n 1 "$input")
if [ -f "$dir/mode" ]; then mode=$(cat "$dir/mode"); fi
echo "start pid=$$ running=$running mathtype=$mathtype profile=$profile input=$(basename "$input")" >> "$dir/log"
out="$outdir/$(basename "${input%.*}").pdf"
set -- $mode
status=0
case "$1" in
  ok) sleep "${2:-0}"; cp "$dir/out.pdf" "$out" ;;
  garbage) echo "this is not a PDF" > "$out" ;;
  hang) sleep 300 & echo $! > "$dir/child.$$"; sleep 300 ;;
  *) echo "stub: this file is broken" >&2; status=1 ;;
esac
rm -f "$dir/running/$$"
exit $status
"#;

/// One run of the stub, from its log.
#[derive(Clone, Debug)]
pub struct Run {
    pub pid: u32,
    /// Runs going at that moment, this one included.
    pub running: usize,
    pub math_type_off: bool,
    pub profile: PathBuf,
    /// The name the Original was given, e.g. `original.docx`.
    pub input: String,
}

pub struct StubConverter {
    dir: PathBuf,
    /// The PDF that `ok` writes.
    pub pdf: Vec<u8>,
}

impl StubConverter {
    pub fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("pdf-stubs")
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("soffice");
        std::fs::write(&script, STUB).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pdf = minimal_pdf("made by the stub converter");
        std::fs::write(dir.join("out.pdf"), &pdf).unwrap();
        Self { dir, pdf }
    }

    /// The server environment that runs conversions with this stub, two at
    /// a time, timing out after `timeout_secs`.
    pub fn server_env(&self, timeout_secs: u64) -> Vec<(String, String)> {
        vec![
            ("PDF_SOFFICE".to_owned(), self.dir.join("soffice").display().to_string()),
            ("PDF_WORKERS".to_owned(), "2".to_owned()),
            ("PDF_TIMEOUT_SECS".to_owned(), timeout_secs.to_string()),
        ]
    }

    /// From now on do `mode` whatever the Original says.
    pub fn set_mode(&self, mode: &str) {
        std::fs::write(self.dir.join("mode"), mode).unwrap();
    }

    /// Every run so far, in the order they started.
    pub fn runs(&self) -> Vec<Run> {
        let log = std::fs::read_to_string(self.dir.join("log")).unwrap_or_default();
        log.lines()
            .map(|line| {
                let field = |name: &str| {
                    line.split(' ')
                        .find_map(|part| part.strip_prefix(&format!("{name}=")))
                        .unwrap_or_else(|| panic!("no {name} in {line:?}"))
                        .to_owned()
                };
                Run {
                    pid: field("pid").parse().unwrap(),
                    running: field("running").parse().unwrap(),
                    math_type_off: field("mathtype") == "off",
                    profile: PathBuf::from(field("profile")),
                    input: field("input"),
                }
            })
            .collect()
    }

    /// The PID of the child a `hang` run started.
    pub fn child_of(&self, run: &Run) -> u32 {
        std::fs::read_to_string(self.dir.join(format!("child.{}", run.pid)))
            .expect("the hanging run's child PID")
            .trim()
            .parse()
            .unwrap()
    }
}

impl Drop for StubConverter {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Whether a process is still alive (not merely a zombie waiting to be
/// reaped).
pub fn alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// Wait until `check` returns `Some`, polling, for at most `limit`.
pub async fn eventually<T, F, Fut>(limit: Duration, what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let started = Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(
            started.elapsed() < limit,
            "timed out after {limit:?} waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A small, valid one-page PDF, carrying `marker` so PDFs can be told apart.
pub fn minimal_pdf(marker: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_owned(),
    ];
    let mut pdf = format!("%PDF-1.4\n% {marker}\n").into_bytes();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}
