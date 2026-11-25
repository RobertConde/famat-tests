//! `GET /download/zip?keys=a,b`: a plain axum route that streams a zip of
//! Garage objects. Each object is copied in 64 KiB chunks through a duplex
//! pipe into the response body, so memory stays flat whatever the file size.

use async_zip::{Compression, ZipEntryBuilder, tokio::write::ZipFileWriter};
use axum::{
    body::Body,
    extract::Query,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::AsyncWriteExt;
use serde::Deserialize;
use tokio_util::{compat::TokioAsyncReadCompatExt, io::ReaderStream};

use super::s3;

#[derive(Deserialize)]
pub struct ZipQuery {
    keys: String,
}

pub async fn download_zip(Query(q): Query<ZipQuery>) -> Response {
    let keys: Vec<String> = q.keys.split(',').filter(|k| !k.is_empty()).map(String::from).collect();
    if keys.is_empty() || keys.len() > 50 {
        return (StatusCode::BAD_REQUEST, "keys=a,b required").into_response();
    }
    // v1 would check every Document is Published here.

    // Open the first object before committing to a 200, so a missing key is a
    // real 404 rather than a truncated zip.
    let first = match s3::get(&keys[0]).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::NOT_FOUND, format!("{e:#}")).into_response(),
    };

    let (w, r) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Err(e) = write_zip(w, keys, first).await {
            // Headers are gone by now; the client sees a truncated download.
            dioxus::logger::tracing::error!("zip stream failed: {e:#}");
        }
    });

    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"documents.zip\"")
        .body(Body::from_stream(ReaderStream::new(r)))
        .unwrap()
}

async fn write_zip(
    w: tokio::io::DuplexStream,
    keys: Vec<String>,
    first: aws_sdk_s3::primitives::ByteStream,
) -> Result<(), anyhow::Error> {
    let mut zip = ZipFileWriter::with_tokio(w);
    let mut first = Some(first);
    for key in keys {
        let body = match first.take() {
            Some(b) => b,
            None => s3::get(&key).await?,
        };
        let name = key.rsplit('/').next().unwrap_or(&key).to_string();
        // PDFs are already compressed; Stored avoids burning CPU for nothing.
        let entry = ZipEntryBuilder::new(name.into(), Compression::Stored);
        let mut entry_w = zip.write_entry_stream(entry).await?;
        let mut reader = body.into_async_read().compat();
        futures::io::copy(&mut reader, &mut entry_w).await?;
        entry_w.close().await?;
    }
    let mut inner = zip.close().await?;
    inner.close().await?;
    Ok(())
}
