// SPDX-License-Identifier: Apache-2.0
//! SMA-856: a mock webhook provider on a loopback port. It checks the request MAC, signs its
//! answer, records every request and lets a test script the answer of each call.
//!
//! The file is self-contained: `tests/tool_policy_iam_source.rs` includes it with `#[path]`, and
//! `tests/support/mod.rs` declares it as a module. A test binary uses only part of the interface,
//! so the dead-code lint is off for the whole file (ruling C6).
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, Response};
use axum::routing::post;
use paigasus_gateway::domain::webhook_protocol::{request_mac, response_mac};
use serde_json::Value;
use tokio::sync::Barrier;
use tokio::task::JoinHandle;

/// What the mock does with one request.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    /// Sign the answer with this secret instead of the secret of the provider (a bad signature).
    pub sign_with: Option<String>,
    /// Wait this long before the answer.
    pub delay: Duration,
    /// Extra headers. A `Paigasus-Signature` or `Content-Type` here replaces the default.
    pub headers: Vec<(String, String)>,
}

impl Reply {
    /// A 200 answer with `body`, signed with the secret of the provider.
    pub fn signed(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            sign_with: None,
            delay: Duration::ZERO,
            headers: Vec::new(),
        }
    }

    /// The same answer with another status.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// The same answer, signed with `secret`.
    pub fn signed_with(mut self, secret: &str) -> Self {
        self.sign_with = Some(secret.to_owned());
        self
    }

    /// The same answer after `delay`.
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// The same answer with one more header.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// One request that the mock received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub body: Value,
    pub raw: Vec<u8>,
    /// The request MAC verified with the secret of the provider.
    pub signature_ok: bool,
}

type Script = dyn Fn(usize, &Value) -> Reply + Send + Sync;

struct Shared {
    name: String,
    secret: String,
    script: Box<Script>,
    seen: Mutex<Vec<Seen>>,
    barrier: Mutex<Option<Arc<Barrier>>>,
}

/// A running mock provider. Dropping it stops the server.
pub struct MockProvider {
    /// The URL that a `[[tool_policy.detectors.webhook]]` table names.
    pub url: String,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockProvider {
    /// Start a provider called `name` that signs with `secret` and answers each request with
    /// `script(index, body)`. The index counts requests from 0. Call it inside a Tokio runtime.
    pub fn spawn(name: &str, secret: &str, script: impl Fn(usize, &Value) -> Reply + Send + Sync + 'static) -> MockProvider {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        std_listener.set_nonblocking(true).expect("nonblocking");
        let port = std_listener.local_addr().expect("addr").port();
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("listener");
        let shared = Arc::new(Shared {
            name: name.to_owned(),
            secret: secret.to_owned(),
            script: Box::new(script),
            seen: Mutex::new(Vec::new()),
            barrier: Mutex::new(None),
        });
        let app = Router::new().route("/hook", post(handle)).with_state(Arc::clone(&shared));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        MockProvider {
            url: format!("http://127.0.0.1:{port}/hook"),
            shared,
            task,
        }
    }

    /// Every request waits on `barrier` before it answers. It proves that requests overlap.
    pub fn with_barrier(self, barrier: Arc<Barrier>) -> Self {
        *self.shared.barrier.lock().unwrap() = Some(barrier);
        self
    }

    /// The requests that the mock received, in arrival order.
    pub fn requests(&self) -> Vec<Seen> {
        self.shared.seen.lock().unwrap().clone()
    }
}

async fn handle(State(shared): State<Arc<Shared>>, req: Request<Body>) -> Response<Body> {
    let (parts, body) = req.into_parts();
    let raw = axum::body::to_bytes(body, 8 * 1024 * 1024).await.map(|b| b.to_vec()).unwrap_or_default();
    let nonce = header(&parts.headers, "paigasus-nonce");
    let signature_ok = check_request(&shared, &parts.headers, &nonce, &raw);
    let value: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let index = {
        let mut seen = shared.seen.lock().unwrap();
        seen.push(Seen {
            body: value.clone(),
            raw,
            signature_ok,
        });
        seen.len() - 1
    };
    let barrier = shared.barrier.lock().unwrap().clone();
    if let Some(b) = barrier {
        b.wait().await;
    }
    let reply = (shared.script)(index, &value);
    if !reply.delay.is_zero() {
        tokio::time::sleep(reply.delay).await;
    }
    let key = reply.sign_with.as_deref().unwrap_or(&shared.secret);
    let mac = response_mac(key.as_bytes(), &shared.name, &nonce, reply.body.as_bytes());
    let mut builder = Response::builder()
        .status(reply.status)
        .header("content-type", "application/json")
        .header("paigasus-signature", format!("v1={mac}"));
    for (k, v) in &reply.headers {
        if let Some(h) = builder.headers_mut() {
            h.remove(k.as_str());
        }
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder.body(Body::from(reply.body)).expect("response")
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
}

/// Verify `Paigasus-Signature: t=<secs>,v1=<hex>` against the secret of the provider.
fn check_request(shared: &Shared, headers: &HeaderMap, nonce: &str, raw: &[u8]) -> bool {
    let sig = header(headers, "paigasus-signature");
    let Some((t, v1)) = sig.split_once(",v1=") else { return false };
    let Some(t) = t.strip_prefix("t=").and_then(|t| t.parse::<u64>().ok()) else { return false };
    v1 == request_mac(shared.secret.as_bytes(), &shared.name, t, nonce, raw)
}
