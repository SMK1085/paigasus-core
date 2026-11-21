// SPDX-License-Identifier: Apache-2.0

//! The follow-up wakes the agent (SMA-788 spec §14.1, the second half of the done-when sentence).
//! The test uses the real parts: the run clock and the id source, the `schedule_follow_up` tool,
//! `seal`, `apply_in` inside a transaction that holds the `inbox_lease` row lock, and the real
//! scheduler loop at a poll interval of 1 s. The follow-up must become a pending inbox event on
//! the thread of the run, not before its due time and less than two poll intervals after it. All
//! times come from the database. This is a timing test, so its nextest block has no retry.

mod support;

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::clock::RunStartClock;
use paigasus_agent_host::adapters::helikon::timer_tools::TimerTool;
use paigasus_agent_host::adapters::id::MintIds;
use paigasus_agent_host::adapters::persistence::scheduler::{PgScheduler, SchedulerSettings};
use paigasus_agent_host::adapters::persistence::timer_store::PgTimerStore;
use paigasus_agent_host::adapters::timer_event::{SOURCE_AGENT, TIMER_FIRED_TYPE};
use paigasus_agent_host::application::scheduler_loop::{LoopSettings, run_scheduler_loop};
use paigasus_agent_host_core::ids::{InboxId, ThreadId};
use paigasus_agent_host_core::timer::{BundledTzResolver, TimerCaps, TimerDeps, TimerWorkspace, WALK_BOUND};
use paigasus_helikon::core::{CancellationToken, Tool, ToolContext, TracerHandle};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use serde_json::json;
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const WAIT_BUDGET: Duration = Duration::from_secs(20);

#[tokio::test]
async fn a_follow_up_wakes_the_agent_at_the_set_time() {
    let Some((_node, _url, db)) = support::inbox::migrated_postgres().await else {
        eprintln!("skipping timer_wake_pg: Docker unavailable");
        return;
    };
    let org = Uuid::new_v4();
    let inbox_uuid = Uuid::new_v4();
    support::inbox::insert_inbox(&db, org, inbox_uuid, Uuid::new_v4(), Uuid::new_v4()).await;
    let thread_uuid = Uuid::new_v4();
    support::lease::insert_thread(&db, inbox_uuid, thread_uuid).await;
    support::timer::ensure_lease_row(&db, inbox_uuid).await;
    let inbox = InboxId::new(org, inbox_uuid);

    let tz = Arc::new(BundledTzResolver::new());
    let store = Arc::new(PgTimerStore::new(db.clone(), tz.clone()));
    let clock = RunStartClock::new(store.db_now().await.expect("db now"));
    let caps = TimerCaps {
        min_follow_up_delay_secs: 1,
        ..TimerCaps::default()
    };
    let deps = TimerDeps {
        store: store.clone(),
        tz: tz.clone(),
        ids: Arc::new(MintIds),
        clock: Arc::new(clock),
    };
    let ws = Arc::new(Mutex::new(TimerWorkspace::new(
        inbox.clone(),
        ThreadId::from_uuid(thread_uuid),
        caps,
        Duration::from_secs(86_400),
        deps,
    )));
    let tool = TimerTool::new(paigasus_agent_host_core::timer::TimerToolName::ScheduleFollowUp, ws.clone());

    // The run is 2 s old when the agent calls the tool. The due time must come from the time of the
    // call, not from the start of the run.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let ctx = ToolContext::new(Arc::new(()), TracerHandle::default(), CancellationToken::new(), 0, 3);
    let reply = Tool::<()>::invoke(&tool, &ctx, json!({"in_seconds": 2, "note": "check the reply"})).await.expect("the tool answers");
    let text = reply.content.as_str().expect("a text reply").to_owned();
    assert!(!text.to_lowercase().contains("refus"), "the tool refused: {text}");

    // The run commit: the change set goes into the transaction that holds the lease row lock.
    let changes = ws.lock().await.seal();
    let txn = db.begin().await.expect("begin");
    support::timer::lock_lease_row(&txn, inbox_uuid).await;
    let report = store.apply_in(&txn, &inbox, &caps, changes).await.expect("apply_in");
    txn.commit().await.expect("commit");
    assert!(report.cancels.is_empty(), "the change set holds no cancel: {report:?}");
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT id::text AS v FROM timer WHERE inbox_id = '{inbox_uuid}'"),
        ))
        .await
        .expect("timer id query")
        .expect("one timer row");
    let timer_id: Uuid = row.try_get::<String>("", "v").expect("id").parse().expect("uuid");

    let before = support::timer::timer_state(&db, timer_id).await;
    assert_eq!(before.status, "active");
    let due_us = before.next_fire_at_us;

    let scheduler = Arc::new(PgScheduler::new(
        db.clone(),
        tz,
        SchedulerSettings {
            statement_timeout: Duration::from_secs(5),
            max_fire_attempts: 3,
            backoff_base: Duration::from_secs(60),
            walk_bound: WALK_BOUND,
        },
    ));
    let (stop, shutdown) = watch::channel(());
    let looped = tokio::spawn(run_scheduler_loop(
        scheduler,
        LoopSettings {
            poll_interval: POLL_INTERVAL,
            pass_limit: 100,
        },
        shutdown,
    ));

    let deadline = Instant::now() + WAIT_BUDGET;
    let events = loop {
        let events = support::timer::timer_events(&db, inbox_uuid).await;
        if !events.is_empty() {
            break events;
        }
        assert!(Instant::now() < deadline, "no event after {WAIT_BUDGET:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    assert_eq!(events.len(), 1, "exactly one event for the timer: {events:?}");
    let event = &events[0];
    assert_eq!(event.status, "pending");
    assert_eq!(event.thread_id, Some(thread_uuid), "the event is on the thread of the run");
    assert_eq!(event.ce_type, TIMER_FIRED_TYPE);
    assert_eq!(event.source, SOURCE_AGENT);
    assert!(event.received_us >= due_us, "the event came {} us before its due time", due_us - event.received_us);
    let late = Duration::from_micros(u64::try_from(event.received_us - due_us).expect("not early"));
    assert!(late < 2 * POLL_INTERVAL, "the event came {late:?} after its due time; the limit is two poll intervals");
    assert_eq!(support::timer::timer_state(&db, timer_id).await.status, "done");

    stop.send(()).expect("the loop still listens");
    let ended = tokio::time::timeout(Duration::from_secs(10), looped).await.expect("the loop ends on shutdown").expect("the loop task");
    assert!(ended.is_ok(), "the loop returns Ok on shutdown: {ended:?}");
}
