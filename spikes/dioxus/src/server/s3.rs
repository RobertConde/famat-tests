//! Garage via aws-sdk-s3. Uploads stream into S3 multipart parts so the
//! server holds at most one part (8 MiB) per upload in memory.

use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation},
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use axum::{
    body::Body,
    extract::multipart::Field,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use dioxus::fullstack::MultipartFormData;
use std::sync::LazyLock;

use super::auth;

const PART: usize = 8 * 1024 * 1024;

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("set {k} (see garage/up.sh)"))
}

pub static BUCKET: LazyLock<String> = LazyLock::new(|| env("S3_BUCKET"));

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    let creds = Credentials::new(env("S3_ACCESS_KEY_ID"), env("S3_SECRET_ACCESS_KEY"), None, None, "env");
    let conf = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(env("S3_REGION")))
        .endpoint_url(env("S3_ENDPOINT"))
        .credentials_provider(creds)
        .force_path_style(true)
        // Newer SDKs add CRC checksums to every PUT by default; only send
        // them when an operation requires it.
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .build();
    Client::from_conf(conf)
});

pub fn init() {
    LazyLock::force(&BUCKET);
    LazyLock::force(&CLIENT);
}

pub async fn list() -> Result<Vec<(String, i64)>, anyhow::Error> {
    let out = CLIENT.list_objects_v2().bucket(&*BUCKET).send().await?;
    Ok(out
        .contents()
        .iter()
        .map(|o| (o.key().unwrap_or_default().to_string(), o.size().unwrap_or(0)))
        .collect())
}

pub async fn get(key: &str) -> Result<ByteStream, anyhow::Error> {
    Ok(CLIENT.get_object().bucket(&*BUCKET).key(key).send().await?.body)
}

fn plain(status: StatusCode, msg: impl Into<String>) -> Response {
    Response::builder().status(status).body(Body::from(msg.into())).unwrap()
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '_' })
        .collect();
    if s.is_empty() || s.starts_with('.') { format!("file{s}") } else { s }
}

pub async fn upload_form(headers: &HeaderMap, mut form: MultipartFormData) -> Response {
    if !auth::is_admin(headers) {
        return auth::see_other("/login", None);
    }
    let mut csrf_ok = false;
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return plain(StatusCode::BAD_REQUEST, format!("multipart: {e}")),
        };
        match field.name() {
            Some("csrf") => {
                let v = field.text().await.unwrap_or_default();
                csrf_ok = auth::verify_csrf(headers, &v);
            }
            Some("file") => {
                if !csrf_ok {
                    return plain(StatusCode::FORBIDDEN, "bad or missing CSRF token");
                }
                let key = format!("uploads/{}", sanitize(field.file_name().unwrap_or("upload")));
                let ct = field.content_type().unwrap_or("application/octet-stream").to_string();
                return match put_streaming(&key, &ct, field).await {
                    Ok(n) => {
                        tracing_info(&format!("uploaded {key} ({n} bytes)"));
                        auth::see_other(&format!("/admin?uploaded={key}"), None)
                    }
                    Err(e) => plain(StatusCode::BAD_GATEWAY, format!("upload failed: {e:#}")),
                };
            }
            _ => {}
        }
    }
    plain(StatusCode::BAD_REQUEST, "no file field")
}

fn tracing_info(msg: &str) {
    dioxus::logger::tracing::info!("{msg}");
}

async fn next_part(field: &mut Field<'_>, buf: &mut Vec<u8>) -> Result<bool, anyhow::Error> {
    while buf.len() < PART {
        match field.chunk().await? {
            Some(c) => buf.extend_from_slice(&c),
            None => return Ok(false),
        }
    }
    Ok(true)
}

/// Stream a multipart field into Garage. Returns the byte count.
async fn put_streaming(key: &str, ct: &str, mut field: Field<'_>) -> Result<u64, anyhow::Error> {
    // Debug switch used to find where upload memory goes (see FINDINGS.md).
    if std::env::var_os("SPIKE_DRAIN_UPLOAD").is_some() {
        let mut n = 0u64;
        while let Some(c) = field.chunk().await? {
            n += c.len() as u64;
        }
        return Ok(n);
    }
    let mut buf = Vec::with_capacity(PART);
    let more = next_part(&mut field, &mut buf).await?;
    if !more {
        let n = buf.len() as u64;
        CLIENT
            .put_object()
            .bucket(&*BUCKET)
            .key(key)
            .content_type(ct)
            .body(ByteStream::from(buf))
            .send()
            .await?;
        return Ok(n);
    }

    let up = CLIENT.create_multipart_upload().bucket(&*BUCKET).key(key).content_type(ct).send().await?;
    let id = up.upload_id().unwrap_or_default().to_string();
    let result: Result<(u64, Vec<CompletedPart>), anyhow::Error> = async {
        let mut parts = vec![];
        let mut total = 0u64;
        let mut more = true;
        let mut n = 1;
        while !buf.is_empty() {
            // Take exactly one part, keeping any overflow for the next one.
            let rest = if buf.len() > PART { buf.split_off(PART) } else { Vec::new() };
            let part = std::mem::replace(&mut buf, rest);
            total += part.len() as u64;
            let out = CLIENT
                .upload_part()
                .bucket(&*BUCKET)
                .key(key)
                .upload_id(&id)
                .part_number(n)
                .body(ByteStream::from(part))
                .send()
                .await?;
            parts.push(CompletedPart::builder().part_number(n).set_e_tag(out.e_tag().map(Into::into)).build());
            n += 1;
            if more {
                more = next_part(&mut field, &mut buf).await?;
            }
        }
        Ok((total, parts))
    }
    .await;

    match result {
        Ok((total, parts)) => {
            CLIENT
                .complete_multipart_upload()
                .bucket(&*BUCKET)
                .key(key)
                .upload_id(&id)
                .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
                .send()
                .await?;
            Ok(total)
        }
        Err(e) => {
            let _ = CLIENT.abort_multipart_upload().bucket(&*BUCKET).key(key).upload_id(&id).send().await;
            Err(e)
        }
    }
}
