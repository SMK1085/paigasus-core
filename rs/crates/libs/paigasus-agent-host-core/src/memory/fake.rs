// SPDX-License-Identifier: Apache-2.0

//! An in-memory `MemoryStore` for tests. It is all or nothing in `apply`, and it uses the same
//! cap rule as every adapter.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use uuid::Uuid;

use super::{Expected, MemoryCaps, MemoryChange, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use crate::ports::{MemoryApplyError, MemoryEntry, MemoryFile, MemoryStore, MemoryStoreError, MemoryUsage};

type Key = (Uuid, Uuid, Uuid, String);
type Files = BTreeMap<Key, (String, u64)>;

#[derive(Default)]
struct State {
    files: Files,
    /// One counter for the whole store. A version never repeats, so an ABA check passes.
    counter: u64,
}

fn key(scope: &MemoryScope, path: &MemoryPath) -> Key {
    (scope.org(), scope.agent().uuid(), scope.principal().uuid(), path.as_str().to_string())
}

fn in_scope(scope: &MemoryScope, k: &Key) -> bool {
    (k.0, k.1, k.2) == (scope.org(), scope.agent().uuid(), scope.principal().uuid())
}

fn usage_of(files: &Files, scope: &MemoryScope) -> MemoryUsage {
    let mut usage = MemoryUsage::default();
    for (k, (content, _)) in files {
        if in_scope(scope, k) {
            usage.files += 1;
            usage.bytes += content.len() as u64;
        }
    }
    usage
}

fn version(n: u64) -> MemoryVersion {
    MemoryVersion::new(n.to_string())
}

/// A shared in-memory store. A clone shares the same state.
#[derive(Clone, Default)]
pub struct InMemoryMemoryStore {
    state: Arc<Mutex<State>>,
}

impl InMemoryMemoryStore {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("the fake store mutex is not poisoned")
    }
}

#[async_trait]
impl MemoryStore for InMemoryMemoryStore {
    async fn get(&self, scope: &MemoryScope, path: &MemoryPath) -> Result<Option<MemoryFile>, MemoryStoreError> {
        let state = self.lock();
        Ok(state.files.get(&key(scope, path)).map(|(content, v)| MemoryFile {
            content: content.clone(),
            version: version(*v),
        }))
    }

    async fn list(&self, scope: &MemoryScope, dir: &MemoryDir) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        let state = self.lock();
        let mut entries = Vec::new();
        for (k, (content, v)) in &state.files {
            if !in_scope(scope, k) {
                continue;
            }
            let path = MemoryPath::parse(&k.3).expect("the store holds valid paths");
            if dir.contains(&path) {
                entries.push(MemoryEntry {
                    path,
                    size_bytes: content.len() as u64,
                    version: version(*v),
                });
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    async fn usage(&self, scope: &MemoryScope) -> Result<MemoryUsage, MemoryStoreError> {
        Ok(usage_of(&self.lock().files, scope))
    }

    async fn apply(&self, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
        let mut state = self.lock();
        let before = usage_of(&state.files, &set.scope);
        // Work on a copy, so a refusal leaves the store as it was.
        let mut files = state.files.clone();
        let mut counter = state.counter;
        for change in set.changes.changes() {
            let k = key(&set.scope, change.path());
            let current = files.get(&k).map(|(_, v)| *v);
            match change {
                MemoryChange::Put { path, content, expected } => {
                    let matches = match (expected, current) {
                        (Expected::Absent, None) => true,
                        (Expected::Version(v), Some(c)) => *v == version(c),
                        _ => false,
                    };
                    if !matches {
                        return Err(MemoryApplyError::Conflict { path: path.clone() });
                    }
                    caps.check_file(path, content.len() as u64).map_err(MemoryApplyError::CapExceeded)?;
                    counter += 1;
                    files.insert(k, (content.clone(), counter));
                }
                MemoryChange::Delete { path, expected } => {
                    if current.map(version).as_ref() != Some(expected) {
                        return Err(MemoryApplyError::Conflict { path: path.clone() });
                    }
                    files.remove(&k);
                }
            }
        }
        let after = usage_of(&files, &set.scope);
        caps.check(before, after).map_err(MemoryApplyError::CapExceeded)?;
        state.files = files;
        state.counter = counter;
        Ok(())
    }

    async fn delete_scope(&self, scope: &MemoryScope) -> Result<(), MemoryStoreError> {
        self.lock().files.retain(|k, _| !in_scope(scope, k));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::conformance::run_all;

    #[tokio::test]
    async fn the_fake_passes_the_conformance_suite() {
        run_all(&InMemoryMemoryStore::default()).await;
    }
}
