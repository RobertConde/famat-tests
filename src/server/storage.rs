//! Garage, through its S3 API.
//!
//! Objects are keyed by entity, ID and role and never move or rename:
//! a Document's files live at `documents/{id}/original` and
//! `documents/{id}/pdf`. Keys are derived here, never stored in Postgres.
//!
//! Uploads stream: [`Storage::put_stream`] holds at most one multipart part
//! ([`PART_BYTES`]) of a file in memory, whatever its size.

use anyhow::Context;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation},
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use axum::body::Bytes;
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};

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

/// The size of each part of a multipart upload, and so the most of a file
/// held in memory. S3 wants every part but the last to be at least 5 MiB.
pub const PART_BYTES: usize = 8 * 1024 * 1024;

/// What [`Storage::put_stream`] stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub byte_size: i64,
    pub sha256: [u8; 32],
}

/// Why [`Storage::put_stream`] stored nothing.
#[derive(Debug)]
pub enum PutError<E> {
    /// The body went past the size limit.
    TooLarge,
    /// Reading the body failed (e.g. the upload was cut off).
    Body(E),
    /// Garage failed.
    Storage(anyhow::Error),
}

impl<E: std::fmt::Display> std::fmt::Display for PutError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "the file is too large"),
            Self::Body(e) => write!(f, "reading the upload: {e}"),
            Self::Storage(e) => write!(f, "{e:#}"),
        }
    }
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

    /// Stream a body into an object, counting and hashing it on the way, and
    /// refusing it once it passes `max_bytes`. A small body is one PUT; a
    /// larger one a multipart upload of [`PART_BYTES`] parts, aborted on any
    /// failure, so a refused or failed upload stores nothing at `key`.
    pub async fn put_stream<S, E>(
        &self,
        key: &str,
        content_type: &str,
        body: S,
        max_bytes: u64,
    ) -> Result<Stored, PutError<E>>
    where
        S: Stream<Item = Result<Bytes, E>>,
    {
        let mut reader = Reader {
            body: std::pin::pin!(body),
            hasher: Sha256::new(),
            total: 0,
            max_bytes,
            ended: false,
        };
        let mut buffer = Vec::with_capacity(PART_BYTES);
        reader.fill(&mut buffer).await?;
        if reader.ended {
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .content_type(content_type)
                .body(ByteStream::from(buffer))
                .send()
                .await
                .with_context(|| format!("writing {key} to Garage"))
                .map_err(PutError::Storage)?;
            return Ok(reader.finish());
        }

        let upload = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .send()
            .await
            .with_context(|| format!("starting a multipart upload of {key}"))
            .map_err(PutError::Storage)?;
        let upload_id = upload.upload_id().unwrap_or_default().to_owned();
        let completed = match self.upload_parts(key, &upload_id, &mut reader, buffer).await {
            Ok(parts) => self
                .client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(&upload_id)
                .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
                .send()
                .await
                .map(|_| ())
                .with_context(|| format!("completing the multipart upload of {key}"))
                .map_err(PutError::Storage),
            Err(e) => Err(e),
        };
        if let Err(e) = completed {
            let aborted = self
                .client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
            if let Err(abort) = aborted {
                dioxus::logger::tracing::warn!("aborting the multipart upload of {key}: {abort}");
            }
            return Err(e);
        }
        Ok(reader.finish())
    }

    async fn upload_parts<S, E>(
        &self,
        key: &str,
        upload_id: &str,
        reader: &mut Reader<S>,
        mut buffer: Vec<u8>,
    ) -> Result<Vec<CompletedPart>, PutError<E>>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
    {
        let mut parts = Vec::new();
        let mut number = 1;
        while !buffer.is_empty() {
            // Exactly one part; any overflow starts the next one.
            let rest = if buffer.len() > PART_BYTES {
                buffer.split_off(PART_BYTES)
            } else {
                Vec::with_capacity(PART_BYTES)
            };
            let part = std::mem::replace(&mut buffer, rest);
            let out = self
                .client
                .upload_part()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(number)
                .body(ByteStream::from(part))
                .send()
                .await
                .with_context(|| format!("uploading part {number} of {key}"))
                .map_err(PutError::Storage)?;
            parts.push(
                CompletedPart::builder()
                    .part_number(number)
                    .set_e_tag(out.e_tag().map(Into::into))
                    .build(),
            );
            number += 1;
            if !reader.ended {
                reader.fill(&mut buffer).await?;
            }
        }
        Ok(parts)
    }

    /// Copy an object within the bucket, server-side.
    pub async fn copy(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .copy_source(format!("{}/{from}", self.bucket))
            .key(to)
            .send()
            .await
            .with_context(|| format!("copying {from} to {to} in Garage"))?;
        Ok(())
    }

    /// Delete an object. Deleting one that does not exist succeeds.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("deleting {key} from Garage"))?;
        Ok(())
    }

    /// Where every Document's files live: the prefix of their keys.
    pub fn documents_prefix(&self) -> String {
        format!("{}documents/", self.key_prefix)
    }

    /// Every key starting with `prefix`.
    pub async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut pages = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            let page = page.with_context(|| format!("listing {prefix} in Garage"))?;
            keys.extend(page.contents().iter().filter_map(|o| o.key().map(str::to_owned)));
        }
        Ok(keys)
    }

    /// Read an object back and hash it, streaming. `Ok(None)` when it does
    /// not exist.
    pub async fn stored(&self, key: &str) -> anyhow::Result<Option<Stored>> {
        let Some(object) = self.get(key).await? else {
            return Ok(None);
        };
        let mut body = object.body;
        let mut hasher = Sha256::new();
        let mut byte_size = 0;
        while let Some(chunk) = body
            .try_next()
            .await
            .with_context(|| format!("reading {key} from Garage"))?
        {
            byte_size += chunk.len() as i64;
            hasher.update(&chunk);
        }
        Ok(Some(Stored {
            byte_size,
            sha256: hasher.finalize().into(),
        }))
    }

    /// Delete every file a Document may have, logging rather than returning
    /// failures: for cleaning up after an upload that was not saved.
    pub async fn delete_document_files(&self, document_id: i64) {
        for role in [FileRole::Original, FileRole::Pdf] {
            let key = self.document_key(document_id, role);
            if let Err(e) = self.delete(&key).await {
                dioxus::logger::tracing::warn!("{e:#}");
            }
        }
    }
}

/// A body being read into parts, counted and hashed.
struct Reader<S> {
    body: S,
    hasher: Sha256,
    total: u64,
    max_bytes: u64,
    ended: bool,
}

impl<S, E> Reader<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    /// Read until `buffer` holds a whole part or the body ends.
    async fn fill(&mut self, buffer: &mut Vec<u8>) -> Result<(), PutError<E>> {
        while buffer.len() < PART_BYTES {
            match self.body.next().await {
                Some(Ok(chunk)) => {
                    self.total += chunk.len() as u64;
                    if self.total > self.max_bytes {
                        return Err(PutError::TooLarge);
                    }
                    self.hasher.update(&chunk);
                    buffer.extend_from_slice(&chunk);
                }
                Some(Err(e)) => return Err(PutError::Body(e)),
                None => {
                    self.ended = true;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Stored {
        Stored {
            byte_size: self.total as i64,
            sha256: self.hasher.finalize().into(),
        }
    }
}
