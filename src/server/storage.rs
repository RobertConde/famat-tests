//! Garage, through its S3 API.
//!
//! Objects are keyed by entity, ID and role and never move or rename:
//! a Document's files live at `documents/{id}/original` and
//! `documents/{id}/pdf`. Keys are derived here, never stored in Postgres.

use anyhow::Context;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation},
    primitives::ByteStream,
};

use super::config::StorageConfig;

#[derive(Clone)]
pub struct Storage {
    client: Client,
    bucket: String,
    key_prefix: String,
}

/// Which of a Document's files an object holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRole {
    Original,
    Pdf,
}

impl FileRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Pdf => "pdf",
        }
    }
}

/// An object being read from Garage. The body streams; nothing is buffered.
pub struct Object {
    pub body: ByteStream,
    pub content_length: Option<i64>,
}

impl Storage {
    pub fn new(config: &StorageConfig) -> Self {
        let credentials = Credentials::new(
            &config.access_key_id,
            &config.secret_access_key,
            None,
            None,
            "famat-config",
        );
        let s3_config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .endpoint_url(&config.endpoint)
            .credentials_provider(credentials)
            .force_path_style(true)
            // Newer SDKs add CRC checksums to every PUT by default; only send
            // them when an operation requires it.
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .build();
        Self {
            client: Client::from_conf(s3_config),
            bucket: config.bucket.clone(),
            key_prefix: config.key_prefix.clone(),
        }
    }

    /// The object key of one of a Document's files.
    pub fn document_key(&self, document_id: i64, role: FileRole) -> String {
        format!("{}documents/{document_id}/{}", self.key_prefix, role.as_str())
    }

    /// Start reading an object. `Ok(None)` when it does not exist, so callers
    /// can answer a real 404 before committing to a response.
    pub async fn get(&self, key: &str) -> anyhow::Result<Option<Object>> {
        match self.client.get_object().bucket(&self.bucket).key(key).send().await {
            Ok(out) => Ok(Some(Object {
                content_length: out.content_length(),
                body: out.body,
            })),
            Err(e) if e.as_service_error().is_some_and(|e| e.is_no_such_key()) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {key} from Garage")),
        }
    }
}
