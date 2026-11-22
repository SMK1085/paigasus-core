// SPDX-License-Identifier: Apache-2.0

//! The S3-compatible `MemoryStore` adapter (SMA-829).
//!
//! The adapter keeps each memory file as one object. The object key comes from `keys`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use object_store::path::Path;
use object_store::{Error, ObjectStore, ObjectStoreExt};
use paigasus_agent_host_core::memory::{MemoryCaps, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use paigasus_agent_host_core::ports::{MemoryApplyError, MemoryEntry, MemoryFile, MemoryStore, MemoryStoreError, MemoryUsage};
use paigasus_observability::names;

use self::envelope::{content_size, decode};
use self::errors::{class, store_text};
use self::keys::{KeyPrefix, ListedKey, dir_path, file_path, listed_key, scope_path};

pub mod envelope;
pub mod errors;
pub mod keys;
pub mod plan;
#[cfg(test)]
pub mod testing;

/// The most delete requests that run at the same time in `delete_scope`.
const DELETE_CONCURRENCY: usize = 8;

/// Count one skipped or refused object. The label is a fixed word, never a key.
fn count_skipped(reason: &'static str) {
    metrics::counter!(names::AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL, "reason" => reason).increment(1);
}

/// The version of a file, from the ETag of the store. The ETag is the version as the store
/// sends it.
fn version_of(e_tag: Option<&str>) -> Option<MemoryVersion> {
    e_tag.map(MemoryVersion::new)
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

    async fn apply(&self, _set: &MemoryCommitSet, _caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
        Err(MemoryApplyError::Store("the S3 memory store cannot apply yet".into()))
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
    use super::envelope::{WriteId, encode};
    use super::testing::{EtagMode, ScriptedStore};
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use paigasus_agent_host_core::ids::{AgentId, PrincipalId};
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

    #[test]
    fn the_debug_text_holds_the_prefix_and_the_deadline_only() {
        let (_scripted, store) = fixture();
        let text = format!("{store:?}");
        assert!(text.contains("\"mem\""), "{text}");
        assert!(text.contains("apply_deadline"), "{text}");
    }
}
