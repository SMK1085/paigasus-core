// SPDX-License-Identifier: Apache-2.0

//! `ToolPolicyService` against the REAL `paigasus-policy-compiler` binary (SMA-774 Task 14).
//! The other tool policy suites use a scripted compiler. These tests prove that the two sides
//! agree on the protocol.
//!
//! The binary comes from `PAIGASUS_POLICY_COMPILER_BIN`. The Moon `test` task of this crate builds
//! the binary and sets the variable. Without the variable a test prints a reason and returns.
//! With the variable set to a path that has no file, a test panics. A test never reads `CI`.
//! Docker-gated like the other Postgres suites.

mod support;

use std::path::Path;

use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::policy_compiler::probe;
use paigasus_iam::application::pagination::Page;
use paigasus_iam::application::tool_policies::WriteResult;
use paigasus_iam::config::ToolPoliciesConfig;
use paigasus_iam_core::PrincipalId;
use paigasus_iam_core::tool_policy::PolicyCompiler;
use paigasus_iam_core::tool_policy::{STARTER_TOOL_POLICY_SOURCE, ToolKind};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1 as pb;
use prost::Message;
use serde_json::{Value, json};
use uuid::Uuid;

const FLOOR_GENERIC: &str = "@id(\"floor-no-unregistered\")\nforbid (principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n";
const COUNT_POLICY: &str = "@id(\"search-rate\")\npermit (principal, action == Pgs::Gateway::Action::\"search\", resource)\nwhen temporal { count_within(1h, Pgs::Gateway::Action::\"search\"::request{ input.q: \"x\" }) <= 2 };\n";
const SEARCH_ONLY: &str = "@id(\"search-only\")\npermit (principal, action == Pgs::Gateway::Action::\"search\", resource);\n";

struct Env {
    _node: testcontainers_modules::testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    state: AppState,
    actor: Prn,
    org: Uuid,
}

impl Env {
    fn service(&self) -> &paigasus_iam::adapters::http::ToolPolicySvc {
        self.state.tool_policies.as_ref().expect("the tool policy service is on")
    }
}

/// `None` when Docker is absent (the shared skip policy). The compiler path is the caller's.
async fn setup(compiler: &Path) -> Option<Env> {
    let (node, db) = support::start_migrated_postgres().await?;
    // The probe logs its reason at ERROR. Capture the logs so that a failure names it.
    let (logs, _guard) = paigasus_logging::test_support::capture_logs();
    let (state, idp) = support::state_with_tool_policies(db, true, compiler).await;
    assert!(
        state.capabilities.tool_policies,
        "the boot probe did not accept the real compiler {}; logs:\n{}",
        compiler.display(),
        logs.text()
    );
    let token = idp.bearer("real-admin", Some("real-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &token).await;
    let actor = Prn::parse(&admin_prn).expect("the admin prn");
    let out = state.orgs.create(&PrincipalId::from_prn(actor.clone()), "acme", "Acme").await.expect("create the organization");
    let org = out.organization.id.uuid();
    Some(Env { _node: node, state, actor, org })
}

async fn put_tool(env: &Env, name: &str, schema: Option<Value>, expected: u64) -> WriteResult<paigasus_iam::application::tool_policies::CatalogAccepted> {
    env.service().put_tool(&env.actor, env.org, name, ToolKind::Function, schema, expected).await.expect("put_tool")
}

fn search_schema() -> Value {
    json!({"type": "object", "properties": {"q": {"type": "string"}}})
}

fn rejection_text<T: std::fmt::Debug>(result: WriteResult<T>) -> (Vec<String>, String) {
    match result {
        WriteResult::Rejected(rejections) => (
            rejections.iter().map(|r| r.reason.code().to_string()).collect(),
            rejections.iter().map(|r| r.message.clone()).collect::<Vec<_>>().join(" || "),
        ),
        WriteResult::Accepted(value) => panic!("expected a rejection, got {value:?}"),
    }
}

#[tokio::test]
async fn the_real_compiler_compiles_the_starter_against_an_empty_catalog() {
    let Some(path) = support::real_compiler_path("the_real_compiler_compiles_the_starter_against_an_empty_catalog") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    let WriteResult::Accepted(version) = env.service().put_set(&env.actor, env.org, STARTER_TOOL_POLICY_SOURCE, 0).await.expect("put_set") else {
        panic!("the starter was rejected")
    };
    assert_eq!(version.version, 1);
    let compiled = pb::CompiledToolPolicySet::decode(version.compiled.as_slice()).expect("decode");
    assert!(compiled.conditions.is_empty());
    assert!(compiled.catalog_names.is_empty());
}

#[tokio::test]
async fn the_real_compiler_compiles_a_generic_floor() {
    let Some(path) = support::real_compiler_path("the_real_compiler_compiles_a_generic_floor") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    let WriteResult::Accepted(version) = env.service().put_floor(&env.actor, FLOOR_GENERIC, 0).await.expect("put_floor") else {
        panic!("the generic floor was rejected")
    };
    assert_eq!(version.version, 1);
    let compiled = pb::CompiledToolPolicySet::decode(version.compiled.as_slice()).expect("decode");
    assert_eq!(compiled.kind, pb::ToolPolicySetKind::Floor as i32);
    assert!(compiled.cedar_policies.contains("floor-no-unregistered"), "{}", compiled.cedar_policies);
}

// SMA-753: IAM refuses a history rule until the gateway evaluates one. When SMA-753 lifts that,
// restore the round-trip test (slot, policy id, window and threshold of the stored condition).
#[tokio::test]
async fn a_count_within_policy_is_refused_until_the_gateway_supports_history() {
    let Some(path) = support::real_compiler_path("a_count_within_policy_is_refused_until_the_gateway_supports_history") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    assert!(matches!(put_tool(&env, "search", Some(search_schema()), 0).await, WriteResult::Accepted(_)));
    let (reasons, message) = rejection_text(env.service().put_set(&env.actor, env.org, COUNT_POLICY, 0).await.expect("put_set"));
    assert_eq!(reasons, vec!["tool-policy-history-unsupported"], "{message}");
    assert!(message.contains("policy search-rate uses a history rule"), "{message}");
}

#[tokio::test]
async fn a_catalog_delete_that_breaks_a_live_policy_is_rejected_by_the_real_compiler() {
    let Some(path) = support::real_compiler_path("a_catalog_delete_that_breaks_a_live_policy_is_rejected_by_the_real_compiler") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    assert!(matches!(put_tool(&env, "search", Some(search_schema()), 0).await, WriteResult::Accepted(_)));
    assert!(matches!(env.service().put_set(&env.actor, env.org, SEARCH_ONLY, 0).await.expect("put_set"), WriteResult::Accepted(_)));
    let (reasons, message) = rejection_text(env.service().delete_tool(&env.actor, env.org, "search", 1).await.expect("delete_tool"));
    // Dogwood reports two findings for the one policy: the unknown action and the empty scope.
    assert!(!reasons.is_empty() && reasons.iter().all(|r| r == "tool-catalog-breaks-policy"), "{reasons:?} {message}");
    assert!(message.contains("unrecognized action"), "{message}");
    // The tool is still in the catalog.
    let (tools, revision) = env.service().list_tools(&env.actor, env.org, Page { limit: 50, offset: 0 }).await.expect("list_tools");
    assert_eq!((tools.len(), revision), (1, 1));
}

#[tokio::test]
async fn the_boot_probe_reads_the_current_generation() {
    let Some(path) = support::real_compiler_path("the_boot_probe_reads_the_current_generation") else {
        return;
    };
    assert!(path.is_file(), "{}", path.display());
    let config = ToolPoliciesConfig {
        enabled: true,
        compiler_path: path,
        ..ToolPoliciesConfig::default()
    };
    let compiler = probe(&config).await.expect("the boot probe accepts the real compiler");
    assert_eq!(compiler.generation(), 3);
}

/// Saves a policy that reads a field the JSON Schema mapping dropped. Prints the real message.
async fn dropped_field_message(env: &Env, policy: &str) -> String {
    assert!(matches!(put_tool(env, "orders", Some(orders_schema()), 0).await, WriteResult::Accepted(_)));
    let (reasons, message) = rejection_text(env.service().put_set(&env.actor, env.org, policy, 0).await.expect("put_set"));
    eprintln!("REAL REJECTION reasons={reasons:?}\nREAL REJECTION message={message}");
    message
}

fn orders_schema() -> Value {
    json!({"type": "object", "properties": {
        "price": {"description": "has no type, so the mapping drops it"},
        "order": {"type": "object", "properties": {"id": {"type": "string"}, "total": {"description": "has no type"}}}
    }})
}

#[tokio::test]
async fn a_dropped_top_level_field_gets_a_note_in_the_real_rejection() {
    let Some(path) = support::real_compiler_path("a_dropped_top_level_field_gets_a_note_in_the_real_rejection") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    let policy = "@id(\"p\")\npermit (principal, action == Pgs::Gateway::Action::\"orders\", resource)\nwhen { context.input.price == 5 };\n";
    let message = dropped_field_message(&env, policy).await;
    assert!(message.contains("the field `price` of the tool `orders` is not in the schema"), "{message}");
}

#[tokio::test]
async fn a_dropped_nested_field_gets_a_note_in_the_real_rejection() {
    let Some(path) = support::real_compiler_path("a_dropped_nested_field_gets_a_note_in_the_real_rejection") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    let policy = "@id(\"p\")\npermit (principal, action == Pgs::Gateway::Action::\"orders\", resource)\nwhen { context.input.order.total == 5 };\n";
    let message = dropped_field_message(&env, policy).await;
    assert!(message.contains("the field `order.total` of the tool `orders` is not in the schema"), "{message}");
}

/// The job against the real compiler: a floor that an older generation compiled gets a new live
/// version of the same output, with the origin `recompile` (F23: the composed check).
#[tokio::test]
async fn the_job_applies_an_identical_result_with_the_real_compiler() {
    use paigasus_iam::adapters::persistence::PgToolPolicies;
    use paigasus_iam::application::tool_policies::{RecompileOutcome, RecompileRun};
    use paigasus_iam_core::tool_policy::{ToolPolicyRepository, ToolPolicySetKey, VersionOrigin};
    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    let Some(path) = support::real_compiler_path("the_job_applies_an_identical_result_with_the_real_compiler") else {
        return;
    };
    let Some(env) = setup(&path).await else { return };
    let WriteResult::Accepted(saved) = env.service().put_floor(&env.actor, FLOOR_GENERIC, 0).await.expect("put_floor") else {
        panic!("the generic floor was rejected")
    };
    assert_eq!(saved.version, 1);
    // An older compiler generation made the stored row: the run must compile it again.
    let stale = "UPDATE tool_policy_set_version SET compiler_generation = compiler_generation - 1 WHERE kind = 'floor' AND status = 'live'";
    env.state
        .db
        .execute_raw(Statement::from_string(DbBackend::Postgres, stale.to_owned()))
        .await
        .expect("make the floor stale");
    let RecompileRun::Ran(results) = env.service().run_recompile_at_boot(std::future::pending()).await.expect("run") else {
        panic!("the run did not happen")
    };
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].key, ToolPolicySetKey::Floor);
    assert_eq!(results[0].outcome, RecompileOutcome::Identical { version: 2 });
    let repo = PgToolPolicies::new(env.state.db.clone());
    let live = repo.live(&ToolPolicySetKey::Floor).await.unwrap().expect("a live floor");
    assert_eq!((live.version, live.origin, live.compiler_generation), (2, VersionOrigin::Recompile, 3));
}
