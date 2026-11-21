// SPDX-License-Identifier: Apache-2.0

//! The run buffer of the memory tool: a read cache, a change set, an early conflict check, and the
//! caps at write time.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::{CapError, Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use crate::ports::{MemoryApplyError, MemoryStore, MemoryStoreError, MemoryUsage};

/// A refusal of a workspace command. The `Display` text is for the model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceError {
    /// The file changed in the store after the run first saw it. `also` holds the other paths whose
    /// buffered changes were dropped with it, because one command staged them together.
    #[error("{}", conflict_text(path, also))]
    Conflict { path: MemoryPath, also: Vec<MemoryPath> },
    /// A cap refuses the change.
    #[error("{0}")]
    Cap(CapError),
    /// The content holds a NUL character.
    #[error("The content for {} contains a NUL character (U+0000). Remove it and try again.", path.to_tool_path())]
    NulInContent { path: MemoryPath },
    /// The workspace is sealed.
    #[error("The run is ending. Memory is read-only now.")]
    Sealed,
    /// The store failed. The cause is for the caller to log, not for the model.
    #[error("The memory store is not available now. Try again later.")]
    Store(String),
}

/// The text of a conflict for the model.
fn conflict_text(path: &MemoryPath, also: &[MemoryPath]) -> String {
    let mut text = format!("The file {} changed outside this run. Your changes to it in this run are gone.", path.to_tool_path());
    if !also.is_empty() {
        let others: Vec<String> = also.iter().map(MemoryPath::to_tool_path).collect();
        text.push_str(&format!(" Your related changes to {} are also gone.", others.join(", ")));
    }
    text.push_str(" Use view to read it again.");
    text
}

impl From<MemoryStoreError> for WorkspaceError {
    fn from(e: MemoryStoreError) -> Self {
        let MemoryStoreError::Store(cause) = e;
        WorkspaceError::Store(cause)
    }
}

/// What the run first saw of one path.
struct Known {
    /// `None` when the file did not exist.
    version: Option<MemoryVersion>,
    size: u64,
    /// `None` when only a listing saw the file, or when the file does not exist.
    content: Option<String>,
}

/// The memory of one run: a repeatable read per path, and a buffer of changes.
pub struct MemoryWorkspace {
    store: Arc<dyn MemoryStore>,
    scope: MemoryScope,
    caps: MemoryCaps,
    known: BTreeMap<MemoryPath, Known>,
    /// The cached store rows of each listed directory. A row cache can be dropped.
    listings: BTreeMap<String, Vec<(MemoryPath, u64)>>,
    /// Every directory that the run listed. A path under one of them that has no `known` entry was
    /// absent when the run looked. This set is never dropped, so the absence stays the baseline.
    listed: BTreeSet<MemoryDir>,
    changes: MemoryChangeSet,
    /// The group of each buffered change. The changes that one `stage` call records form a group,
    /// and a later stage that touches a path of a group joins that whole group. A conflict drops
    /// every change of the group, so a staged move is never dropped by half.
    groups: BTreeMap<MemoryPath, u64>,
    /// The armed directory guards, each with its group. A guard stays in its group when the
    /// changes of the group merge away, and it drops with the group.
    dir_guards: Vec<(MemoryDir, u64)>,
    /// The paths of an early conflict that the run must see again. The next fresh listing that
    /// holds such a path records it as a first sight and removes the mark.
    resight: BTreeSet<MemoryPath>,
    next_group: u64,
    sealed: bool,
}

impl MemoryWorkspace {
    /// Make a workspace over a store, for one scope.
    #[must_use]
    pub fn new(store: Arc<dyn MemoryStore>, scope: MemoryScope, caps: MemoryCaps) -> Self {
        MemoryWorkspace {
            store,
            scope,
            caps,
            known: BTreeMap::new(),
            listings: BTreeMap::new(),
            listed: BTreeSet::new(),
            changes: MemoryChangeSet::new(),
            groups: BTreeMap::new(),
            dir_guards: Vec::new(),
            resight: BTreeSet::new(),
            next_group: 0,
            sealed: false,
        }
    }

    /// The caps of the workspace.
    #[must_use]
    pub fn caps(&self) -> &MemoryCaps {
        &self.caps
    }

    /// True after `take_changes`.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Read a file: the buffer first, then the cache, then the store.
    pub async fn read(&mut self, path: &MemoryPath) -> Result<Option<String>, WorkspaceError> {
        match self.changes.get(path) {
            Some(MemoryChange::Put { content, .. }) => return Ok(Some(content.clone())),
            Some(MemoryChange::Delete { .. }) => return Ok(None),
            None => {}
        }
        if let Some(k) = self.known.get(path) {
            if k.version.is_none() {
                return Ok(None);
            }
            if let Some(c) = &k.content {
                return Ok(Some(c.clone()));
            }
        }
        let file = self.store.get(&self.scope, path).await?;
        let fetched = Known {
            version: file.as_ref().map(|f| f.version.clone()),
            size: file.as_ref().map_or(0, |f| f.content.len() as u64),
            content: file.as_ref().map(|f| f.content.clone()),
        };
        // The model now sees the fetched file, so the fetched file is the baseline. When a cached
        // listing shows another state of the path, that listing is dropped and taken again.
        let listing_is_stale = match self.known.get(path) {
            Some(k) => k.version != fetched.version,
            None => fetched.version.is_some(),
        };
        if listing_is_stale {
            self.drop_listings_of(path);
        }
        self.resight.remove(path);
        self.known.insert(path.clone(), fetched);
        Ok(file.map(|f| f.content))
    }

    /// Read a file whose content moves but does not go to the model (the sources of `rename`). It
    /// answers from the buffer and the cache first, like `read`. Unlike `read`, it keeps the
    /// first-sight version as the baseline: when the store now holds another version than the run
    /// first saw, it is a conflict, and nothing moves.
    pub(super) async fn read_for_move(&mut self, path: &MemoryPath) -> Result<Option<String>, WorkspaceError> {
        match self.changes.get(path) {
            Some(MemoryChange::Put { content, .. }) => return Ok(Some(content.clone())),
            Some(MemoryChange::Delete { .. }) => return Ok(None),
            None => {}
        }
        if let Some(k) = self.known.get(path) {
            if k.version.is_none() {
                return Ok(None);
            }
            if let Some(c) = &k.content {
                return Ok(Some(c.clone()));
            }
        }
        let version = self.baseline(path).await?;
        Ok(version.and_then(|_| self.known.get(path).and_then(|k| k.content.clone())))
    }

    /// True when the file exists for this run. This is an internal check: the model does not get
    /// the content, so the first-sight version stays the baseline. It answers from the buffer
    /// and the cache first. A path that the run has not seen is fetched. When a listing saw the
    /// path absent and the store now holds it, the answer is `true`, but the baseline stays
    /// "absent", so a write to the path is a conflict.
    pub async fn exists(&mut self, path: &MemoryPath) -> Result<bool, WorkspaceError> {
        match self.changes.get(path) {
            Some(MemoryChange::Put { .. }) => return Ok(true),
            Some(MemoryChange::Delete { .. }) => return Ok(false),
            None => {}
        }
        if let Some(k) = self.known.get(path) {
            return Ok(k.version.is_some());
        }
        let file = self.store.get(&self.scope, path).await?;
        let listed_absent = self.listed.iter().any(|d| d.contains(path));
        match file {
            Some(_) if listed_absent => Ok(true),
            Some(f) => {
                self.resight.remove(path);
                self.known.insert(
                    path.clone(),
                    Known {
                        version: Some(f.version),
                        size: f.content.len() as u64,
                        content: Some(f.content),
                    },
                );
                Ok(true)
            }
            None => {
                self.resight.remove(path);
                self.known.insert(
                    path.clone(),
                    Known {
                        version: None,
                        size: 0,
                        content: None,
                    },
                );
                Ok(false)
            }
        }
    }

    /// List every file under `dir` with its size, in byte order of the path.
    pub async fn list(&mut self, dir: &MemoryDir) -> Result<Vec<(MemoryPath, u64)>, WorkspaceError> {
        if !self.listings.contains_key(dir.as_str()) {
            let entries = self.store.list(&self.scope, dir).await?;
            let mut rows = Vec::with_capacity(entries.len());
            for e in entries {
                // A path with no `known` entry under a directory that the run listed before was
                // absent at its first sight. That absence stays the baseline, so a write to the
                // path is a conflict. The listing still shows the file to the model.
                // A path with a re-sight mark is a first sight again.
                let first_sight_absent = !self.known.contains_key(&e.path) && !self.resight.contains(&e.path) && self.listed.iter().any(|d| d.contains(&e.path));
                if !first_sight_absent {
                    self.resight.remove(&e.path);
                    self.known.entry(e.path.clone()).or_insert_with(|| Known {
                        version: Some(e.version.clone()),
                        size: e.size_bytes,
                        content: None,
                    });
                }
                rows.push((e.path, e.size_bytes));
            }
            self.listed.insert(dir.clone());
            self.listings.insert(dir.as_str().to_string(), rows);
        }
        let mut merged: BTreeMap<String, (MemoryPath, u64)> = self.listings[dir.as_str()].iter().map(|(p, s)| (p.as_str().to_string(), (p.clone(), *s))).collect();
        for change in self.changes.changes() {
            if !dir.contains(change.path()) {
                continue;
            }
            match change {
                MemoryChange::Put { path, content, .. } => {
                    merged.insert(path.as_str().to_string(), (path.clone(), content.len() as u64));
                }
                MemoryChange::Delete { path, .. } => {
                    merged.remove(path.as_str());
                }
            }
        }
        Ok(merged.into_values().collect())
    }

    /// List `dir` from the store again, with the rules of `list`. It does not use the cached rows.
    pub(super) async fn list_fresh(&mut self, dir: &MemoryDir) -> Result<Vec<(MemoryPath, u64)>, WorkspaceError> {
        self.listings.remove(dir.as_str());
        self.list(dir).await
    }

    /// The early check of a directory. It lists `dir` in the store and compares each row with what
    /// the run saw. A stored path that the run saw with another version, or saw as absent, is a
    /// conflict. A path that the run saw as present and that the store no longer holds is a
    /// conflict. A stored path that the run never saw, or that has a re-sight mark and no `known`
    /// entry, becomes a first sight. It returns the first conflict in byte order. Buffered changes do not count as store
    /// state. When it passes, the fresh rows replace the cached rows of `dir`.
    pub(super) async fn check_dir(&mut self, dir: &MemoryDir) -> Result<(), WorkspaceError> {
        let entries = self.store.list(&self.scope, dir).await?;
        let stored: BTreeMap<MemoryPath, MemoryVersion> = entries.iter().map(|e| (e.path.clone(), e.version.clone())).collect();
        let mut candidates: BTreeSet<&MemoryPath> = stored.keys().collect();
        candidates.extend(self.known.iter().filter(|(p, k)| k.version.is_some() && dir.contains(p)).map(|(p, _)| p));
        let mut first = None;
        for path in candidates {
            let conflict = match stored.get(path) {
                // A `known` entry is always compared. The re-sight mark only lifts the absence that
                // a covering listing implies.
                Some(v) => match self.known.get(path) {
                    Some(k) => k.version.as_ref() != Some(v),
                    None => !self.resight.contains(path) && self.listed.iter().any(|d| d.contains(path)),
                },
                // Only a path that the run saw as present gets here.
                None => true,
            };
            if conflict {
                first = Some((path.clone(), stored.contains_key(path)));
                break;
            }
        }
        if let Some((path, in_store)) = first {
            return Err(self.conflict(&path, in_store));
        }
        let mut rows = Vec::with_capacity(entries.len());
        for e in entries {
            self.resight.remove(&e.path);
            self.known.entry(e.path.clone()).or_insert_with(|| Known {
                version: Some(e.version.clone()),
                size: e.size_bytes,
                content: None,
            });
            rows.push((e.path, e.size_bytes));
        }
        self.listed.insert(dir.clone());
        self.listings.insert(dir.as_str().to_string(), rows);
        Ok(())
    }

    /// True when the directory holds a file. The root is always a directory.
    pub async fn is_dir(&mut self, dir: &MemoryDir) -> Result<bool, WorkspaceError> {
        if dir.is_root() {
            return Ok(true);
        }
        Ok(!self.list(dir).await?.is_empty())
    }

    /// Buffer a write.
    pub async fn write(&mut self, path: &MemoryPath, content: String) -> Result<(), WorkspaceError> {
        self.stage(vec![(path.clone(), content)], Vec::new()).await
    }

    /// Buffer a delete. A file that does not exist is not an error.
    pub async fn remove(&mut self, path: &MemoryPath) -> Result<(), WorkspaceError> {
        self.stage(Vec::new(), vec![path.clone()]).await
    }

    /// Buffer several writes and deletes as one change. Either all of them are buffered or none.
    /// The caps check the net change of the whole set once.
    pub async fn stage(&mut self, puts: Vec<(MemoryPath, String)>, removes: Vec<MemoryPath>) -> Result<(), WorkspaceError> {
        self.stage_guarded(puts, removes, Vec::new()).await
    }

    /// Buffer several writes and deletes as one change, like `stage`, and arm directory guards in
    /// the same group. At commit, the store refuses the set when it holds a path under a guard
    /// that no change names.
    pub async fn stage_guarded(&mut self, puts: Vec<(MemoryPath, String)>, removes: Vec<MemoryPath>, guards: Vec<MemoryDir>) -> Result<(), WorkspaceError> {
        if self.sealed {
            return Err(WorkspaceError::Sealed);
        }
        for (path, content) in &puts {
            if content.contains('\0') {
                return Err(WorkspaceError::NulInContent { path: path.clone() });
            }
            self.caps.check_file(path, content.len() as u64).map_err(WorkspaceError::Cap)?;
        }
        // The early conflict check for every path, before anything is recorded.
        let mut put_expected = Vec::with_capacity(puts.len());
        for (path, _) in &puts {
            put_expected.push(match self.baseline(path).await? {
                Some(v) => Expected::Version(v),
                None => Expected::Absent,
            });
        }
        let mut remove_expected = Vec::with_capacity(removes.len());
        for path in &removes {
            remove_expected.push(self.baseline(path).await?);
        }
        // A fresh read for each stage, so that the usage and the baselines describe nearly the same
        // time. A usage cached for the run goes stale when another writer changes the store.
        let stored = self.store.usage(&self.scope).await?;
        let before = self.effective_usage(stored, &self.changes);
        let mut tentative = self.changes.clone();
        let touched: Vec<MemoryPath> = puts.iter().map(|(p, _)| p.clone()).chain(removes.iter().cloned()).collect();
        for ((path, content), expected) in puts.into_iter().zip(put_expected) {
            tentative.record(MemoryChange::Put { path, content, expected });
        }
        for (path, expected) in removes.into_iter().zip(remove_expected) {
            match expected {
                Some(expected) => tentative.record(MemoryChange::Delete { path, expected }),
                None => tentative.remove(&path),
            }
        }
        let after = self.effective_usage(stored, &tentative);
        self.caps.check(before, after).map_err(WorkspaceError::Cap)?;
        self.changes = tentative;
        self.join_group(&touched, guards);
        Ok(())
    }

    /// Put the touched paths and the new guards into one new group. Every path and guard of a
    /// group joins it too when one of the touched paths is in that group, or when a new guard
    /// covers one of the paths of that group. A buffered change can hide its path from the command
    /// that arms a guard, so the touched paths alone do not find every covered group. A path that
    /// has no buffered change any more leaves its group. A guard does not leave its group.
    fn join_group(&mut self, touched: &[MemoryPath], guards: Vec<MemoryDir>) {
        let group = self.next_group;
        self.next_group += 1;
        let mut joined: BTreeSet<u64> = touched.iter().filter_map(|p| self.groups.get(p).copied()).collect();
        joined.extend(self.groups.iter().filter(|(p, _)| guards.iter().any(|d| d.covers(p))).map(|(_, g)| *g));
        for g in self.groups.values_mut().chain(self.dir_guards.iter_mut().map(|(_, g)| g)) {
            if joined.contains(g) {
                *g = group;
            }
        }
        for path in touched {
            self.groups.insert(path.clone(), group);
        }
        self.dir_guards.extend(guards.into_iter().map(|dir| (dir, group)));
        let changes = &self.changes;
        self.groups.retain(|p, _| changes.get(p).is_some());
    }

    /// Remove every buffered change and every guard of the given groups. It returns the paths whose
    /// changes it removed, in byte order.
    fn drop_groups(&mut self, dropped: &BTreeSet<u64>) -> Vec<MemoryPath> {
        let paths: Vec<MemoryPath> = self.groups.iter().filter(|(_, g)| dropped.contains(g)).map(|(p, _)| p.clone()).collect();
        for path in &paths {
            self.groups.remove(path);
            self.changes.remove(path);
        }
        self.dir_guards.retain(|(_, g)| !dropped.contains(g));
        paths
    }

    /// The armed guards, in byte order, with no duplicates.
    #[cfg(test)]
    pub(crate) fn armed_guards(&self) -> Vec<MemoryDir> {
        self.dir_guards.iter().map(|(d, _)| d.clone()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    /// Give the buffer to the commit and seal the workspace.
    pub fn take_changes(&mut self) -> MemoryCommitSet {
        self.sealed = true;
        let mut set = MemoryCommitSet::new(self.scope.clone(), std::mem::take(&mut self.changes));
        set.guards = std::mem::take(&mut self.dir_guards).into_iter().map(|(d, _)| d).collect();
        set
    }

    /// Seal the workspace and apply the buffer to the store.
    ///
    /// It takes the buffer and seals the workspace before `apply`. So on any error, a `Store`
    /// error included, the buffer is gone. The caller must not retry with the same workspace.
    pub async fn commit(&mut self) -> Result<(), MemoryApplyError> {
        let set = self.take_changes();
        self.store.apply(&set, &self.caps).await
    }

    /// The early conflict check. It reads the store version of `path` and compares it with the
    /// baseline. The first sight of a path becomes its baseline: a read, a listing that returns
    /// the path, or a listing of a directory that holds the path but does not return it (then the
    /// baseline is "absent"). A conflict goes through `conflict`.
    async fn baseline(&mut self, path: &MemoryPath) -> Result<Option<MemoryVersion>, WorkspaceError> {
        let fetched = self.store.get(&self.scope, path).await?;
        let fetched_version = fetched.as_ref().map(|f| f.version.clone());
        let seen = match self.known.get(path) {
            Some(k) => Some(k.version.clone()),
            None if self.listed.iter().any(|d| d.contains(path)) => Some(None),
            None => None,
        };
        match seen {
            Some(v) if v != fetched_version => Err(self.conflict(path, fetched.is_some())),
            Some(v) => {
                // A new `known` entry is a new baseline, so the re-sight mark is used up.
                self.resight.remove(path);
                let known = self.known.entry(path.clone()).or_insert(Known {
                    version: None,
                    size: 0,
                    content: None,
                });
                // The store holds the baseline version, so its content is the baseline content.
                if known.content.is_none()
                    && let Some(f) = fetched
                {
                    known.content = Some(f.content);
                }
                Ok(v)
            }
            None => {
                self.resight.remove(path);
                self.known.insert(
                    path.clone(),
                    Known {
                        version: fetched_version.clone(),
                        size: fetched.as_ref().map_or(0, |f| f.content.len() as u64),
                        content: fetched.map(|f| f.content),
                    },
                );
                Ok(fetched_version)
            }
        }
    }

    /// Handle an early conflict on `path`. `stored` is true when the store holds `path` now.
    ///
    /// 1. It drops the `known` entry of `path` and the cached listings that hold it.
    /// 2. It drops the group of `path`.
    /// 3. When the store holds `path`, it drops every group that has a guard that covers `path`.
    ///    This also drops a group that holds only guards.
    /// 4. It marks `path` for a re-sight, so the next fresh listing that holds it is a first sight.
    /// 5. It returns the conflict. `also` holds the other dropped paths, in byte order.
    fn conflict(&mut self, path: &MemoryPath, stored: bool) -> WorkspaceError {
        self.known.remove(path);
        self.drop_listings_of(path);
        self.changes.remove(path);
        let mut dropped: BTreeSet<u64> = self.groups.get(path).copied().into_iter().collect();
        if stored {
            dropped.extend(self.dir_guards.iter().filter(|(d, _)| d.covers(path)).map(|(_, g)| *g));
        }
        let mut also = self.drop_groups(&dropped);
        also.retain(|p| p != path);
        self.resight.insert(path.clone());
        WorkspaceError::Conflict { path: path.clone(), also }
    }

    /// Drop the cached rows of every listing that holds `path`. The `listed` set stays.
    fn drop_listings_of(&mut self, path: &MemoryPath) {
        self.listings.retain(|d, _| !MemoryDir::parse(d).is_ok_and(|d| d.contains(path)));
    }

    /// The stored usage plus the net delta of a change set. An overwrite or a delete subtracts
    /// the old size.
    fn effective_usage(&self, stored: MemoryUsage, changes: &MemoryChangeSet) -> MemoryUsage {
        let mut files = i128::from(stored.files);
        let mut bytes = i128::from(stored.bytes);
        for change in changes.changes() {
            let old = i128::from(self.known.get(change.path()).map_or(0, |k| k.size));
            match change {
                MemoryChange::Put { content, expected, .. } => {
                    bytes += content.len() as i128;
                    match expected {
                        Expected::Absent => files += 1,
                        Expected::Version(_) => bytes -= old,
                    }
                }
                MemoryChange::Delete { .. } => {
                    files -= 1;
                    bytes -= old;
                }
            }
        }
        MemoryUsage {
            files: files.max(0) as u64,
            bytes: bytes.max(0) as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::conformance::fresh_scope;
    use crate::memory::fake::InMemoryMemoryStore;

    fn p(s: &str) -> MemoryPath {
        MemoryPath::parse(s).unwrap()
    }

    /// Change a file directly in the fake, as another writer would.
    async fn direct(fake: &InMemoryMemoryStore, scope: &MemoryScope, path: &str, content: &str) {
        let current = fake.get(scope, &p(path)).await.unwrap();
        let expected = match current {
            Some(f) => Expected::Version(f.version),
            None => Expected::Absent,
        };
        let mut changes = MemoryChangeSet::new();
        changes.record(MemoryChange::Put {
            path: p(path),
            content: content.into(),
            expected,
        });
        let set = MemoryCommitSet::new(scope.clone(), changes);
        fake.apply(&set, &MemoryCaps::default()).await.unwrap();
    }

    fn setup(caps: MemoryCaps) -> (InMemoryMemoryStore, MemoryScope, MemoryWorkspace) {
        let fake = InMemoryMemoryStore::default();
        let scope = fresh_scope();
        let ws = MemoryWorkspace::new(Arc::new(fake.clone()), scope.clone(), caps);
        (fake, scope, ws)
    }

    #[tokio::test]
    async fn read_your_writes() {
        let (_f, _s, mut ws) = setup(MemoryCaps::default());
        ws.write(&p("/a"), "one".into()).await.unwrap();
        assert_eq!(ws.read(&p("/a")).await.unwrap().as_deref(), Some("one"));
        ws.remove(&p("/a")).await.unwrap();
        assert_eq!(ws.read(&p("/a")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn repeatable_read() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "v1").await;
        assert_eq!(ws.read(&p("/a")).await.unwrap().as_deref(), Some("v1"));
        direct(&f, &s, "/a", "v2").await;
        assert_eq!(ws.read(&p("/a")).await.unwrap().as_deref(), Some("v1"));
    }

    #[tokio::test]
    async fn early_conflict_refuses_and_clears() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "v1").await;
        ws.read(&p("/a")).await.unwrap();
        direct(&f, &s, "/a", "v2").await;
        let err = ws.write(&p("/a"), "mine".into()).await.unwrap_err();
        assert_eq!(err, WorkspaceError::Conflict { path: p("/a"), also: Vec::new() });
        assert_eq!(
            err.to_string(),
            "The file /memories/a changed outside this run. Your changes to it in this run are gone. Use view to read it again."
        );
        assert_eq!(ws.read(&p("/a")).await.unwrap().as_deref(), Some("v2"));
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn early_conflict_drops_an_earlier_buffered_change() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "v1").await;
        ws.write(&p("/a"), "mine".into()).await.unwrap();
        direct(&f, &s, "/a", "v2").await;
        assert!(matches!(ws.write(&p("/a"), "more".into()).await, Err(WorkspaceError::Conflict { .. })));
        assert_eq!(ws.read(&p("/a")).await.unwrap().as_deref(), Some("v2"));
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn expected_values() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        ws.write(&p("/new"), "n".into()).await.unwrap();
        direct(&f, &s, "/old", "o").await;
        ws.read(&p("/old")).await.unwrap();
        let v = f.get(&s, &p("/old")).await.unwrap().unwrap().version;
        ws.write(&p("/old"), "o2".into()).await.unwrap();
        let set = ws.take_changes();
        assert_eq!(
            set.changes.get(&p("/new")),
            Some(&MemoryChange::Put {
                path: p("/new"),
                content: "n".into(),
                expected: Expected::Absent
            })
        );
        assert_eq!(
            set.changes.get(&p("/old")),
            Some(&MemoryChange::Put {
                path: p("/old"),
                content: "o2".into(),
                expected: Expected::Version(v)
            })
        );
    }

    #[tokio::test]
    async fn nul_is_refused() {
        let (_f, _s, mut ws) = setup(MemoryCaps::default());
        let err = ws.write(&p("/a"), "a\0b".into()).await.unwrap_err();
        assert_eq!(err, WorkspaceError::NulInContent { path: p("/a") });
        assert!(err.to_string().contains("/memories/a"));
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn caps_use_the_net_delta() {
        let caps = MemoryCaps {
            max_store_files: 3,
            ..MemoryCaps::default()
        };
        let (f, s, mut ws) = setup(caps);
        for n in ["/a", "/b", "/c"] {
            direct(&f, &s, n, "x").await;
        }
        ws.write(&p("/a"), "y".into()).await.unwrap();
        ws.remove(&p("/b")).await.unwrap();
        ws.write(&p("/d"), "z".into()).await.unwrap();
        let err = ws.write(&p("/e"), "z".into()).await.unwrap_err();
        assert!(matches!(err, WorkspaceError::Cap(CapError::StoreFiles { .. })), "{err:?}");
    }

    #[tokio::test]
    async fn byte_cap_subtracts_the_old_size_of_an_overwrite() {
        let caps = MemoryCaps {
            max_store_bytes: 10,
            ..MemoryCaps::default()
        };
        let (f, s, mut ws) = setup(caps);
        direct(&f, &s, "/a", "0123456789").await;
        ws.write(&p("/a"), "abcdefghij".into()).await.unwrap();
        let err = ws.write(&p("/a"), "abcdefghijk".into()).await.unwrap_err();
        assert!(matches!(err, WorkspaceError::Cap(CapError::StoreBytes { .. })), "{err:?}");
    }

    #[tokio::test]
    async fn file_cap_is_checked() {
        let caps = MemoryCaps {
            max_file_bytes: 3,
            ..MemoryCaps::default()
        };
        let (_f, _s, mut ws) = setup(caps);
        assert!(matches!(ws.write(&p("/a"), "abcd".into()).await, Err(WorkspaceError::Cap(CapError::File { .. }))));
    }

    #[tokio::test]
    async fn seal_blocks_writes() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "v1").await;
        ws.take_changes();
        assert!(ws.is_sealed());
        assert_eq!(ws.write(&p("/a"), "x".into()).await, Err(WorkspaceError::Sealed));
        assert_eq!(ws.remove(&p("/a")).await, Err(WorkspaceError::Sealed));
        assert_eq!(ws.read(&p("/a")).await.unwrap().as_deref(), Some("v1"));
        assert_eq!(WorkspaceError::Sealed.to_string(), "The run is ending. Memory is read-only now.");
    }

    #[tokio::test]
    async fn drop_discards() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        ws.write(&p("/a"), "x".into()).await.unwrap();
        drop(ws);
        assert_eq!(f.usage(&s).await.unwrap(), MemoryUsage::default());
    }

    #[tokio::test]
    async fn commit_applies() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        ws.write(&p("/a"), "x".into()).await.unwrap();
        ws.commit().await.unwrap();
        assert_eq!(f.get(&s, &p("/a")).await.unwrap().unwrap().content, "x");
    }

    #[tokio::test]
    async fn list_merges_the_buffer_in_byte_order() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/d/b", "bb").await;
        direct(&f, &s, "/d/c", "c").await;
        ws.write(&p("/d/a"), "aaa".into()).await.unwrap();
        ws.remove(&p("/d/c")).await.unwrap();
        ws.write(&p("/d/b"), "B".into()).await.unwrap();
        let dir = MemoryDir::parse("/d").unwrap();
        assert_eq!(ws.list(&dir).await.unwrap(), vec![(p("/d/a"), 3), (p("/d/b"), 1)]);
        assert!(ws.is_dir(&dir).await.unwrap());
        assert!(ws.is_dir(&MemoryDir::root()).await.unwrap());
        assert!(!ws.is_dir(&MemoryDir::parse("/none").unwrap()).await.unwrap());
    }

    #[tokio::test]
    async fn a_listed_file_changed_outside_the_run_is_a_conflict_on_write() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/x.md", "v1").await;
        ws.list(&MemoryDir::parse("/tasks").unwrap()).await.unwrap();
        direct(&f, &s, "/tasks/x.md", "console").await;
        let err = ws.write(&p("/tasks/x.md"), "mine".into()).await.unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                path: p("/tasks/x.md"),
                also: Vec::new()
            }
        );
        ws.commit().await.unwrap();
        assert_eq!(f.get(&s, &p("/tasks/x.md")).await.unwrap().unwrap().content, "console");
    }

    #[tokio::test]
    async fn a_path_absent_from_a_listing_and_created_outside_the_run_is_a_conflict_on_write() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/other.md", "o").await;
        let listed = ws.list(&MemoryDir::parse("/tasks").unwrap()).await.unwrap();
        assert_eq!(listed, vec![(p("/tasks/other.md"), 1)]);
        direct(&f, &s, "/tasks/x.md", "console").await;
        let err = ws.write(&p("/tasks/x.md"), "mine".into()).await.unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                path: p("/tasks/x.md"),
                also: Vec::new()
            }
        );
        ws.commit().await.unwrap();
        assert_eq!(f.get(&s, &p("/tasks/x.md")).await.unwrap().unwrap().content, "console");
    }

    #[tokio::test]
    async fn a_root_listing_makes_every_unseen_path_absent() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        ws.list(&MemoryDir::root()).await.unwrap();
        direct(&f, &s, "/deep/a/b.md", "console").await;
        assert!(matches!(ws.write(&p("/deep/a/b.md"), "mine".into()).await, Err(WorkspaceError::Conflict { .. })));
        // The conflict sends the model back to view. After it reads the file, the write passes.
        assert_eq!(ws.read(&p("/deep/a/b.md")).await.unwrap().as_deref(), Some("console"));
        ws.write(&p("/deep/a/b.md"), "mine".into()).await.unwrap();
        ws.commit().await.unwrap();
        assert_eq!(f.get(&s, &p("/deep/a/b.md")).await.unwrap().unwrap().content, "mine");
    }

    #[tokio::test]
    async fn a_read_after_a_listing_takes_the_fetched_file_as_the_baseline() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/d/x", "v1").await;
        let dir = MemoryDir::parse("/d").unwrap();
        assert_eq!(ws.list(&dir).await.unwrap(), vec![(p("/d/x"), 2)]);
        direct(&f, &s, "/d/x", "changed").await;
        // The model sees the new content, so the new content is the baseline.
        assert_eq!(ws.read(&p("/d/x")).await.unwrap().as_deref(), Some("changed"));
        assert_eq!(ws.list(&dir).await.unwrap(), vec![(p("/d/x"), 7)]);
        let v = f.get(&s, &p("/d/x")).await.unwrap().unwrap().version;
        ws.write(&p("/d/x"), "mine".into()).await.unwrap();
        assert_eq!(
            ws.take_changes().changes.get(&p("/d/x")),
            Some(&MemoryChange::Put {
                path: p("/d/x"),
                content: "mine".into(),
                expected: Expected::Version(v)
            })
        );
    }

    #[tokio::test]
    async fn a_read_after_a_listing_of_a_file_deleted_outside_the_run_shows_it_absent() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/d/x", "v1").await;
        let dir = MemoryDir::parse("/d").unwrap();
        ws.list(&dir).await.unwrap();
        let v = f.get(&s, &p("/d/x")).await.unwrap().unwrap().version;
        let mut changes = MemoryChangeSet::new();
        changes.record(MemoryChange::Delete { path: p("/d/x"), expected: v });
        f.apply(&MemoryCommitSet::new(s.clone(), changes), &MemoryCaps::default()).await.unwrap();
        assert_eq!(ws.read(&p("/d/x")).await.unwrap(), None);
        assert_eq!(ws.list(&dir).await.unwrap(), vec![]);
    }

    #[tokio::test]
    async fn a_later_listing_keeps_an_earlier_absence_as_the_baseline() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        assert_eq!(ws.list(&MemoryDir::root()).await.unwrap(), vec![]);
        direct(&f, &s, "/d/x", "console").await;
        // The listing shows the file to the model, but the first sight of the path was "absent".
        assert_eq!(ws.list(&MemoryDir::parse("/d").unwrap()).await.unwrap(), vec![(p("/d/x"), 7)]);
        let err = ws.write(&p("/d/x"), "mine".into()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "The file /memories/d/x changed outside this run. Your changes to it in this run are gone. Use view to read it again."
        );
        ws.commit().await.unwrap();
        assert_eq!(f.get(&s, &p("/d/x")).await.unwrap().unwrap().content, "console");
    }

    #[tokio::test]
    async fn a_first_listing_records_the_version_of_a_listed_file() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/d/x", "v1").await;
        let v = f.get(&s, &p("/d/x")).await.unwrap().unwrap().version;
        ws.list(&MemoryDir::parse("/d").unwrap()).await.unwrap();
        ws.write(&p("/d/x"), "mine".into()).await.unwrap();
        assert_eq!(
            ws.take_changes().changes.get(&p("/d/x")),
            Some(&MemoryChange::Put {
                path: p("/d/x"),
                content: "mine".into(),
                expected: Expected::Version(v)
            })
        );
    }

    #[tokio::test]
    async fn the_byte_cap_uses_a_fresh_stored_usage() {
        let caps = MemoryCaps {
            max_store_bytes: 10,
            ..MemoryCaps::default()
        };
        let (f, s, mut ws) = setup(caps);
        direct(&f, &s, "/old", "0123456789").await;
        // A first stage reads the stored usage (10 bytes).
        ws.remove(&p("/nope")).await.unwrap();
        // Another writer deletes /old and creates a 1-byte /new.
        let v = f.get(&s, &p("/old")).await.unwrap().unwrap().version;
        let mut changes = MemoryChangeSet::new();
        changes.record(MemoryChange::Delete { path: p("/old"), expected: v });
        f.apply(&MemoryCommitSet::new(s.clone(), changes), &MemoryCaps::default()).await.unwrap();
        direct(&f, &s, "/new", "n").await;
        // The real result is 2 bytes, so the cap must not refuse the write.
        ws.write(&p("/new"), "ab".into()).await.unwrap();
        ws.commit().await.unwrap();
        assert_eq!(f.usage(&s).await.unwrap(), MemoryUsage { files: 1, bytes: 2 });
    }

    #[tokio::test]
    async fn a_conflict_on_one_path_of_a_staged_move_drops_the_whole_move() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "original").await;
        let moved = ws.read(&p("/a")).await.unwrap().unwrap();
        // A rename: one stage call puts /b and deletes /a.
        ws.stage(vec![(p("/b"), moved)], vec![p("/a")]).await.unwrap();
        direct(&f, &s, "/b", "other").await;
        let err = ws.write(&p("/b"), "mine".into()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "The file /memories/b changed outside this run. Your changes to it in this run are gone. Your related changes to /memories/a are also gone. Use view to read it again."
        );
        let set = ws.take_changes();
        assert!(set.changes.get(&p("/b")).is_none(), "{set:?}");
        assert!(set.changes.get(&p("/a")).is_none(), "{set:?}");
        assert!(set.changes.is_empty(), "{set:?}");
        f.apply(&set, &MemoryCaps::default()).await.unwrap();
        assert_eq!(f.get(&s, &p("/a")).await.unwrap().unwrap().content, "original");
        assert_eq!(f.get(&s, &p("/b")).await.unwrap().unwrap().content, "other");
    }

    #[tokio::test]
    async fn a_later_stage_into_a_moved_path_joins_the_move() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "original").await;
        direct(&f, &s, "/c", "c").await;
        let moved = ws.read(&p("/a")).await.unwrap().unwrap();
        ws.stage(vec![(p("/b"), moved)], vec![p("/a")]).await.unwrap();
        // A second move takes /b on to /c. The Put of /b merges away, so the buffer holds only the
        // Delete of /a and the Put of /c, and /c joins the group of /a.
        ws.read(&p("/c")).await.unwrap();
        ws.stage(vec![(p("/c"), "original".into())], vec![p("/b")]).await.unwrap();
        direct(&f, &s, "/c", "console").await;
        let err = ws.write(&p("/c"), "mine".into()).await.unwrap_err();
        assert!(err.to_string().contains("Your related changes to /memories/a are also gone."), "{err}");
        let set = ws.take_changes();
        assert!(set.changes.is_empty(), "{set:?}");
        f.apply(&set, &MemoryCaps::default()).await.unwrap();
        assert_eq!(f.get(&s, &p("/a")).await.unwrap().unwrap().content, "original");
    }

    #[tokio::test]
    async fn a_single_file_conflict_drops_only_that_path() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a", "v1").await;
        ws.write(&p("/a"), "mine".into()).await.unwrap();
        ws.write(&p("/other"), "keep".into()).await.unwrap();
        direct(&f, &s, "/a", "v2").await;
        let err = ws.write(&p("/a"), "more".into()).await.unwrap_err();
        assert_eq!(err, WorkspaceError::Conflict { path: p("/a"), also: Vec::new() });
        let set = ws.take_changes();
        assert_eq!(set.changes.len(), 1);
        assert!(set.changes.get(&p("/other")).is_some());
    }

    #[tokio::test]
    async fn remove_of_a_missing_file_is_a_no_op() {
        let (_f, _s, mut ws) = setup(MemoryCaps::default());
        ws.remove(&p("/nope")).await.unwrap();
        assert!(ws.take_changes().changes.is_empty());
    }

    fn d(s: &str) -> MemoryDir {
        MemoryDir::parse(s).unwrap()
    }

    /// Delete a file directly in the fake, as another writer would.
    async fn direct_delete(fake: &InMemoryMemoryStore, scope: &MemoryScope, path: &str) {
        let v = fake.get(scope, &p(path)).await.unwrap().unwrap().version;
        let mut changes = MemoryChangeSet::new();
        changes.record(MemoryChange::Delete { path: p(path), expected: v });
        fake.apply(&MemoryCommitSet::new(scope.clone(), changes), &MemoryCaps::default()).await.unwrap();
    }

    #[tokio::test]
    async fn a_guard_is_armed_and_reaches_the_commit_set() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.stage_guarded(vec![], vec![p("/tasks/a")], vec![d("/tasks")]).await.unwrap();
        assert_eq!(ws.armed_guards(), vec![d("/tasks")]);
        assert_eq!(ws.take_changes().guards, BTreeSet::from([d("/tasks")]));
    }

    #[tokio::test]
    async fn a_conflict_in_a_group_drops_its_guard() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.stage_guarded(vec![], vec![p("/tasks/a")], vec![d("/tasks")]).await.unwrap();
        direct(&f, &s, "/tasks/a", "changed").await;
        assert!(matches!(ws.write(&p("/tasks/a"), "mine".into()).await, Err(WorkspaceError::Conflict { .. })));
        assert!(ws.armed_guards().is_empty());
        assert!(ws.take_changes().is_empty());
    }

    #[tokio::test]
    async fn a_later_stage_into_the_group_carries_the_guard() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.stage_guarded(vec![], vec![p("/tasks/a")], vec![d("/tasks")]).await.unwrap();
        ws.write(&p("/tasks/a"), "again".into()).await.unwrap();
        assert_eq!(ws.armed_guards(), vec![d("/tasks")]);
        let set = ws.take_changes();
        assert_eq!(set.guards, BTreeSet::from([d("/tasks")]));
        assert_eq!(set.changes.len(), 1);
    }

    #[tokio::test]
    async fn a_joined_guard_drops_with_its_group_when_the_path_is_gone() {
        // The store no longer holds the path, so step 3 does not apply. Only the renumbered group
        // (step 2) drops the guard.
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.stage_guarded(vec![], vec![p("/tasks/a")], vec![d("/tasks")]).await.unwrap();
        ws.write(&p("/tasks/a"), "again".into()).await.unwrap();
        direct_delete(&f, &s, "/tasks/a").await;
        assert!(matches!(ws.write(&p("/tasks/a"), "more".into()).await, Err(WorkspaceError::Conflict { .. })));
        assert!(ws.armed_guards().is_empty());
        assert!(ws.take_changes().is_empty());
    }

    #[tokio::test]
    async fn an_early_conflict_under_a_guard_drops_the_guard_group() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.list(&d("/tasks")).await.unwrap();
        ws.stage_guarded(vec![], vec![p("/tasks/a")], vec![d("/tasks")]).await.unwrap();
        ws.write(&p("/tasks/n"), "n".into()).await.unwrap();
        direct(&f, &s, "/tasks/n", "theirs").await;
        let err = ws.write(&p("/tasks/n"), "more".into()).await.unwrap_err();
        assert_eq!(
            err,
            WorkspaceError::Conflict {
                path: p("/tasks/n"),
                also: vec![p("/tasks/a")]
            }
        );
        assert!(ws.armed_guards().is_empty());
        let set = ws.take_changes();
        assert!(set.is_empty(), "{set:?}");
        f.apply(&set, &MemoryCaps::default()).await.unwrap();
    }

    #[tokio::test]
    async fn a_guard_only_group_drops_on_an_early_conflict_under_it() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        ws.list(&d("/t")).await.unwrap();
        ws.write(&p("/t/x"), "x".into()).await.unwrap();
        ws.stage_guarded(vec![], vec![p("/t/x")], vec![d("/t")]).await.unwrap();
        // The remove cancels the buffered create, so the group holds only the guard.
        assert!(ws.changes.is_empty());
        assert_eq!(ws.armed_guards(), vec![d("/t")]);
        direct(&f, &s, "/t/y", "theirs").await;
        let err = ws.write(&p("/t/y"), "mine".into()).await.unwrap_err();
        assert_eq!(err, WorkspaceError::Conflict { path: p("/t/y"), also: Vec::new() });
        assert!(ws.armed_guards().is_empty());
        assert!(ws.take_changes().is_empty());
    }

    #[tokio::test]
    async fn a_conflict_leaves_unrelated_guards() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/a/x", "x").await;
        direct(&f, &s, "/b/y", "y").await;
        ws.stage_guarded(vec![], vec![p("/a/x")], vec![d("/a")]).await.unwrap();
        ws.stage_guarded(vec![], vec![p("/b/y")], vec![d("/b")]).await.unwrap();
        direct(&f, &s, "/a/x", "changed").await;
        assert!(matches!(ws.write(&p("/a/x"), "mine".into()).await, Err(WorkspaceError::Conflict { .. })));
        assert_eq!(ws.armed_guards(), vec![d("/b")]);
        let set = ws.take_changes();
        assert_eq!(set.guards, BTreeSet::from([d("/b")]));
        assert_eq!(set.changes.len(), 1);
        assert!(set.changes.get(&p("/b/y")).is_some());
    }

    #[tokio::test]
    async fn check_dir_conflicts_on_a_new_path_under_a_listed_dir() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.list(&d("/tasks")).await.unwrap();
        direct(&f, &s, "/tasks/b", "b").await;
        assert_eq!(
            ws.check_dir(&d("/tasks")).await,
            Err(WorkspaceError::Conflict {
                path: p("/tasks/b"),
                also: Vec::new()
            })
        );
    }

    #[tokio::test]
    async fn check_dir_conflicts_on_a_seen_path_that_is_gone() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.list(&d("/tasks")).await.unwrap();
        direct_delete(&f, &s, "/tasks/a").await;
        assert_eq!(
            ws.check_dir(&d("/tasks")).await,
            Err(WorkspaceError::Conflict {
                path: p("/tasks/a"),
                also: Vec::new()
            })
        );
    }

    #[tokio::test]
    async fn check_dir_records_a_never_seen_path_and_replaces_the_cache() {
        // The stale cached row: the root listing makes /tasks/x absent at first sight, a later
        // listing of /tasks caches its row, and then the file goes away.
        let (f, s, mut ws) = setup(MemoryCaps::default());
        assert_eq!(ws.list(&MemoryDir::root()).await.unwrap(), vec![]);
        direct(&f, &s, "/tasks/x", "x").await;
        assert_eq!(ws.list(&d("/tasks")).await.unwrap(), vec![(p("/tasks/x"), 1)]);
        direct_delete(&f, &s, "/tasks/x").await;
        ws.check_dir(&d("/tasks")).await.unwrap();
        assert_eq!(ws.list(&d("/tasks")).await.unwrap(), vec![]);

        // A path that the run never saw becomes a first sight with its stored version.
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/u/k", "k").await;
        let v = f.get(&s, &p("/u/k")).await.unwrap().unwrap().version;
        ws.check_dir(&d("/u")).await.unwrap();
        assert_eq!(ws.list(&d("/u")).await.unwrap(), vec![(p("/u/k"), 1)]);
        ws.write(&p("/u/k"), "mine".into()).await.unwrap();
        assert_eq!(
            ws.take_changes().changes.get(&p("/u/k")),
            Some(&MemoryChange::Put {
                path: p("/u/k"),
                content: "mine".into(),
                expected: Expected::Version(v)
            })
        );
    }

    #[tokio::test]
    async fn a_re_sight_mark_lets_a_directory_view_end_the_loop() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.list(&d("/tasks")).await.unwrap();
        direct(&f, &s, "/tasks/b", "b").await;
        assert!(matches!(ws.check_dir(&d("/tasks")).await, Err(WorkspaceError::Conflict { .. })));
        assert_eq!(ws.list(&d("/tasks")).await.unwrap(), vec![(p("/tasks/a"), 1), (p("/tasks/b"), 1)]);
        ws.check_dir(&d("/tasks")).await.unwrap();
    }

    #[tokio::test]
    async fn a_re_sight_mark_is_used_once() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "a").await;
        ws.list(&d("/tasks")).await.unwrap();
        direct(&f, &s, "/tasks/b", "b").await;
        assert!(matches!(ws.check_dir(&d("/tasks")).await, Err(WorkspaceError::Conflict { .. })));
        ws.list(&d("/tasks")).await.unwrap();
        ws.check_dir(&d("/tasks")).await.unwrap();
        direct(&f, &s, "/tasks/b", "b2").await;
        assert_eq!(
            ws.check_dir(&d("/tasks")).await,
            Err(WorkspaceError::Conflict {
                path: p("/tasks/b"),
                also: Vec::new()
            })
        );
    }

    #[tokio::test]
    async fn a_re_sight_mark_keeps_the_version_check_of_a_later_baseline() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "v1").await;
        ws.read(&p("/tasks/a")).await.unwrap();
        direct(&f, &s, "/tasks/a", "v2").await;
        assert!(matches!(ws.write(&p("/tasks/a"), "mine".into()).await, Err(WorkspaceError::Conflict { .. })));
        // The second write is a plain first sight of v2.
        ws.write(&p("/tasks/a"), "mine".into()).await.unwrap();
        direct(&f, &s, "/tasks/a", "v3").await;
        assert_eq!(
            ws.check_dir(&d("/tasks")).await,
            Err(WorkspaceError::Conflict {
                path: p("/tasks/a"),
                also: Vec::new()
            })
        );
    }

    #[tokio::test]
    async fn a_re_sight_mark_keeps_the_check_of_a_later_absent_baseline() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        direct(&f, &s, "/tasks/a", "v1").await;
        ws.read(&p("/tasks/a")).await.unwrap();
        direct_delete(&f, &s, "/tasks/a").await;
        assert!(matches!(ws.write(&p("/tasks/a"), "mine".into()).await, Err(WorkspaceError::Conflict { .. })));
        // The second write is a plain first sight of the absent path.
        ws.write(&p("/tasks/a"), "mine".into()).await.unwrap();
        direct(&f, &s, "/tasks/a", "theirs").await;
        assert_eq!(
            ws.check_dir(&d("/tasks")).await,
            Err(WorkspaceError::Conflict {
                path: p("/tasks/a"),
                also: Vec::new()
            })
        );
    }

    #[tokio::test]
    async fn list_fresh_ignores_the_cache() {
        let (f, s, mut ws) = setup(MemoryCaps::default());
        assert_eq!(ws.list(&d("/done")).await.unwrap(), vec![]);
        direct(&f, &s, "/done/z", "z").await;
        assert_eq!(ws.list(&d("/done")).await.unwrap(), vec![]);
        assert_eq!(ws.list_fresh(&d("/done")).await.unwrap(), vec![(p("/done/z"), 1)]);
    }
}
