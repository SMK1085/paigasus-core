// SPDX-License-Identifier: Apache-2.0

//! G7 of SMA-789, on a real Postgres. Run 1 is a real Helikon agent run with the memory tool.
//! After run 1 commits, a committed task is readable from a fresh store and a fresh workspace
//! through the run-start helpers (`run_start_context`, `render_run_start`) and `execute`. Phase 2
//! calls these helpers directly and does not run an agent: no production path calls
//! `run_start_context` yet (SMA-790). A run 1 that does not commit leaves nothing.

mod support;

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::helikon::memory_tool::{MemoryTool, run_nonce};
use paigasus_agent_host::adapters::helikon::scripted::ScriptedModel;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::adapters::persistence::migration_lock::migrate_under_lock;
use paigasus_agent_host_core::memory::conformance::fresh_scope;
use paigasus_agent_host_core::memory::layout::{render_run_start, run_start_context};
use paigasus_agent_host_core::memory::{MemoryCaps, MemoryScope, MemoryWorkspace, execute, parse_command};
use paigasus_helikon::core::{AgentEvent, AgentInput, ContentPart, Item, LlmAgent, RunConfig, RunContext, Runner};
use paigasus_helikon::runtime_tokio::TokioRunner;
use serde_json::json;
use tokio::sync::Mutex;

/// The texts of every tool result of a run, in order.
fn tool_result_texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolOutputItem {
                item: Item::ToolResult { content, .. },
            } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect()
}

/// Run 1: three tool calls through a real Helikon run. It returns the workspace, still open.
async fn run_one(url: &str, scope: &MemoryScope) -> Arc<Mutex<MemoryWorkspace>> {
    let store = Arc::new(PgMemoryStore::new(support::connect(url).await));
    let ws = Arc::new(Mutex::new(MemoryWorkspace::new(store, scope.clone(), MemoryCaps::default())));
    let model = ScriptedModel::new(vec![
        vec![(
            "c1".into(),
            "memory".into(),
            json!({"command": "create", "path": "/memories/tasks/t1.md", "file_text": "step 1 done; next: step 2"}),
        )],
        vec![(
            "c2".into(),
            "memory".into(),
            json!({"command": "create", "path": "/memories/tasks/index.md", "file_text": "- t1: in progress"}),
        )],
        vec![("c3".into(), "memory".into(), json!({"command": "view", "path": "/memories/tasks/t1.md"}))],
    ]);
    let agent = LlmAgent::builder::<()>()
        .name("g7")
        .model(model)
        .instructions("Use the memory tool.")
        .tool(MemoryTool::new(ws.clone()))
        .build();
    let result = TokioRunner
        .run(&agent, RunContext::ephemeral(()), AgentInput::from_user_text("work on t1"), RunConfig::default())
        .await
        .expect("the run completes");
    let texts = tool_result_texts(&result.events);
    assert_eq!(texts.len(), 3, "three tool results: {texts:?}");
    assert_eq!(texts[0], "File created successfully at: /memories/tasks/t1.md");
    assert_eq!(texts[1], "File created successfully at: /memories/tasks/index.md");
    assert!(texts[2].contains("step 1 done"), "view shows the file: {}", texts[2]);
    ws
}

async fn migrated_url() -> Option<(testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>, String)> {
    let (node, url) = support::start_raw_postgres().await?;
    let db = support::connect(&url).await;
    migrate_under_lock(&db, Duration::from_secs(60)).await.expect("migrate");
    Some((node, url))
}

/// Phase 2: a new store on a new connection, a new workspace. No agent runs.
async fn fresh_workspace(url: &str, scope: &MemoryScope) -> MemoryWorkspace {
    let store = Arc::new(PgMemoryStore::new(support::connect(url).await));
    MemoryWorkspace::new(store, scope.clone(), MemoryCaps::default())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_task_is_readable_from_a_fresh_store_and_workspace_through_the_run_start_helpers() {
    let Some((_node, url)) = migrated_url().await else {
        eprintln!("skipping memory_resume_pg: Docker unavailable");
        return;
    };
    let scope = fresh_scope();
    let ws1 = run_one(&url, &scope).await;
    ws1.lock().await.commit().await.expect("commit");
    drop(ws1);

    let mut ws2 = fresh_workspace(&url, &scope).await;
    let items = run_start_context(&mut ws2).await.expect("run-start context");
    let index = items.iter().find(|i| i.path.as_str() == "/tasks/index.md").expect("the index is loaded");
    assert!(index.content.contains("- t1: in progress"));
    let rendered = render_run_start(&items, &run_nonce());
    assert!(rendered.contains("- t1: in progress"), "rendered: {rendered}");
    let view = execute(&mut ws2, parse_command(json!({"command": "view", "path": "/memories/tasks/t1.md"})).expect("parse"))
        .await
        .expect("view");
    assert!(view.contains("step 1 done; next: step 2"), "view: {view}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_without_commit_leaves_nothing() {
    let Some((_node, url)) = migrated_url().await else {
        eprintln!("skipping memory_resume_pg: Docker unavailable");
        return;
    };
    let scope = fresh_scope();
    let ws1 = run_one(&url, &scope).await;
    drop(ws1);

    let mut ws2 = fresh_workspace(&url, &scope).await;
    let items = run_start_context(&mut ws2).await.expect("run-start context");
    assert!(items.is_empty(), "no item: {items:?}");
    let missing = execute(&mut ws2, parse_command(json!({"command": "view", "path": "/memories/tasks/t1.md"})).expect("parse")).await;
    match missing {
        Err(e) => assert_eq!(e.to_string(), "The path /memories/tasks/t1.md does not exist. Please provide a valid path."),
        Ok(t) => panic!("the view must report a missing path, got: {t}"),
    }
}
