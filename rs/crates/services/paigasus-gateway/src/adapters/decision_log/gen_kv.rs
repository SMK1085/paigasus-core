// SPDX-License-Identifier: Apache-2.0

//! The NATS KV adapter of the generation record (SMA-821 § 5.2). It implements the `GenKv` port on one key of
//! the `GATEWAY_DECISIONS_GEN` bucket. async-nats 0.53 has no get-or-create for a bucket, so the adapter does
//! get, then create when the stream is not found. It reads with the direct get (`Store::entry`) only. It drops
//! the cached store handle after every error except `WrongRevision`, so the next call makes the handle again
//! (D9).

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

    /// The cached store, or a new one. A missing bucket is made with `gen_kv_config`.
    async fn store(&self) -> Result<kv::Store, KvError> {
        let mut guard = self.store.lock().await;
        if let Some(store) = guard.as_ref() {
            return Ok(store.clone());
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
        *guard = Some(store.clone());
        Ok(store)
    }

    /// Drop the cached store after an error that is not a lost compare-and-set.
    async fn after(&self, error: KvError) -> KvError {
        if error != KvError::WrongRevision {
            *self.store.lock().await = None;
        }
        error
    }
}

enum BucketFailure {
    NotFound,
    Timeout,
    Other,
}

/// Classify a `get_key_value` error from the `GetStreamError` source: JetStream code 10059 is "not found", a
/// request error is a timeout.
fn get_bucket_failure(err: &KeyValueError) -> BucketFailure {
    if err.kind() != jetstream::context::KeyValueErrorKind::GetBucket {
        return BucketFailure::Other;
    }
    let Some(source) = std::error::Error::source(err).and_then(|s| s.downcast_ref::<jetstream::context::GetStreamError>()) else {
        return BucketFailure::Other;
    };
    match source.kind() {
        GetStreamErrorKind::JetStream(e) if e.error_code() == ErrorCode::STREAM_NOT_FOUND => BucketFailure::NotFound,
        GetStreamErrorKind::Request => BucketFailure::Timeout,
        _ => BucketFailure::Other,
    }
}

impl GenKv for NatsGenKv {
    async fn get(&self) -> Result<Option<KvEntry>, KvError> {
        let store = self.store().await?;
        match store.entry(self.entry_key.clone()).await {
            Ok(entry) => Ok(entry.map(|e| KvEntry {
                value: e.value,
                revision: e.revision,
                live: e.operation == kv::Operation::Put,
            })),
            Err(e) => Err(self.after(if e.kind() == kv::EntryErrorKind::TimedOut { KvError::Timeout } else { KvError::Other }).await),
        }
    }

    async fn create(&self, value: Bytes) -> Result<u64, KvError> {
        let store = self.store().await?;
        match store.create(&self.entry_key, value).await {
            Ok(revision) => Ok(revision),
            Err(e) => Err(self
                .after(match e.kind() {
                    kv::CreateErrorKind::AlreadyExists => KvError::WrongRevision,
                    kv::CreateErrorKind::Publish => KvError::Timeout,
                    _ => KvError::Other,
                })
                .await),
        }
    }

    async fn update(&self, value: Bytes, revision: u64) -> Result<u64, KvError> {
        let store = self.store().await?;
        match store.update(&self.entry_key, value, revision).await {
            Ok(revision) => Ok(revision),
            Err(e) => Err(self
                .after(match e.kind() {
                    kv::UpdateErrorKind::WrongLastRevision => KvError::WrongRevision,
                    kv::UpdateErrorKind::TimedOut => KvError::Timeout,
                    _ => KvError::Other,
                })
                .await),
        }
    }
}
