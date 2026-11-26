//! Server configuration, read once from the environment at startup.
//!
//! The listen address comes from `IP` and `PORT`, which Dioxus reads itself.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, bail};

#[derive(Clone, Debug)]
pub struct Config {
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub pdf: PdfConfig,
}

#[derive(Clone, Debug)]
pub struct DatabaseConfig {
    /// `DATABASE_URL`
    pub url: String,
    /// `DATABASE_MAX_CONNECTIONS`, default 10.
    pub max_connections: u32,
}

#[derive(Clone)]
pub struct StorageConfig {
    /// `S3_ENDPOINT`, e.g. `http://127.0.0.1:3900` for Garage.
    pub endpoint: String,
    /// `S3_REGION`; Garage's default is `garage`.
    pub region: String,
    /// `S3_BUCKET`
    pub bucket: String,
    /// `S3_ACCESS_KEY_ID`
    pub access_key_id: String,
    /// `S3_SECRET_ACCESS_KEY`
    pub secret_access_key: String,
    /// `S3_KEY_PREFIX`, prepended to every object key. Empty in production;
    /// the HTTP tests give each test its own prefix in a shared bucket.
    pub key_prefix: String,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConfig")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database: DatabaseConfig::from_env()?,
            storage: StorageConfig {
                endpoint: required("S3_ENDPOINT")?,
                region: required("S3_REGION")?,
                bucket: required("S3_BUCKET")?,
                access_key_id: required("S3_ACCESS_KEY_ID")?,
                secret_access_key: required("S3_SECRET_ACCESS_KEY")?,
                key_prefix: optional("S3_KEY_PREFIX").unwrap_or_default(),
            },
            pdf: PdfConfig::from_env()?,
        })
    }
}

impl DatabaseConfig {
    /// Just the database settings, for CLI commands that need nothing else.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            url: required("DATABASE_URL")?,
            max_connections: optional("DATABASE_MAX_CONNECTIONS")
                .map(|v| v.parse().context("DATABASE_MAX_CONNECTIONS must be a number"))
                .transpose()?
                .unwrap_or(10),
        })
    }
}

/// The PDF conversion queue (`server::pdf`).
#[derive(Clone, Debug)]
pub struct PdfConfig {
    /// `PDF_WORKERS`: conversions run at once, default 2. 0 turns
    /// conversion off (jobs are still queued).
    pub workers: usize,
    /// `PDF_TIMEOUT_SECS`: how long one conversion may take before its
    /// whole process group is killed, default 120.
    pub timeout: Duration,
    /// `PDF_SOFFICE`: the LibreOffice command, default `soffice`.
    pub soffice: PathBuf,
    /// `PDF_PROFILE_TEMPLATE`: a LibreOffice user profile that each job's
    /// throwaway profile is copied from (the image bakes one in). Unset, each
    /// job starts from an empty profile. Either way `MathTypeToMath` is off.
    pub profile_template: Option<PathBuf>,
    /// `PDF_WORK_DIR`: where jobs make their temporary directories, default
    /// the system's temporary directory.
    pub work_dir: PathBuf,
}

impl PdfConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let number = |name: &str, default: u64| -> anyhow::Result<u64> {
            optional(name)
                .map(|v| v.parse().with_context(|| format!("{name} must be a whole number")))
                .transpose()
                .map(|v| v.unwrap_or(default))
        };
        let timeout = number("PDF_TIMEOUT_SECS", 120)?;
        if timeout == 0 {
            bail!("PDF_TIMEOUT_SECS must be at least 1");
        }
        Ok(Self {
            workers: usize::try_from(number("PDF_WORKERS", 2)?).context("PDF_WORKERS is too large")?,
            timeout: Duration::from_secs(timeout),
            soffice: optional("PDF_SOFFICE").map_or_else(|| PathBuf::from("soffice"), PathBuf::from),
            profile_template: optional("PDF_PROFILE_TEMPLATE").map(PathBuf::from),
            work_dir: optional("PDF_WORK_DIR").map_or_else(std::env::temp_dir, PathBuf::from),
        })
    }
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn required(name: &str) -> anyhow::Result<String> {
    match optional(name) {
        Some(v) => Ok(v),
        None => bail!("{name} is not set (for local development, run scripts/dev-up.sh and use scripts/dev.sh)"),
    }
}
