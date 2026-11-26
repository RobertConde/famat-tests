//! Uploading each file in its own request (issue #48).
//!
//! The site runs behind Cloudflare, which refuses any request over 100 MB,
//! so once a page has hydrated, choosing files in Add test, **+ Add files**
//! or **Replace original** sends each one at once to the staging endpoint
//! ([`stage_file`]), one at a time, showing its progress and any refusal.
//! The form then posts a `staged` field per file, in the files' order, in
//! place of the bytes (the file input loses its name), and the server saves
//! it exactly as it would have saved the files themselves.
//!
//! Without JavaScript none of this runs: the form posts the files natively,
//! all in one request of at most about 95 MB.
//!
//! The logic is server-only, in `crate::server::staging`.

#[allow(unused_imports)] // only the server-side extractors use them
use dioxus::fullstack::{HeaderMap, MultipartFormData, response::Response};
use dioxus::prelude::*;
use serde::Deserialize;

/// Where [`stage_file`] answers.
pub const STAGE_PATH: &str = "/api/admin/uploads/stage";

/// `POST /api/admin/uploads/stage`: one file to save later, a multipart post
/// of `csrf` (first) and `file`. Answers 200 with JSON `{"staged": id,
/// "name", "byte_size"}`; 401/403 without a session or token; 415/413 with
/// JSON `{"error": message}` for a file of the wrong type or over 90 MB.
#[post("/api/admin/uploads/stage",
    headers: HeaderMap,
    state: dioxus::fullstack::extract::Extension<crate::server::AppState>)]
#[middleware(crate::server::upload::UploadLimit::max(crate::app::upload_rules::MAX_ONE_FILE_BYTES))]
pub async fn stage_file(form: MultipartFormData) -> Result<Response, ServerFnError> {
    crate::server::staging::stage(&state, &headers, form).await
}

/// Where one chosen file is.
#[derive(Clone, Debug, PartialEq)]
pub enum Staging {
    /// Not sent: the page already refused it.
    Skipped,
    /// Sent once the files before it are in.
    Waiting,
    Sending {
        loaded: u64,
        total: u64,
    },
    /// In, under this ID.
    Staged(String),
    /// Refused or failed, with the message to show.
    Failed(String),
}

impl Staging {
    pub fn busy(&self) -> bool {
        matches!(self, Self::Waiting | Self::Sending { .. })
    }
}

/// The files' state, and which run of [`stage_files`] it belongs to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Staged {
    run: u64,
    pub files: Vec<Staging>,
}

impl Staged {
    /// Whether every file is in (and there is at least one).
    pub fn ready(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| matches!(f, Staging::Staged(_)))
    }

    /// Whether the form must wait: a file still going up, or one refused.
    pub fn blocked(&self) -> bool {
        !self.files.is_empty() && !self.ready()
    }

    /// The staged IDs, in the files' order, once [`Staged::ready`].
    pub fn ids(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|f| match f {
                Staging::Staged(id) => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
}

/// What the script reports.
#[derive(Deserialize)]
struct Report {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    index: usize,
    #[serde(default)]
    loaded: u64,
    #[serde(default)]
    total: u64,
    staged: Option<String>,
    /// The HTTP status of a failed upload, 0 when the connection failed.
    status: Option<u16>,
    error: Option<String>,
}

/// Sends the chosen files of `__INPUT__` one at a time, skipping the
/// indices in `__SKIP__`, with the CSRF token of the input's form. Choosing
/// again stops the previous run.
const SCRIPT: &str = r#"
const input = document.getElementById("__INPUT__");
const skip = [__SKIP__];
if (input && input.form) {
  if (input.famatStop) input.famatStop();
  let stopped = false, xhr = null;
  const stop = () => { stopped = true; if (xhr) xhr.abort(); };
  input.famatStop = stop;
  const csrf = input.form.elements.namedItem("csrf").value;
  const files = Array.from(input.files || []);
  for (let i = 0; i < files.length && !stopped; i++) {
    if (skip.includes(i)) continue;
    const file = files[i];
    const answer = await new Promise((resolve) => {
      xhr = new XMLHttpRequest();
      xhr.open("POST", "__PATH__");
      xhr.responseType = "json";
      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable) dioxus.send({ index: i, loaded: e.loaded, total: e.total });
      };
      xhr.onload = () => resolve({ status: xhr.status, body: xhr.response });
      xhr.onerror = () => resolve({ status: 0, body: null });
      xhr.onabort = () => resolve({ status: -1, body: null });
      const data = new FormData();
      data.append("csrf", csrf);
      data.append("file", file, file.name);
      xhr.send(data);
    });
    if (answer.status === -1) break;
    const body = answer.body || {};
    if (answer.status === 200 && body.staged) {
      dioxus.send({ index: i, staged: body.staged });
    } else {
      dioxus.send({ index: i, status: answer.status, error: typeof body.error === "string" ? body.error : null });
    }
  }
  if (input.famatStop === stop) input.famatStop = null;
}
dioxus.send({ done: true });
"#;

/// The message for a failed upload.
fn failure(status: u16, error: Option<String>, name: &str) -> String {
    if let Some(error) = error {
        return error;
    }
    match status {
        0 => format!("“{name}” didn’t upload: the connection failed. Choose the files again to retry."),
        401 => "You are not logged in any more. Log in again, then choose the files again.".to_owned(),
        403 => "This page has expired. Reload it and choose the files again.".to_owned(),
        // Cloudflare's own refusal, which isn't ours.
        413 => super::upload_rules::size_refusal(name),
        _ => format!("“{name}” didn’t upload. Choose the files again to retry."),
    }
}

/// Start sending the files chosen in the file input `input` (whose `names`
/// are given), except those at `skip`, recording their progress in
/// `staged`. A run started later takes over.
pub fn stage_files(input: &str, names: Vec<String>, skip: Vec<usize>, mut staged: Signal<Staged>) {
    let run = staged.peek().run + 1;
    staged.set(Staged {
        run,
        files: (0..names.len())
            .map(|i| {
                if skip.contains(&i) {
                    Staging::Skipped
                } else {
                    Staging::Waiting
                }
            })
            .collect(),
    });
    if names.len() == skip.len() {
        return;
    }
    // The ID is ours and the indices are digits, so both are safe to splice in.
    let skip_list = skip.iter().map(usize::to_string).collect::<Vec<_>>().join(",");
    let script = SCRIPT
        .replace("__INPUT__", input)
        .replace("__SKIP__", &skip_list)
        .replace("__PATH__", STAGE_PATH);
    spawn(async move {
        let mut eval = document::eval(&script);
        while let Ok(report) = eval.recv::<Report>().await {
            if report.done || staged.peek().run != run {
                break;
            }
            let mut state = staged.write();
            let Some(file) = state.files.get_mut(report.index) else {
                continue;
            };
            *file = match (report.staged, report.status) {
                (Some(id), _) => Staging::Staged(id),
                (None, Some(status)) => {
                    let name = names.get(report.index).map_or("", String::as_str);
                    Staging::Failed(failure(status, report.error, name))
                }
                (None, None) => Staging::Sending {
                    loaded: report.loaded,
                    total: report.total,
                },
            };
        }
    });
}

/// Record one chosen file the page refused, so nothing is sent, and stop any
/// run still sending.
pub fn refuse_file(mut staged: Signal<Staged>, message: String) {
    let run = staged.peek().run + 1;
    staged.set(Staged {
        run,
        files: vec![Staging::Failed(message)],
    });
}

/// One file's upload state, as a table cell's contents.
#[component]
pub fn StagingStatus(state: Staging) -> Element {
    match state {
        Staging::Skipped => rsx! {},
        Staging::Waiting => rsx! { span { class: "staging muted", "Waiting…" } },
        Staging::Sending { loaded, total } => {
            let percent = (loaded * 100).checked_div(total).unwrap_or(0);
            rsx! {
                span { class: "staging",
                    progress { value: "{loaded}", max: "{total.max(1)}", aria_label: "Uploaded {percent}%" }
                    " {percent}%"
                }
            }
        }
        Staging::Staged(_) => rsx! { span { class: "staging staged", "Uploaded" } },
        Staging::Failed(message) => rsx! { span { class: "staging refusal", "{message}" } },
    }
}
