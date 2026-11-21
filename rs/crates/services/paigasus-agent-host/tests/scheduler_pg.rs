// SPDX-License-Identifier: Apache-2.0

//! The Postgres scheduler against a real Postgres (SMA-788 spec §11.1, §14.1, §14.2): one short
//! transaction per due row, exactly-once delivery, the isolation of bad rows and the rule that a
//! transient database error never ends a timer. Every test that uses the back-off sets the base
//! to 60 s, so a slow host cannot see a backed-off row come due again inside the test.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp, civil};
use metrics_util::debugging::DebuggingRecorder;
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::scheduler::{AfterInsertHook, PgScheduler, SchedulerSettings};
use paigasus_agent_host::adapters::persistence::timer_store::PgTimerStore;
use paigasus_agent_host::adapters::timer_event::{SOURCE_AGENT, TIMER_FIRED_TYPE};
use paigasus_agent_host_core::ids::{EventId, InboxId};
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::ports::{InboxRepository, Scheduler, TzResolver};
use paigasus_agent_host_core::timer::{BundledTzResolver, CronSchedule, Slot, WALK_BOUND, format_fire_at};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, Statement, TransactionTrait};
use serde_json::json;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

struct Env {
    _node: ContainerAsync<Postgres>,
    url: String,
    db: DatabaseConnection,
    org: Uuid,
}

struct Ib {
    id: InboxId,
    uuid: Uuid,
    thread: Uuid,
}

async fn env() -> Option<Env> {
    let (node, url, db) = support::inbox::migrated_postgres().await?;
    Some(Env {
        _node: node,
        url,
        db,
        org: Uuid::new_v4(),
    })
}

fn settings() -> SchedulerSettings {
    SchedulerSettings {
        statement_timeout: Duration::from_secs(5),
        max_fire_attempts: 3,
        backoff_base: Duration::from_secs(60),
        walk_bound: WALK_BOUND,
    }
}

fn scheduler(db: &DatabaseConnection, settings: SchedulerSettings) -> PgScheduler {
    PgScheduler::new(db.clone(), Arc::new(BundledTzResolver::new()), settings)
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

/// An active follow-up that was due `secs` seconds ago, at a whole second.
async fn due_follow_up(env: &Env, ib: &Ib, secs: i64) -> Uuid {
    let id = Uuid::new_v4();
    support::timer::insert_follow_up_row(&env.db, ib.uuid, id, "agent", &format!("date_trunc('second', now() - interval '{secs} seconds')"), "check the build").await;
    id
}

/// An active schedule with the given stored slot (a UTC civil time) and a due `next_fire_at`.
async fn due_schedule(env: &Env, ib: &Ib, cron: &str, tz: &str, slot: &str, at: Timestamp) -> Uuid {
    let id = Uuid::new_v4();
    support::timer::insert_schedule_row(&env.db, ib.uuid, id, cron, tz, slot, &format!("to_timestamp({})", at.as_second())).await;
    id
}

fn civil_text(dt: civil::DateTime) -> String {
    dt.strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

async fn db_now(env: &Env) -> Timestamp {
    PgTimerStore::new(env.db.clone(), Arc::new(BundledTzResolver::new())).db_now().await.expect("now")
}

async fn exec(env: &Env, sql: &str) {
    env.db.execute_unprepared(sql).await.expect("exec");
}

async fn events_like(env: &Env, ib: &Ib, ce_id_prefix: &str) -> i64 {
    support::scalar_i64(
        &env.db,
        &format!("SELECT count(*)::bigint AS n FROM inbox_event WHERE inbox_id = '{}' AND ce_id LIKE '{ce_id_prefix}%'", ib.uuid),
    )
    .await
}

#[tokio::test]
async fn restart_fires_a_missed_daily_slot_once() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let tz = BundledTzResolver::new();
    let berlin = tz.resolve("Europe/Berlin").expect("zone");
    let now = db_now(&env).await;
    let local_now = now.to_zoned(berlin.clone());
    let slot_civil = local_now.date().checked_sub(Span::new().days(2)).expect("date").at(9, 0, 0, 0);
    let schedule = CronSchedule::parse("0 9 * * *", "Europe/Berlin", &tz).expect("schedule");
    let stored_at = schedule.instant(Slot { civil: slot_civil, second: false }).expect("instant");
    let id = due_schedule(&env, &ib, "0 9 * * *", "Europe/Berlin", &civil_text(slot_civil), stored_at).await;

    let today_nine = local_now.with().time(civil::time(9, 0, 0, 0)).build().expect("zoned");
    let want_next = if today_nine.timestamp() > now {
        today_nine
    } else {
        today_nine.checked_add(Span::new().days(1)).expect("zoned")
    };

    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    let events = support::timer::timer_events(&env.db, ib.uuid).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].ce_id, format!("timer:{id}:{}", format_fire_at(stored_at)));
    assert_eq!(events[0].source, SOURCE_AGENT);
    assert_eq!(events[0].ce_type, TIMER_FIRED_TYPE);
    assert_eq!(events[0].status, "pending");
    let state = support::timer::timer_state(&env.db, id).await;
    assert_eq!((state.status.as_str(), state.fire_count, state.fire_attempts), ("active", 1, 0));
    assert_eq!(state.next_fire_at_us, want_next.timestamp().as_microsecond());
    assert_eq!(state.next_slot.as_deref(), Some(civil_text(want_next.datetime()).as_str()));
    assert_eq!(sched.fire_due(10).await.expect("pass"), 0);
}

#[tokio::test]
async fn a_follow_up_fires_once_and_is_done() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let id = due_follow_up(&env, &ib, 30).await;
    exec(&env, &format!("UPDATE timer SET thread_id = '{}' WHERE id = '{id}'", ib.thread)).await;

    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    let events = support::timer::timer_events(&env.db, ib.uuid).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].thread_id, Some(ib.thread));
    let state = support::timer::timer_state(&env.db, id).await;
    assert_eq!((state.status.as_str(), state.fire_count), ("done", 1));
    assert_eq!(sched.fire_due(10).await.expect("pass"), 0);
}

/// Sends the backend pid of the fire transaction, then waits for ever.
struct CrashHook(Mutex<Option<tokio::sync::oneshot::Sender<i64>>>);

#[async_trait]
impl AfterInsertHook for CrashHook {
    async fn after_insert(&self, txn: &DatabaseTransaction) {
        let row = txn
            .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, "SELECT pg_backend_pid()::bigint AS n".to_string()))
            .await
            .expect("pid")
            .expect("row");
        let pid: i64 = row.try_get("", "n").expect("n");
        if let Some(tx) = self.0.lock().expect("lock").take() {
            let _ = tx.send(pid);
        }
        std::future::pending::<()>().await;
    }
}

#[tokio::test]
async fn crash_between_insert_and_commit() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let id = due_follow_up(&env, &ib, 30).await;
    let at_us = support::timer::timer_state(&env.db, id).await.next_fire_at_us;
    let want_ce_id = format!("timer:{id}:{}", format_fire_at(Timestamp::from_microsecond(at_us).expect("ts")));

    let (tx, rx) = tokio::sync::oneshot::channel();
    let pool_a = support::connect(&env.url).await;
    let a = scheduler(&pool_a, settings()).with_after_insert_hook(Arc::new(CrashHook(Mutex::new(Some(tx)))));
    let task = tokio::spawn(async move { a.fire_due(10).await });
    let pid = tokio::time::timeout(Duration::from_secs(20), rx).await.expect("the hook ran").expect("pid");

    exec(&env, &format!("SELECT pg_terminate_backend({pid})")).await;
    task.abort();
    let _ = task.await;
    // The termination is asynchronous: wait until the backend is gone and its row lock with it.
    for _ in 0..100 {
        if support::scalar_i64(&env.db, &format!("SELECT count(*)::bigint AS n FROM pg_stat_activity WHERE pid = {pid}")).await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(support::timer::timer_events(&env.db, ib.uuid).await.len(), 0, "the insert of A never committed");

    let pool_b = support::connect(&env.url).await;
    let b = scheduler(&pool_b, settings());
    assert_eq!(b.fire_due(10).await.expect("pass"), 1);
    let events = support::timer::timer_events(&env.db, ib.uuid).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].ce_id, want_ce_id);
    assert_eq!(support::timer::timer_state(&env.db, id).await.fire_count, 1);
}

#[tokio::test]
async fn skip_locked_leaves_a_locked_row() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let older = due_follow_up(&env, &ib, 600).await;
    let newer = due_follow_up(&env, &ib, 300).await;

    let holder = env.db.begin().await.expect("begin");
    holder
        .execute_unprepared(&format!("SELECT 1 FROM timer WHERE id = '{older}' FOR UPDATE"))
        .await
        .expect("hold the older row");

    let sched = scheduler(&env.db, settings());
    let result = tokio::time::timeout(Duration::from_secs(5), sched.fire_due(10)).await;
    holder.rollback().await.expect("release");
    assert_eq!(result.expect("the pass did not wait for the locked row").expect("pass"), 1);
    assert_eq!(support::timer::timer_state(&env.db, newer).await.status, "done");
    let state = support::timer::timer_state(&env.db, older).await;
    assert_eq!((state.status.as_str(), state.fire_count, state.fire_attempts), ("active", 0, 0));
}

/// Tells the test that the insert happened, then holds the transaction open until released.
struct HoldHook {
    reached: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Notify,
}

#[async_trait]
impl AfterInsertHook for HoldHook {
    async fn after_insert(&self, _txn: &DatabaseTransaction) {
        if let Some(tx) = self.reached.lock().expect("lock").take() {
            let _ = tx.send(());
        }
        self.release.notified().await;
    }
}

#[tokio::test]
async fn ingress_does_not_wait_for_a_fire() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    due_follow_up(&env, &ib, 30).await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let hook = Arc::new(HoldHook {
        reached: Mutex::new(Some(tx)),
        release: tokio::sync::Notify::new(),
    });
    let a = scheduler(&env.db, settings()).with_after_insert_hook(hook.clone());
    let task = tokio::spawn(async move { a.fire_due(10).await });
    tokio::time::timeout(Duration::from_secs(20), rx).await.expect("the hook ran").expect("signal");

    let repo = PgInboxRepository::new(env.db.clone());
    let event = NewInboxEvent {
        id: EventId::from_uuid(Uuid::new_v4()),
        inbox: ib.id.clone(),
        source: "urn:test".to_string(),
        ce_id: "ingress-1".to_string(),
        ce_type: "test.event".to_string(),
        thread: None,
        envelope: json!({"data": {}}),
    };
    let outcome = tokio::time::timeout(Duration::from_secs(1), repo.insert_event(event)).await;
    hook.release.notify_one();
    assert_eq!(task.await.expect("join").expect("pass"), 1);
    outcome.expect("ingress waited for the fire transaction (AG7)").expect("insert");
}

#[tokio::test]
async fn a_double_fire_is_a_matching_duplicate() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let now = db_now(&env).await;
    let slot = (now - SignedDuration::from_hours(3)).to_zoned(TimeZone::UTC).datetime();
    let slot = slot.with().minute(0).second(0).subsec_nanosecond(0).build().expect("slot");
    let at = slot.to_zoned(TimeZone::UTC).expect("zoned").timestamp();
    let id = due_schedule(&env, &ib, "0 * * * *", "UTC", &civil_text(slot), at).await;

    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    assert_eq!(support::lease::counter(&recorder, names::AGENT_HOST_TIMER_FIRED_TOTAL, Some(("outcome", "inserted"))), 1);

    exec(
        &env,
        &format!(
            "UPDATE timer SET next_slot = '{}'::timestamp, next_fire_at = to_timestamp({}) WHERE id = '{id}'",
            civil_text(slot),
            at.as_second()
        ),
    )
    .await;
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    assert_eq!(events_like(&env, &ib, &format!("timer:{id}:{}", format_fire_at(at))).await, 1, "one event for the ce_id");
    assert_eq!(support::lease::counter(&recorder, names::AGENT_HOST_TIMER_FIRED_TOTAL, Some(("outcome", "duplicate"))), 1);
    let state = support::timer::timer_state(&env.db, id).await;
    assert_eq!(state.fire_count, 2);
    assert!(state.next_fire_at_us > now.as_microsecond(), "the row advanced again");
}

#[tokio::test]
async fn missed_slots_fire_once() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let now = db_now(&env).await;
    let back = (now - SignedDuration::from_hours(6)).as_second();
    let at = Timestamp::from_second(back - back.rem_euclid(900)).expect("ts");
    let slot = at.to_zoned(TimeZone::UTC).datetime();
    let id = due_schedule(&env, &ib, "*/15 * * * *", "UTC", &civil_text(slot), at).await;

    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    let events = support::timer::timer_events(&env.db, ib.uuid).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].ce_id, format!("timer:{id}:{}", format_fire_at(at)));
    let state = support::timer::timer_state(&env.db, id).await;
    assert!(state.next_fire_at_us > now.as_microsecond(), "next_fire_at is after now");
    assert_eq!(sched.fire_due(10).await.expect("pass"), 0);
}

#[tokio::test]
async fn an_unknown_zone_backs_off_then_fails() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let now = db_now(&env).await;
    let bad = due_schedule(&env, &ib, "*/15 * * * *", "Mars/Olympus", "2030-01-01T09:00:00", now - SignedDuration::from_secs(600)).await;
    exec(&env, &format!("UPDATE timer SET note = 'SECRET-NOTE' WHERE id = '{bad}'")).await;
    let good = due_follow_up(&env, &ib, 30).await;

    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let sched = scheduler(&env.db, SchedulerSettings { max_fire_attempts: 2, ..settings() });
    assert_eq!(sched.fire_due(10).await.expect("pass"), 2);
    assert_eq!(support::timer::timer_state(&env.db, good).await.status, "done");
    let state = support::timer::timer_state(&env.db, bad).await;
    assert_eq!((state.status.as_str(), state.fire_attempts), ("active", 1));
    assert!(state.next_fire_at_us > now.as_microsecond(), "backed off to a later time");

    exec(&env, &format!("UPDATE timer SET next_fire_at = now() - interval '1 second' WHERE id = '{bad}'")).await;
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    let state = support::timer::timer_state(&env.db, bad).await;
    assert_eq!((state.status.as_str(), state.fire_attempts), ("failed", 2));
    let failure = state.failure.expect("a failure text");
    assert!(failure.contains("unknown time zone"), "{failure}");
    assert!(!failure.contains("SECRET-NOTE"));
    assert_eq!(support::lease::counter(&recorder, names::AGENT_HOST_TIMER_FAILED_TOTAL, None), 1);
    assert_eq!(events_like(&env, &ib, &format!("timer:{bad}:")).await, 0);
}

#[tokio::test]
async fn a_low_walk_bound_is_permanent() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let now = db_now(&env).await;
    let at = Timestamp::from_second(now.as_second() - 365 * 86_400).expect("ts");
    let slot = at.to_zoned(TimeZone::UTC).datetime();
    let id = due_schedule(&env, &ib, "*/15 * * * *", "UTC", &civil_text(slot), at).await;

    let sched = scheduler(&env.db, SchedulerSettings { walk_bound: 1, ..settings() });
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1);
    let state = support::timer::timer_state(&env.db, id).await;
    assert_eq!((state.status.as_str(), state.fire_attempts), ("active", 1));
    assert_eq!(events_like(&env, &ib, "timer:").await, 0);
}

#[tokio::test]
async fn bigint_overflow_is_permanent() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let bad = due_follow_up(&env, &ib, 600).await;
    exec(&env, &format!("UPDATE timer SET fire_count = 9223372036854775807 WHERE id = '{bad}'")).await;
    let good = due_follow_up(&env, &ib, 300).await;

    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.fire_due(10).await.expect("pass"), 2);
    assert_eq!(events_like(&env, &ib, &format!("timer:{bad}:")).await, 0, "the insert rolled back");
    let state = support::timer::timer_state(&env.db, bad).await;
    assert_eq!((state.status.as_str(), state.fire_attempts), ("active", 1));
    assert_eq!(support::timer::timer_state(&env.db, good).await.status, "done");
}

#[tokio::test]
async fn a_statement_timeout_is_transient_and_stops_the_pass() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let first = due_follow_up(&env, &ib, 600).await;
    let second = due_follow_up(&env, &ib, 300).await;
    let before_first = support::timer::timer_state(&env.db, first).await;
    let before_second = support::timer::timer_state(&env.db, second).await;
    let ce_id = format!("timer:{first}:{}", format_fire_at(Timestamp::from_microsecond(before_first.next_fire_at_us).expect("ts")));

    // An uncommitted event with the key of the first row: the insert of the scheduler waits on it.
    let blocker = env.db.begin().await.expect("begin");
    blocker
        .execute_unprepared(&format!(
            "INSERT INTO inbox_event (id, inbox_id, source, ce_id, ce_type, envelope) VALUES ('{}', '{}', '{SOURCE_AGENT}', '{ce_id}', '{TIMER_FIRED_TYPE}', '{{}}'::jsonb)",
            Uuid::new_v4(),
            ib.uuid
        ))
        .await
        .expect("blocking insert");

    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let sched = scheduler(
        &env.db,
        SchedulerSettings {
            statement_timeout: Duration::from_millis(200),
            ..settings()
        },
    );
    let result = sched.fire_due(10).await;
    blocker.rollback().await.expect("release");
    assert!(result.is_err(), "a statement timeout stops the pass");
    assert_eq!(support::timer::timer_state(&env.db, first).await, before_first);
    assert_eq!(support::timer::timer_state(&env.db, second).await, before_second);
    assert_eq!(support::lease::counter(&recorder, names::AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL, Some(("class", "transient"))), 1);
}

#[tokio::test]
async fn a_schedule_over_the_budget_still_fires() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    for _ in 0..5 {
        let id = Uuid::new_v4();
        support::timer::insert_follow_up_row(&env.db, ib.uuid, id, "agent", "now() + interval '1 day'", "later").await;
    }
    let now = db_now(&env).await;
    let at = Timestamp::from_second(now.as_second() - 3600).expect("ts");
    let slot = at.to_zoned(TimeZone::UTC).datetime();
    let id = due_schedule(&env, &ib, "0 * * * *", "UTC", &civil_text(slot), at).await;

    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.fire_due(10).await.expect("pass"), 1, "no budget applies at fire time");
    assert_eq!(support::timer::timer_state(&env.db, id).await.fire_count, 1);
}

#[tokio::test]
async fn due_lag_is_zero_with_nothing_due_and_positive_with_a_backlog() {
    let Some(env) = env().await else {
        eprintln!("skipping scheduler_pg: Docker unavailable");
        return;
    };
    let ib = new_inbox(&env).await;
    let sched = scheduler(&env.db, settings());
    assert_eq!(sched.due_lag().await.expect("lag"), Duration::ZERO);
    support::timer::insert_follow_up_row(&env.db, ib.uuid, Uuid::new_v4(), "agent", "now() + interval '1 hour'", "later").await;
    assert_eq!(sched.due_lag().await.expect("lag"), Duration::ZERO, "a future row is not due");
    due_follow_up(&env, &ib, 120).await;
    due_follow_up(&env, &ib, 30).await;
    let lag = sched.due_lag().await.expect("lag");
    assert!(lag >= Duration::from_secs(119) && lag < Duration::from_secs(300), "the lag is that of the oldest due row: {lag:?}");
}
