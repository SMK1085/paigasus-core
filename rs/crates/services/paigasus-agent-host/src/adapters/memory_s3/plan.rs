// SPDX-License-Identifier: Apache-2.0

//! The pure pre-check and the result rule of `apply` (spec §6.5 and §6.7).
//!
//! `precheck` runs on a snapshot of the scope and does no I/O. `result_of` turns the outcome
//! of each change into the one result of `apply`.

use std::collections::BTreeMap;

use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryCommitSet, MemoryPath, MemoryVersion};
use paigasus_agent_host_core::ports::{MemoryApplyError, MemoryUsage};

/// The text of the refusal of content with a NUL character. It is the text of the fake store.
const NUL_TEXT: &str = "the content contains a NUL character";
/// The text when no change failed, but a write did not start: the deadline ended the apply.
fn unfinished_text() -> String {
    format!("the change set did not finish ({})", super::errors::TIMEOUT_CLASS)
}

/// The files of a scope: the path, the content size in bytes and the version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub files: BTreeMap<MemoryPath, (u64, MemoryVersion)>,
}

impl Snapshot {
    /// The number of files and the total content bytes.
    pub fn usage(&self) -> MemoryUsage {
        MemoryUsage {
            files: self.files.len() as u64,
            bytes: self.files.values().map(|(size, _)| *size).sum(),
        }
    }
}

/// The writes of a change set, as indexes into `set.changes.changes()` in set order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WritePlan {
    pub puts: Vec<usize>,
    pub deletes: Vec<usize>,
}

/// Check the set against the snapshot. The order is the order of the fake store: the guards,
/// then each change (NUL, version, file cap), then the store caps.
pub fn precheck(set: &MemoryCommitSet, snapshot: &Snapshot, caps: &MemoryCaps) -> Result<WritePlan, MemoryApplyError> {
    for guard in &set.guards {
        for path in snapshot.files.keys() {
            if guard.covers(path) && set.changes.get(path).is_none() {
                return Err(MemoryApplyError::Conflict { path: path.clone() });
            }
        }
    }
    let before = snapshot.usage();
    let mut after = before;
    let mut plan = WritePlan::default();
    for (i, change) in set.changes.changes().iter().enumerate() {
        let current = snapshot.files.get(change.path());
        match change {
            MemoryChange::Put { path, content, expected } => {
                if content.contains('\0') {
                    return Err(MemoryApplyError::Store(NUL_TEXT.to_owned()));
                }
                let matches = match (expected, current) {
                    (Expected::Absent, None) => true,
                    (Expected::Version(v), Some((_, c))) => v == c,
                    _ => false,
                };
                if !matches {
                    return Err(MemoryApplyError::Conflict { path: path.clone() });
                }
                let size = content.len() as u64;
                caps.check_file(path, size).map_err(MemoryApplyError::CapExceeded)?;
                match current {
                    Some((old, _)) => after.bytes = after.bytes - old + size,
                    None => {
                        after.files += 1;
                        after.bytes += size;
                    }
                }
                plan.puts.push(i);
            }
            MemoryChange::Delete { path, expected } => {
                match current {
                    Some((old, v)) if v == expected => {
                        after.files -= 1;
                        after.bytes -= old;
                    }
                    _ => return Err(MemoryApplyError::Conflict { path: path.clone() }),
                }
                plan.deletes.push(i);
            }
        }
    }
    caps.check(before, after).map_err(MemoryApplyError::CapExceeded)?;
    Ok(plan)
}

/// What happened to one change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The store holds the change.
    Confirmed,
    /// The store does not hold the change.
    Failed(MemoryApplyError),
    /// It is not known whether the store holds the change.
    Unknown(MemoryApplyError),
    /// The write did not start.
    NotStarted,
}

/// The result of `apply`. `outcomes[i]` belongs to change `i`.
pub fn result_of(set: &MemoryCommitSet, outcomes: &[Outcome]) -> Result<(), MemoryApplyError> {
    let confirmed = outcomes.iter().filter(|o| matches!(o, Outcome::Confirmed)).count();
    if confirmed == outcomes.len() {
        return Ok(());
    }
    let first = outcomes.iter().enumerate().find_map(|(i, o)| match o {
        Outcome::Failed(e) | Outcome::Unknown(e) => Some((i, e)),
        _ => None,
    });
    let any_unknown = outcomes.iter().any(|o| matches!(o, Outcome::Unknown(_)));
    match first {
        // No failure and no unknown outcome, but a write did not start. The store holds the
        // confirmed writes, so the result is partial. With none confirmed, it holds nothing.
        None if confirmed == 0 => Err(MemoryApplyError::Store(unfinished_text())),
        None => {
            let i = outcomes.iter().position(|o| !matches!(o, Outcome::Confirmed)).unwrap_or(0);
            Err(MemoryApplyError::Partial {
                applied: confirmed,
                failed_at: set.changes.changes()[i].path().clone(),
                cause: Box::new(MemoryApplyError::Store(unfinished_text())),
            })
        }
        Some((_, cause)) if confirmed == 0 && !any_unknown => Err(cause.clone()),
        Some((i, cause)) => Err(MemoryApplyError::Partial {
            applied: confirmed,
            failed_at: set.changes.changes()[i].path().clone(),
            cause: Box::new(cause.clone()),
        }),
    }
}

/// What the check `GET` after a failed write found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// The object holds the write ID of this write.
    Mine,
    /// The object holds the write ID of another write.
    Other,
    /// The object does not exist.
    Absent,
    /// The object is not an envelope of this store.
    Foreign,
    /// The check `GET` failed.
    ReadFailed,
}

/// The outcome of a `Put` whose request failed (spec §6.5). `uncertain` is true when the error
/// class is `timeout` or `transport`: the server can still commit the write after the client
/// gave up, so an absent object does not prove that the write failed. `update` is true when the
/// `Put` expects a version (an `If-Match` `PUT`). Such a `PUT` can still land on the object that
/// the check `GET` found, so another writer's object does not prove that the update failed
/// (ruling F3). A create-only `PUT` (`If-None-Match`) cannot land on an object that exists.
pub fn put_outcome(refused: bool, uncertain: bool, update: bool, cause: MemoryApplyError, seen: Seen, path: &MemoryPath) -> Outcome {
    match seen {
        Seen::Mine => Outcome::Confirmed,
        Seen::Absent if uncertain => Outcome::Unknown(cause),
        Seen::Other if uncertain && update => Outcome::Unknown(cause),
        Seen::Other | Seen::Absent if refused => Outcome::Failed(MemoryApplyError::Conflict { path: path.clone() }),
        Seen::Other | Seen::Absent => Outcome::Failed(cause),
        Seen::Foreign | Seen::ReadFailed => Outcome::Unknown(cause),
    }
}

/// What the check `HEAD` after a failed `DELETE` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenAfterDelete {
    /// The object does not exist.
    Absent,
    /// The object has the version that the change expected.
    Expected,
    /// The object has another version.
    Other,
    /// The check `HEAD` failed.
    ReadFailed,
}

/// The outcome of a `Delete` whose request failed. `uncertain` is true when the error class is
/// `timeout` or `transport`: the server can still run the `DELETE` after the client gave up.
pub fn delete_outcome(cause: MemoryApplyError, uncertain: bool, seen: SeenAfterDelete) -> Outcome {
    match seen {
        SeenAfterDelete::Absent => Outcome::Confirmed,
        SeenAfterDelete::Expected if uncertain => Outcome::Unknown(cause),
        SeenAfterDelete::Expected => Outcome::Failed(cause),
        SeenAfterDelete::Other | SeenAfterDelete::ReadFailed => Outcome::Unknown(cause),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_agent_host_core::ids::{AgentId, PrincipalId};
    use paigasus_agent_host_core::memory::{CapError, MemoryChangeSet, MemoryDir, MemoryScope};
    use uuid::Uuid;

    fn p(s: &str) -> MemoryPath {
        MemoryPath::parse(s).unwrap()
    }

    fn v(s: &str) -> MemoryVersion {
        MemoryVersion::new(s)
    }

    fn scope() -> MemoryScope {
        MemoryScope::new(AgentId::new(Uuid::from_u128(1), Uuid::from_u128(2)), PrincipalId::from_uuid(Uuid::from_u128(3)))
    }

    fn put(path: &str, content: &str, expected: Expected) -> MemoryChange {
        MemoryChange::Put {
            path: p(path),
            content: content.to_owned(),
            expected,
        }
    }

    fn del(path: &str, version: &str) -> MemoryChange {
        MemoryChange::Delete { path: p(path), expected: v(version) }
    }

    fn set_of(changes: Vec<MemoryChange>) -> MemoryCommitSet {
        let mut cs = MemoryChangeSet::new();
        for c in changes {
            cs.record(c);
        }
        MemoryCommitSet::new(scope(), cs)
    }

    fn snap(files: &[(&str, u64, &str)]) -> Snapshot {
        Snapshot {
            files: files.iter().map(|(path, size, ver)| (p(path), (*size, v(ver)))).collect(),
        }
    }

    fn conflict(path: &str) -> MemoryApplyError {
        MemoryApplyError::Conflict { path: p(path) }
    }

    fn store(text: &str) -> MemoryApplyError {
        MemoryApplyError::Store(text.to_owned())
    }

    #[test]
    fn an_empty_set_needs_no_write() {
        let plan = precheck(&set_of(vec![]), &Snapshot::default(), &MemoryCaps::default()).unwrap();
        assert_eq!(plan, WritePlan::default());
    }

    #[test]
    fn the_version_rows() {
        let caps = MemoryCaps::default();
        let s = snap(&[("/a", 1, "v")]);
        let refused = |change: MemoryChange, at: &str| {
            assert_eq!(precheck(&set_of(vec![change]), &s, &caps), Err(conflict(at)));
        };
        refused(put("/a", "x", Expected::Absent), "/a");
        refused(put("/b", "x", Expected::Version(v("v"))), "/b");
        refused(put("/a", "x", Expected::Version(v("w"))), "/a");
        refused(del("/b", "v"), "/b");
        refused(del("/a", "w"), "/a");
        assert!(precheck(&set_of(vec![put("/b", "x", Expected::Absent)]), &s, &caps).is_ok());
        assert!(precheck(&set_of(vec![put("/a", "x", Expected::Version(v("v")))]), &s, &caps).is_ok());
        assert!(precheck(&set_of(vec![del("/a", "v")]), &s, &caps).is_ok());
    }

    #[test]
    fn a_guard_conflict_wins() {
        let caps = MemoryCaps {
            max_file_bytes: 2,
            ..MemoryCaps::default()
        };
        let mut set = set_of(vec![put("/x", "too long", Expected::Absent)]);
        set.guards.insert(MemoryDir::parse("/d").unwrap());
        let r = precheck(&set, &snap(&[("/d/a", 1, "v")]), &caps);
        assert_eq!(r, Err(conflict("/d/a")));
    }

    #[test]
    fn a_guard_accepts_a_path_that_a_change_names() {
        let mut set = set_of(vec![del("/d/a", "v")]);
        set.guards.insert(MemoryDir::parse("/d").unwrap());
        assert!(precheck(&set, &snap(&[("/d/a", 1, "v")]), &MemoryCaps::default()).is_ok());
    }

    #[test]
    fn guards_report_the_first_outsider_in_byte_order() {
        let mut set = set_of(vec![]);
        set.guards.insert(MemoryDir::parse("/b").unwrap());
        set.guards.insert(MemoryDir::parse("/a").unwrap());
        let s = snap(&[("/b/1", 1, "v"), ("/a/2", 1, "v"), ("/a/1", 1, "v")]);
        assert_eq!(precheck(&set, &s, &MemoryCaps::default()), Err(conflict("/a/1")));
    }

    #[test]
    fn the_first_change_reports_first() {
        let set = set_of(vec![put("/a", "x", Expected::Version(v("old"))), put("/b", "a\0b", Expected::Absent)]);
        let r = precheck(&set, &snap(&[("/a", 1, "new")]), &MemoryCaps::default());
        assert_eq!(r, Err(conflict("/a")));
    }

    #[test]
    fn nul_comes_before_the_version_check() {
        let set = set_of(vec![put("/a", "a\0b", Expected::Version(v("old")))]);
        let r = precheck(&set, &snap(&[("/a", 1, "new")]), &MemoryCaps::default());
        assert_eq!(r, Err(store("the content contains a NUL character")));
    }

    #[test]
    fn the_version_check_comes_before_the_file_cap() {
        let caps = MemoryCaps {
            max_file_bytes: 2,
            ..MemoryCaps::default()
        };
        let set = set_of(vec![put("/a", "too long", Expected::Version(v("old")))]);
        assert_eq!(precheck(&set, &snap(&[("/a", 1, "new")]), &caps), Err(conflict("/a")));
    }

    #[test]
    fn the_file_cap_refuses_a_put() {
        let caps = MemoryCaps {
            max_file_bytes: 2,
            ..MemoryCaps::default()
        };
        let r = precheck(&set_of(vec![put("/a", "abc", Expected::Absent)]), &Snapshot::default(), &caps);
        assert!(matches!(r, Err(MemoryApplyError::CapExceeded(CapError::File { .. }))), "{r:?}");
    }

    #[test]
    fn the_store_caps_run_last() {
        let caps = MemoryCaps {
            max_store_files: 1,
            ..MemoryCaps::default()
        };
        let r = precheck(&set_of(vec![put("/b", "x", Expected::Absent)]), &snap(&[("/a", 1, "v")]), &caps);
        assert_eq!(r, Err(MemoryApplyError::CapExceeded(CapError::StoreFiles { after: 2, limit: 1, now: 1 })));
    }

    #[test]
    fn the_store_byte_cap_refuses_a_growing_set() {
        // The set grows the store from 8 to 16 bytes. The limit is 15 (ruling P2).
        let caps = MemoryCaps {
            max_store_bytes: 15,
            ..MemoryCaps::default()
        };
        let s = snap(&[("/a", 4, "v"), ("/b", 4, "v")]);
        let r = precheck(&set_of(vec![put("/a", "123456789012", Expected::Version(v("v")))]), &s, &caps);
        assert_eq!(r, Err(MemoryApplyError::CapExceeded(CapError::StoreBytes { after: 16, limit: 15, now: 8 })));
    }

    #[test]
    fn after_counts_overwrite_delete_and_create() {
        // The set grows the store from 3 to 7 bytes. A set that shrinks the store passes every
        // cap (ruling P2), so the brief row with 15 bytes before could not refuse.
        let s = snap(&[("/a", 2, "v"), ("/b", 1, "v")]);
        let set = set_of(vec![put("/a", "abc", Expected::Version(v("v"))), del("/b", "v"), put("/c", "abcd", Expected::Absent)]);
        let ok = MemoryCaps {
            max_store_bytes: 7,
            max_store_files: 2,
            ..MemoryCaps::default()
        };
        assert!(precheck(&set, &s, &ok).is_ok());
        let tight = MemoryCaps {
            max_store_bytes: 6,
            max_store_files: 2,
            ..MemoryCaps::default()
        };
        let r = precheck(&set, &s, &tight);
        assert_eq!(r, Err(MemoryApplyError::CapExceeded(CapError::StoreBytes { after: 7, limit: 6, now: 3 })));
    }

    #[test]
    fn puts_come_before_deletes_in_the_plan() {
        let set = set_of(vec![del("/old", "v"), put("/new", "x", Expected::Absent)]);
        let plan = precheck(&set, &snap(&[("/old", 1, "v")]), &MemoryCaps::default()).unwrap();
        assert_eq!(plan.puts, vec![1]);
        assert_eq!(plan.deletes, vec![0]);
    }

    #[test]
    fn the_result_rules() {
        use Outcome::*;
        let set = set_of(vec![put("/a", "x", Expected::Absent), put("/b", "x", Expected::Absent), put("/c", "x", Expected::Absent)]);
        let partial = |applied, at: &str, cause| {
            Err(MemoryApplyError::Partial {
                applied,
                failed_at: p(at),
                cause: Box::new(cause),
            })
        };
        assert_eq!(result_of(&set, &[Confirmed, Confirmed, Confirmed]), Ok(()));
        let timeout = || store("the change set did not finish (timeout)");
        assert_eq!(result_of(&set, &[Confirmed, NotStarted, NotStarted]), partial(1, "/b", timeout()));
        assert_eq!(result_of(&set, &[Confirmed, Confirmed, NotStarted]), partial(2, "/c", timeout()));
        assert_eq!(result_of(&set, &[NotStarted, NotStarted, NotStarted]), Err(timeout()));
        assert_eq!(result_of(&set, &[Failed(conflict("/a")), NotStarted, NotStarted]), Err(conflict("/a")));
        assert_eq!(result_of(&set, &[Confirmed, Failed(store("s")), NotStarted]), partial(1, "/b", store("s")));
        assert_eq!(result_of(&set, &[Unknown(store("t")), NotStarted, NotStarted]), partial(0, "/a", store("t")));
        assert_eq!(result_of(&set, &[Confirmed, Unknown(store("x")), Failed(store("y"))]), partial(1, "/b", store("x")));
        assert_eq!(result_of(&set, &[Failed(store("y")), Unknown(store("x")), NotStarted]), partial(0, "/a", store("y")));
    }

    #[test]
    fn the_put_outcome_rows() {
        let path = p("/a");
        let cause = store("c");
        // A definite error gives the same rows for a create and an update.
        for update in [false, true] {
            let o = |refused, seen| put_outcome(refused, false, update, cause.clone(), seen, &path);
            assert_eq!(o(true, Seen::Mine), Outcome::Confirmed);
            assert_eq!(o(false, Seen::Mine), Outcome::Confirmed);
            for seen in [Seen::Other, Seen::Absent] {
                assert_eq!(o(true, seen), Outcome::Failed(conflict("/a")));
                assert_eq!(o(false, seen), Outcome::Failed(cause.clone()));
            }
            for seen in [Seen::Foreign, Seen::ReadFailed] {
                assert_eq!(o(true, seen), Outcome::Unknown(cause.clone()));
                assert_eq!(o(false, seen), Outcome::Unknown(cause.clone()));
            }
        }
    }

    #[test]
    fn an_uncertain_put_that_is_absent_is_unknown() {
        // Ruling F1: after a timeout or a transport error the server can still commit.
        let path = p("/a");
        let cause = store("c");
        for update in [false, true] {
            assert_eq!(put_outcome(false, true, update, cause.clone(), Seen::Absent, &path), Outcome::Unknown(cause.clone()));
            // A write ID of this write is still proof.
            assert_eq!(put_outcome(false, true, update, cause.clone(), Seen::Mine, &path), Outcome::Confirmed);
        }
        // A create-only PUT cannot land on another writer's object, so that object is a failure.
        assert_eq!(put_outcome(false, true, false, cause.clone(), Seen::Other, &path), Outcome::Failed(cause));
    }

    #[test]
    fn an_uncertain_update_that_finds_another_object_is_unknown() {
        // Ruling F3: an If-Match PUT can still land on the object that the check GET found.
        let path = p("/a");
        let cause = store("c");
        assert_eq!(put_outcome(false, true, true, cause.clone(), Seen::Other, &path), Outcome::Unknown(cause));
    }

    #[test]
    fn the_delete_outcome_rows() {
        let cause = store("c");
        assert_eq!(delete_outcome(cause.clone(), false, SeenAfterDelete::Absent), Outcome::Confirmed);
        assert_eq!(delete_outcome(cause.clone(), false, SeenAfterDelete::Expected), Outcome::Failed(cause.clone()));
        assert_eq!(delete_outcome(cause.clone(), false, SeenAfterDelete::Other), Outcome::Unknown(cause.clone()));
        assert_eq!(delete_outcome(cause.clone(), false, SeenAfterDelete::ReadFailed), Outcome::Unknown(cause));
    }

    #[test]
    fn an_uncertain_delete_with_the_expected_etag_is_unknown() {
        // Ruling F1: after a timeout or a transport error the server can still run the DELETE.
        let cause = store("c");
        assert_eq!(delete_outcome(cause.clone(), true, SeenAfterDelete::Expected), Outcome::Unknown(cause.clone()));
        assert_eq!(delete_outcome(cause.clone(), true, SeenAfterDelete::Absent), Outcome::Confirmed);
    }
}
