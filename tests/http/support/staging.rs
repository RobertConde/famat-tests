//! Staging files the way the hydrated pages do: each file in its own
//! multipart post of `csrf` and `file` to `/api/admin/uploads/stage`, which
//! answers JSON. The form then posts each file's `staged` ID instead.

use reqwest::multipart::{Form, Part};

use super::{TestApp, add_test::generated};

pub const STAGE: &str = "/api/admin/uploads/stage";

/// What a staging post answered.
pub struct StageAnswer {
    pub status: u16,
    pub json: serde_json::Value,
}

impl StageAnswer {
    /// The staged file's ID, asserting it was staged.
    pub fn id(&self) -> String {
        assert_eq!(self.status, 200, "staging: {}", self.json);
        self.json["staged"].as_str().expect("a staged ID").to_owned()
    }

    /// The refusal's message, asserting `status`.
    pub fn refusal(&self, status: u16) -> String {
        assert_eq!(self.status, status, "staging: {}", self.json);
        self.json["error"].as_str().expect("an error message").to_owned()
    }
}

/// A file to stage.
pub enum File<'a> {
    Bytes(&'a [u8]),
    /// That many bytes, starting with the prefix, generated as they are sent.
    Generated(&'a [u8], u64),
}

impl TestApp {
    /// Stage a file from this site, with a session cookie (if given).
    pub async fn stage_as(
        &self,
        cookie: Option<&str>,
        origin: &str,
        csrf: &str,
        name: &str,
        file: File<'_>,
    ) -> reqwest::Response {
        let part = match file {
            File::Bytes(bytes) => Part::bytes(bytes.to_vec()),
            File::Generated(prefix, len) => Part::stream_with_length(generated(prefix, len), len),
        };
        let form = Form::new().text("csrf", csrf.to_owned()).part(
            "file",
            part.file_name(name.to_owned())
                .mime_str("application/octet-stream")
                .unwrap(),
        );
        let mut request = self.http.post(self.url(STAGE)).header("origin", origin);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.multipart(form).send().await.expect("POST a file to stage")
    }

    /// Stage a file as an Admin on this site, and read the JSON answer.
    pub async fn stage(&self, cookie: &str, csrf: &str, name: &str, file: File<'_>) -> StageAnswer {
        let response = self.stage_as(Some(cookie), &self.base_url, csrf, name, file).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = response.text().await.expect("the whole answer");
        assert!(
            content_type.starts_with("application/json"),
            "{status} {content_type}: {body}"
        );
        StageAnswer {
            status,
            json: serde_json::from_str(&body).expect("JSON"),
        }
    }

    /// Stage a file's bytes, expecting it staged, and return its ID.
    pub async fn stage_bytes(&self, cookie: &str, csrf: &str, name: &str, bytes: &[u8]) -> String {
        self.stage(cookie, csrf, name, File::Bytes(bytes)).await.id()
    }

    /// The staged files' rows, as `(id, name, byte size)`, oldest first.
    pub async fn staged_rows(&self) -> Vec<(String, String, Option<i64>)> {
        sqlx::query_as("SELECT id, name, byte_size FROM staged_files ORDER BY created_at, id")
            .fetch_all(&self.db)
            .await
            .expect("read staged_files")
    }

    /// Make a staged file look `hours` old.
    pub async fn age_staged(&self, id: &str, hours: i32) {
        sqlx::query("UPDATE staged_files SET created_at = now() - make_interval(hours => $2) WHERE id = $1")
            .bind(id)
            .bind(hours)
            .execute(&self.db)
            .await
            .expect("age a staged file");
    }
}
