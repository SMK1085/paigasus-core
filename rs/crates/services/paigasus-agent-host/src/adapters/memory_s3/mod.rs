// SPDX-License-Identifier: Apache-2.0

//! The S3-compatible `MemoryStore` adapter (SMA-829).
//!
//! The adapter keeps each memory file as one object. The object key comes from `keys`.
//!
//! # The races (spec §6.7)
//!
//! The adapter is correct for one writer at a time. These races remain when two writers write
//! one scope at the same moment:
//!
//! 1. The guard rule: a writer can add a file under a guarded directory between the pre-check
//!    and the writes, and that file then stays.
//! 2. The store caps: two writers can both pass the cap check, so the store can go over a cap.
//! 3. The delete: a writer that changes an object between the `HEAD` and the `DELETE` loses its
//!    change, because the delete removes the newer object.
//! 4. Partial sets: a conflict between the pre-check and the writes gives `Partial`.
//! 5. The erasure: a write that lands after the last listing of `delete_scope` survives.
//!
//! A `Put` has no race: the store checks its condition.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use object_store::path::Path;
use object_store::{Attribute, Attributes, Error, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, UpdateVersion};
use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use paigasus_agent_host_core::ports::{MemoryApplyError, MemoryEntry, MemoryFile, MemoryStore, MemoryStoreError, MemoryUsage};
use paigasus_observability::names;
use tokio::time::Instant;

use self::envelope::{WriteId, content_size, decode, encode};
use self::errors::{TIMEOUT_CLASS, class, is_refusal, store_text};
use self::keys::{KeyPrefix, ListedKey, dir_path, file_path, listed_key, scope_path};
use self::plan::{Outcome, Seen, SeenAfterDelete, Snapshot, delete_outcome, precheck, put_outcome, result_of};

pub mod envelope;
pub mod errors;
pub mod keys;
pub mod plan;
#[cfg(test)]
pub mod testing;

/// The most delete requests that run at the same time in `delete_scope`.
const DELETE_CONCURRENCY: usize = 8;

/// The most writes that run at the same time inside one phase of `apply`.
const WRITE_CONCURRENCY: usize = 8;

/// The content type of each object. It is for operators only.
const CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// Count one skipped or refused object. The label is a fixed word, never a key.
fn count_skipped(reason: &'static str) {
    metrics::counter!(names::AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL, "reason" => reason).increment(1);
}

/// The version of a file, from the ETag of the store. The ETag is the version as the store
/// sends it.
fn version_of(e_tag: Option<&str>) -> Option<MemoryVersion> {
    e_tag.map(MemoryVersion::new)
}

/// The text of the error when the apply deadline expired.
fn deadline_text() -> String {
    format!("the apply deadline expired ({TIMEOUT_CLASS})")
}

/// The text of the error for an object that has no ETag.
const NO_ETAG: &str = "the object has no ETag";

/// The error of a failed listing. A key that `object_store` refuses is counted.
fn listing_error(e: &Error) -> MemoryStoreError {
    if class(e) == "unparsable_key" {
        count_skipped("unparsable");
    }
    MemoryStoreError::Store(store_text("the memory listing failed", e))
}

/// The class of the cause of a `Partial` result, for the log line. A `Store` text of this
/// adapter is a fixed sentence and a class, so it holds no path, key or content.
fn cause_class(e: &MemoryApplyError) -> &str {
    match e {
        MemoryApplyError::Conflict { .. } => "conflict",
        MemoryApplyError::CapExceeded(_) => "cap",
        MemoryApplyError::Partial { .. } => "partial",
        MemoryApplyError::Store(text) => text,
    }
}

/// The memory store over an object store.
#[derive(Clone)]
pub struct S3MemoryStore {
    store: Arc<dyn ObjectStore>,
    prefix: KeyPrefix,
    apply_deadline: Duration,
}

impl fmt::Debug for S3MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3MemoryStore")
            .field("prefix", &self.prefix.as_str())
            .field("apply_deadline", &self.apply_deadline)
            .finish_non_exhaustive()
    }
}

impl S3MemoryStore {
    /// Make a store over an object store.
    pub fn new(store: Arc<dyn ObjectStore>, prefix: KeyPrefix, apply_deadline: Duration) -> Self {
        S3MemoryStore { store, prefix, apply_deadline }
    }

    /// List the files under `list_base`, a scope path or a directory path inside the scope. Each
    /// path is the full path in the scope. A key that is not a memory file, or is too short, is
    /// skipped and counted. The result is in byte order of the path.
    async fn listing(&self, scope_base: &Path, list_base: &Path) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        let mut stream = self.store.list(Some(list_base));
        let mut entries = Vec::new();
        while let Some(item) = stream.next().await {
            let meta = item.map_err(|e| listing_error(&e))?;
            let outside = || MemoryStoreError::Store("the store listed a key outside the scope".into());
            // The key must be under the directory. The path rules apply to the full path.
            listed_key(list_base, &meta.location).map_err(|_| outside())?;
            let path = match listed_key(scope_base, &meta.location) {
                Err(_) => return Err(outside()),
                Ok(ListedKey::Refused) => {
                    count_skipped("key");
                    continue;
                }
                Ok(ListedKey::File(path)) => path,
            };
            let Some(size_bytes) = content_size(meta.size) else {
                count_skipped("size");
                continue;
            };
            let version = version_of(meta.e_tag.as_deref()).ok_or_else(|| MemoryStoreError::Store(NO_ETAG.into()))?;
            entries.push(MemoryEntry { path, size_bytes, version });
        }
        entries.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        Ok(entries)
    }

    /// Run the writes of one phase. At most `WRITE_CONCURRENCY` writes run at the same time, in
    /// the calling task. `outcomes[i]` gets the outcome of change `i`.
    ///
    /// After the first `Failed` or `Unknown` outcome, no new write starts. The writes in flight
    /// finish, and the rest stay `NotStarted`. When the deadline expires, no new write starts: each
    /// write in flight is dropped and becomes `Unknown`, and each write that did not start becomes
    /// `Failed`.
    async fn run_phase(&self, scope: &MemoryScope, set: &MemoryCommitSet, indexes: &[usize], outcomes: &mut [Outcome], deadline: Instant) {
        let changes = set.changes.changes();
        let mut waiting = indexes.iter().copied();
        let mut running = FuturesUnordered::new();
        let mut in_flight = Vec::new();
        let mut stopped = false;
        loop {
            if Instant::now() >= deadline {
                break;
            }
            while !stopped && running.len() < WRITE_CONCURRENCY {
                let Some(i) = waiting.next() else { break };
                in_flight.push(i);
                running.push(async move { (i, self.write(scope, &changes[i]).await) });
            }
            if running.is_empty() {
                return;
            }
            match tokio::time::timeout_at(deadline, running.next()).await {
                Ok(Some((i, outcome))) => {
                    in_flight.retain(|&j| j != i);
                    stopped |= matches!(outcome, Outcome::Failed(_) | Outcome::Unknown(_));
                    outcomes[i] = outcome;
                }
                Ok(None) => return,
                Err(_) => break,
            }
        }
        // The deadline expired. Drop the writes in flight: their outcome is not known.
        drop(running);
        for i in in_flight {
            outcomes[i] = Outcome::Unknown(MemoryApplyError::Store(deadline_text()));
        }
        if !stopped {
            for i in waiting {
                outcomes[i] = Outcome::Failed(MemoryApplyError::Store(deadline_text()));
            }
        }
    }

    /// Write one change. A `Put` is a conditional `PUT`. A `Delete` is a `HEAD` that checks the
    /// version, then a `DELETE`. Each failed write is checked with a read (spec §6.5).
    async fn write(&self, scope: &MemoryScope, change: &MemoryChange) -> Outcome {
        let key = file_path(&self.prefix, scope, change.path());
        match change {
            MemoryChange::Put { path, content, expected } => {
                let mode = match expected {
                    Expected::Absent => PutMode::Create,
                    Expected::Version(v) => PutMode::Update(UpdateVersion {
                        e_tag: Some(v.as_str().to_owned()),
                        version: None,
                    }),
                };
                let opts = PutOptions {
                    mode,
                    attributes: Attributes::from_iter([(Attribute::ContentType, CONTENT_TYPE)]),
                    ..PutOptions::default()
                };
                let id = WriteId::random();
                match self.store.put_opts(&key, PutPayload::from(encode(&id, content)), opts).await {
                    Ok(_) => Outcome::Confirmed,
                    Err(e) => {
                        let cause = MemoryApplyError::Store(store_text("the memory write failed", &e));
                        put_outcome(is_refusal(&e), cause, self.seen_after_put(&key, &id).await, path)
                    }
                }
            }
            MemoryChange::Delete { path, expected } => {
                let conflict = || Outcome::Failed(MemoryApplyError::Conflict { path: path.clone() });
                match self.store.head(&key).await {
                    Err(Error::NotFound { .. }) => return conflict(),
                    Err(e) => return Outcome::Failed(MemoryApplyError::Store(store_text("the memory read failed", &e))),
                    Ok(meta) if version_of(meta.e_tag.as_deref()).as_ref() != Some(expected) => return conflict(),
                    Ok(_) => {}
                }
                match self.store.delete(&key).await {
                    Ok(()) => Outcome::Confirmed,
                    Err(e) => {
                        let cause = MemoryApplyError::Store(store_text("the memory delete failed", &e));
                        delete_outcome(cause, self.seen_after_delete(&key, expected).await)
                    }
                }
            }
        }
    }

    /// Read the object after a failed `PUT`: does it hold the write ID of this `PUT`?
    async fn seen_after_put(&self, key: &Path, id: &WriteId) -> Seen {
        let got = match self.store.get(key).await {
            Ok(got) => got,
            Err(Error::NotFound { .. }) => return Seen::Absent,
            Err(_) => return Seen::ReadFailed,
        };
        let Ok(bytes) = got.bytes().await else { return Seen::ReadFailed };
        match decode(&bytes) {
            Ok((found, _)) if &found == id => Seen::Mine,
            Ok(_) => Seen::Other,
            Err(_) => Seen::Foreign,
        }
    }

    /// Read the object after a failed `DELETE`: is it absent, or does it have the expected version?
    async fn seen_after_delete(&self, key: &Path, expected: &MemoryVersion) -> SeenAfterDelete {
        match self.store.head(key).await {
            Err(Error::NotFound { .. }) => SeenAfterDelete::Absent,
            Err(_) => SeenAfterDelete::ReadFailed,
            Ok(meta) if version_of(meta.e_tag.as_deref()).as_ref() == Some(expected) => SeenAfterDelete::Expected,
            Ok(_) => SeenAfterDelete::Other,
        }
    }

    /// List every key under the scope, with no skip. It fails on a key outside the scope.
    async fn raw_keys(&self, base: &Path) -> Result<Vec<Path>, MemoryStoreError> {
        let mut stream = self.store.list(Some(base));
        let mut keys = Vec::new();
        while let Some(item) = stream.next().await {
            let meta = item.map_err(|e| listing_error(&e))?;
            if listed_key(base, &meta.location).is_err() {
                return Err(MemoryStoreError::Store("the store listed a key outside the scope".into()));
            }
            keys.push(meta.location);
        }
        Ok(keys)
    }
}

#[async_trait]
impl MemoryStore for S3MemoryStore {
    async fn get(&self, scope: &MemoryScope, path: &MemoryPath) -> Result<Option<MemoryFile>, MemoryStoreError> {
        let key = file_path(&self.prefix, scope, path);
        let got = match self.store.get(&key).await {
            Ok(got) => got,
            Err(Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(MemoryStoreError::Store(store_text("the memory read failed", &e))),
        };
        let version = version_of(got.meta.e_tag.as_deref()).ok_or_else(|| MemoryStoreError::Store(NO_ETAG.into()))?;
        let bytes = got.bytes().await.map_err(|e| MemoryStoreError::Store(store_text("the memory read failed", &e)))?;
        match decode(&bytes) {
            Ok((_, content)) => Ok(Some(MemoryFile { content, version })),
            Err(_) => {
                count_skipped("header");
                Err(MemoryStoreError::Store("object is not a memory file".into()))
            }
        }
    }

    async fn list(&self, scope: &MemoryScope, dir: &MemoryDir) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        self.listing(&scope_path(&self.prefix, scope), &dir_path(&self.prefix, scope, dir)).await
    }

    async fn usage(&self, scope: &MemoryScope) -> Result<MemoryUsage, MemoryStoreError> {
        let base = scope_path(&self.prefix, scope);
        let entries = self.listing(&base, &base).await?;
        Ok(MemoryUsage {
            files: entries.len() as u64,
            bytes: entries.iter().map(|e| e.size_bytes).sum(),
        })
    }

    /// Apply a change set (spec §6.5): one listing of the scope, the pure pre-check, then every
    /// `Put`, then every `Delete`. The `Delete` phase starts only when every `Put` succeeded.
    async fn apply(&self, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
        if set.is_empty() {
            return Ok(());
        }
        let deadline = Instant::now() + self.apply_deadline;
        let base = scope_path(&self.prefix, &set.scope);
        let entries = match tokio::time::timeout_at(deadline, self.listing(&base, &base)).await {
            Ok(listed) => listed.map_err(|MemoryStoreError::Store(text)| MemoryApplyError::Store(text))?,
            Err(_) => return Err(MemoryApplyError::Store(deadline_text())),
        };
        let snapshot = Snapshot {
            files: entries.into_iter().map(|e| (e.path, (e.size_bytes, e.version))).collect(),
        };
        let plan = precheck(set, &snapshot, caps)?;
        let mut outcomes = vec![Outcome::NotStarted; set.changes.len()];
        self.run_phase(&set.scope, set, &plan.puts, &mut outcomes, deadline).await;
        if plan.puts.iter().all(|&i| outcomes[i] == Outcome::Confirmed) {
            self.run_phase(&set.scope, set, &plan.deletes, &mut outcomes, deadline).await;
        }
        let result = result_of(set, &outcomes);
        if let Err(MemoryApplyError::Partial { applied, cause, .. }) = &result {
            tracing::warn!(applied, cause = cause_class(cause), "the S3 memory change set was partly applied");
            metrics::counter!(names::AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL).increment(1);
        }
        result
    }

    async fn delete_scope(&self, scope: &MemoryScope) -> Result<(), MemoryStoreError> {
        let base = scope_path(&self.prefix, scope);
        let keys = self.raw_keys(&base).await?;
        let mut deletes = futures::stream::iter(keys)
            .map(|key| {
                let store = Arc::clone(&self.store);
                async move { store.delete(&key).await }
            })
            .buffer_unordered(DELETE_CONCURRENCY);
        while let Some(result) = deletes.next().await {
            match result {
                Ok(()) | Err(Error::NotFound { .. }) => {}
                Err(e) => return Err(MemoryStoreError::Store(store_text("the memory delete failed", &e))),
            }
        }
        if self.raw_keys(&base).await?.is_empty() {
            Ok(())
        } else {
            Err(MemoryStoreError::Store("keys remain after the scope delete".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{EtagMode, Fault, Hook, ScriptedStore};
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use object_store::memory::InMemory;
    use paigasus_agent_host_core::ids::{AgentId, PrincipalId};
    use paigasus_agent_host_core::memory::{MemoryChangeSet, conformance};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn scope_n(n: u128) -> MemoryScope {
        MemoryScope::new(AgentId::new(Uuid::from_u128(1), Uuid::from_u128(2)), PrincipalId::from_uuid(Uuid::from_u128(n)))
    }

    fn scope() -> MemoryScope {
        scope_n(3)
    }

    fn prefix() -> KeyPrefix {
        KeyPrefix::parse("mem").unwrap()
    }

    fn fixture() -> (Arc<ScriptedStore>, S3MemoryStore) {
        let scripted = ScriptedStore::new(EtagMode::Counter);
        let store = S3MemoryStore::new(scripted.clone(), prefix(), Duration::from_secs(5));
        (scripted, store)
    }

    fn key_of(scope: &MemoryScope, path: &str) -> String {
        file_path(&prefix(), scope, &MemoryPath::parse(path).unwrap()).to_string()
    }

    fn put(scripted: &ScriptedStore, scope: &MemoryScope, path: &str, content: &str) {
        scripted.insert_raw(&key_of(scope, path), &encode(&WriteId::random(), content));
    }

    fn mp(path: &str) -> MemoryPath {
        MemoryPath::parse(path).unwrap()
    }

    /// Read the counter by reason. A snapshot of a `DebuggingRecorder` resets its counters, so a
    /// test takes one snapshot for all its assertions.
    fn skip_counts(snap: &Snapshotter) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        for (key, _, _, value) in snap.snapshot().into_vec() {
            if key.key().name() != names::AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL {
                continue;
            }
            let DebugValue::Counter(n) = value else { panic!("not a counter") };
            let reason = key.key().labels().find(|l| l.key() == "reason").expect("a reason label").value().to_owned();
            *counts.entry(reason).or_insert(0) += n;
        }
        counts
    }

    fn skipped(snap: &Snapshotter, reason: &str) -> u64 {
        skip_counts(snap).get(reason).copied().unwrap_or(0)
    }

    fn store_err(r: Result<impl std::fmt::Debug, MemoryStoreError>) -> String {
        let MemoryStoreError::Store(text) = r.expect_err("a store error");
        text
    }

    #[tokio::test]
    async fn get_reads_the_content_and_the_etag() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/a", "x");
        let file = store.get(&scope(), &mp("/a")).await.unwrap().expect("the file");
        let head = scripted.head(&Path::parse(key_of(&scope(), "/a")).unwrap()).await.unwrap();
        assert_eq!(file.content, "x");
        assert_eq!(file.version.as_str(), head.e_tag.unwrap());
    }

    #[tokio::test]
    async fn get_of_a_missing_file_is_none() {
        let (_scripted, store) = fixture();
        assert_eq!(store.get(&scope(), &mp("/a")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_foreign_object_is_a_store_error() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (scripted, store) = fixture();
        scripted.insert_raw(&key_of(&scope(), "/instructions.md"), b"plain text");
        let text = store_err(store.get(&scope(), &mp("/instructions.md")).await);
        assert_eq!(text, "object is not a memory file");
        assert_eq!(skipped(&snap, "header"), 1);
    }

    #[tokio::test]
    async fn list_skips_short_and_refused_keys() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (scripted, store) = fixture();
        scripted.insert_raw(&key_of(&scope(), "/notes"), b"");
        scripted.insert_raw(&format!("{}/~", scope_path(&prefix(), &scope())), &encode(&WriteId::random(), "x"));
        put(&scripted, &scope(), "/ok", "y");
        let list = store.list(&scope(), &MemoryDir::root()).await.unwrap();
        let paths: Vec<&str> = list.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/ok"]);
        let counts = skip_counts(&snap);
        assert_eq!(counts, BTreeMap::from([("key".to_owned(), 1), ("size".to_owned(), 1)]));
    }

    #[tokio::test]
    async fn an_empty_file_is_listed_with_size_zero() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/empty", "");
        let list = store.list(&scope(), &MemoryDir::root()).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].size_bytes, 0);
    }

    #[tokio::test]
    async fn list_of_a_dir_does_not_return_a_sibling_prefix() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/ab/y", "1");
        put(&scripted, &scope(), "/a/x", "1");
        put(&scripted, &scope(), "/a/b/z", "1");
        let dir = MemoryDir::parse("/a").unwrap();
        let list = store.list(&scope(), &dir).await.unwrap();
        let paths: Vec<&str> = list.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/a/b/z", "/a/x"]);
    }

    #[tokio::test]
    async fn list_of_a_dir_checks_the_full_path() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/a/b/c/d/e/f/g/ok", "1");
        // Eight segments under the directory are valid alone, but nine segments in full are not.
        scripted.insert_raw(&format!("{}/a/b/c/d/e/f/g/h/i", scope_path(&prefix(), &scope())), &encode(&WriteId::random(), "x"));
        let dir = MemoryDir::parse("/a/b/c/d/e/f/g").unwrap();
        let list = store.list(&scope(), &dir).await.unwrap();
        let paths: Vec<&str> = list.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/a/b/c/d/e/f/g/ok"]);
        assert_eq!(skip_counts(&snap), BTreeMap::from([("key".to_owned(), 1)]));
    }

    #[tokio::test]
    async fn a_listed_key_outside_the_scope_is_a_store_error() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/a", "1");
        scripted.inject_listed_key(&key_of(&scope_n(4), "/a"));
        let text = store_err(store.list(&scope(), &MemoryDir::root()).await);
        assert_eq!(text, "the store listed a key outside the scope");
    }

    #[tokio::test]
    async fn an_unparsable_key_makes_the_scope_unreadable() {
        let (scripted, store) = fixture();
        {
            let recorder = DebuggingRecorder::new();
            let snap = recorder.snapshotter();
            let _guard = metrics::set_default_local_recorder(&recorder);
            scripted.fail_list_with_invalid_path();
            let text = store_err(store.list(&scope(), &MemoryDir::root()).await);
            assert!(text.contains("unparsable_key"), "{text}");
            assert_eq!(skipped(&snap, "unparsable"), 1);
        }
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        scripted.fail_list_with_invalid_path();
        let text = store_err(store.usage(&scope()).await);
        assert!(text.contains("unparsable_key"), "{text}");
        assert_eq!(skipped(&snap, "unparsable"), 1);
    }

    #[tokio::test]
    async fn usage_sums_the_listed_files() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/a", "xx");
        put(&scripted, &scope(), "/d/b", "yyy");
        put(&scripted, &scope_n(4), "/c", "zzzz");
        let usage = store.usage(&scope()).await.unwrap();
        assert_eq!(usage, MemoryUsage { files: 2, bytes: 5 });
    }

    #[tokio::test]
    async fn delete_scope_removes_skipped_keys() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/a", "x");
        scripted.insert_raw(&key_of(&scope(), "/notes"), b"");
        scripted.insert_raw(&format!("{}/~", scope_path(&prefix(), &scope())), b"raw");
        put(&scripted, &scope_n(4), "/keep", "k");
        store.delete_scope(&scope()).await.unwrap();
        assert!(store.list(&scope(), &MemoryDir::root()).await.unwrap().is_empty());
        let kept = scripted.get(&Path::parse(key_of(&scope_n(4), "/keep")).unwrap()).await;
        assert!(kept.is_ok());
        let left = store.raw_keys(&scope_path(&prefix(), &scope())).await.unwrap();
        assert!(left.is_empty());
    }

    #[tokio::test]
    async fn delete_scope_fails_when_a_key_remains() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/a", "x");
        scripted.ignore_deletes();
        let text = store_err(store.delete_scope(&scope()).await);
        assert_eq!(text, "keys remain after the scope delete");
    }

    #[tokio::test]
    async fn delete_scope_counts_an_unparsable_key() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (scripted, store) = fixture();
        scripted.fail_list_with_invalid_path();
        let text = store_err(store.delete_scope(&scope()).await);
        assert!(text.contains("unparsable_key"), "{text}");
        assert_eq!(skipped(&snap, "unparsable"), 1);
    }

    fn set_of(scope: &MemoryScope, changes: Vec<MemoryChange>) -> MemoryCommitSet {
        let mut set = MemoryChangeSet::new();
        for change in changes {
            set.record(change);
        }
        MemoryCommitSet::new(scope.clone(), set)
    }

    #[tokio::test]
    async fn in_memory_passes_the_conformance_suite() {
        let store = S3MemoryStore::new(Arc::new(InMemory::new()), prefix(), Duration::from_secs(10));
        conformance::run_all(&store).await;
    }

    #[tokio::test]
    async fn content_hash_etags_pass_the_conformance_suite() {
        let store = S3MemoryStore::new(ScriptedStore::new(EtagMode::ContentHash), prefix(), Duration::from_secs(10));
        conformance::run_all(&store).await;
    }

    #[tokio::test]
    async fn a_rename_writes_the_put_before_the_delete() {
        let (scripted, store) = fixture();
        put(&scripted, &scope(), "/old", "x");
        let old = store.get(&scope(), &mp("/old")).await.unwrap().expect("the old file");
        let set = set_of(
            &scope(),
            vec![
                MemoryChange::Delete {
                    path: mp("/old"),
                    expected: old.version,
                },
                MemoryChange::Put {
                    path: mp("/new"),
                    content: "x".into(),
                    expected: Expected::Absent,
                },
            ],
        );
        store.apply(&set, &MemoryCaps::default()).await.unwrap();
        let calls = scripted.calls();
        let put_at = calls.iter().position(|c| *c == format!("PUT {}", key_of(&scope(), "/new"))).expect("the PUT");
        let delete_at = calls.iter().position(|c| *c == format!("DELETE {}", key_of(&scope(), "/old"))).expect("the DELETE");
        assert!(put_at < delete_at, "{calls:?}");
        assert_eq!(store.get(&scope(), &mp("/new")).await.unwrap().expect("the new file").content, "x");
        assert_eq!(store.get(&scope(), &mp("/old")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_stored_object_is_the_envelope() {
        let inner = Arc::new(InMemory::new());
        let store = S3MemoryStore::new(inner.clone(), prefix(), Duration::from_secs(10));
        let set = set_of(
            &scope(),
            vec![MemoryChange::Put {
                path: mp("/tasks/t1.md"),
                content: "x".into(),
                expected: Expected::Absent,
            }],
        );
        store.apply(&set, &MemoryCaps::default()).await.unwrap();
        let key = key_of(&scope(), "/tasks/t1.md");
        assert_eq!(key, format!("mem/{}/{}/{}/tasks/t1.md", Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)));
        let got = inner.get(&Path::parse(&key).unwrap()).await.unwrap();
        assert_eq!(got.attributes.get(&Attribute::ContentType).map(|v| v.as_ref()), Some("text/plain; charset=utf-8"));
        let bytes = got.bytes().await.unwrap();
        assert!(bytes.starts_with(b"paigasus-memory/1 "), "{bytes:?}");
        assert_eq!(bytes.len(), 52);
        assert!(bytes.ends_with(b"\nx"), "{bytes:?}");
    }

    fn fixture_with(mode: EtagMode, deadline: Duration) -> (Arc<ScriptedStore>, S3MemoryStore) {
        let scripted = ScriptedStore::new(mode);
        let store = S3MemoryStore::new(scripted.clone(), prefix(), deadline);
        (scripted, store)
    }

    fn hashed() -> (Arc<ScriptedStore>, S3MemoryStore) {
        fixture_with(EtagMode::ContentHash, Duration::from_secs(5))
    }

    fn put_change(path: &str, content: &str, expected: Expected) -> MemoryChange {
        MemoryChange::Put {
            path: mp(path),
            content: content.into(),
            expected,
        }
    }

    fn delete_change(path: &str, expected: MemoryVersion) -> MemoryChange {
        MemoryChange::Delete { path: mp(path), expected }
    }

    async fn version(store: &S3MemoryStore, path: &str) -> MemoryVersion {
        store.get(&scope(), &mp(path)).await.unwrap().expect("the file").version
    }

    async fn content(store: &S3MemoryStore, path: &str) -> Option<String> {
        store.get(&scope(), &mp(path)).await.unwrap().map(|f| f.content)
    }

    /// A hook that writes `content` at `path` as another writer.
    fn other_writer(path: &str, content: &'static str) -> impl FnOnce(&ScriptedStore) + Send + 'static {
        let key = key_of(&scope(), path);
        move |s: &ScriptedStore| s.insert_raw(&key, &encode(&WriteId::random(), content))
    }

    fn generic_error() -> Error {
        Error::Generic {
            store: "S3",
            source: "the request failed".into(),
        }
    }

    fn timeout_error() -> Error {
        Error::Generic {
            store: "S3",
            source: Box::new(object_store::client::HttpError::new(
                object_store::client::HttpErrorKind::Timeout,
                std::io::Error::other("the request timed out"),
            )),
        }
    }

    fn already_exists() -> Error {
        Error::AlreadyExists {
            path: "x".into(),
            source: "exists".into(),
        }
    }

    fn partial_parts(r: Result<(), MemoryApplyError>) -> (usize, MemoryPath, MemoryApplyError) {
        match r {
            Err(MemoryApplyError::Partial { applied, failed_at, cause }) => (applied, failed_at, *cause),
            other => panic!("not a Partial: {other:?}"),
        }
    }

    fn cause_text(cause: &MemoryApplyError) -> &str {
        match cause {
            MemoryApplyError::Store(text) => text,
            other => panic!("not a Store cause: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_racing_create_is_a_conflict() {
        let (scripted, store) = hashed();
        scripted.on(Hook::BeforeFirstPut, other_writer("/a", "other"));
        let set = set_of(&scope(), vec![put_change("/a", "mine", Expected::Absent)]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Err(MemoryApplyError::Conflict { path: mp("/a") }));
        assert_eq!(content(&store, "/a").await.as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn a_racing_update_is_a_conflict() {
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/a", "old");
        let v = version(&store, "/a").await;
        scripted.on(Hook::BeforeFirstPut, other_writer("/a", "other"));
        let set = set_of(&scope(), vec![put_change("/a", "mine", Expected::Version(v))]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Err(MemoryApplyError::Conflict { path: mp("/a") }));
        assert_eq!(content(&store, "/a").await.as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn a_racing_change_before_the_delete_head_is_a_conflict() {
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/a", "old");
        let v = version(&store, "/a").await;
        scripted.on(Hook::BeforeFirstHead, other_writer("/a", "newer"));
        let set = set_of(&scope(), vec![delete_change("/a", v)]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Err(MemoryApplyError::Conflict { path: mp("/a") }));
        assert_eq!(content(&store, "/a").await.as_deref(), Some("newer"));
    }

    #[tokio::test]
    async fn a_racing_change_between_head_and_delete_is_lost() {
        // Spec §6.7 item 3: the HEAD and the DELETE are two requests. A change between them is
        // lost. This test documents the race; it is not a wanted result.
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/a", "old");
        let v = version(&store, "/a").await;
        scripted.on(Hook::BeforeFirstDelete, other_writer("/a", "newer"));
        let set = set_of(&scope(), vec![delete_change("/a", v)]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Ok(()));
        assert_eq!(content(&store, "/a").await, None);
    }

    #[tokio::test]
    async fn a_later_racing_write_gives_partial() {
        // The writes of a phase start in set order: `FuturesUnordered` polls new futures in the
        // order of the push. So PUT number n is the nth Put of the set.
        let (scripted, store) = hashed();
        scripted.on(Hook::BeforeFirstPut, other_writer("/b", "other"));
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent), put_change("/b", "2", Expected::Absent)]);
        let r = store.apply(&set, &MemoryCaps::default()).await;
        assert_eq!(
            r,
            Err(MemoryApplyError::Partial {
                applied: 1,
                failed_at: mp("/b"),
                cause: Box::new(MemoryApplyError::Conflict { path: mp("/b") }),
            })
        );
        assert_eq!(content(&store, "/b").await.as_deref(), Some("other"));
    }

    /// `[Put /a, Put /b, Delete /c]` with a failed second `PUT`. The PUTs start in set order,
    /// because `FuturesUnordered` polls new futures in the order of the push.
    async fn the_second_put_fails_on(scripted: &ScriptedStore, store: &S3MemoryStore) -> Result<(), MemoryApplyError> {
        put(scripted, &scope(), "/c", "c");
        let v = version(store, "/c").await;
        scripted.fault_put(2, Fault::Fail(generic_error()));
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent), put_change("/b", "2", Expected::Absent), delete_change("/c", v)]);
        store.apply(&set, &MemoryCaps::default()).await
    }

    #[tokio::test]
    async fn the_second_put_fails() {
        let (scripted, store) = hashed();
        let (applied, failed_at, _) = partial_parts(the_second_put_fails_on(&scripted, &store).await);
        assert_eq!((applied, failed_at), (1, mp("/b")));
        assert_eq!(content(&store, "/a").await.as_deref(), Some("1"));
        assert_eq!(content(&store, "/b").await, None);
        assert_eq!(content(&store, "/c").await.as_deref(), Some("c"));
        assert!(!scripted.calls().iter().any(|c| c.starts_with("DELETE ")), "{:?}", scripted.calls());
    }

    /// `[Put /new, Delete /old]`, a rename of `/old`.
    async fn rename(store: &S3MemoryStore) -> MemoryCommitSet {
        let v = version(store, "/old").await;
        set_of(&scope(), vec![put_change("/new", "x", Expected::Absent), delete_change("/old", v)])
    }

    #[tokio::test]
    async fn a_refused_rename_put_keeps_the_old_file() {
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/old", "x");
        let set = rename(&store).await;
        scripted.fault_put(1, Fault::Fail(already_exists()));
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Err(MemoryApplyError::Conflict { path: mp("/new") }));
        assert_eq!(content(&store, "/old").await.as_deref(), Some("x"));
        assert_eq!(content(&store, "/new").await, None);
    }

    #[tokio::test]
    async fn a_failed_rename_delete_keeps_both() {
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/old", "x");
        let set = rename(&store).await;
        scripted.fault_delete(1, Fault::Fail(generic_error()));
        let (applied, failed_at, _) = partial_parts(store.apply(&set, &MemoryCaps::default()).await);
        assert_eq!((applied, failed_at), (1, mp("/old")));
        assert_eq!(content(&store, "/new").await.as_deref(), Some("x"));
        assert_eq!(content(&store, "/old").await.as_deref(), Some("x"));
    }

    #[tokio::test]
    async fn a_failed_delete_that_landed_is_ok() {
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/old", "x");
        let set = rename(&store).await;
        scripted.fault_delete(1, Fault::WriteThenFail(generic_error()));
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Ok(()));
        assert_eq!(content(&store, "/old").await, None);
    }

    /// `[Delete /a v]` with a failed `DELETE`. `newer` makes another writer change `/a` first.
    async fn a_failed_delete_of_a(newer: bool) -> Result<(), MemoryApplyError> {
        let (scripted, store) = hashed();
        put(&scripted, &scope(), "/a", "old");
        let v = version(&store, "/a").await;
        scripted.fault_delete(1, Fault::Fail(generic_error()));
        if newer {
            scripted.on(Hook::BeforeFirstDelete, other_writer("/a", "newer"));
        }
        let set = set_of(&scope(), vec![delete_change("/a", v)]);
        store.apply(&set, &MemoryCaps::default()).await
    }

    #[tokio::test]
    async fn a_failed_delete_with_another_etag_after_it_is_partial() {
        // A failed DELETE whose check HEAD shows another ETag is an unknown outcome. No write is
        // confirmed, so only an unknown outcome gives Partial here.
        let (applied, failed_at, _) = partial_parts(a_failed_delete_of_a(true).await);
        assert_eq!((applied, failed_at), (0, mp("/a")));
    }

    #[tokio::test]
    async fn a_failed_delete_with_the_expected_etag_after_it_is_a_bare_store_error() {
        match a_failed_delete_of_a(false).await {
            Err(MemoryApplyError::Store(text)) => assert_eq!(text, "the memory delete failed (other)"),
            other => panic!("not a bare Store: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_retried_put_that_landed_is_ok() {
        let (scripted, store) = hashed();
        scripted.fault_put(1, Fault::WriteThenFail(already_exists()));
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent)]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Ok(()));
        assert_eq!(content(&store, "/a").await.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn a_timed_out_put_that_landed_is_ok() {
        let (scripted, store) = hashed();
        scripted.fault_put(1, Fault::WriteThenFail(timeout_error()));
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent)]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Ok(()));
    }

    #[tokio::test]
    async fn a_timed_out_put_with_a_failed_check_is_partial() {
        let (scripted, store) = hashed();
        scripted.fault_put(1, Fault::WriteThenFail(timeout_error()));
        scripted.fail_next_get();
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent)]);
        let (applied, failed_at, cause) = partial_parts(store.apply(&set, &MemoryCaps::default()).await);
        assert_eq!((applied, failed_at), (0, mp("/a")));
        assert!(cause_text(&cause).contains("timeout"), "{cause:?}");
    }

    #[tokio::test]
    async fn a_foreign_object_after_a_failed_put_is_partial() {
        let (scripted, store) = hashed();
        let key = key_of(&scope(), "/a");
        scripted.on(Hook::BeforeFirstPut, move |s: &ScriptedStore| s.insert_raw(&key, b"plain text"));
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent)]);
        let (applied, failed_at, _) = partial_parts(store.apply(&set, &MemoryCaps::default()).await);
        assert_eq!((applied, failed_at), (0, mp("/a")));
    }

    #[tokio::test]
    async fn a_timed_out_put_that_did_not_land_is_a_bare_store_error() {
        let (scripted, store) = hashed();
        scripted.fault_put(1, Fault::Fail(timeout_error()));
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent)]);
        match store.apply(&set, &MemoryCaps::default()).await {
            Err(MemoryApplyError::Store(text)) => assert!(text.contains("timeout"), "{text}"),
            other => panic!("not a bare Store: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failure_stops_new_writes_in_the_phase() {
        // The writes of a phase start in set order: `FuturesUnordered` polls new futures in the
        // order of the push. So PUT number n is the nth Put of the set.
        let (scripted, store) = hashed();
        scripted.fault_put(1, Fault::Fail(generic_error()));
        let changes = (0..WRITE_CONCURRENCY + 2).map(|n| put_change(&format!("/f{n:02}"), "x", Expected::Absent)).collect();
        let set = set_of(&scope(), changes);
        let r = store.apply(&set, &MemoryCaps::default()).await;
        let puts = scripted.calls().iter().filter(|c| c.starts_with("PUT ")).count();
        // The first batch starts at once. After the failure no new write starts.
        assert_eq!(puts, WRITE_CONCURRENCY, "{:?}", scripted.calls());
        let (applied, failed_at, _) = partial_parts(r);
        assert_eq!((applied, failed_at), (WRITE_CONCURRENCY - 1, mp("/f00")));
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_bounds_apply() {
        // The writes of a phase start in set order: `FuturesUnordered` polls new futures in the
        // order of the push. So PUT number n is the nth Put of the set.
        let (scripted, store) = fixture_with(EtagMode::ContentHash, Duration::from_secs(2));
        scripted.fault_put(2, Fault::Stall);
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent), put_change("/b", "2", Expected::Absent)]);
        let start = tokio::time::Instant::now();
        let r = tokio::time::timeout(Duration::from_secs(60), store.apply(&set, &MemoryCaps::default()))
            .await
            .expect("the apply deadline must end the call before the outer timeout");
        let elapsed = start.elapsed();
        let (applied, failed_at, cause) = partial_parts(r);
        assert_eq!((applied, failed_at), (1, mp("/b")));
        assert!(cause_text(&cause).contains("timeout"), "{cause:?}");
        assert!(elapsed >= Duration::from_secs(2) && elapsed < Duration::from_millis(2100), "{elapsed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_deadline_before_the_first_write_is_a_bare_store_error() {
        let (scripted, store) = fixture_with(EtagMode::ContentHash, Duration::from_secs(1));
        scripted.stall_list();
        let set = set_of(&scope(), vec![put_change("/a", "1", Expected::Absent)]);
        let r = tokio::time::timeout(Duration::from_secs(60), store.apply(&set, &MemoryCaps::default()))
            .await
            .expect("the apply deadline must end the call before the outer timeout");
        match r {
            Err(MemoryApplyError::Store(text)) => assert!(text.contains("timeout"), "{text}"),
            other => panic!("not a bare Store: {other:?}"),
        }
        assert!(!scripted.calls().iter().any(|c| c.starts_with("PUT ")), "{:?}", scripted.calls());
    }

    fn partial_count(snap: &Snapshotter) -> u64 {
        snap.snapshot()
            .into_vec()
            .into_iter()
            .filter(|(key, _, _, _)| key.key().name() == names::AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL)
            .map(|(_, _, _, value)| match value {
                DebugValue::Counter(n) => n,
                _ => panic!("not a counter"),
            })
            .sum()
    }

    #[tokio::test]
    async fn partial_increments_the_counter() {
        {
            let recorder = DebuggingRecorder::new();
            let snap = recorder.snapshotter();
            let _guard = metrics::set_default_local_recorder(&recorder);
            let (scripted, store) = hashed();
            partial_parts(the_second_put_fails_on(&scripted, &store).await);
            assert_eq!(partial_count(&snap), 1);
        }
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (scripted, store) = hashed();
        scripted.on(Hook::BeforeFirstPut, other_writer("/a", "other"));
        let set = set_of(&scope(), vec![put_change("/a", "mine", Expected::Absent)]);
        assert_eq!(store.apply(&set, &MemoryCaps::default()).await, Err(MemoryApplyError::Conflict { path: mp("/a") }));
        assert_eq!(partial_count(&snap), 0);
    }

    #[test]
    fn the_cause_classes() {
        assert_eq!(cause_class(&MemoryApplyError::Conflict { path: mp("/a") }), "conflict");
        let cap = MemoryApplyError::CapExceeded(paigasus_agent_host_core::memory::CapError::StoreFiles { after: 2, limit: 1, now: 1 });
        assert_eq!(cause_class(&cap), "cap");
        let partial = MemoryApplyError::Partial {
            applied: 1,
            failed_at: mp("/a"),
            cause: Box::new(MemoryApplyError::Conflict { path: mp("/a") }),
        };
        assert_eq!(cause_class(&partial), "partial");
        assert_eq!(cause_class(&MemoryApplyError::Store("the memory write failed (timeout)".into())), "the memory write failed (timeout)");
    }

    #[test]
    fn the_debug_text_holds_the_prefix_and_the_deadline_only() {
        let (_scripted, store) = fixture();
        let text = format!("{store:?}");
        assert!(text.contains("\"mem\""), "{text}");
        assert!(text.contains("apply_deadline"), "{text}");
    }
}
