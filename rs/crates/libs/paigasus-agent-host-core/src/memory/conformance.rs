// SPDX-License-Identifier: Apache-2.0

//! The conformance suite of `MemoryStore`. Every adapter must pass `run_all`. Each check makes
//! fresh scopes with unique UUIDs, so one store serves all checks. A failed check panics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use super::{Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use crate::ids::{AgentId, PrincipalId};
use crate::ports::{MemoryApplyError, MemoryStore, MemoryUsage};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A UUID that is unique in this process and, with the clock, across runs.
fn unique_uuid() -> Uuid {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = u128::from(NEXT.fetch_add(1, Ordering::Relaxed));
    Uuid::from_u128((nanos << 32) ^ n ^ (u128::from(std::process::id()) << 96))
}

/// Build a scope from three UUIDs.
#[must_use]
pub fn scope(org: Uuid, agent: Uuid, principal: Uuid) -> MemoryScope {
    MemoryScope::new(AgentId::new(org, agent), PrincipalId::from_uuid(principal))
}

/// A scope with unique UUIDs.
#[must_use]
pub fn fresh_scope() -> MemoryScope {
    scope(unique_uuid(), unique_uuid(), unique_uuid())
}

fn p(raw: &str) -> MemoryPath {
    MemoryPath::parse(raw).expect("a valid test path")
}

fn dir(raw: &str) -> MemoryDir {
    MemoryDir::parse(raw).expect("a valid test dir")
}

fn caps() -> MemoryCaps {
    MemoryCaps::default()
}

fn set(scope: &MemoryScope, changes: Vec<MemoryChange>) -> MemoryCommitSet {
    let mut cs = MemoryChangeSet::new();
    for c in changes {
        cs.record(c);
    }
    MemoryCommitSet { scope: scope.clone(), changes: cs }
}

fn put_new(path: &str, content: &str) -> MemoryChange {
    MemoryChange::Put {
        path: p(path),
        content: content.to_string(),
        expected: Expected::Absent,
    }
}

fn put_over(path: &str, content: &str, v: &MemoryVersion) -> MemoryChange {
    MemoryChange::Put {
        path: p(path),
        content: content.to_string(),
        expected: Expected::Version(v.clone()),
    }
}

fn del(path: &str, v: &MemoryVersion) -> MemoryChange {
    MemoryChange::Delete { path: p(path), expected: v.clone() }
}

async fn write(store: &dyn MemoryStore, scope: &MemoryScope, files: &[(&str, &str)]) {
    let changes = files.iter().map(|(path, content)| put_new(path, content)).collect();
    store.apply(&set(scope, changes), &caps()).await.expect("the setup write succeeds");
}

async fn version_of(store: &dyn MemoryStore, scope: &MemoryScope, path: &str) -> MemoryVersion {
    store.get(scope, &p(path)).await.expect("get works").expect("the file exists").version
}

async fn listed(store: &dyn MemoryStore, scope: &MemoryScope, d: &str) -> Vec<String> {
    store.list(scope, &dir(d)).await.expect("list works").into_iter().map(|e| e.path.as_str().to_string()).collect()
}

fn assert_conflict(result: Result<(), MemoryApplyError>, path: &str) {
    match result {
        Err(MemoryApplyError::Conflict { path: got }) => assert_eq!(got.as_str(), path),
        other => panic!("expected a conflict at {path}, got {other:?}"),
    }
}

/// Run every check.
pub async fn run_all(store: &dyn MemoryStore) {
    get_missing_is_none(store).await;
    put_absent_then_get(store).await;
    list_is_byte_ordered(store).await;
    list_is_recursive_and_scoped_to_dir(store).await;
    list_like_wildcards_are_literal(store).await;
    list_root_returns_every_file(store).await;
    absent_conflicts_when_present(store).await;
    stale_version_conflicts(store).await;
    delete_with_version(store).await;
    aba_never_matches(store).await;
    apply_is_all_or_nothing(store).await;
    apply_refuses_cap_breach(store).await;
    apply_allows_shrinking_an_over_cap_store(store).await;
    cross_scope_get_and_list(store).await;
    cross_scope_apply_cannot_touch_other(store).await;
    cross_scope_usage(store).await;
    same_uuids_two_orgs_are_isolated(store).await;
    delete_scope_leaves_others(store).await;
}

/// A missing file is `None`.
pub async fn get_missing_is_none(store: &dyn MemoryStore) {
    let s = fresh_scope();
    assert!(store.get(&s, &p("/nothing.md")).await.unwrap().is_none());
}

/// A put with `Absent` creates a file that `get` returns with a version.
pub async fn put_absent_then_get(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/a.md", "hello")]).await;
    let file = store.get(&s, &p("/a.md")).await.unwrap().expect("the file exists");
    assert_eq!(file.content, "hello");
    assert!(!file.version.as_str().is_empty());
}

/// `list` returns paths in byte order.
pub async fn list_is_byte_ordered(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/ab.md", "1"), ("/a-c.md", "2"), ("/a/x.md", "3")]).await;
    assert_eq!(listed(store, &s, "/").await, ["/a-c.md", "/a/x.md", "/ab.md"]);
}

/// `list` is recursive and matches segments, not string prefixes.
pub async fn list_is_recursive_and_scoped_to_dir(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/notes/a", "1"), ("/notes/x/b", "2"), ("/notes-old/x.md", "3")]).await;
    assert_eq!(listed(store, &s, "/notes").await, ["/notes/a", "/notes/x/b"]);
}

/// SQL `LIKE` wildcards in a directory name are literal.
pub async fn list_like_wildcards_are_literal(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/a_b/f", "1"), ("/axb/f", "2")]).await;
    assert_eq!(listed(store, &s, "/a_b").await, ["/a_b/f"]);
}

/// `list` of the root returns every file.
pub async fn list_root_returns_every_file(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/x", "1"), ("/d/y", "22"), ("/d/e/z", "333")]).await;
    let entries = store.list(&s, &MemoryDir::root()).await.unwrap();
    assert_eq!(entries.len(), 3);
    let sizes: Vec<u64> = entries.iter().map(|e| e.size_bytes).collect();
    assert_eq!(sizes, [3, 2, 1]);
}

/// `Absent` conflicts when the file exists.
pub async fn absent_conflicts_when_present(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/a", "1")]).await;
    assert_conflict(store.apply(&set(&s, vec![put_new("/a", "2")]), &caps()).await, "/a");
    assert_eq!(store.get(&s, &p("/a")).await.unwrap().unwrap().content, "1");
}

/// A stale version always conflicts, for put and for delete.
pub async fn stale_version_conflicts(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/a", "1")]).await;
    let v1 = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![put_over("/a", "2", &v1)]), &caps()).await.unwrap();
    assert_conflict(store.apply(&set(&s, vec![put_over("/a", "3", &v1)]), &caps()).await, "/a");
    assert_conflict(store.apply(&set(&s, vec![del("/a", &v1)]), &caps()).await, "/a");
    assert_eq!(store.get(&s, &p("/a")).await.unwrap().unwrap().content, "2");
}

/// A delete with the current version removes the file.
pub async fn delete_with_version(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/a", "1")]).await;
    let v = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![del("/a", &v)]), &caps()).await.unwrap();
    assert!(store.get(&s, &p("/a")).await.unwrap().is_none());
    assert_conflict(store.apply(&set(&s, vec![del("/a", &v)]), &caps()).await, "/a");
}

/// A version never matches a file that was deleted and created again.
pub async fn aba_never_matches(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/a", "same")]).await;
    let v = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![del("/a", &v)]), &caps()).await.unwrap();
    write(store, &s, &[("/a", "same")]).await;
    assert_conflict(store.apply(&set(&s, vec![put_over("/a", "new", &v)]), &caps()).await, "/a");
}

/// A change set with one conflict writes nothing.
pub async fn apply_is_all_or_nothing(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/c", "old")]).await;
    let changes = vec![put_new("/a", "1"), put_new("/b", "2"), put_new("/c", "3")];
    assert_conflict(store.apply(&set(&s, changes), &caps()).await, "/c");
    assert!(store.get(&s, &p("/a")).await.unwrap().is_none());
    assert!(store.get(&s, &p("/b")).await.unwrap().is_none());
    assert_eq!(store.get(&s, &p("/c")).await.unwrap().unwrap().content, "old");
}

/// A cap breach refuses the whole set.
pub async fn apply_refuses_cap_breach(store: &dyn MemoryStore) {
    let s = fresh_scope();
    let small = MemoryCaps {
        max_store_files: 2,
        ..MemoryCaps::default()
    };
    let result = store.apply(&set(&s, vec![put_new("/a", "1"), put_new("/b", "2"), put_new("/c", "3")]), &small).await;
    assert!(matches!(result, Err(MemoryApplyError::CapExceeded(_))), "{result:?}");
    assert_eq!(store.usage(&s).await.unwrap(), MemoryUsage::default());
    let file_cap = MemoryCaps {
        max_file_bytes: 3,
        ..MemoryCaps::default()
    };
    let result = store.apply(&set(&s, vec![put_new("/big", "1234")]), &file_cap).await;
    assert!(matches!(result, Err(MemoryApplyError::CapExceeded(_))), "{result:?}");
    assert!(store.get(&s, &p("/big")).await.unwrap().is_none());
}

/// A change that makes an over-cap store smaller is allowed.
pub async fn apply_allows_shrinking_an_over_cap_store(store: &dyn MemoryStore) {
    let s = fresh_scope();
    let five = MemoryCaps {
        max_store_files: 5,
        ..MemoryCaps::default()
    };
    store.apply(&set(&s, vec![put_new("/a", "1"), put_new("/b", "2"), put_new("/c", "3")]), &five).await.unwrap();
    let one = MemoryCaps {
        max_store_files: 1,
        ..MemoryCaps::default()
    };
    let v = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![del("/a", &v)]), &one).await.unwrap();
    assert_eq!(store.usage(&s).await.unwrap().files, 2);
}

/// Two scopes with the same path hold their own content.
pub async fn cross_scope_get_and_list(store: &dyn MemoryStore) {
    let (a, b) = (fresh_scope(), fresh_scope());
    write(store, &a, &[("/f.md", "from a")]).await;
    write(store, &b, &[("/f.md", "from b"), ("/g.md", "only b")]).await;
    assert_eq!(store.get(&a, &p("/f.md")).await.unwrap().unwrap().content, "from a");
    assert_eq!(store.get(&b, &p("/f.md")).await.unwrap().unwrap().content, "from b");
    assert_eq!(listed(store, &a, "/").await, ["/f.md"]);
    assert_eq!(listed(store, &b, "/").await, ["/f.md", "/g.md"]);
}

/// A version of scope B does not let scope A change B's file.
pub async fn cross_scope_apply_cannot_touch_other(store: &dyn MemoryStore) {
    let (a, b) = (fresh_scope(), fresh_scope());
    write(store, &b, &[("/f.md", "b")]).await;
    let vb = version_of(store, &b, "/f.md").await;
    assert_conflict(store.apply(&set(&a, vec![put_over("/f.md", "x", &vb)]), &caps()).await, "/f.md");
    assert_conflict(store.apply(&set(&a, vec![del("/f.md", &vb)]), &caps()).await, "/f.md");
    assert_eq!(store.get(&b, &p("/f.md")).await.unwrap().unwrap().content, "b");
    assert!(store.get(&a, &p("/f.md")).await.unwrap().is_none());
}

/// Usage counts only the files of the scope.
pub async fn cross_scope_usage(store: &dyn MemoryStore) {
    let (a, b) = (fresh_scope(), fresh_scope());
    write(store, &a, &[("/x", "12345")]).await;
    write(store, &b, &[("/x", "1"), ("/y", "22")]).await;
    assert_eq!(store.usage(&a).await.unwrap(), MemoryUsage { files: 1, bytes: 5 });
    assert_eq!(store.usage(&b).await.unwrap(), MemoryUsage { files: 2, bytes: 3 });
}

/// The same agent and principal UUIDs in two orgs are two scopes.
pub async fn same_uuids_two_orgs_are_isolated(store: &dyn MemoryStore) {
    let (agent, principal) = (unique_uuid(), unique_uuid());
    let a = scope(unique_uuid(), agent, principal);
    let b = scope(unique_uuid(), agent, principal);
    write(store, &a, &[("/f", "org a")]).await;
    assert!(store.get(&b, &p("/f")).await.unwrap().is_none());
    assert!(listed(store, &b, "/").await.is_empty());
    assert_eq!(store.usage(&b).await.unwrap(), MemoryUsage::default());
    write(store, &b, &[("/f", "org b")]).await;
    assert_eq!(store.get(&a, &p("/f")).await.unwrap().unwrap().content, "org a");
}

/// `delete_scope` removes one scope and leaves the others.
pub async fn delete_scope_leaves_others(store: &dyn MemoryStore) {
    let (a, b) = (fresh_scope(), fresh_scope());
    write(store, &a, &[("/f", "a"), ("/d/g", "a")]).await;
    write(store, &b, &[("/f", "b")]).await;
    store.delete_scope(&a).await.unwrap();
    assert!(store.get(&a, &p("/f")).await.unwrap().is_none());
    assert_eq!(store.usage(&a).await.unwrap(), MemoryUsage::default());
    assert_eq!(store.get(&b, &p("/f")).await.unwrap().unwrap().content, "b");
}
