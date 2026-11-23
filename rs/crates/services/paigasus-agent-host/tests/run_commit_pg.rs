// SPDX-License-Identifier: Apache-2.0

//! The fenced run commit and the failure commit against a real Postgres (spec §6.7, §6.8). The
//! tests of the SMA-787 `complete` moved here when the run commit replaced it: the claimed events
//! only, the thread update and the `Queue` mid-run policy (SMA-787 spec §8.5, G6).

mod support;

use std::sync::Arc;
use std::time::Duration;

use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::adapters::persistence::run_commit::PgRunCommitter;
use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::{EventId, InboxId};
use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use paigasus_agent_host_core::ports::{LeaseRepository, MemoryApplyError, MemoryStore, RunCommitter};
use paigasus_agent_host_core::run::{BackOff, CommitError, EventDelay, FlaggedEvent, LeaseToken, RunClass, RunRecord, RunUsage, ScreenOut, ScreenResult, SessionRow};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;
use support::commit::{committer, completed_record, inbox_scope, scope_of};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{counter, expire, insert_event_on_thread, insert_pending_event, insert_thread, lease_state, snapshot};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(30);
const HB_TIMEOUT: Duration = Duration::from_secs(5);
const ORG: u128 = 1;
const INBOX: u128 = 2;
const AGENT: u128 = 3;
const PRINCIPAL: u128 = 4;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn cfg() -> ClaimConfig {
    ClaimConfig {
        batch_limit: 20,
        max_attempts: 5,
        thread_ttl: Duration::from_secs(86400),
    }
}

/// One test: a migrated database, one inbox, two claimed events and a committer.
struct Setup {
    _node: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    db: DatabaseConnection,
    handle: LeaseHandle,
    claim: Claim,
    committer: Arc<PgRunCommitter>,
}

async fn setup() -> Option<Setup> {
    let (node, _url, db) = migrated_postgres().await?;
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(PRINCIPAL)).await;
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    insert_pending_event(&db, u(INBOX), u(11), 10).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let mut handle = store.acquire(&InboxId::new(u(ORG), u(INBOX)), "w1", TTL).await.expect("acquire").expect("some");
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    let committer = committer(&db);
    Some(Setup {
        _node: node,
        db,
        handle,
        claim,
        committer,
    })
}

fn scope() -> MemoryScope {
    scope_of(u(ORG), u(AGENT), u(PRINCIPAL))
}

fn path(p: &str) -> MemoryPath {
    MemoryPath::parse(p).expect("path")
}

fn rows() -> Vec<SessionRow> {
    (0..2)
        .map(|n| SessionRow {
            kind: "assistant_message".into(),
            payload: json!({"type": "assistant_message", "content": [{"type": "text", "text": format!("m{n}")}]}),
        })
        .collect()
}

/// A memory set of the inbox scope with one `put` of a new file.
fn put_new(p: &str) -> MemoryCommitSet {
    let mut cs = MemoryChangeSet::new();
    cs.record(MemoryChange::Put {
        path: path(p),
        content: "note".into(),
        expected: Expected::Absent,
    });
    MemoryCommitSet::new(scope(), cs)
}

/// Write one file to the store of the inbox scope, outside a run, and return its version.
async fn seed(db: &DatabaseConnection, p: &str, content: &str) -> MemoryVersion {
    let store = PgMemoryStore::new(db.clone());
    let mut cs = MemoryChangeSet::new();
    cs.record(MemoryChange::Put {
        path: path(p),
        content: content.into(),
        expected: Expected::Absent,
    });
    store.apply(&MemoryCommitSet::new(scope(), cs), &MemoryCaps::default()).await.expect("seed");
    store.get(&scope(), &path(p)).await.expect("get").expect("file").version
}

async fn n(db: &DatabaseConnection, sql: &str) -> i64 {
    support::scalar_i64(db, sql).await
}

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn text(db: &DatabaseConnection, sql: &str) -> Option<String> {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("row")
        .try_get::<Option<String>>("", "s")
        .expect("text column named s")
}

fn ids(mut v: Vec<EventId>) -> Vec<EventId> {
    v.sort_by_key(EventId::uuid);
    v
}

fn claimed(claim: &Claim) -> Vec<EventId> {
    ids(claim.events.iter().map(|e| e.id).collect())
}

/// A function and a trigger on `session_event` with the plpgsql `body`, plus the sequence
/// `fail_seq` that the body can count calls with. Sequences do not roll back.
async fn session_trigger(db: &DatabaseConnection, body: &str) {
    exec(db, "CREATE SEQUENCE fail_seq").await;
    exec(db, &format!("CREATE FUNCTION fail_session() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN {body} RETURN NEW; END $$")).await;
    exec(db, "CREATE TRIGGER fail_session BEFORE INSERT ON session_event FOR EACH ROW EXECUTE FUNCTION fail_session()").await;
}

#[tokio::test]
async fn a_full_commit_writes_the_run_the_session_the_memory_and_done() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let thread = s.claim.thread.uuid();
    let mut record = completed_record(&s.claim, scope());
    record.class = RunClass::Completed {
        output: Some("ok".into()),
        usage: RunUsage {
            input_tokens: 7,
            output_tokens: 3,
            cached_input_tokens: 1,
            reasoning_tokens: 2,
        },
    };
    record.session = rows();
    record.memory = put_new("/a.md");
    let c = s.committer.commit_record(&mut s.handle, record).await.expect("commit");
    assert_eq!(ids(c.done), claimed(&s.claim));
    assert_eq!(
        n(
            &s.db,
            &format!(
                "SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'completed' AND finished_at IS NOT NULL AND final_output = 'ok' \
                 AND input_tokens = 7 AND output_tokens = 3 AND cached_input_tokens = 1 AND reasoning_tokens = 2"
            )
        )
        .await,
        1
    );
    assert_eq!(
        n(
            &s.db,
            &format!("SELECT count(*) AS n FROM session_event WHERE run_id = '{run}' AND thread_id = '{thread}' AND sequence IN (0, 1)")
        )
        .await,
        2
    );
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM session_event").await, 2);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM memory_file WHERE path = '/a.md'").await, 1);
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{thread}' AND last_run_at IS NOT NULL")).await, 1);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE status = 'done'").await, 2);
}

/// The commit writes under the thread that `UPDATE run … RETURNING thread_id` returns. The run row
/// moves to a second thread of the same inbox before the commit; the rows must follow it.
#[tokio::test]
async fn the_commit_uses_the_thread_of_the_run_row() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let first = s.claim.thread.uuid();
    insert_thread(&s.db, u(INBOX), u(72)).await;
    exec(&s.db, &format!("UPDATE run SET thread_id = '{}' WHERE id = '{run}'", u(72))).await;
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    s.committer.commit_record(&mut s.handle, record).await.expect("commit");
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM session_event WHERE thread_id = '{}'", u(72))).await, 2);
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM session_event WHERE thread_id = '{first}'")).await, 0);
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND last_run_at IS NOT NULL", u(72))).await, 1);
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{first}' AND last_run_at IS NULL")).await, 1);
}

fn failure(s: &Setup, close_thread: bool) -> RunRecord {
    let mut record = completed_record(&s.claim, scope());
    record.class = RunClass::Deterministic {
        error: "timed out".into(),
        retry: s
            .claim
            .events
            .iter()
            .map(|e| EventDelay {
                event: e.id,
                delay: Duration::from_secs(60),
            })
            .collect(),
    };
    record.session = rows();
    record.memory = put_new("/progress.md");
    record.close_thread = close_thread;
    record
}

#[tokio::test]
async fn a_failure_commit_writes_memory_and_back_off_and_no_session() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let record = failure(&s, false);
    let c = s.committer.commit_record(&mut s.handle, record).await.expect("commit");
    assert!(c.done.is_empty());
    assert_eq!(
        n(
            &s.db,
            &format!(
                "SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'failed' AND error_class = 'deterministic' AND error = 'timed out' \
                 AND input_tokens IS NULL AND output_tokens IS NULL AND cached_input_tokens IS NULL AND reasoning_tokens IS NULL"
            )
        )
        .await,
        1
    );
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM session_event").await, 0);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM memory_file WHERE path = '/progress.md'").await, 1);
    assert_eq!(
        n(
            &s.db,
            "SELECT count(*) AS n FROM inbox_event WHERE status = 'pending' AND last_error = 'timed out' AND not_before > now() + interval '55 seconds'"
        )
        .await,
        2
    );
    let thread = s.claim.thread.uuid();
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{thread}' AND NOT closed")).await, 1);
}

#[tokio::test]
async fn close_thread_closes_the_thread_in_the_failure_commit() {
    let Some(mut s) = setup().await else { return };
    let record = failure(&s, true);
    s.committer.commit_record(&mut s.handle, record).await.expect("commit");
    let thread = s.claim.thread.uuid();
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{thread}' AND closed")).await, 1);
}

#[tokio::test]
async fn a_memory_conflict_rolls_back_everything() {
    let Some(mut s) = setup().await else { return };
    let v = seed(&s.db, "/a.md", "seeded").await;
    let stale = MemoryVersion::new((v.as_str().parse::<i64>().expect("bigint version") + 1000).to_string());
    let mut cs = MemoryChangeSet::new();
    cs.record(MemoryChange::Put {
        path: path("/a.md"),
        content: "new".into(),
        expected: Expected::Version(stale),
    });
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    record.memory = MemoryCommitSet::new(scope(), cs);
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert!(matches!(res, Err(CommitError::MemoryRejected(MemoryApplyError::Conflict { .. }))), "{res:?}");
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 1);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM session_event").await, 0);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE status = 'pending'").await, 2);
    assert_eq!(text(&s.db, "SELECT content AS s FROM memory_file WHERE path = '/a.md'").await.as_deref(), Some("seeded"));
    assert_eq!(counter(&recorder, names::AGENT_HOST_COMMIT_MEMORY_CONFLICTS_TOTAL, None), 1);
    assert!(!s.handle.is_poisoned());
}

/// A set with a guard and no change is not empty, so the commit must call `apply_in`, and the
/// guard refuses the file that no change names.
#[tokio::test]
async fn a_guard_with_no_change_still_reaches_apply_in() {
    let Some(mut s) = setup().await else { return };
    seed(&s.db, "/dir/x.md", "x").await;
    let mut set = MemoryCommitSet::new(scope(), MemoryChangeSet::new());
    set.guards.insert(MemoryDir::parse("/dir").expect("dir"));
    let mut record = completed_record(&s.claim, scope());
    record.memory = set;
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert!(matches!(res, Err(CommitError::MemoryRejected(_))), "{res:?}");
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 1);
}

#[tokio::test]
async fn a_guard_over_an_empty_dir_commits() {
    let Some(mut s) = setup().await else { return };
    let mut set = MemoryCommitSet::new(scope(), MemoryChangeSet::new());
    set.guards.insert(MemoryDir::parse("/empty").expect("dir"));
    let mut record = completed_record(&s.claim, scope());
    record.memory = set;
    let c = s.committer.commit_record(&mut s.handle, record).await.expect("commit");
    assert_eq!(ids(c.done), claimed(&s.claim));
}

#[tokio::test]
async fn a_scope_of_another_principal_is_refused() {
    let Some(mut s) = setup().await else { return };
    let before = snapshot(&s.db, u(INBOX)).await;
    let mut cs = MemoryChangeSet::new();
    cs.record(MemoryChange::Put {
        path: path("/a.md"),
        content: "note".into(),
        expected: Expected::Absent,
    });
    let mut record = completed_record(&s.claim, scope());
    record.memory = MemoryCommitSet::new(scope_of(u(ORG), u(AGENT), u(99)), cs);
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert!(matches!(res, Err(CommitError::Store(ref t)) if t.contains("scope")), "{res:?}");
    assert_eq!(snapshot(&s.db, u(INBOX)).await, before);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM memory_file").await, 0);
}

#[tokio::test]
async fn a_store_error_retries_the_whole_transaction() {
    let Some(mut s) = setup().await else { return };
    // Only the first row of an attempt counts, so the sequence counts attempts.
    session_trigger(&s.db, "IF NEW.sequence = 0 THEN IF nextval('fail_seq') <= 2 THEN RAISE EXCEPTION 'injected'; END IF; END IF;").await;
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert!(res.is_ok(), "{res:?}");
    assert_eq!(n(&s.db, "SELECT last_value AS n FROM fail_seq").await, 3);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM session_event").await, 2);
}

#[tokio::test]
async fn a_store_error_after_two_retries_is_store() {
    let Some(mut s) = setup().await else { return };
    session_trigger(&s.db, "IF NEW.sequence = 0 THEN PERFORM nextval('fail_seq'); RAISE EXCEPTION 'injected'; END IF;").await;
    let before = snapshot(&s.db, u(INBOX)).await;
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert!(matches!(res, Err(CommitError::Store(_))), "{res:?}");
    assert!(!res.as_ref().unwrap_err().is_unencodable());
    assert_eq!(n(&s.db, "SELECT last_value AS n FROM fail_seq").await, 3);
    assert_eq!(snapshot(&s.db, u(INBOX)).await, before);
    assert!(!s.handle.is_poisoned());
}

/// A deferred constraint trigger raises at COMMIT, after every statement succeeded.
#[tokio::test]
async fn a_failed_commit_statement_is_outcome_unknown_and_poisons() {
    let Some(mut s) = setup().await else { return };
    exec(
        &s.db,
        "CREATE FUNCTION refuse_session() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$",
    )
    .await;
    exec(
        &s.db,
        "CREATE CONSTRAINT TRIGGER refuse_session AFTER INSERT ON session_event DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_session()",
    )
    .await;
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    let res = s.committer.commit_record(&mut s.handle, record.clone()).await;
    assert_eq!(res, Err(CommitError::OutcomeUnknown));
    assert!(s.handle.is_poisoned());
    // With the trigger gone, the same commit would pass. The poisoned handle sends no SQL and
    // counts nothing.
    exec(&s.db, "DROP TRIGGER refuse_session ON session_event").await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(s.committer.commit_record(&mut s.handle, record).await, Err(CommitError::LeaseLost));
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, None), 0, "the poisoned path counts nothing");
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE status = 'pending'").await, 2);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 1);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM session_event").await, 0);
}

#[tokio::test]
async fn sqlstate_22p05_is_unencodable_and_not_retried() {
    let Some(mut s) = setup().await else { return };
    session_trigger(&s.db, "PERFORM nextval('fail_seq'); RAISE EXCEPTION 'x' USING ERRCODE = '22P05';").await;
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert!(res.as_ref().unwrap_err().is_unencodable(), "{res:?}");
    assert_eq!(n(&s.db, "SELECT last_value AS n FROM fail_seq").await, 1);
}

#[tokio::test]
async fn nul_in_the_output_is_replaced() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let mut record = completed_record(&s.claim, scope());
    record.class = RunClass::Completed {
        output: Some("a\u{0}b".into()),
        usage: RunUsage::default(),
    };
    s.committer.commit_record(&mut s.handle, record).await.expect("commit");
    assert_eq!(text(&s.db, &format!("SELECT final_output AS s FROM run WHERE id = '{run}'")).await.as_deref(), Some("a\u{FFFD}b"));
}

#[tokio::test]
async fn a_stale_commit_is_lease_lost_and_changes_no_row() {
    let Some(mut s) = setup().await else { return };
    expire(&s.db, u(INBOX)).await;
    let before = snapshot(&s.db, u(INBOX)).await;
    let mut record = completed_record(&s.claim, scope());
    record.session = rows();
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let res = s.committer.commit_record(&mut s.handle, record).await;
    assert_eq!(res, Err(CommitError::LeaseLost));
    assert_eq!(snapshot(&s.db, u(INBOX)).await, before);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM session_event").await, 0);
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "commit"))), 1);
}

fn flag(event: EventId) -> FlaggedEvent {
    FlaggedEvent {
        event,
        detector: "pii".into(),
        reason: "an email".into(),
    }
}

/// The claimed events, sorted by id (`u(10)` first).
fn events(s: &Setup) -> Vec<EventId> {
    claimed(&s.claim)
}

fn screen_all(s: &Setup) -> ScreenOut {
    ScreenOut {
        run: s.claim.run.clone(),
        token: s.claim.lease_token,
        flagged: events(s).into_iter().map(flag).collect(),
        all: true,
    }
}

fn back_off_of(s: &Setup, delay: Duration) -> BackOff {
    BackOff {
        run: s.claim.run.clone(),
        token: s.claim.lease_token,
        error: "model unavailable".into(),
        retry: s.claim.events.iter().map(|e| EventDelay { event: e.id, delay }).collect(),
    }
}

/// A deferred constraint trigger that raises at COMMIT after any `inbox_event` update.
async fn refuse_event_update_at_commit(db: &DatabaseConnection) {
    exec(
        db,
        "CREATE FUNCTION refuse_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$",
    )
    .await;
    exec(
        db,
        "CREATE CONSTRAINT TRIGGER refuse_event AFTER UPDATE ON inbox_event DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_event()",
    )
    .await;
}

/// Every value that the histogram `name` recorded in one snapshot of the recorder.
fn histogram(recorder: &DebuggingRecorder, name: &str) -> Vec<f64> {
    recorder
        .snapshotter()
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(k, _, _, _)| k.key().name() == name)
        .flat_map(|(_, _, _, v)| match v {
            DebugValue::Histogram(h) => h.into_iter().map(|x| x.into_inner()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[tokio::test]
async fn screen_out_of_some_events_keeps_the_run_running() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let e = events(&s);
    let out = ScreenOut {
        run: s.claim.run.clone(),
        token: s.claim.lease_token,
        flagged: vec![flag(e[0])],
        all: false,
    };
    let res = s.committer.screen_out(&mut s.handle, out).await;
    assert_eq!(res, Ok(ScreenResult::Continue));
    let (e1, e2) = (e[0].uuid(), e[1].uuid());
    assert_eq!(
        n(
            &s.db,
            &format!("SELECT count(*) AS n FROM inbox_event WHERE id = '{e1}' AND status = 'dead' AND last_error = 'screened: pii: an email'")
        )
        .await,
        1
    );
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM inbox_event WHERE id = '{e2}' AND status = 'pending'")).await, 1);
    assert_eq!(
        n(&s.db, &format!("SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'running' AND finished_at IS NULL")).await,
        1
    );
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM run_event WHERE run_id = '{run}'")).await, 2);
    assert!(!s.handle.is_poisoned());
}

#[tokio::test]
async fn screen_out_of_every_event_screens_the_run() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let thread = s.claim.thread.uuid();
    let out = screen_all(&s);
    let dead = DebuggingRecorder::new();
    let res = {
        let _guard = metrics::set_default_local_recorder(&dead);
        s.committer.screen_out(&mut s.handle, out).await
    };
    assert_eq!(res, Ok(ScreenResult::AllScreened));
    assert_eq!(
        n(&s.db, &format!("SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'screened' AND finished_at IS NOT NULL")).await,
        1
    );
    assert_eq!(
        n(
            &s.db,
            &format!("SELECT count(*) AS n FROM thread WHERE id = '{thread}' AND last_run_at IS NULL AND successor_id IS NULL")
        )
        .await,
        1
    );
    assert_eq!(
        n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE status = 'dead' AND last_error = 'screened: pii: an email'").await,
        2
    );
    // A snapshot drains the recorder, so the three metrics read one snapshot.
    let snap = dead.snapshotter().snapshot().into_vec();
    let total = |name: &str| -> u64 {
        snap.iter()
            .filter(|(k, _, _, _)| k.key().name() == name)
            .map(|(_, _, _, v)| match v {
                DebugValue::Counter(n) => *n,
                _ => 0,
            })
            .sum()
    };
    assert_eq!(total(names::AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL), 2);
    assert_eq!(total(names::AGENT_HOST_EVENTS_SCREENED_TOTAL), 2);
    let attempts: Vec<f64> = snap
        .iter()
        .filter(|(k, _, _, _)| k.key().name() == names::AGENT_HOST_EVENT_ATTEMPTS)
        .flat_map(|(_, _, _, v)| match v {
            DebugValue::Histogram(h) => h.iter().map(|x| x.into_inner()).collect(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(attempts, vec![1.0, 1.0]);
}

#[tokio::test]
async fn a_failed_commit_of_screen_out_is_outcome_unknown() {
    let Some(mut s) = setup().await else { return };
    refuse_event_update_at_commit(&s.db).await;
    let out = screen_all(&s);
    let res = s.committer.screen_out(&mut s.handle, out).await;
    assert_eq!(res, Err(CommitError::OutcomeUnknown));
    assert!(s.handle.is_poisoned());
}

#[tokio::test]
async fn a_stale_screen_out_is_lease_lost_and_changes_no_row() {
    let Some(mut s) = setup().await else { return };
    expire(&s.db, u(INBOX)).await;
    let before = snapshot(&s.db, u(INBOX)).await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let out = screen_all(&s);
    assert_eq!(s.committer.screen_out(&mut s.handle, out).await, Err(CommitError::LeaseLost));
    assert_eq!(snapshot(&s.db, u(INBOX)).await, before);
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "screen_out"))), 1);
}

#[tokio::test]
async fn back_off_fails_the_run_and_delays_the_events() {
    let Some(mut s) = setup().await else { return };
    let run = s.claim.run.uuid();
    let b = back_off_of(&s, Duration::from_secs(30));
    assert_eq!(s.committer.back_off(&mut s.handle, b).await, Ok(()));
    assert_eq!(
        n(
            &s.db,
            &format!(
                "SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'failed' AND finished_at IS NOT NULL \
                 AND error_class = 'transient' AND error = 'model unavailable'"
            )
        )
        .await,
        1
    );
    assert_eq!(
        n(
            &s.db,
            "SELECT count(*) AS n FROM inbox_event WHERE status = 'pending' AND last_error = 'model unavailable' \
             AND not_before > now() + interval '25 seconds'"
        )
        .await,
        2
    );
    assert!(!s.handle.is_poisoned());
}

#[tokio::test]
async fn a_failed_commit_of_back_off_is_outcome_unknown() {
    let Some(mut s) = setup().await else { return };
    refuse_event_update_at_commit(&s.db).await;
    let b = back_off_of(&s, Duration::from_secs(30));
    assert_eq!(s.committer.back_off(&mut s.handle, b).await, Err(CommitError::OutcomeUnknown));
    assert!(s.handle.is_poisoned());
}

#[tokio::test]
async fn a_stale_back_off_is_lease_lost_and_changes_no_row() {
    let Some(mut s) = setup().await else { return };
    expire(&s.db, u(INBOX)).await;
    let before = snapshot(&s.db, u(INBOX)).await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let b = back_off_of(&s, Duration::from_secs(30));
    assert_eq!(s.committer.back_off(&mut s.handle, b).await, Err(CommitError::LeaseLost));
    assert_eq!(snapshot(&s.db, u(INBOX)).await, before);
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "back_off"))), 1);
}

#[tokio::test]
async fn back_off_then_the_claim_moves_to_dead_with_the_kept_error() {
    let Some(mut s) = setup().await else { return };
    let b = back_off_of(&s, Duration::ZERO);
    s.committer.back_off(&mut s.handle, b).await.expect("back_off");
    // F8: the adapter does not clear the open claim of a raw handle; the keeper does.
    s.handle.record_complete();
    let store = PgLeaseStore::new(s.db.clone(), HB_TIMEOUT);
    let one = ClaimConfig { max_attempts: 1, ..cfg() };
    let recorder = DebuggingRecorder::new();
    let res = {
        let _guard = metrics::set_default_local_recorder(&recorder);
        store.claim_events(&mut s.handle, &one).await
    };
    assert_eq!(res, Ok(None));
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE status = 'dead' AND last_error = 'model unavailable'").await, 2);
    // The claim observes the attempts of each dead row.
    assert_eq!(histogram(&recorder, names::AGENT_HOST_EVENT_ATTEMPTS), vec![1.0, 1.0]);
}

#[tokio::test]
async fn release_after_shutdown_refunds_the_attempt() {
    let Some(s) = setup().await else { return };
    let run = s.claim.run.uuid();
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE attempts = 1").await, 2);
    assert_eq!(s.committer.release_after_shutdown(s.handle).await, Ok(()));
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE attempts = 0 AND status = 'pending'").await, 2);
    assert_eq!(
        n(&s.db, &format!("SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'discarded' AND finished_at IS NOT NULL")).await,
        1
    );
    assert_eq!(lease_state(&s.db, u(INBOX)).await.map(|(_, live)| live), Some(false));
}

/// A run that is no longer `running` is not an open run: a shutdown refunds no attempt of it.
#[tokio::test]
async fn a_shutdown_after_back_off_refunds_nothing() {
    let Some(mut s) = setup().await else { return };
    let b = back_off_of(&s, Duration::from_secs(30));
    s.committer.back_off(&mut s.handle, b).await.expect("back_off");
    // The open claim stays on the handle (no `record_complete`).
    assert_eq!(s.committer.release_after_shutdown(s.handle).await, Ok(()));
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM inbox_event WHERE attempts = 1").await, 2);
    assert_eq!(n(&s.db, "SELECT count(*) AS n FROM run WHERE status = 'failed'").await, 1);
    assert_eq!(lease_state(&s.db, u(INBOX)).await.map(|(_, live)| live), Some(false));
}

#[tokio::test]
async fn a_poisoned_release_after_shutdown_sends_no_sql() {
    let Some(mut s) = setup().await else { return };
    let before = snapshot(&s.db, u(INBOX)).await;
    s.handle.poison();
    assert_eq!(s.committer.release_after_shutdown(s.handle).await, Ok(()));
    assert_eq!(snapshot(&s.db, u(INBOX)).await, before);
}

#[tokio::test]
async fn a_stale_worker_changes_no_attempt_of_the_new_holder() {
    let Some(s) = setup().await else { return };
    let a_handle = s.handle;
    expire(&s.db, u(INBOX)).await;
    let store = PgLeaseStore::new(s.db.clone(), HB_TIMEOUT);
    let mut b = store.acquire(&InboxId::new(u(ORG), u(INBOX)), "w2", TTL).await.expect("acquire").expect("some");
    let b_claim = store.claim_events(&mut b, &cfg()).await.expect("claim").expect("some");
    let b_run = b_claim.run.uuid();
    assert_eq!(n(&s.db, &format!("SELECT count(*) AS n FROM inbox_event WHERE attempts = 2 AND claimed_run_id = '{b_run}'")).await, 2);
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let before = snapshot(&s.db, u(INBOX)).await;
    assert_eq!(s.committer.release_after_shutdown(a_handle).await, Err(CommitError::LeaseLost));
    assert_eq!(before, snapshot(&s.db, u(INBOX)).await);
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "release_after_shutdown"))), 1);
}

// The tests of the SMA-787 `complete`, moved to the commit (SMA-790 §6.8). Each test inserts its
// own events, so it starts from an inbox with no event and claims itself.

/// One test: a migrated database, one inbox with no event, its lease, its scope and a committer.
struct Bare {
    _node: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    db: DatabaseConnection,
    store: PgLeaseStore,
    handle: LeaseHandle,
    scope: MemoryScope,
    committer: Arc<PgRunCommitter>,
}

async fn bare() -> Option<Bare> {
    let (node, _url, db) = migrated_postgres().await?;
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(PRINCIPAL)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = take_as(&store, "w1").await;
    let scope = inbox_scope(&db, handle.inbox()).await;
    let committer = committer(&db);
    Some(Bare {
        _node: node,
        db,
        store,
        handle,
        scope,
        committer,
    })
}

async fn take_as(store: &PgLeaseStore, holder: &str) -> LeaseHandle {
    store.acquire(&InboxId::new(u(ORG), u(INBOX)), holder, TTL).await.expect("acquire").expect("some")
}

async fn status_count(db: &DatabaseConnection, status: &str) -> i64 {
    n(db, &format!("SELECT count(*) AS n FROM inbox_event WHERE status = '{status}'")).await
}

/// An old thread (last run two days ago) with one pending event on it.
async fn insert_expired_thread_with_event(db: &DatabaseConnection, thread: Uuid, event: Uuid) {
    insert_thread(db, u(INBOX), thread).await;
    exec(db, &format!("UPDATE thread SET last_run_at = now() - interval '2 days' WHERE id = '{thread}'")).await;
    insert_event_on_thread(db, u(INBOX), event, 10, thread).await;
}

impl Bare {
    async fn claim(&mut self) -> Claim {
        self.store.claim_events(&mut self.handle, &cfg()).await.expect("claim").expect("some")
    }

    /// Commit a `Completed` record of `claim`. A raw handle has no keeper, so the test finishes
    /// the claim on the handle after `Ok`.
    async fn commit(&mut self, claim: &Claim) -> Vec<EventId> {
        let c = self.committer.commit(&mut self.handle, completed_record(claim, self.scope.clone())).await.expect("commit");
        self.handle.record_complete();
        ids(c.done)
    }
}

#[tokio::test]
async fn commit_marks_only_the_claimed_events_done() {
    let Some(mut b) = bare().await else { return };
    insert_pending_event(&b.db, u(INBOX), u(10), 20).await;
    insert_pending_event(&b.db, u(INBOX), u(11), 10).await;
    let claim = b.claim().await;
    let run = claim.run.uuid();
    let c = b.committer.commit(&mut b.handle, completed_record(&claim, b.scope.clone())).await.expect("commit");
    // `UPDATE ... RETURNING` has no defined order, and the port does not promise one.
    assert_eq!(ids(c.done), vec![EventId::from_uuid(u(10)), EventId::from_uuid(u(11))]);
    assert_eq!(status_count(&b.db, "done").await, 2);
    assert_eq!(
        n(&b.db, &format!("SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'completed' AND finished_at IS NOT NULL")).await,
        1
    );
    // The adapter does not finish the claim on the handle: the keeper does (spec §6.8).
    assert_eq!(b.handle.open_claim(), Some(claim.run));
}

#[tokio::test]
async fn commit_sets_last_run_at_and_the_successor() {
    let Some(mut b) = bare().await else { return };
    insert_expired_thread_with_event(&b.db, u(70), u(10)).await;
    let claim = b.claim().await;
    let new = claim.thread.uuid();
    assert_ne!(new, u(70));
    b.commit(&claim).await;
    assert_eq!(n(&b.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{new}' AND last_run_at IS NOT NULL")).await, 1);
    assert_eq!(n(&b.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND successor_id = '{new}'", u(70))).await, 1);
}

#[tokio::test]
async fn commit_does_not_overwrite_a_successor() {
    let Some(mut b) = bare().await else { return };
    insert_expired_thread_with_event(&b.db, u(70), u(10)).await;
    let claim = b.claim().await;
    insert_thread(&b.db, u(INBOX), u(71)).await;
    exec(&b.db, &format!("UPDATE thread SET successor_id = '{}' WHERE id = '{}'", u(71), u(70))).await;
    b.commit(&claim).await;
    assert_eq!(n(&b.db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND successor_id = '{}'", u(70), u(71))).await, 1);
}

#[tokio::test]
async fn an_old_claim_on_a_new_handle_is_lease_lost() {
    let Some(mut b) = bare().await else { return };
    insert_pending_event(&b.db, u(INBOX), u(10), 10).await;
    let c1 = b.claim().await;
    expire(&b.db, u(INBOX)).await;
    let mut h2 = take_as(&b.store, "w2").await;
    let c2 = b.store.claim_events(&mut h2, &cfg()).await.expect("claim").expect("some");
    assert_ne!(c1.run, c2.run);
    assert_eq!(b.committer.commit(&mut h2, completed_record(&c1, b.scope.clone())).await, Err(CommitError::LeaseLost));
    assert_eq!(status_count(&b.db, "pending").await, 1);
    assert_eq!(n(&b.db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 2);
    assert_eq!(n(&b.db, &format!("SELECT count(*) AS n FROM inbox_event WHERE claimed_run_id = '{}'", c2.run.uuid())).await, 1);
}

#[tokio::test]
async fn queue_keeps_a_mid_run_event_pending() {
    let Some(mut b) = bare().await else { return };
    insert_pending_event(&b.db, u(INBOX), u(10), 20).await;
    let claim = b.claim().await;
    let thread = claim.thread;
    insert_event_on_thread(&b.db, u(INBOX), u(11), 10, thread.uuid()).await;
    assert_eq!(b.commit(&claim).await, vec![EventId::from_uuid(u(10))]);
    assert_eq!(status_count(&b.db, "pending").await, 1);
    let next = b.claim().await;
    assert_eq!(next.lease_token, LeaseToken(2));
    assert_eq!(next.thread, thread);
    assert!(!next.fresh_thread);
    assert!(next.events.iter().map(|e| e.id).eq([EventId::from_uuid(u(11))]));
}

#[tokio::test]
async fn a_crash_retry_reports_a_fresh_thread() {
    let Some(mut b) = bare().await else { return };
    insert_pending_event(&b.db, u(INBOX), u(10), 10).await;
    let c1 = b.claim().await;
    expire(&b.db, u(INBOX)).await;
    let mut h2 = take_as(&b.store, "w2").await;
    let c2 = b.store.claim_events(&mut h2, &cfg()).await.expect("claim").expect("some");
    assert_eq!(c2.thread, c1.thread);
    assert!(c2.fresh_thread);
    assert!(c2.events.iter().map(|e| e.id).eq([EventId::from_uuid(u(10))]));
    assert_eq!(n(&b.db, &format!("SELECT attempts::bigint AS n FROM inbox_event WHERE id = '{}'", u(10))).await, 2);
}

#[tokio::test]
async fn a_done_event_in_the_claim_is_not_marked_again() {
    let Some(mut b) = bare().await else { return };
    insert_pending_event(&b.db, u(INBOX), u(10), 20).await;
    insert_pending_event(&b.db, u(INBOX), u(11), 10).await;
    let claim = b.claim().await;
    exec(&b.db, &format!("UPDATE inbox_event SET status = 'done' WHERE id = '{}'", u(11))).await;
    assert_eq!(b.commit(&claim).await, vec![EventId::from_uuid(u(10))]);
    assert_eq!(status_count(&b.db, "done").await, 2);
}

/// The id list is a guard of its own: an event that the run claimed but the record does not list
/// as passed stays pending. (`claimed_run_id` alone would mark it.)
#[tokio::test]
async fn commit_marks_only_the_ids_of_the_claim_value() {
    let Some(mut b) = bare().await else { return };
    insert_pending_event(&b.db, u(INBOX), u(10), 20).await;
    insert_pending_event(&b.db, u(INBOX), u(11), 10).await;
    let mut claim = b.claim().await;
    claim.events.truncate(1);
    assert_eq!(b.commit(&claim).await, vec![EventId::from_uuid(u(10))]);
    assert_eq!(status_count(&b.db, "pending").await, 1);
}
