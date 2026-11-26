//! Running the real `famat import` against this test's database and Garage
//! prefix, on the committed fixture set in `tests/fixtures/import/`: a
//! synthetic students' sheet, AI labels, docs export and legacy SQL dump in
//! the real inputs' formats.

use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use super::TestApp;

/// `tests/fixtures/import/`.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import")
}

/// A file of the fixture's docs export, by name.
pub fn fixture_file(name: &str) -> Vec<u8> {
    let path = fixture_dir().join("docs").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// What one run printed.
pub struct ImportRun {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl From<Output> for ImportRun {
    fn from(output: Output) -> Self {
        Self {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

impl TestApp {
    /// Run `famat import` on the fixture set, with `extra` arguments.
    pub fn run_import(&self, extra: &[&str]) -> ImportRun {
        let dir = fixture_dir();
        Command::new(&self.binary)
            .arg("import")
            .arg("--sheet")
            .arg(dir.join("sheet.csv"))
            .arg("--ai-labels")
            .arg(dir.join("ai-labels.csv"))
            .arg("--docs")
            .arg(dir.join("docs"))
            .arg("--legacy-sql")
            .arg(dir.join("legacy.sql"))
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
            .expect("run famat import")
            .into()
    }

    /// Import the fixture set, which must succeed.
    pub fn import_fixtures(&self) -> String {
        let run = self.run_import(&[]);
        assert!(run.success, "famat import failed:\n{}{}", run.stdout, run.stderr);
        run.stdout
    }
}
