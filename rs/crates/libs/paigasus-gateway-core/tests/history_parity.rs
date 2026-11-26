// SPDX-License-Identifier: Apache-2.0
//! The parity replay of the Dogwood decision corpus (SMA-753 spec P15). Each case of
//! `rs/tools/paigasus-policy-compiler/corpus/*.json` runs through the production chain: the
//! compiled conditions as JSON, the IAM protocol `HistoryCondition`, `condition_to_pb`, the
//! gateway-core proto mapping, `HistoryPolicy::build`, the kernel evaluator and
//! `decide_with_slots`. The replay requires the Dogwood decision of each request event, or the
//! `paigasus` decision where a `paigasus_differs` entry names the event.
//!
//! The corpus is data only: this test does not depend on the policy compiler crate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use paigasus_gateway_core::{HistoryPolicy, map_conditions};
use paigasus_iam_core::tool_policy::condition_to_pb;
use paigasus_iam_core::tool_policy::protocol::HistoryCondition;
use paigasus_kernel::CedarUid;
use paigasus_kernel::tool_policy::history::{Catalog, Digest, Event, EventKind, FieldPath, HistoryDigester, KeyId, Part, Scalar, Scope, SetTool, Window, WindowSet, evaluate, extract_fields};
use paigasus_kernel::tool_policy::{Declared, Effect, Facts, Findings, RequestInfo, SessionInfo, ToolInput, ToolInvocation, ToolPolicies, decide_with_slots, normalise};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

/// The longest window that the replay loads (`MAX_WINDOW_MS`, one day).
const DAY_MS: u64 = 86_400_000;

/// The fixed org of the replay. The digest input holds it (spec P3).
const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);

/// A keyed SHA-256 digester with one fixed key: `SHA-256(key || org || 0 || path || 0 || tag ||
/// value)`, cut to 16 bytes. The test needs a stable keyed digest, not the production HMAC.
struct FixedKeyDigester {
    key: [u8; 32],
    kid: KeyId,
}

impl FixedKeyDigester {
    fn new() -> Self {
        let key = [0x5a; 32];
        let kid_hash = Sha256::new().chain_update(key).chain_update(b"paigasus-history-kid").finalize();
        let mut kid = [0u8; 8];
        kid.copy_from_slice(&kid_hash[..8]);
        Self { key, kid: KeyId::new(kid) }
    }

    fn digest(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Digest {
        let hash = Sha256::new()
            .chain_update(self.key)
            .chain_update(org.as_hyphenated().to_string())
            .chain_update([0])
            .chain_update(path.joined())
            .chain_update([0])
            .chain_update(value.canonical_bytes())
            .finalize();
        let mut out = [0u8; 16];
        out.copy_from_slice(&hash[..16]);
        Digest::new(out)
    }
}

impl HistoryDigester for FixedKeyDigester {
    fn current(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)> {
        Some((self.kid, self.digest(org, path, value)))
    }

    fn all(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)> {
        vec![(self.kid, self.digest(org, path, value))]
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/corpus")
}

fn corpus_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .expect("the corpus directory")
        .map(|entry| entry.expect("a corpus entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

fn load(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("a corpus file")).expect("corpus JSON")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("`{key}` is a string"))
}

/// The scope value of an event: the session (one per case), the principal or the API key.
fn scope_value(event: &Value, scope: Scope) -> Option<&str> {
    match scope {
        Scope::Session => Some("session"),
        Scope::Principal => event["principal"].as_str(),
        Scope::Key => event["api_key"].as_str(),
        _ => panic!("a scope that the replay does not know"),
    }
}

/// `Pgs::Iam::Principal::"p1"` as a Cedar uid.
fn principal(event: &Value) -> CedarUid {
    let uid = text(event, "principal");
    let (entity_type, id) = uid.split_once("::\"").expect("a Cedar uid");
    CedarUid {
        entity_type: entity_type.to_owned(),
        entity_id: id.strip_suffix('"').expect("a quoted id").to_owned(),
    }
}

fn kind(event: &Value) -> EventKind {
    match text(event, "kind") {
        "request" => EventKind::Request,
        "response" => EventKind::Response,
        other => panic!("an unknown event kind {other}"),
    }
}

fn ts_ms(event: &Value) -> u64 {
    event["ts"].as_u64().expect("`ts` in seconds") * 1000
}

/// One corpus case, loaded through the production chain.
struct Replay {
    name: String,
    case: Value,
    history: HistoryPolicy,
    policies: ToolPolicies,
    digester: FixedKeyDigester,
}

impl Replay {
    fn load(path: &Path) -> Self {
        let case = load(path);
        let name = path.file_stem().expect("a file name").to_string_lossy().into_owned();
        let compiled = &case["compiled"];
        let conditions: Vec<HistoryCondition> = serde_json::from_value(compiled["conditions"].clone()).unwrap_or_else(|e| panic!("{name}: the conditions: {e}"));
        let pb: Vec<_> = conditions.iter().map(condition_to_pb).collect();
        let mapped = map_conditions(Part::Tenant, &pb, DAY_MS, &BTreeSet::new()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let catalog: Vec<String> = compiled["cedar_schema_json"]["Pgs::Gateway"]["actions"]
            .as_object()
            .expect("the schema actions")
            .keys()
            .filter(|action| action.as_str() != "InvokeTool")
            .cloned()
            .collect();
        let digester = FixedKeyDigester::new();
        let history = HistoryPolicy::build(&[], &mapped, ORG, Catalog::known(catalog.iter().cloned()), None, &digester);
        let floor = Arc::new(ToolPolicies::compile_floor("").expect("the empty floor"));
        let policies = ToolPolicies::compile(floor, text(compiled, "cedar_policies"), &catalog).unwrap_or_else(|e| panic!("{name}: the policies: {e}"));
        Self {
            name,
            case,
            history,
            policies,
            digester,
        }
    }

    fn events(&self) -> &[Value] {
        self.case["events"].as_array().expect("events")
    }

    /// The stored event `i`: the planned fields of its arguments, digested with the fixed key.
    fn event(&self, i: usize) -> Event {
        let event = &self.events()[i];
        let tool = normalise(text(event, "tool"));
        let input = ToolInput::Json(text(event, "arguments").to_owned());
        let (fields, kid) = extract_fields(&input, &self.history.fields().paths_for(&tool)).into_fields(ORG, &self.digester);
        Event::new(Uuid::from_u128(i as u128 + 1), ts_ms(event), kind(event), tool, fields, kid)
    }

    /// The store reply for event `i`: one set for each read set, with the earlier events of the
    /// same scope value and tool.
    fn window(&self, i: usize) -> Window {
        let current = &self.events()[i];
        let Some(read) = self.history.read() else {
            return Window::new(Vec::new());
        };
        let sets = read
            .sets
            .iter()
            .map(|set| {
                let events = (0..i)
                    .filter(|&j| scope_value(&self.events()[j], set.scope) == scope_value(current, set.scope))
                    .map(|j| self.event(j))
                    .filter(|e| match &set.tool {
                        SetTool::Tool(name) => &e.tool == name,
                        _ => true,
                    })
                    .collect();
                WindowSet::new(set.scope, set.tool.clone(), events, None, false)
            })
            .collect();
        Window::new(sets)
    }

    /// The gateway decision of the request event `i`.
    fn decide(&self, i: usize) -> &'static str {
        let event = &self.events()[i];
        let now_ms = ts_ms(event);
        let evaluation = evaluate(self.history.conditions(), &self.window(i), &[self.event(i)], 0, now_ms, self.history.catalog());
        let facts = Facts::new(principal(event), ORG, RequestInfo::new("prn:test", "chat_completions", "model"), SessionInfo::new("session", 1));
        let call = ToolInvocation::new(text(event, "tool"), ToolInput::Json(text(event, "arguments").to_owned()), Declared::Yes);
        match decide_with_slots(&call, &facts, &Findings::new(), &evaluation.slots, &self.policies).decision {
            Effect::Allow => "allow",
            _ => "deny",
        }
    }

    /// The decision that the corpus requires for event `i`: the `paigasus` value of a
    /// `paigasus_differs` entry that names it, else the Dogwood decision. `None` for a response.
    fn expected(&self, i: usize) -> Option<String> {
        let differs = self.case["paigasus_differs"].as_array().expect("paigasus_differs");
        if let Some(entry) = differs.iter().find(|d| d["event"].as_u64() == Some(i as u64)) {
            return Some(text(entry, "paigasus").to_owned());
        }
        self.case["decisions"][i].as_str().map(str::to_owned)
    }

    /// `(event, gateway decision, expected decision)` for each request event.
    fn replay(&self) -> Vec<(usize, &'static str, String)> {
        (0..self.events().len())
            .filter(|&i| kind(&self.events()[i]) == EventKind::Request)
            .map(|i| {
                let expected = self.expected(i).unwrap_or_else(|| panic!("{} event {i}: a request event has a decision", self.name));
                (i, self.decide(i), expected)
            })
            .collect()
    }
}

#[test]
fn every_corpus_decision_replays_through_the_production_chain() {
    let files = corpus_files();
    assert!(files.len() >= 23, "the corpus of Task 16");
    let mut failures = Vec::new();
    let mut replayed = 0;
    for f in files {
        let replay = Replay::load(&f);
        for (i, got, expected) in replay.replay() {
            replayed += 1;
            if got != expected {
                failures.push(format!("{} event {i}: the gateway decides {got}, the corpus requires {expected}", replay.name));
            }
        }
    }
    assert!(replayed >= 80, "every request event of the corpus replays ({replayed})");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The response events of the corpus carry no decision, and a response event is in the window
/// of a later request (the copied-fields case).
#[test]
fn a_response_event_has_no_decision_and_counts_in_a_later_window() {
    let replay = Replay::load(&corpus_dir().join("response_copies_fields.json"));
    assert_eq!(kind(&replay.events()[1]), EventKind::Response);
    assert_eq!(replay.expected(1), None);
    assert_eq!(replay.decide(2), "allow", "the response with to = x is in the window of event 2");
}

/// Carry (a) of the Task 16 review: the kernel reads the argument `-0.0` as `Long(0)`, so the
/// decimal literal `0.0` is unknown on it and the call is denied, as `decimal_edges` states.
#[test]
fn the_kernel_reads_negative_zero_as_a_long() {
    let replay = Replay::load(&corpus_dir().join("decimal_edges.json"));
    assert_eq!(text(&replay.events()[1], "arguments"), "{\"price\": -0.0}");
    let path = FieldPath::new(vec!["price".to_owned()]).expect("a path");
    let extracted = extract_fields(&ToolInput::Json("{\"price\": -0.0}".to_owned()), &[path.clone()].into());
    assert_eq!(format!("{:?}", extracted.get(&path)), "Some(Extracted::Long(..))");
    assert_eq!(replay.decide(1), "deny");
    assert_eq!(replay.expected(1).as_deref(), Some("deny"));
}

/// Carry (b) of the Task 16 review: the replay keys the `key` and `principal` sets on the
/// `api_key` and the `principal` of each event. With the earlier events under another key (or
/// principal), the third call sees an empty window and is allowed.
#[test]
fn the_key_and_principal_scopes_read_only_events_of_the_same_value() {
    for (file, member, other) in [("scope_key.json", "api_key", "k2"), ("scope_principal.json", "principal", "Pgs::Iam::Principal::\"p2\"")] {
        let mut replay = Replay::load(&corpus_dir().join(file));
        assert_eq!(replay.decide(2), "deny", "{file}: two earlier calls of the same {member}");
        for i in 0..2 {
            replay.case["events"][i][member] = Value::String(other.to_owned());
        }
        assert_eq!(replay.decide(2), "allow", "{file}: the earlier calls have another {member}");
    }
}

#[test]
fn the_predicate_depth_limit_matches_iam() {
    assert_eq!(paigasus_gateway_core::MAX_PREDICATE_DEPTH, paigasus_iam_core::tool_policy::MAX_PREDICATE_DEPTH);
}
