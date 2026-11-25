// SPDX-License-Identifier: Apache-2.0

//! The session history store contract suite (SMA-753) on `MemorySessionStore`.

mod support;

use std::sync::Arc;

use paigasus_gateway::adapters::history::memory::MemorySessionStore;
use paigasus_gateway::domain::tool_history::{Scope, SessionStore, StoredEvent};
use support::history_contract::{self as contract, Harness};
use uuid::Uuid;

struct MemoryHarness {
    store: Arc<MemorySessionStore>,
}

#[async_trait::async_trait]
impl Harness for MemoryHarness {
    async fn store(&self) -> Arc<dyn SessionStore> {
        self.store.clone()
    }

    async fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent> {
        self.store.events(org, scope, scope_id, tool)
    }

    async fn ttls_ms(&self, org: Uuid, now_ms: u64) -> Vec<Option<u64>> {
        self.store.ttls_ms(org, now_ms)
    }

    async fn put_raw_body(&self, org: Uuid, id: Uuid, body: &str) {
        self.store.put_raw_body(org, id, body).expect("a body that decodes");
    }
}

fn harness() -> MemoryHarness {
    MemoryHarness {
        store: Arc::new(MemorySessionStore::new()),
    }
}

#[tokio::test]
async fn a_reserve_appends_one_pending_event_per_history() {
    contract::a_reserve_appends_one_pending_event_per_history(&harness()).await;
}

#[tokio::test]
async fn a_caller_without_a_key_writes_the_principal_history_once() {
    contract::a_caller_without_a_key_writes_the_principal_history_once(&harness()).await;
}

#[tokio::test]
async fn the_split_by_tool_keeps_other_tools() {
    contract::the_split_by_tool_keeps_other_tools(&harness()).await;
}

#[tokio::test]
async fn retention_trims_only_events_older_than_25_hours() {
    contract::retention_trims_only_events_older_than_25_hours(&harness()).await;
}

#[tokio::test]
async fn the_cap_keeps_the_newest_10_000_events_per_tool() {
    contract::the_cap_keeps_the_newest_10_000_events_per_tool(&harness()).await;
}

#[tokio::test]
async fn the_truncated_flag_needs_a_lost_event_inside_the_window() {
    contract::the_truncated_flag_needs_a_lost_event_inside_the_window(&harness()).await;
}

#[tokio::test]
async fn settle_sets_the_verdict_and_writes_the_call_link() {
    contract::settle_sets_the_verdict_and_writes_the_call_link(&harness()).await;
}

#[tokio::test]
async fn settle_twice_is_idempotent() {
    contract::settle_twice_is_idempotent(&harness()).await;
}

#[tokio::test]
async fn a_deny_settle_writes_no_call_link() {
    contract::a_deny_settle_writes_no_call_link(&harness()).await;
}

#[tokio::test]
async fn a_lost_settle_leaves_the_event_pending_and_no_call_link() {
    contract::a_lost_settle_leaves_the_event_pending_and_no_call_link(&harness()).await;
}

#[tokio::test]
async fn a_settle_for_an_unknown_event_writes_nothing() {
    contract::a_settle_for_an_unknown_event_writes_nothing(&harness()).await;
}

#[tokio::test]
async fn the_reserve_writes_decision_links() {
    contract::the_reserve_writes_decision_links(&harness()).await;
}

#[tokio::test]
async fn links_are_isolated_per_principal_and_org() {
    contract::links_are_isolated_per_principal_and_org(&harness()).await;
}

#[tokio::test]
async fn a_result_event_is_written_once_per_call_id() {
    contract::a_result_event_is_written_once_per_call_id(&harness()).await;
}

#[tokio::test]
async fn a_write_without_results_and_without_turn_writes_nothing() {
    contract::a_write_without_results_and_without_turn_writes_nothing(&harness()).await;
}

#[tokio::test]
async fn touch_keeps_the_latest_activity_and_the_highest_turn() {
    contract::touch_keeps_the_latest_activity_and_the_highest_turn(&harness()).await;
}

#[tokio::test]
async fn a_future_event_survives_a_reserve_with_an_older_clock() {
    contract::a_future_event_survives_a_reserve_with_an_older_clock(&harness()).await;
}

#[tokio::test]
async fn every_written_key_expires_after_the_retention() {
    contract::every_written_key_expires_after_the_retention(&harness()).await;
}

#[tokio::test]
async fn append_writes_an_allowed_event_and_its_call_link() {
    contract::append_writes_an_allowed_event_and_its_call_link(&harness()).await;
}

#[tokio::test]
async fn a_local_shell_settle_links_the_item_id_too() {
    contract::a_local_shell_settle_links_the_item_id_too(&harness()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reserves_all_land() {
    contract::concurrent_reserves_all_land(&harness()).await;
}

#[tokio::test]
async fn a_cap_trim_in_the_principal_history_keeps_the_event_in_its_session_history() {
    contract::a_cap_trim_in_the_principal_history_keeps_the_event_in_its_session_history(&harness()).await;
}

#[tokio::test]
async fn a_cut_of_two_flagged_events_gives_one_truncated_entry_per_scope_and_tool() {
    contract::a_cut_of_two_flagged_events_gives_one_truncated_entry_per_scope_and_tool(&harness()).await;
}

#[tokio::test]
async fn a_reserve_on_a_new_session_writes_its_state() {
    contract::a_reserve_on_a_new_session_writes_its_state(&harness()).await;
}

#[tokio::test]
async fn a_reserve_with_a_lower_turn_does_not_lower_the_turn() {
    contract::a_reserve_with_a_lower_turn_does_not_lower_the_turn(&harness()).await;
}

#[tokio::test]
async fn an_append_on_a_new_session_writes_its_state() {
    contract::an_append_on_a_new_session_writes_its_state(&harness()).await;
}

#[tokio::test]
async fn a_second_allow_settle_of_a_call_id_relinks_it_to_the_new_event() {
    contract::a_second_allow_settle_of_a_call_id_relinks_it_to_the_new_event(&harness()).await;
}

#[tokio::test]
async fn the_first_settle_wins() {
    contract::the_first_settle_wins(&harness()).await;
}

#[tokio::test]
async fn a_repeated_settle_of_the_same_event_keeps_result_written() {
    contract::a_repeated_settle_of_the_same_event_keeps_result_written(&harness()).await;
}

#[tokio::test]
async fn a_result_for_a_call_event_that_is_gone_writes_nothing() {
    contract::a_result_for_a_call_event_that_is_gone_writes_nothing(&harness()).await;
}

#[tokio::test]
async fn a_later_allow_settle_of_a_denied_event_writes_no_call_link() {
    contract::a_later_allow_settle_of_a_denied_event_writes_no_call_link(&harness()).await;
}

#[tokio::test]
async fn a_local_shell_result_under_both_aliases_is_written_once() {
    contract::a_local_shell_result_under_both_aliases_is_written_once(&harness()).await;
}

#[tokio::test]
async fn a_relink_between_lookup_and_write_marks_the_original_event() {
    contract::a_relink_between_lookup_and_write_marks_the_original_event(&harness()).await;
}

#[tokio::test]
async fn a_reserved_event_keeps_its_fields_and_key_id() {
    contract::a_reserved_event_keeps_its_fields_and_key_id(&harness()).await;
}

#[tokio::test]
async fn an_unparsed_call_is_stored_as_unparsed() {
    contract::an_unparsed_call_is_stored_as_unparsed(&harness()).await;
}

#[tokio::test]
async fn a_pr1_body_reads_as_no_fields() {
    contract::a_pr1_body_reads_as_no_fields(&harness()).await;
}

#[tokio::test]
async fn a_result_event_copies_the_fields_of_its_call() {
    contract::a_result_event_copies_the_fields_of_its_call(&harness()).await;
}

#[tokio::test]
async fn the_reserve_writes_the_call_links_of_each_call() {
    contract::the_reserve_writes_the_call_links_of_each_call(&harness()).await;
}

#[tokio::test]
async fn the_reserve_keeps_a_link_of_another_event() {
    contract::the_reserve_keeps_a_link_of_another_event(&harness()).await;
}

#[tokio::test]
async fn a_deny_settle_deletes_only_its_own_link() {
    contract::a_deny_settle_deletes_only_its_own_link(&harness()).await;
}

#[tokio::test]
async fn an_allow_settle_relinks_a_reused_call_id() {
    contract::an_allow_settle_relinks_a_reused_call_id(&harness()).await;
}

#[tokio::test]
async fn an_appended_denied_call_counts_and_writes_no_link() {
    contract::an_appended_denied_call_counts_and_writes_no_link(&harness()).await;
}

#[tokio::test]
async fn an_append_under_a_reserved_id_keeps_one_event() {
    contract::an_append_under_a_reserved_id_keeps_one_event(&harness()).await;
}

#[tokio::test]
async fn a_denied_append_under_a_reserved_id_deletes_its_own_link() {
    contract::a_denied_append_under_a_reserved_id_deletes_its_own_link(&harness()).await;
}

#[tokio::test]
async fn a_result_copies_the_exact_fields_of_an_adversarial_call() {
    contract::a_result_copies_the_exact_fields_of_an_adversarial_call(&harness()).await;
}

#[tokio::test]
async fn run_all_passes_on_one_shared_store() {
    contract::run_all(&harness()).await;
}
