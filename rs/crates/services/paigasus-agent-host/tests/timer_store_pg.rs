// SPDX-License-Identifier: Apache-2.0

//! The Postgres timer store against a real Postgres (SMA-788 spec §7.3, §8, §14.2): the reads,
//! the tenancy isolation, and the commit contract `apply_in` that SMA-790 will call. Each test
//! runs `apply_in` in a test transaction that holds the `inbox_lease` row lock, as SMA-790 does.

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp, civil};
use metrics_util::debugging::DebuggingRecorder;
use paigasus_agent_host::adapters::persistence::timer_store::PgTimerStore;
use paigasus_agent_host_core::ids::{InboxId, ThreadId, TimerId};
use paigasus_agent_host_core::ports::TimerRepository;
use paigasus_agent_host_core::timer::{
    ApplyError, ApplyReport, BundledTzResolver, CancelOutcome, CronSchedule, Note, Slot, Timer, TimerCaps, TimerChanges, TimerKind, TimerOrigin, TimerParts, TimerStatus,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

/// A migrated Postgres, a store on it, and the org of every inbox of the test.
struct Env {
    _node: ContainerAsync<Postgres>,
    db: DatabaseConnection,
    store: PgTimerStore,
    org: Uuid,
}

/// One inbox with one thread.
struct Ib {
    id: InboxId,
    uuid: Uuid,
    thread: Uuid,
}

async fn env() -> Option<Env> {
    let (node, _url, db) = support::inbox::migrated_postgres().await?;
    let store = PgTimerStore::new(db.clone(), Arc::new(BundledTzResolver::new()));
    Some(Env {
        _node: node,
        db,
        store,
        org: Uuid::new_v4(),
    })
}

async fn new_inbox(env: &Env) -> Ib {
    let uuid = Uuid::new_v4();
    support::inbox::insert_inbox(&env.db, env.org, uuid, Uuid::new_v4(), Uuid::new_v4()).await;
    let thread = Uuid::new_v4();
    support::lease::insert_thread(&env.db, uuid, thread).await;
    Ib {
        id: InboxId::new(env.org, uuid),
        uuid,
        thread,
    }
}

fn follow_up(ib: &Ib, origin: TimerOrigin, at: Timestamp) -> Timer {
    timer(ib, origin, TimerStatus::Active, TimerKind::FollowUp, at)
}

fn timer(ib: &Ib, origin: TimerOrigin, status: TimerStatus, kind: TimerKind, at: Timestamp) -> Timer {
    Timer::new(TimerParts {
        id: TimerId::from_uuid(Uuid::new_v4()),
        inbox: ib.id.clone(),
        kind,
        thread: Some(ThreadId::from_uuid(ib.thread)),
        note: Note::parse("check the build").expect("note"),
        origin,
        status,
        next_fire_at: at,
    })
}

fn ins(timers: Vec<Timer>) -> TimerChanges {
    TimerChanges { inserts: timers, cancels: vec![] }
}

fn cancel(id: Uuid, origin: TimerOrigin) -> TimerChanges {
    TimerChanges {
        inserts: vec![],
        cancels: vec![(TimerId::from_uuid(id), origin)],
    }
}

fn hours(now: Timestamp, h: i64) -> Timestamp {
    now + SignedDuration::from_hours(h)
}

/// Run `apply_in` in a test transaction with the lease row lock, then commit the transaction.
/// A refused apply leaves the transaction usable, so the commit works then too.
async fn apply(env: &Env, ib: &Ib, caps: &TimerCaps, changes: TimerChanges) -> Result<ApplyReport, ApplyError> {
    let txn = env.db.begin().await.expect("begin");
    support::timer::lock_lease_row(&txn, ib.uuid).await;
    let result = env.store.apply_in(&txn, &ib.id, caps, changes).await;
    txn.commit().await.expect("commit");
    result
}

async fn count(env: &Env, ib: &Ib, extra: &str) -> i64 {
    support::scalar_i64(&env.db, &format!("SELECT count(*)::bigint AS n FROM timer WHERE inbox_id = '{}' {extra}", ib.uuid)).await
}

#[tokio::test]
async fn apply_in_inserts_and_the_store_reads_back() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");
    let one = follow_up(&ib, TimerOrigin::Agent, hours(now, 2));
    let schedule = CronSchedule::parse("0 9 * * *", "Europe/Berlin", &BundledTzResolver::new()).expect("schedule");
    let slot = Slot {
        civil: civil::date(2030, 1, 1).at(9, 0, 0, 0),
        second: false,
    };
    let at = schedule.instant(slot).expect("instant");
    let sched = timer(&ib, TimerOrigin::Agent, TimerStatus::Active, TimerKind::Schedule { schedule, next_slot: slot }, at);

    let report = apply(&env, &ib, &TimerCaps::default(), ins(vec![one.clone(), sched.clone()])).await.expect("apply");
    assert!(report.cancels.is_empty());

    let got = env.store.get(&ib.id, one.id()).await.expect("get").expect("the follow-up");
    assert_eq!(got.timer, one);
    assert_eq!((got.fire_count, got.last_fired_at), (0, None));
    assert!((got.created_at.as_second() - now.as_second()).abs() < 60, "created_at is the database now");

    let got = env.store.get(&ib.id, sched.id()).await.expect("get").expect("the schedule");
    assert_eq!(got.timer, sched, "the cron text, the zone, the slot and the instant round-trip");

    let listed = env.store.list_active(&ib.id, TimerOrigin::Agent).await.expect("list");
    let ids: Vec<TimerId> = listed.iter().map(|v| v.timer.id()).collect();
    assert_eq!(ids, [one.id(), sched.id()], "ordered by next_fire_at");
    assert!(env.store.list_active(&ib.id, TimerOrigin::System).await.expect("list").is_empty());

    let mut active = env.store.active_agent_ids(&ib.id).await.expect("ids");
    active.sort_by_key(TimerId::uuid);
    let mut want = [one.id(), sched.id()];
    want.sort_by_key(TimerId::uuid);
    assert_eq!(active, want);
    assert_eq!(env.store.count_agent_created_since(&ib.id, hours(now, -1)).await.expect("count"), 2);
    assert_eq!(env.store.count_agent_created_since(&ib.id, hours(now, 1)).await.expect("count"), 0);
}

#[tokio::test]
async fn store_reads_are_scoped_to_the_inbox() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let y = new_inbox(&env).await;
    let (tx, ty, ty_sys) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    support::timer::insert_follow_up_row(&env.db, x.uuid, tx, "agent", "date_trunc('second', now() + interval '1 hour')", "x").await;
    support::timer::insert_follow_up_row(&env.db, y.uuid, ty, "agent", "date_trunc('second', now() + interval '1 hour')", "y").await;
    support::timer::insert_follow_up_row(&env.db, y.uuid, ty_sys, "system", "date_trunc('second', now() + interval '1 hour')", "y").await;
    let now = env.store.db_now().await.expect("now");

    let listed = env.store.list_active(&x.id, TimerOrigin::Agent).await.expect("list");
    assert_eq!(listed.iter().map(|v| v.timer.id().uuid()).collect::<Vec<_>>(), [tx]);
    assert!(env.store.list_active(&x.id, TimerOrigin::System).await.expect("list").is_empty());
    assert!(env.store.get(&x.id, TimerId::from_uuid(ty)).await.expect("get").is_none());
    assert!(env.store.get(&x.id, TimerId::from_uuid(ty_sys)).await.expect("get").is_none());
    assert!(env.store.get(&y.id, TimerId::from_uuid(ty)).await.expect("get").is_some());
    let ids: Vec<Uuid> = env.store.active_agent_ids(&x.id).await.expect("ids").iter().map(|i| i.uuid()).collect();
    assert_eq!(ids, [tx]);
    assert_eq!(env.store.count_agent_created_since(&x.id, hours(now, -1)).await.expect("count"), 1);
}

#[tokio::test]
async fn a_row_that_does_not_decode_does_not_block_the_list_and_can_be_cancelled() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let (good, bad) = (Uuid::new_v4(), Uuid::new_v4());
    support::timer::insert_follow_up_row(&env.db, ib.uuid, good, "agent", "date_trunc('second', now() + interval '1 hour')", "good").await;
    support::timer::insert_schedule_row(&env.db, ib.uuid, bad, "*/15 * * * *", "Mars/Olympus", "2030-01-01T09:00:00", "now() + interval '2 hours'").await;
    env.db.execute_unprepared(&format!("UPDATE timer SET note = 'x' WHERE id = '{bad}'")).await.expect("note");

    let listed = env.store.list_active(&ib.id, TimerOrigin::Agent).await.expect("list skips the bad row");
    assert_eq!(listed.iter().map(|v| v.timer.id().uuid()).collect::<Vec<_>>(), [good]);
    assert!(env.store.get(&ib.id, TimerId::from_uuid(bad)).await.is_err(), "get cannot decode the bad row");
    let ids: Vec<Uuid> = env.store.active_agent_ids(&ib.id).await.expect("ids").iter().map(|i| i.uuid()).collect();
    assert!(ids.contains(&bad), "the bad row is still an active agent timer");

    let report = apply(&env, &ib, &TimerCaps::default(), cancel(bad, TimerOrigin::Agent)).await.expect("apply");
    assert_eq!(report.cancels, vec![(TimerId::from_uuid(bad), CancelOutcome::Cancelled)]);
    assert_eq!(count(&env, &ib, "AND status = 'cancelled'").await, 1);
}

#[tokio::test]
async fn apply_in_refuses_an_insert_for_another_inbox() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let y = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");

    let other = follow_up(&y, TimerOrigin::Agent, hours(now, 1));
    let err = apply(&env, &x, &TimerCaps::default(), ins(vec![other.clone()])).await.expect_err("mismatch");
    assert_eq!(err, ApplyError::InboxMismatch { id: other.id() });

    let done = timer(&x, TimerOrigin::Agent, TimerStatus::Done, TimerKind::FollowUp, hours(now, 1));
    let err = apply(&env, &x, &TimerCaps::default(), ins(vec![done.clone()])).await.expect_err("invalid");
    assert_eq!(err, ApplyError::InvalidInsert { id: done.id() });

    let all = support::scalar_i64(&env.db, "SELECT count(*)::bigint AS n FROM timer").await;
    assert_eq!(all, 0, "a refused change set writes nothing");
}

#[tokio::test]
async fn a_cancel_with_an_id_of_another_inbox_changes_nothing() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let y = new_inbox(&env).await;
    let ty = Uuid::new_v4();
    support::timer::insert_follow_up_row(&env.db, y.uuid, ty, "agent", "date_trunc('second', now() + interval '1 hour')", "y").await;

    let report = apply(&env, &x, &TimerCaps::default(), cancel(ty, TimerOrigin::Agent)).await.expect("apply");
    assert_eq!(report.cancels, [(TimerId::from_uuid(ty), CancelOutcome::NotActive)]);
    assert_eq!(support::timer::timer_state(&env.db, ty).await.status, "active");
}

#[tokio::test]
async fn system_and_agent_cancels_do_not_cross() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let (sys, agt) = (Uuid::new_v4(), Uuid::new_v4());
    support::timer::insert_follow_up_row(&env.db, x.uuid, sys, "system", "date_trunc('second', now() + interval '1 hour')", "").await;
    support::timer::insert_follow_up_row(&env.db, x.uuid, agt, "agent", "date_trunc('second', now() + interval '1 hour')", "").await;

    let r = apply(&env, &x, &TimerCaps::default(), cancel(sys, TimerOrigin::Agent)).await.expect("apply");
    assert_eq!(r.cancels, [(TimerId::from_uuid(sys), CancelOutcome::NotActive)]);
    let r = apply(&env, &x, &TimerCaps::default(), cancel(agt, TimerOrigin::System)).await.expect("apply");
    assert_eq!(r.cancels, [(TimerId::from_uuid(agt), CancelOutcome::NotActive)]);
    assert_eq!(support::timer::timer_state(&env.db, sys).await.status, "active");
    assert_eq!(support::timer::timer_state(&env.db, agt).await.status, "active");

    // The matching origins do cancel.
    let r = apply(&env, &x, &TimerCaps::default(), cancel(sys, TimerOrigin::System)).await.expect("apply");
    assert_eq!(r.cancels, [(TimerId::from_uuid(sys), CancelOutcome::Cancelled)]);
    let r = apply(&env, &x, &TimerCaps::default(), cancel(agt, TimerOrigin::Agent)).await.expect("apply");
    assert_eq!(r.cancels, [(TimerId::from_uuid(agt), CancelOutcome::Cancelled)]);
    assert_eq!(support::timer::timer_state(&env.db, sys).await.status, "cancelled");
    assert_eq!(support::timer::timer_state(&env.db, agt).await.status, "cancelled");
}

#[tokio::test]
async fn cancel_after_a_fire_is_not_active() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let x = new_inbox(&env).await;
    let id = Uuid::new_v4();
    support::timer::insert_follow_up_row(&env.db, x.uuid, id, "agent", "date_trunc('second', now() + interval '1 hour')", "").await;
    env.db.execute_unprepared(&format!("UPDATE timer SET status = 'done' WHERE id = '{id}'")).await.expect("set done");

    let r = apply(&env, &x, &TimerCaps::default(), cancel(id, TimerOrigin::Agent)).await.expect("apply");
    assert_eq!(r.cancels, [(TimerId::from_uuid(id), CancelOutcome::NotActive)]);
    assert_eq!(support::timer::timer_state(&env.db, id).await.status, "done");
    assert_eq!(support::lease::counter(&recorder, paigasus_observability::names::AGENT_HOST_TIMER_CANCEL_NOT_ACTIVE_TOTAL, None), 1);
}

#[tokio::test]
async fn count_cap_at_and_past_the_limit() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");
    let caps = TimerCaps {
        max_active_per_inbox: 2,
        ..TimerCaps::default()
    };
    let two = vec![follow_up(&x, TimerOrigin::Agent, hours(now, 1)), follow_up(&x, TimerOrigin::Agent, hours(now, 2))];
    apply(&env, &x, &caps, ins(two)).await.expect("two pass");
    let err = apply(&env, &x, &caps, ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 3))])).await.expect_err("three");
    assert_eq!(err, ApplyError::CapExceeded { before: 2, after: 3, limit: 2 });
    assert_eq!(count(&env, &x, "").await, 2, "the refused insert left no row");
    // A `system` timer is not under the cap of the agent.
    apply(&env, &x, &caps, ins(vec![follow_up(&x, TimerOrigin::System, hours(now, 3))])).await.expect("system insert");
}

#[tokio::test]
async fn an_inbox_over_a_lowered_cap_commits_a_run_without_timers() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    for id in &ids {
        support::timer::insert_follow_up_row(&env.db, x.uuid, *id, "agent", "date_trunc('second', now() + interval '1 hour')", "").await;
    }
    let caps = TimerCaps {
        max_active_per_inbox: 1,
        ..TimerCaps::default()
    };
    let now = env.store.db_now().await.expect("now");

    apply(&env, &x, &caps, TimerChanges::default()).await.expect("an empty change set");
    let r = apply(&env, &x, &caps, cancel(ids[0], TimerOrigin::Agent)).await.expect("a cancel");
    assert_eq!(r.cancels, [(TimerId::from_uuid(ids[0]), CancelOutcome::Cancelled)]);
    // Control: an insert that raises the count is refused.
    let err = apply(&env, &x, &caps, ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 5))])).await.expect_err("insert");
    assert_eq!(err, ApplyError::CapExceeded { before: 2, after: 3, limit: 1 });
    // A cancel with an insert that keeps the count is not refused.
    let both = TimerChanges {
        inserts: vec![follow_up(&x, TimerOrigin::Agent, hours(now, 5))],
        cancels: vec![(TimerId::from_uuid(ids[1]), TimerOrigin::Agent)],
    };
    apply(&env, &x, &caps, both).await.expect("swap");
}

#[tokio::test]
async fn budget_at_and_past_the_limit() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let old = Uuid::new_v4();
    support::timer::insert_follow_up_row(&env.db, x.uuid, old, "agent", "date_trunc('second', now() + interval '1 hour')", "").await;
    env.db
        .execute_unprepared(&format!("UPDATE timer SET created_at = now() - interval '25 hours' WHERE id = '{old}'"))
        .await
        .expect("age the row");
    let now = env.store.db_now().await.expect("now");
    let caps = TimerCaps {
        max_agent_timers_created_per_day: 2,
        ..TimerCaps::default()
    };

    // The row of 25 hours ago does not count, so two inserts pass.
    let first = follow_up(&x, TimerOrigin::Agent, hours(now, 1));
    let second = follow_up(&x, TimerOrigin::Agent, hours(now, 2));
    apply(&env, &x, &caps, ins(vec![first.clone(), second])).await.expect("two pass");
    let err = apply(&env, &x, &caps, ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 3))])).await.expect_err("third");
    assert_eq!(err, ApplyError::BudgetExceeded { created: 3, limit: 2 });

    // A cancelled timer still counts.
    apply(&env, &x, &caps, cancel(first.id().uuid(), TimerOrigin::Agent)).await.expect("cancel only");
    let err = apply(&env, &x, &caps, ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 3))])).await.expect_err("after a cancel");
    assert_eq!(err, ApplyError::BudgetExceeded { created: 3, limit: 2 });

    // A `system` insert is not under the budget.
    apply(&env, &x, &caps, ins(vec![follow_up(&x, TimerOrigin::System, hours(now, 3))])).await.expect("system insert");

    // A change set with no insert is not refused after the budget is lowered.
    let none = TimerCaps {
        max_agent_timers_created_per_day: 0,
        ..TimerCaps::default()
    };
    let r = apply(&env, &x, &none, cancel(old, TimerOrigin::Agent)).await.expect("cancel only under a budget of 0");
    assert_eq!(r.cancels.len(), 1);
    apply(&env, &x, &none, TimerChanges::default()).await.expect("empty under a budget of 0");
}

#[tokio::test]
async fn a_refusal_keeps_the_outer_transaction_usable() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");
    let caps = TimerCaps {
        max_active_per_inbox: 1,
        ..TimerCaps::default()
    };
    let two = ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 1)), follow_up(&x, TimerOrigin::Agent, hours(now, 2))]);

    let txn = env.db.begin().await.expect("begin");
    support::timer::lock_lease_row(&txn, x.uuid).await;
    let err = env.store.apply_in(&txn, &x.id, &caps, two).await.expect_err("cap");
    assert!(matches!(err, ApplyError::CapExceeded { before: 0, after: 2, limit: 1 }), "{err:?}");
    let row = txn
        .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, "SELECT 1::bigint AS n"))
        .await
        .expect("the transaction is usable")
        .expect("row");
    assert_eq!(row.try_get::<i64>("", "n").expect("n"), 1);
    txn.commit().await.expect("commit");
    assert_eq!(count(&env, &x, "").await, 0, "the commit leaves no new row");
}

#[tokio::test]
async fn a_sql_error_in_the_savepoint_is_a_store_error_and_the_transaction_stays_usable() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let y = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");
    // The thread belongs to inbox Y. The composite foreign key (thread_id, inbox_id) fails with 23503.
    let bad = Timer::new(TimerParts {
        id: TimerId::from_uuid(Uuid::new_v4()),
        inbox: x.id.clone(),
        kind: TimerKind::FollowUp,
        thread: Some(ThreadId::from_uuid(y.thread)),
        note: Note::parse("check the build").expect("note"),
        origin: TimerOrigin::Agent,
        status: TimerStatus::Active,
        next_fire_at: hours(now, 1),
    });

    let txn = env.db.begin().await.expect("begin");
    support::timer::lock_lease_row(&txn, x.uuid).await;
    let err = env.store.apply_in(&txn, &x.id, &TimerCaps::default(), ins(vec![bad])).await.expect_err("foreign key");
    assert!(matches!(err, ApplyError::Store(_)), "{err:?}");
    let row = txn
        .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, "SELECT 1::bigint AS n"))
        .await
        .expect("the outer transaction still runs a query")
        .expect("row");
    assert_eq!(row.try_get::<i64>("", "n").expect("n"), 1);
    txn.commit().await.expect("the outer transaction commits");
    assert_eq!(count(&env, &x, "").await, 0);
}

#[tokio::test]
async fn apply_in_without_a_lease_row_fails_and_writes_nothing() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");
    // No lease row exists for this inbox.
    let txn = env.db.begin().await.expect("begin");
    let err = env
        .store
        .apply_in(&txn, &x.id, &TimerCaps::default(), ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 1))]))
        .await
        .expect_err("no lease row");
    assert_eq!(err, ApplyError::LeaseMissing);
    txn.commit().await.expect("the outer transaction commits");
    assert_eq!(count(&env, &x, "").await, 0);
}

#[tokio::test]
async fn an_insert_counts_from_its_statement_time_not_the_transaction_start() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    let now = env.store.db_now().await.expect("now");
    let txn = env.db.begin().await.expect("begin");
    support::timer::lock_lease_row(&txn, x.uuid).await;
    // `now()` is the start of this transaction. Wait, then insert.
    txn.execute_unprepared("SELECT pg_sleep(1.2)").await.expect("sleep");
    env.store
        .apply_in(&txn, &x.id, &TimerCaps::default(), ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 1))]))
        .await
        .expect("apply");
    let row = txn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM timer WHERE created_at > now() + interval '1 second'",
        ))
        .await
        .expect("query")
        .expect("row");
    assert_eq!(row.try_get::<i64>("", "n").expect("n"), 1, "created_at is the statement time");
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn the_lease_row_lock_serializes_apply_in() {
    let Some(env) = env().await else {
        eprintln!("skipping timer_store_pg: Docker unavailable");
        return;
    };
    let x = new_inbox(&env).await;
    support::timer::ensure_lease_row(&env.db, x.uuid).await;
    let now = env.store.db_now().await.expect("now");

    // A second session holds the lease row.
    let holder = env.db.begin().await.expect("begin holder");
    support::timer::lock_lease_row(&holder, x.uuid).await;

    let store = env.store.clone();
    let db = env.db.clone();
    let inbox = x.id.clone();
    let changes = ins(vec![follow_up(&x, TimerOrigin::Agent, hours(now, 1))]);
    let task = tokio::spawn(async move {
        let txn = db.begin().await.expect("begin");
        let r = store.apply_in(&txn, &inbox, &TimerCaps::default(), changes).await;
        txn.commit().await.expect("commit");
        r
    });
    // Poll until a backend waits on a lock, so the check does not rely on a fixed sleep. The wait is bounded.
    let mut waiting = 0;
    for _ in 0..100 {
        waiting = support::scalar_i64(&env.db, "SELECT count(*)::bigint AS n FROM pg_stat_activity WHERE wait_event_type = 'Lock'").await;
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(waiting > 0, "apply_in waits for the lease row lock");
    assert!(!task.is_finished(), "apply_in is still blocked while the holder keeps the lock");
    holder.commit().await.expect("release");
    let r = tokio::time::timeout(Duration::from_secs(5), task).await.expect("done within 5 s").expect("join");
    r.expect("apply");
    assert_eq!(count(&env, &x, "").await, 1);
}
