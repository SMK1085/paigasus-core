// SPDX-License-Identifier: Apache-2.0

//! The NATS reader of `paigasus-gateway decisions export` (SMA-755 § 5.11, P-13). It reads the events of one
//! org from `GATEWAY_DECISIONS` with an ordered, ephemeral pull consumer, and `run` is the whole command.
//! An error text never holds the NATS URL, because a URL can hold credentials.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use async_nats::jetstream::{self, consumer::DeliverPolicy, consumer::pull::OrderedConfig};
use futures::StreamExt;
use time::OffsetDateTime;

use super::jetstream::{STREAM, subject_for};
use crate::application::decision_export::{Exit, ExportArgs, Exported, USAGE, export, parse_args};

/// The inbox prefix of `gateway-decision-reader` (P-13).
pub const READER_INBOX_PREFIX: &str = "_INBOX_GW_RD";
/// The client name that the broker shows for this connection.
const CLIENT_NAME: &str = "paigasus-gateway-decision-export";
/// How long the reader waits for the next message of the stream.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The environment variable of the NATS URL.
pub const URL_ENV: &str = "PAIGASUS_NATS_URL";

/// Why the read failed.
#[derive(Debug, thiserror::Error)]
pub enum ReaderError {
    /// No URL. Exit 1.
    #[error("no NATS URL: give --nats-url-file or set {URL_ENV}")]
    NoUrl,
    /// The raw message count passed `2 x --max-events` before the end of the read. Exit 4.
    #[error("more than twice --max-events messages in the read window: use a smaller range")]
    TooMany,
    /// A file could not be read. The text names the path, never the content.
    #[error("{what} {path} could not be read")]
    File { what: &'static str, path: String },
    /// A step of the NATS exchange failed. The text holds the step and the error kind only.
    #[error("NATS error: {0}")]
    Nats(String),
}

impl ReaderError {
    fn exit(&self) -> Exit {
        match self {
            ReaderError::NoUrl => Exit::BadArgs,
            ReaderError::TooMany => Exit::TooMany,
            ReaderError::File { .. } | ReaderError::Nats(_) => Exit::Nats,
        }
    }
}

fn nats(step: &str, kind: impl std::fmt::Debug) -> ReaderError {
    ReaderError::Nats(format!("{step}: {kind:?}"))
}

fn nats_url(args: &ExportArgs) -> Result<String, ReaderError> {
    let url = match &args.nats_url_file {
        Some(path) => std::fs::read_to_string(path).map_err(|_| ReaderError::File {
            what: "--nats-url-file",
            path: path.display().to_string(),
        })?,
        None => std::env::var(URL_ENV).map_err(|_| ReaderError::NoUrl)?,
    };
    let url = url.trim().to_owned();
    if url.is_empty() { Err(ReaderError::NoUrl) } else { Ok(url) }
}

async fn connect(args: &ExportArgs, url: &str) -> Result<async_nats::Client, ReaderError> {
    let mut opts = match &args.creds {
        Some(path) => {
            let path = path.display().to_string();
            let raw = tokio::fs::read_to_string(&path).await.map_err(|_| ReaderError::File { what: "--creds", path: path.clone() })?;
            paigasus_nats_creds::parse_credentials(&raw).map_err(|_| ReaderError::File { what: "--creds", path: path.clone() })?;
            async_nats::ConnectOptions::with_auth_callback(move |nonce| {
                let path = path.clone();
                async move { paigasus_nats_creds::auth_from_credentials(&path, &nonce).await }
            })
        }
        None => async_nats::ConnectOptions::new(),
    };
    opts = opts.name(CLIENT_NAME).custom_inbox_prefix(READER_INBOX_PREFIX);
    if let Some(ca) = &args.ca {
        opts = opts.add_root_certificates(PathBuf::from(ca));
    }
    opts.connect(url).await.map_err(|e| nats("connect", e.kind()))
}

/// What the reader returns: the events, and the time bound when the read stopped at it (R13).
#[derive(Debug)]
pub struct ReadResult {
    pub events: Vec<Exported>,
    /// `Some(until + max_lag)` when a message stored after that time ended the read, so that the stream holds
    /// later messages. `None` when the read reached the end of the stream, the count `num_pending` or the raw cap.
    pub stopped_at: Option<SystemTime>,
}

/// Read the events of `args.org` that JetStream stored from `since - max_lag` on, in stream order. The end of
/// the read is the first of three limits (P-13):
/// - the `num_pending` of the consumer at its creation;
/// - the first message that JetStream stored after `until + max_lag` (symmetric to the start at
///   `since - max_lag`; that message is not returned). The summary warns when this ended the read (R5, R13);
/// - a raw count above `2 x max_events` (saturating), which is `ReaderError::TooMany` (exit 4). This keeps
///   the buffer bounded before `export` makes its exact check of the unique events in the range.
///
/// # Errors
///
/// [`ReaderError`] when the URL, the connection, the consumer or the read fails.
pub async fn read(args: &ExportArgs) -> Result<Vec<Exported>, ReaderError> {
    read_bounded(args).await.map(|r| r.events)
}

/// [`read`], with the time bound that ended the read, for the summary warning (R13).
///
/// # Errors
///
/// [`ReaderError`] when the URL, the connection, the consumer or the read fails.
pub async fn read_bounded(args: &ExportArgs) -> Result<ReadResult, ReaderError> {
    let url = nats_url(args)?;
    let client = connect(args, &url).await?;
    let js = jetstream::new(client);
    let subject = subject_for(&args.org.to_string()).ok_or_else(|| ReaderError::Nats("the org has no subject".to_owned()))?;
    let start = args.since.checked_sub(args.max_lag).unwrap_or(SystemTime::UNIX_EPOCH);
    let stream = js.get_stream_no_info(STREAM).await.map_err(|e| nats("get stream", e.kind()))?;
    let consumer = stream
        .create_consumer(OrderedConfig {
            filter_subject: subject,
            deliver_policy: DeliverPolicy::ByStartTime {
                start_time: OffsetDateTime::from(start),
            },
            ..Default::default()
        })
        .await
        .map_err(|e| nats("create consumer", e.kind()))?;
    let pending = consumer.cached_info().num_pending;
    let mut out = Vec::new();
    if pending == 0 {
        return Ok(ReadResult { events: out, stopped_at: None });
    }
    let mut messages = consumer.messages().await.map_err(|e| nats("start read", e.kind()))?;
    let cap = args.max_events.saturating_mul(2);
    // `None` when the bound does not fit into a `SystemTime`: then there is no time bound.
    let stop_after = args.until.checked_add(args.max_lag);
    while (out.len() as u64) < pending {
        let next = tokio::time::timeout(IDLE_TIMEOUT, messages.next())
            .await
            .map_err(|_| ReaderError::Nats("read: no message for 30 s".to_owned()))?;
        let msg = match next {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => return Err(nats("read", e.kind())),
            None => return Err(ReaderError::Nats("read: the stream ended".to_owned())),
        };
        let info = msg.info().map_err(|e| ReaderError::Nats(format!("read: bad message info: {e}")))?;
        let stored_at = SystemTime::from(info.published);
        if let Some(bound) = stop_after.filter(|bound| stored_at > *bound) {
            return Ok(ReadResult { events: out, stopped_at: Some(bound) });
        }
        out.push(Exported {
            stored_at,
            bytes: msg.message.payload.clone(),
        });
        if out.len() > cap {
            return Err(ReaderError::TooMany);
        }
    }
    Ok(ReadResult { events: out, stopped_at: None })
}

fn write_all(args: &ExportArgs, lines: &[String]) -> std::io::Result<()> {
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    match &args.out {
        Some(path) => std::fs::write(path, text),
        None => std::io::stdout().lock().write_all(text.as_bytes()),
    }
}

/// The whole command, after `decisions export`: read, export, write. The trace goes to `--out` or stdout. The
/// summary goes to stderr and to `--report`.
pub async fn run(args: Vec<String>) -> Exit {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(text) => {
            eprintln!("error: {text}\n{USAGE}");
            return Exit::BadArgs;
        }
    };
    let read = match read_bounded(&args).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return e.exit();
        }
    };
    let mut result = match export(&args, read.events) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return e.exit();
        }
    };
    result.summary.read_stopped_at = read.stopped_at;
    let summary = result.summary.to_string();
    if let Err(e) = write_all(&args, &result.lines) {
        eprintln!("error: the trace could not be written: {e}");
        return Exit::Nats;
    }
    eprint!("{summary}");
    if let Some(path) = &args.report
        && let Err(e) = std::fs::write(path, &summary)
    {
        eprintln!("error: the report could not be written: {e}");
        return Exit::Nats;
    }
    Exit::Done
}
