// SPDX-License-Identifier: Apache-2.0

//! The NATS KV adapter of the generation record (SMA-821 § 5.2). It implements the `GenKv` port on one key of
//! the `GATEWAY_DECISIONS_GEN` bucket. async-nats 0.53 has no get-or-create for a bucket, so the adapter does
//! get, then create when the stream is not found. It reads with the direct get (`Store::entry`) only. Each call
//! takes the cached store handle out, and puts it back only after a success or a `WrongRevision`. So after any
//! other error, and after a call that the step timeout cancels, the next call makes the handle again (D9).

use async_nats::jetstream::{
    self, ErrorCode,
    context::{GetStreamErrorKind, KeyValueError},
    kv,
};
use bytes::Bytes;
use tokio::sync::Mutex;

use super::generation::{GenKv, KvEntry, KvError};
use super::jetstream::{GEN_BUCKET, gen_kv_config};

/// The generation KV entry of one Redis stream key, in the NATS bucket.
pub struct NatsGenKv {
    js: jetstream::Context,
    entry_key: String,
    store: Mutex<Option<kv::Store>>,
}

impl NatsGenKv {
    #[must_use]
    pub fn new(js: jetstream::Context, entry_key: String) -> Self {
        Self {
            js,
            entry_key,
            store: Mutex::new(None),
        }
    }

    /// Take the cached store out, or make a new one. A missing bucket is made with `gen_kv_config`. The cache stays
    /// empty until [`Self::finish`] puts the handle back, so a call that is dropped before it returns leaves no
    /// handle.
    async fn store(&self) -> Result<kv::Store, KvError> {
        let mut guard = self.store.lock().await;
        if let Some(store) = guard.take() {
            return Ok(store);
        }
        let store = match self.js.get_key_value(GEN_BUCKET).await {
            Ok(store) => store,
            Err(err) => match get_bucket_failure(&err) {
                BucketFailure::NotFound => self.js.create_key_value(gen_kv_config()).await.map_err(|e| {
                    // The account INFO call runs first. Its timeout comes wrapped as a JetStream error.
                    let info_timed_out = std::error::Error::source(&e)
                        .and_then(|s| s.downcast_ref::<jetstream::context::AccountError>())
                        .is_some_and(|a| a.kind() == jetstream::context::AccountErrorKind::TimedOut);
                    if info_timed_out || e.kind() == jetstream::context::CreateKeyValueErrorKind::TimedOut {
                        KvError::Timeout
                    } else {
                        KvError::NoBucket
                    }
                })?,
                BucketFailure::Timeout => return Err(KvError::Timeout),
                BucketFailure::Other => return Err(KvError::Other),
            },
        };
        Ok(store)
    }

    /// Put the store back after a success or a lost compare-and-set. After any other error it stays out.
    async fn finish<T>(&self, store: kv::Store, result: Result<T, KvError>) -> Result<T, KvError> {
        if matches!(result, Ok(_) | Err(KvError::WrongRevision)) {
            *self.store.lock().await = Some(store);
        }
        result
    }
}

enum BucketFailure {
    NotFound,
    Timeout,
    Other,
}

/// Classify a `get_key_value` error from the `GetStreamError` source: JetStream code 10059 is "not found", a
/// request that timed out is a timeout. Any other request error (for example no responders) is `Other`.
fn get_bucket_failure(err: &KeyValueError) -> BucketFailure {
    if err.kind() != jetstream::context::KeyValueErrorKind::GetBucket {
        return BucketFailure::Other;
    }
    let Some(source) = std::error::Error::source(err).and_then(|s| s.downcast_ref::<jetstream::context::GetStreamError>()) else {
        return BucketFailure::Other;
    };
    match source.kind() {
        GetStreamErrorKind::JetStream(e) if e.error_code() == ErrorCode::STREAM_NOT_FOUND => BucketFailure::NotFound,
        GetStreamErrorKind::Request
            if std::error::Error::source(source)
                .and_then(|s| s.downcast_ref::<jetstream::context::RequestError>())
                .is_some_and(|r| r.kind() == jetstream::context::RequestErrorKind::TimedOut) =>
        {
            BucketFailure::Timeout
        }
        _ => BucketFailure::Other,
    }
}

impl GenKv for NatsGenKv {
    async fn get(&self) -> Result<Option<KvEntry>, KvError> {
        let store = self.store().await?;
        let result = match store.entry(self.entry_key.clone()).await {
            Ok(entry) => Ok(entry.map(|e| KvEntry {
                value: e.value,
                revision: e.revision,
                live: e.operation == kv::Operation::Put,
            })),
            Err(e) => Err(if e.kind() == kv::EntryErrorKind::TimedOut { KvError::Timeout } else { KvError::Other }),
        };
        self.finish(store, result).await
    }

    async fn create(&self, value: Bytes) -> Result<u64, KvError> {
        let store = self.store().await?;
        let result = store.create(&self.entry_key, value).await.map_err(|e| match e.kind() {
            kv::CreateErrorKind::AlreadyExists => KvError::WrongRevision,
            kv::CreateErrorKind::Publish => KvError::Timeout,
            _ => KvError::Other,
        });
        self.finish(store, result).await
    }

    async fn update(&self, value: Bytes, revision: u64) -> Result<u64, KvError> {
        let store = self.store().await?;
        let result = store.update(&self.entry_key, value, revision).await.map_err(|e| match e.kind() {
            kv::UpdateErrorKind::WrongLastRevision => KvError::WrongRevision,
            kv::UpdateErrorKind::TimedOut => KvError::Timeout,
            _ => KvError::Other,
        });
        self.finish(store, result).await
    }
}
