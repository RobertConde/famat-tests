//! Multipart upload endpoints: every answer, refusals included, reaches the
//! client in full.
//!
//! An endpoint often answers before it has read the whole body: no session,
//! a bad CSRF token, a file of the wrong type or over its size limit. If the
//! connection then closed with the rest of the body unread, the server's
//! kernel would answer the client's next bytes with a reset, and the client
//! (still sending) could lose the answer and see a connection error instead
//! (issue #47). So the rest of the body is read and discarded first:
//!
//! - [`UploadLimit`] wraps a multipart server function. It sets the body
//!   limit (like `DefaultBodyLimit::max`) and, once the function has
//!   answered, whichever way, reads what is left of the body before the
//!   answer goes out.
//! - [`answer_after_draining`] does the same for middleware that refuses a
//!   request before it reaches the endpoint (the Origin check).
//!
//! Draining stops at the limit. A body bigger than that is answered then,
//! with `Connection: close`, and read for up to [`LINGER`] more while the
//! answer goes out, so the client still gets it before the socket closes.
//!
//! A new upload endpoint takes the attribute in place of `DefaultBodyLimit`:
//!
//! ```ignore
//! #[post("/api/admin/…", headers: HeaderMap, state: Extension<AppState>)]
//! #[middleware(crate::server::upload::UploadLimit::max(MAX_BODY_BYTES))]
//! pub async fn upload_something(form: MultipartFormData) -> Result<Response, ServerFnError> { … }
//! ```
//!
//! and can then return from anywhere without reading the rest itself.

use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{Body, BodyDataStream, Bytes},
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, header},
    response::Response,
};
use futures::{Stream, StreamExt};
use tower_layer::Layer;
use tower_service::Service;

use crate::app::upload_rules::MAX_UPLOAD_BYTES;

/// The largest body any endpoint accepts: what middleware in front of every
/// route drains at most.
pub const MAX_BODY_BYTES: usize = MAX_UPLOAD_BYTES;

/// How long a body over the limit is still read after it was answered.
pub const LINGER: Duration = Duration::from_secs(10);

/// A layer for a multipart server function: a body limit of `max` bytes,
/// and the body read to its end (or to `max`) before the answer is sent.
#[derive(Clone, Copy, Debug)]
pub struct UploadLimit {
    max: usize,
}

impl UploadLimit {
    pub const fn max(max: usize) -> Self {
        Self { max }
    }
}

impl<S> Layer<S> for UploadLimit {
    type Service = Drained<<DefaultBodyLimit as Layer<S>>::Service>;

    fn layer(&self, inner: S) -> Self::Service {
        Drained {
            inner: DefaultBodyLimit::max(self.max).layer(inner),
            max: self.max,
        }
    }
}

/// The service [`UploadLimit`] makes.
#[derive(Clone, Debug)]
pub struct Drained<S> {
    inner: S,
    max: usize,
}

impl<S> Service<Request> for Drained<S>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        // The clone that was polled ready serves this request.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let max = self.max;
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let shared = Shared::new(body);
            let request = Request::from_parts(parts, Body::from_stream(shared.clone()));
            let Ok(response) = inner.call(request).await;
            let (rest, read) = shared.rest();
            Ok(drain(rest, read, max, response).await)
        })
    }
}

/// Read and discard the rest of `body`, up to `max` bytes, then return
/// `response`; past `max`, see the [module docs](self).
pub async fn answer_after_draining(body: Body, max: usize, response: Response) -> Response {
    drain(body.into_data_stream(), 0, max, response).await
}

async fn drain(mut rest: BodyDataStream, mut read: u64, max: usize, mut response: Response) -> Response {
    while read <= max as u64 {
        match rest.next().await {
            Some(Ok(chunk)) => read += chunk.len() as u64,
            // The end, or the client gave up: either way nothing is left.
            None | Some(Err(_)) => return response,
        }
    }
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    tokio::spawn(async move {
        let _ = tokio::time::timeout(LINGER, async { while let Some(Ok(_)) = rest.next().await {} }).await;
    });
    response
}

/// The request body, readable by the endpoint (through [`Body::from_stream`])
/// and afterwards by [`Drained`], counting what was read.
#[derive(Clone)]
struct Shared(Arc<Mutex<Inner>>);

struct Inner {
    body: BodyDataStream,
    read: u64,
}

impl Shared {
    fn new(body: Body) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            body: body.into_data_stream(),
            read: 0,
        })))
    }

    /// What is left of the body, and how much of it was read.
    fn rest(&self) -> (BodyDataStream, u64) {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let rest = std::mem::replace(&mut inner.body, Body::empty().into_data_stream());
        (rest, inner.read)
    }
}

impl Stream for Shared {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let polled = inner.body.poll_next_unpin(cx);
        if let Poll::Ready(Some(Ok(chunk))) = &polled {
            inner.read += chunk.len() as u64;
        }
        polled
    }
}
