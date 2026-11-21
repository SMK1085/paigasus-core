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
    MemoryCommitSet::new(scope.clone(), cs)
}

fn guarded(scope: &MemoryScope, changes: Vec<MemoryChange>, guards: &[&str]) -> MemoryCommitSet {
    let mut cs = set(scope, changes);
    cs.guards = guards.iter().map(|g| dir(g)).collect();
    cs
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
    delete_scope_of_a_never_written_scope_succeeds(store).await;
    same_org_same_agent_two_principals_are_isolated(store).await;
    same_org_same_principal_two_agents_are_isolated(store).await;
    usage_tracks_overwrite_delete_and_recreate(store).await;
    store_bytes_cap_refuses_growth_and_allows_shrinking(store).await;
    sizes_are_utf8_bytes_and_limits_are_inclusive(store).await;
    dir_rename_conflicts_on_a_file_added_under_the_source(store).await;
    dir_delete_conflicts_on_a_file_added_under_the_dir(store).await;
    dir_rename_conflicts_on_a_file_added_under_the_destination(store).await;
    a_guard_allows_the_paths_that_the_set_names(store).await;
    a_guard_matches_segments_and_literal_wildcards(store).await;
    a_guard_only_set_is_checked(store).await;
    a_guard_conflict_wins_over_a_cap_refusal(store).await;
    a_guard_ignores_files_of_another_scope(store).await;
    guards_report_the_first_outsider_in_byte_order(store).await;
}

/// Check that scope `a` and scope `b` do not see or change each other.
async fn assert_isolated(store: &dyn MemoryStore, a: &MemoryScope, b: &MemoryScope) {
    write(store, a, &[("/f.md", "from a")]).await;
    write(store, b, &[("/f.md", "from b"), ("/g.md", "only b")]).await;
    assert_eq!(store.get(a, &p("/f.md")).await.unwrap().unwrap().content, "from a");
    assert_eq!(store.get(b, &p("/f.md")).await.unwrap().unwrap().content, "from b");
    assert_eq!(listed(store, a, "/").await, ["/f.md"]);
    assert_eq!(listed(store, b, "/").await, ["/f.md", "/g.md"]);
    assert_eq!(store.usage(a).await.unwrap(), MemoryUsage { files: 1, bytes: 6 });
    assert_eq!(store.usage(b).await.unwrap(), MemoryUsage { files: 2, bytes: 12 });
    // The version of a file of b must not match in a, where the path is absent.
    let vb = version_of(store, b, "/g.md").await;
    assert_conflict(store.apply(&set(a, vec![put_over("/g.md", "x", &vb)]), &caps()).await, "/g.md");
    assert_conflict(store.apply(&set(a, vec![del("/g.md", &vb)]), &caps()).await, "/g.md");
    assert!(store.get(a, &p("/g.md")).await.unwrap().is_none());
    assert_eq!(store.get(b, &p("/g.md")).await.unwrap().unwrap().content, "only b");
    store.delete_scope(a).await.unwrap();
    assert!(store.get(a, &p("/f.md")).await.unwrap().is_none());
    assert_eq!(store.usage(a).await.unwrap(), MemoryUsage::default());
    assert_eq!(store.usage(b).await.unwrap(), MemoryUsage { files: 2, bytes: 12 });
    assert_eq!(listed(store, b, "/").await, ["/f.md", "/g.md"]);
}

/// Same org and same agent, different principals: two scopes.
pub async fn same_org_same_agent_two_principals_are_isolated(store: &dyn MemoryStore) {
    let (org, agent) = (unique_uuid(), unique_uuid());
    let a = scope(org, agent, unique_uuid());
    let b = scope(org, agent, unique_uuid());
    assert_isolated(store, &a, &b).await;
}

/// Same org and same principal, different agents: two scopes.
pub async fn same_org_same_principal_two_agents_are_isolated(store: &dyn MemoryStore) {
    let (org, principal) = (unique_uuid(), unique_uuid());
    let a = scope(org, unique_uuid(), principal);
    let b = scope(org, unique_uuid(), principal);
    assert_isolated(store, &a, &b).await;
}

/// Usage follows an overwrite, a delete and a re-create.
pub async fn usage_tracks_overwrite_delete_and_recreate(store: &dyn MemoryStore) {
    let s = fresh_scope();
    let usage = |files, bytes| MemoryUsage { files, bytes };
    write(store, &s, &[("/a", "abc"), ("/b", "12")]).await;
    assert_eq!(store.usage(&s).await.unwrap(), usage(2, 5));
    let v = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![put_over("/a", "abcdef", &v)]), &caps()).await.unwrap();
    assert_eq!(store.usage(&s).await.unwrap(), usage(2, 8));
    let v = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![put_over("/a", "a", &v)]), &caps()).await.unwrap();
    assert_eq!(store.usage(&s).await.unwrap(), usage(2, 3));
    let v = version_of(store, &s, "/a").await;
    store.apply(&set(&s, vec![del("/a", &v)]), &caps()).await.unwrap();
    assert_eq!(store.usage(&s).await.unwrap(), usage(1, 2));
    write(store, &s, &[("/a", "xyz")]).await;
    assert_eq!(store.usage(&s).await.unwrap(), usage(2, 5));
}

/// The store-bytes cap refuses growth and allows shrinking, also for an over-cap store.
pub async fn store_bytes_cap_refuses_growth_and_allows_shrinking(store: &dyn MemoryStore) {
    let s = fresh_scope();
    let ten = MemoryCaps {
        max_store_bytes: 10,
        ..MemoryCaps::default()
    };
    // Exactly at the limit passes. One byte more is refused.
    store.apply(&set(&s, vec![put_new("/a", "12345678")]), &ten).await.unwrap();
    store.apply(&set(&s, vec![put_new("/b", "12")]), &ten).await.unwrap();
    let result = store.apply(&set(&s, vec![put_new("/c", "1")]), &ten).await;
    assert!(matches!(result, Err(MemoryApplyError::CapExceeded(_))), "{result:?}");
    assert!(store.get(&s, &p("/c")).await.unwrap().is_none());
    assert_eq!(store.usage(&s).await.unwrap().bytes, 10);

    // An over-cap store: write 14 bytes under a large cap, then use the small cap.
    let o = fresh_scope();
    write(store, &o, &[("/a", "1234567"), ("/b", "1234567")]).await;
    let va = version_of(store, &o, "/a").await;
    let grow = store.apply(&set(&o, vec![put_over("/a", "12345678", &va)]), &ten).await;
    assert!(matches!(grow, Err(MemoryApplyError::CapExceeded(_))), "{grow:?}");
    assert_eq!(store.usage(&o).await.unwrap().bytes, 14);
    store.apply(&set(&o, vec![put_over("/a", "12345", &va)]), &ten).await.unwrap();
    assert_eq!(store.usage(&o).await.unwrap().bytes, 12);
    let vb = version_of(store, &o, "/b").await;
    store.apply(&set(&o, vec![del("/b", &vb)]), &ten).await.unwrap();
    assert_eq!(store.usage(&o).await.unwrap(), MemoryUsage { files: 1, bytes: 5 });
}

/// Size is the UTF-8 byte length, and a limit is inclusive.
pub async fn sizes_are_utf8_bytes_and_limits_are_inclusive(store: &dyn MemoryStore) {
    let s = fresh_scope();
    // 2 + 3 = 5 bytes, 2 chars.
    write(store, &s, &[("/m", "é日")]).await;
    assert_eq!(store.usage(&s).await.unwrap(), MemoryUsage { files: 1, bytes: 5 });
    assert_eq!(store.list(&s, &MemoryDir::root()).await.unwrap()[0].size_bytes, 5);
    let five = MemoryCaps {
        max_file_bytes: 5,
        ..MemoryCaps::default()
    };
    // Exactly 5 bytes passes, also with multibyte characters.
    store.apply(&set(&s, vec![put_new("/exact", "12345"), put_new("/exact2", "é日")]), &five).await.unwrap();
    // Two chars, 6 bytes: refused.
    let result = store.apply(&set(&s, vec![put_new("/big", "日本")]), &five).await;
    assert!(matches!(result, Err(MemoryApplyError::CapExceeded(_))), "{result:?}");
    assert!(store.get(&s, &p("/big")).await.unwrap().is_none());
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

/// `delete_scope` of a scope that was never written succeeds and leaves the usage at zero.
pub async fn delete_scope_of_a_never_written_scope_succeeds(store: &dyn MemoryStore) {
    let a = fresh_scope();
    store.delete_scope(&a).await.unwrap();
    assert_eq!(store.usage(&a).await.unwrap(), MemoryUsage::default());
    assert!(listed(store, &a, "/").await.is_empty());
    // The scope stays usable after the erasure.
    write(store, &a, &[("/f", "x")]).await;
    assert_eq!(store.usage(&a).await.unwrap(), MemoryUsage { files: 1, bytes: 1 });
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

/// The listing and the content of the files in `paths`, to compare before and after a refusal.
async fn snapshot(store: &dyn MemoryStore, s: &MemoryScope, paths: &[&str]) -> (Vec<String>, Vec<Option<String>>) {
    let mut contents = Vec::new();
    for path in paths {
        contents.push(store.get(s, &p(path)).await.unwrap().map(|f| f.content));
    }
    (listed(store, s, "/").await, contents)
}

/// A rename set is refused when a file appears under the source directory after the read.
pub async fn dir_rename_conflicts_on_a_file_added_under_the_source(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/tasks/a.md", "a")]).await;
    let v = version_of(store, &s, "/tasks/a.md").await;
    let rename = guarded(&s, vec![put_new("/done/a.md", "a"), del("/tasks/a.md", &v)], &["/tasks", "/done"]);
    write(store, &s, &[("/tasks/b.md", "b")]).await;
    let paths = ["/tasks/a.md", "/tasks/b.md", "/done/a.md"];
    let before = snapshot(store, &s, &paths).await;
    assert_conflict(store.apply(&rename, &caps()).await, "/tasks/b.md");
    assert_eq!(snapshot(store, &s, &paths).await, before);
}

/// A delete set is refused when a file appears under the directory after the read.
pub async fn dir_delete_conflicts_on_a_file_added_under_the_dir(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/tasks/a.md", "a")]).await;
    let v = version_of(store, &s, "/tasks/a.md").await;
    let delete = guarded(&s, vec![del("/tasks/a.md", &v)], &["/tasks"]);
    write(store, &s, &[("/tasks/b.md", "b")]).await;
    let paths = ["/tasks/a.md", "/tasks/b.md"];
    let before = snapshot(store, &s, &paths).await;
    assert_conflict(store.apply(&delete, &caps()).await, "/tasks/b.md");
    assert_eq!(snapshot(store, &s, &paths).await, before);
}

/// A rename set is refused when a file appears under the destination, or at the destination path.
pub async fn dir_rename_conflicts_on_a_file_added_under_the_destination(store: &dyn MemoryStore) {
    for (added, expected) in [("/done/z.md", "/done/z.md"), ("/done", "/done")] {
        let s = fresh_scope();
        write(store, &s, &[("/tasks/a.md", "a")]).await;
        let v = version_of(store, &s, "/tasks/a.md").await;
        let rename = guarded(&s, vec![put_new("/done/a.md", "a"), del("/tasks/a.md", &v)], &["/tasks", "/done"]);
        write(store, &s, &[(added, "z")]).await;
        let paths = ["/tasks/a.md", "/done/a.md", added];
        let before = snapshot(store, &s, &paths).await;
        assert_conflict(store.apply(&rename, &caps()).await, expected);
        assert_eq!(snapshot(store, &s, &paths).await, before);
    }
}

/// A guard allows every path that the set names. It refuses a path that the set does not name.
pub async fn a_guard_allows_the_paths_that_the_set_names(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/tasks/a.md", "a")]).await;
    let v = version_of(store, &s, "/tasks/a.md").await;
    let ok = guarded(&s, vec![del("/tasks/a.md", &v), put_new("/tasks/new.md", "n")], &["/tasks"]);
    store.apply(&ok, &caps()).await.expect("the set names every path under the guard");
    assert_eq!(listed(store, &s, "/tasks").await, vec!["/tasks/new.md".to_string()]);

    let s2 = fresh_scope();
    write(store, &s2, &[("/tasks/a.md", "a"), ("/tasks/sub/q.md", "q")]).await;
    let v = version_of(store, &s2, "/tasks/a.md").await;
    let outsider = guarded(&s2, vec![del("/tasks/a.md", &v), put_new("/tasks/new.md", "n")], &["/tasks"]);
    assert_conflict(store.apply(&outsider, &caps()).await, "/tasks/sub/q.md");
}

/// A guard matches whole path segments. `_` and `%` in a guard are literal.
pub async fn a_guard_matches_segments_and_literal_wildcards(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/tasks-old/x", "1"), ("/tasks.md", "2"), ("/axb/f", "3")]).await;
    store.apply(&guarded(&s, vec![], &["/tasks", "/a_b"]), &caps()).await.expect("no path is under the guards");
}

/// A set with a guard and no change is still checked.
pub async fn a_guard_only_set_is_checked(store: &dyn MemoryStore) {
    let s = fresh_scope();
    let only = guarded(&s, vec![], &["/t"]);
    store.apply(&only, &caps()).await.expect("the scope is empty");
    write(store, &s, &[("/t/y", "y")]).await;
    assert_conflict(store.apply(&only, &caps()).await, "/t/y");
}

/// A guard conflict is reported before a cap refusal.
pub async fn a_guard_conflict_wins_over_a_cap_refusal(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/tasks/b.md", "b")]).await;
    let tight = MemoryCaps {
        max_store_bytes: 1,
        ..MemoryCaps::default()
    };
    let result = store.apply(&guarded(&s, vec![put_new("/big", "12345")], &["/tasks"]), &tight).await;
    assert_conflict(result, "/tasks/b.md");
}

/// A guard looks at its own scope only.
pub async fn a_guard_ignores_files_of_another_scope(store: &dyn MemoryStore) {
    let a = fresh_scope();
    let b = fresh_scope();
    write(store, &b, &[("/tasks/b.md", "b")]).await;
    let only = guarded(&a, vec![], &["/tasks"]);
    store.apply(&only, &caps()).await.expect("the file is in another scope");
    write(store, &a, &[("/tasks/b.md", "b")]).await;
    assert_conflict(store.apply(&only, &caps()).await, "/tasks/b.md");
}

/// The first path in guard order, then byte order, is the one reported.
pub async fn guards_report_the_first_outsider_in_byte_order(store: &dyn MemoryStore) {
    let s = fresh_scope();
    write(store, &s, &[("/b/2", "2"), ("/b/1", "1"), ("/a/9", "9")]).await;
    assert_conflict(store.apply(&guarded(&s, vec![], &["/b", "/a"]), &caps()).await, "/a/9");
}
