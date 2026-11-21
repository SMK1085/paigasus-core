// SPDX-License-Identifier: Apache-2.0

//! Versions and the change set of a run.

use std::collections::BTreeSet;

use super::{MemoryDir, MemoryPath, MemoryScope};

/// An opaque version of a file. The adapter makes it. The core only compares versions for equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryVersion(String);

impl MemoryVersion {
    /// Wrap an adapter version string.
    #[must_use]
    pub fn new(version: impl Into<String>) -> Self {
        MemoryVersion(version.into())
    }

    /// The version as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The state that a write expects to find in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// The file must not exist.
    Absent,
    /// The file must have this version.
    Version(MemoryVersion),
}

/// One change to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryChange {
    /// Create or replace a file.
    Put { path: MemoryPath, content: String, expected: Expected },
    /// Delete a file that has this version.
    Delete { path: MemoryPath, expected: MemoryVersion },
}

impl MemoryChange {
    /// The path that the change applies to.
    #[must_use]
    pub fn path(&self) -> &MemoryPath {
        match self {
            MemoryChange::Put { path, .. } | MemoryChange::Delete { path, .. } => path,
        }
    }
}

/// An ordered list of changes with at most one change per path. The merge rule keeps the first
/// `expected` value and the last operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryChangeSet {
    changes: Vec<MemoryChange>,
}

impl MemoryChangeSet {
    /// An empty change set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a change and merge it with an earlier change of the same path.
    pub fn record(&mut self, change: MemoryChange) {
        let Some(i) = self.changes.iter().position(|c| c.path() == change.path()) else {
            self.changes.push(change);
            return;
        };
        let merged = match (&self.changes[i], change) {
            (MemoryChange::Put { expected, .. }, MemoryChange::Put { path, content, .. }) => Some(MemoryChange::Put {
                path,
                content,
                expected: expected.clone(),
            }),
            (MemoryChange::Put { expected: Expected::Absent, .. }, MemoryChange::Delete { .. }) => None,
            (MemoryChange::Put { expected: Expected::Version(v), .. }, MemoryChange::Delete { path, .. }) => Some(MemoryChange::Delete { path, expected: v.clone() }),
            (MemoryChange::Delete { expected, .. }, MemoryChange::Put { path, content, .. }) => Some(MemoryChange::Put {
                path,
                content,
                expected: Expected::Version(expected.clone()),
            }),
            // A second delete keeps the first expected version.
            (MemoryChange::Delete { .. }, MemoryChange::Delete { .. }) => return,
        };
        match merged {
            Some(m) => self.changes[i] = m,
            None => {
                self.changes.remove(i);
            }
        }
    }

    /// The changes, in the order of the first record of each path.
    #[must_use]
    pub fn changes(&self) -> &[MemoryChange] {
        &self.changes
    }

    /// The change of a path, if any.
    #[must_use]
    pub fn get(&self, path: &MemoryPath) -> Option<&MemoryChange> {
        self.changes.iter().find(|c| c.path() == path)
    }

    /// Remove the change of a path.
    pub fn remove(&mut self, path: &MemoryPath) {
        self.changes.retain(|c| c.path() != path);
    }

    /// True when there is no change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The number of changes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }
}

/// A change set with the scope that it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCommitSet {
    /// The scope of every change.
    pub scope: MemoryScope,
    /// The changes.
    pub changes: MemoryChangeSet,
    /// The directories that the run read or listed. A store checks each one at commit time.
    pub guards: BTreeSet<MemoryDir>,
}

impl MemoryCommitSet {
    /// A commit set with no guard.
    #[must_use]
    pub fn new(scope: MemoryScope, changes: MemoryChangeSet) -> Self {
        MemoryCommitSet {
            scope,
            changes,
            guards: BTreeSet::new(),
        }
    }

    /// True when there is no change and no guard.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.guards.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> MemoryPath {
        MemoryPath::parse(s).unwrap()
    }
    fn v(s: &str) -> MemoryVersion {
        MemoryVersion::new(s)
    }
    fn put(path: &str, content: &str, expected: Expected) -> MemoryChange {
        MemoryChange::Put {
            path: p(path),
            content: content.into(),
            expected,
        }
    }
    fn del(path: &str, ver: &str) -> MemoryChange {
        MemoryChange::Delete { path: p(path), expected: v(ver) }
    }

    #[test]
    fn a_commit_set_is_empty_only_with_no_change_and_no_guard() {
        let scope = crate::memory::conformance::fresh_scope();
        let mut set = MemoryCommitSet::new(scope.clone(), MemoryChangeSet::new());
        assert!(set.is_empty());
        set.guards.insert(MemoryDir::parse("/tasks").unwrap());
        assert!(!set.is_empty());
        let mut cs = MemoryChangeSet::new();
        cs.record(put("/f", "one", Expected::Absent));
        assert!(!MemoryCommitSet::new(scope, cs).is_empty());
    }

    #[test]
    fn change_set_merge_table() {
        let mut s = MemoryChangeSet::new();
        s.record(put("/f", "one", Expected::Absent));
        s.record(put("/f", "two", Expected::Version(v("9"))));
        assert_eq!(s.changes(), [put("/f", "two", Expected::Absent)]);

        let mut s = MemoryChangeSet::new();
        s.record(put("/f", "one", Expected::Absent));
        s.record(del("/f", "9"));
        assert!(s.is_empty());

        let mut s = MemoryChangeSet::new();
        s.record(put("/f", "one", Expected::Version(v("3"))));
        s.record(del("/f", "9"));
        assert_eq!(s.changes(), [del("/f", "3")]);

        let mut s = MemoryChangeSet::new();
        s.record(del("/f", "3"));
        s.record(put("/f", "new", Expected::Absent));
        assert_eq!(s.changes(), [put("/f", "new", Expected::Version(v("3")))]);

        let mut s = MemoryChangeSet::new();
        s.record(put("/f", "one", Expected::Version(v("3"))));
        s.record(put("/f", "two", Expected::Version(v("9"))));
        assert_eq!(s.changes(), [put("/f", "two", Expected::Version(v("3")))]);
    }

    #[test]
    fn change_set_edge_cases() {
        let mut s = MemoryChangeSet::new();
        s.record(del("/f", "3"));
        s.record(del("/f", "9"));
        assert_eq!(s.changes(), [del("/f", "3")]);

        let mut s = MemoryChangeSet::new();
        s.record(put("/a", "1", Expected::Absent));
        s.record(put("/f", "1", Expected::Absent));
        s.record(del("/f", "9"));
        s.record(put("/g", "1", Expected::Absent));
        s.record(put("/f", "2", Expected::Absent));
        let paths: Vec<_> = s.changes().iter().map(|c| c.path().as_str().to_string()).collect();
        assert_eq!(paths, ["/a", "/g", "/f"]);
    }

    #[test]
    fn change_set_keeps_first_record_order_and_supports_lookup() {
        let mut s = MemoryChangeSet::new();
        s.record(put("/b", "1", Expected::Absent));
        s.record(put("/a", "1", Expected::Absent));
        s.record(put("/b", "2", Expected::Absent));
        let paths: Vec<_> = s.changes().iter().map(|c| c.path().as_str().to_string()).collect();
        assert_eq!(paths, ["/b", "/a"]);
        assert_eq!(s.len(), 2);
        assert!(s.get(&p("/a")).is_some());
        s.remove(&p("/b"));
        assert!(s.get(&p("/b")).is_none());
        assert_eq!(s.len(), 1);
    }
}
