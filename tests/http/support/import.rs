//! Running the real `famat import` against this test's database and Garage
//! prefix, on the committed fixture set in `tests/fixtures/import/`: a
//! synthetic students' sheet, AI labels, docs export and legacy SQL dump,
//! plus an unpacked bundle with its unpacked.csv, attach decisions and the
//! AI's review flags, all in the real inputs' formats.

use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use super::TestApp;

/// `tests/fixtures/import/`.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import")
}

/// What one run printed.
pub struct ImportRun {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// The report file; empty when none was written.
    pub report: String,
}

impl From<Output> for ImportRun {
    fn from(output: Output) -> Self {
        Self {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            report: String::new(),
        }
    }
}

/// A fixture file, by its path under `tests/fixtures/import/`, e.g.
/// `docs/766 1999 Test.pdf` or `unpacked/8053/…`.
pub fn fixture(path: &str) -> Vec<u8> {
    let path = fixture_dir().join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

impl TestApp {
    /// Run `famat import` on the fixture set, with `extra` arguments. The
    /// reconciliation report goes to a file of its own, read into
    /// [`ImportRun::report`].
    pub fn run_import(&self, extra: &[&str]) -> ImportRun {
        let dir = fixture_dir();
        let report = std::env::temp_dir().join(format!("famat-import-report-{}.txt", uuid::Uuid::new_v4()));
        let output = Command::new(&self.binary)
            .arg("import")
            .arg("--sheet")
            .arg(dir.join("sheet.csv"))
            .arg("--ai-labels")
            .arg(dir.join("ai-labels.csv"))
            .arg("--docs")
            .arg(dir.join("docs"))
            .arg("--legacy-sql")
            .arg(dir.join("legacy.sql"))
            .arg("--unpacked")
            .arg(dir.join("unpacked.csv"))
            .arg("--unpacked-dir")
            .arg(dir.join("unpacked"))
            .arg("--attach")
            .arg(dir.join("attach-1.out.csv"))
            .arg("--attach")
            .arg(dir.join("attach-2.out.csv"))
            .arg("--review")
            .arg(dir.join("review-core.csv"))
            .arg("--report")
            .arg(&report)
            .args(extra)
            .env_clear()
            .env("DATABASE_URL", &self.database_url)
            .env("S3_ENDPOINT", &self.env.s3_endpoint)
            .env("S3_REGION", &self.env.s3_region)
            .env("S3_BUCKET", &self.env.s3_bucket)
            .env("S3_ACCESS_KEY_ID", &self.env.s3_access_key_id)
            .env("S3_SECRET_ACCESS_KEY", &self.env.s3_secret_access_key)
            .env("S3_KEY_PREFIX", &self.key_prefix)
            .stdin(Stdio::null())
            .output()
            .expect("run famat import");
        let mut run = ImportRun::from(output);
        run.report = std::fs::read_to_string(&report).unwrap_or_default();
        let _ = std::fs::remove_file(&report);
        run
    }

    /// Import the fixture set, which must succeed.
    pub fn import_fixtures(&self) -> String {
        self.import_fixtures_run().stdout
    }

    /// [`TestApp::import_fixtures`], keeping the report too.
    pub fn import_fixtures_run(&self) -> ImportRun {
        let run = self.run_import(&[]);
        assert!(run.success, "famat import failed:\n{}{}", run.stdout, run.stderr);
        run
    }
}
