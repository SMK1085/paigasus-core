// SPDX-License-Identifier: Apache-2.0
//! Responses item assembly under the canonical rules (spec § 5, § 6.2, § 6.4, § 15).
//!
//! The decided value of a client-run item is always its `output_item.done` item (§ 5). The rules
//! make every other view a client SDK reads agree with it: the `added` item (the subset rule, A2),
//! the deltas and `*.done` events (R11–R14), and the item at the same index in the terminal
//! `output` (R9, R10). A stream that breaks a rule is a [`Deviation`], named after § 6.4.
//!
//! R5 lets one item be open at a time, so [`Assembly`] is one item-at-a-time state machine. Per
//! response it keeps the item ids and call ids (R8), the identity fields of every item (R9), and a
//! hash of every client-run `done` item (R10).
//!
//! Deltas are never retained. Each delta stream keeps a running SHA-256 hash, which is compared
//! with the hash of the `*.done` text and of the `done` item's text. So only the open item's
//! `added` item and the completed calls hold bytes ([`Assembly::retained`]). When the calls hold
//! more than the cap, they are dropped and the hold is never decided (749 D13, as an observed
//! hold): see [`Assembly::overflowed`]. Every rule is still checked after that point.
//!
//! The order of the checks on one event is: R1, R7, then the rules of the event's scope. After a
//! deviation, every later call to [`Assembly::accept`] returns the same deviation.

use std::collections::HashSet;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::json::{Malformed, check_numbers, opt_str, parse_value_strict};
use crate::protocol::{Deviation, MAX_IDENTIFIER_BYTES, raw_len};
use crate::report::{ReportDecision, ReportEntry};
use crate::responses::classify::{ItemClass, Scope, content_part_item, item_class, scope_of};
use crate::types::{ToolCall, ToolCallKind};

/// The fields that R9 compares in `added`, `done` and the terminal `output`.
const IDENTITY: [&str; 5] = ["type", "id", "call_id", "namespace", "name"];

/// The client-run item types whose model in the openai-python 3.48.0 output union
/// (`ResponseOutputItem`, `openai/types/responses/response_output_item.py`) has a `status` field.
/// For these an absent `status` at `done` is a truncation (R15, ruling T4b). The models of
/// `custom_tool_call`, `mcp_approval_request` and `mcp_approval_response` have no `status` field,
/// and a type that the union does not have has no model. `custom_tool_call_output` is
/// `ResponseCustomToolCallOutputItem` in the union, which declares `status`; the input model
/// `ResponseCustomToolCallOutput` does not.
const STATUS_TYPES: [&str; 8] = [
    "function_call",
    "function_call_output",
    "computer_call",
    "tool_search_call",
    "local_shell_call",
    "shell_call",
    "apply_patch_call",
    "custom_tool_call_output",
];

/// The prefix of the gateway's own rewrite ids (§ 6.5). No upstream item id may start with it.
pub(crate) const RESERVED_ID_PREFIX: &str = "msg_paigasus_";

type Hash = [u8; 32];

fn finish(h: &Sha256) -> Hash {
    let mut out = [0; 32];
    out.copy_from_slice(&h.clone().finalize());
    out
}

fn hash_str(s: &str) -> Hash {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    finish(&h)
}

fn feed_str(h: &mut Sha256, s: &str) {
    h.update((s.len() as u64).to_be_bytes());
    h.update(s.as_bytes());
}

/// Feed a JSON value so that two values hash equal exactly when they are equal as JSON values
/// (`Value::eq`): object keys in sorted order, so key order is no difference.
fn feed_value(h: &mut Sha256, v: &Value) {
    match v {
        Value::Null => h.update([0]),
        Value::Bool(b) => h.update([1, u8::from(*b)]),
        Value::Number(n) => {
            h.update([2]);
            // `-0.0 == 0.0` as a `Value`, but the two print differently.
            let text = if n.is_f64() && n.as_f64() == Some(0.0) { "0.0".to_owned() } else { n.to_string() };
            feed_str(h, &text);
        }
        Value::String(s) => {
            h.update([3]);
            feed_str(h, s);
        }
        Value::Array(a) => {
            h.update([4]);
            h.update((a.len() as u64).to_be_bytes());
            a.iter().for_each(|x| feed_value(h, x));
        }
        Value::Object(o) => {
            h.update([5]);
            h.update((o.len() as u64).to_be_bytes());
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            for k in keys {
                feed_str(h, k);
                feed_value(h, &o[k.as_str()]);
            }
        }
    }
}

fn hash_value(v: &Value) -> Hash {
    let mut h = Sha256::new();
    feed_value(&mut h, v);
    finish(&h)
}

/// The fixed name of a built-in client-run kind (§ 5).
fn fixed_name(kind: ToolCallKind) -> Option<&'static str> {
    match kind {
        ToolCallKind::Computer => Some("computer"),
        ToolCallKind::LocalShell => Some("local_shell"),
        ToolCallKind::Shell => Some("shell"),
        ToolCallKind::ApplyPatch => Some("apply_patch"),
        ToolCallKind::ToolSearch => Some("tool_search"),
        _ => None,
    }
}

fn text_of(item: &Map<String, Value>, key: &'static str) -> Result<String, Malformed> {
    match item.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(Malformed(key)),
    }
}

/// `(namespace, name)` of a client-run item (§ 5).
fn name_of(item: &Map<String, Value>, kind: ToolCallKind) -> Result<(Option<String>, String), Malformed> {
    if let Some(name) = fixed_name(kind) {
        return Ok((None, name.to_owned()));
    }
    Ok(match kind {
        ToolCallKind::Function => (opt_str(item.get("namespace"), "namespace")?, text_of(item, "name")?),
        ToolCallKind::Custom => (opt_str(item.get("namespace"), "namespace")?, text_of(item, "name")?),
        ToolCallKind::McpApprovalRequest => (opt_str(item.get("server_label"), "server_label")?, text_of(item, "name")?),
        ToolCallKind::Other => (None, text_of(item, "type")?),
        _ => return Err(Malformed("kind")),
    })
}

/// The call of one client-run Responses item, by the § 5 table. Shared by the stream (the `done`
/// item) and the body path (an `output[]` item).
///
/// `namespace` is the `namespace` of a function or custom call (when present) and the
/// `server_label` of an `mcp_approval_request` (§ 5, ruling T4a).
/// `id` is the `call_id`, or the item `id` for `mcp_approval_request`. A server `tool_search_call`
/// may have no `call_id`. Kind `Other` takes the `call_id` when the item has one, else the item
/// `id` (§ 5 gives no row for it). `arguments` is `arguments` for a function (`""` becomes `"{}"`),
/// `input` for a custom call, and for every other kind the item as compact JSON in its own key
/// order, with `status` removed. The item's numbers must be exact (`check_numbers`, § 7.1).
pub(crate) fn tool_call(position: u32, item: &Map<String, Value>, kind: ToolCallKind) -> Result<ToolCall, Malformed> {
    item.values().try_for_each(check_numbers)?;
    let (namespace, name) = name_of(item, kind)?;
    let id = match kind {
        ToolCallKind::McpApprovalRequest => Some(text_of(item, "id")?),
        ToolCallKind::ToolSearch => opt_str(item.get("call_id"), "call_id")?,
        ToolCallKind::Other => match opt_str(item.get("call_id"), "call_id")? {
            Some(c) => Some(c),
            None => opt_str(item.get("id"), "id")?,
        },
        _ => Some(text_of(item, "call_id")?),
    };
    let arguments = match kind {
        ToolCallKind::Function => {
            let a = text_of(item, "arguments")?;
            if a.is_empty() { "{}".to_owned() } else { a }
        }
        ToolCallKind::Custom => text_of(item, "input")?,
        _ => {
            let stripped: Map<String, Value> = item.iter().filter(|(k, _)| *k != "status").map(|(k, v)| (k.clone(), v.clone())).collect();
            serde_json::to_string(&Value::Object(stripped)).expect("a Value always serializes")
        }
    };
    Ok(ToolCall {
        choice: 0,
        position,
        id,
        name,
        namespace,
        kind,
        arguments,
    })
}

/// What a frame is to the item it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemFrame {
    /// No item: a lifecycle, terminal, response-level, keepalive or error record.
    Other,
    Added,
    /// An item-scoped event between `added` and `done`.
    Middle,
    Done,
}

/// What one accepted record means to the stream.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Effect {
    /// The response hold opened at this record: the first client-run `added` (D10).
    pub(crate) opened: bool,
    /// The `output_index` of the client-run item this frame belongs to. `None` for every other
    /// frame, the frames of tool-free and provider-run items included.
    pub(crate) item: Option<u32>,
    pub(crate) frame: ItemFrame,
    /// The `unenforceable` entry of a provider-run item, at its `added` (§ 6.2).
    pub(crate) report: Option<ReportEntry>,
    /// The record is the terminal event (R3).
    pub(crate) terminal: bool,
}

impl Effect {
    fn other() -> Self {
        Effect {
            opened: false,
            item: None,
            frame: ItemFrame::Other,
            report: None,
            terminal: false,
        }
    }
}

/// A delta stream: the running hash of the deltas and the hash of its `*.done` text.
#[derive(Default)]
struct Text {
    deltas: Sha256,
    done: Option<Hash>,
    /// Any delta or `*.done` event arrived.
    touched: bool,
    /// A delta after the `*.done` event, a second `*.done` event, or a value that is no string.
    broken: bool,
}

impl Text {
    fn delta(&mut self, d: Option<&Value>) -> bool {
        self.touched = true;
        match d {
            Some(Value::String(s)) if self.done.is_none() => self.deltas.update(s.as_bytes()),
            _ => self.broken = true,
        }
        !self.broken
    }

    /// Record the `*.done` text. `false` when the stream is broken or the text is not the deltas.
    fn done(&mut self, t: Option<&Value>) -> bool {
        self.touched = true;
        match t {
            Some(Value::String(s)) if self.done.is_none() => self.done = Some(hash_str(s)),
            _ => self.broken = true,
        }
        !self.broken && self.done == Some(finish(&self.deltas))
    }

    /// The deltas, and the `*.done` text when one arrived, both equal `value`.
    fn agrees(&self, value: &str) -> bool {
        let h = hash_str(value);
        !self.broken && finish(&self.deltas) == h && self.done.is_none_or(|d| d == h)
    }
}

/// One `shell_call` command (R13): the hash of its `added` command plus its deltas.
struct Command {
    text: Sha256,
    done: Option<Hash>,
}

/// The one open item (R5).
struct Open {
    index: u32,
    ty: String,
    id: String,
    class: ItemClass,
    /// The `added` item of a client-run item, for the subset rule. Counted in `held`.
    added: Option<Value>,
    text: Text,
    commands: Vec<Command>,
}

/// What the stream keeps of every item.
struct Seen {
    identity: [Option<Value>; 5],
    done: bool,
    /// The hash of a client-run item's `done` item, for R10.
    done_hash: Option<Hash>,
}

fn identity(item: &Map<String, Value>) -> [Option<Value>; 5] {
    IDENTITY.map(|k| item.get(k).cloned())
}

/// R8: an object with a string `type` and a non-empty string `id`. The identity fields and
/// `server_label` are strings or `null` where present, and none is longer than
/// `MAX_IDENTIFIER_BYTES`.
pub(crate) fn item_shape(v: Option<&Value>) -> Result<(&Map<String, Value>, &str, &str), Deviation> {
    let bad = Deviation("item_shape");
    let Some(Value::Object(m)) = v else { return Err(bad) };
    let Some(Value::String(ty)) = m.get("type") else { return Err(bad) };
    let Some(Value::String(id)) = m.get("id") else { return Err(bad) };
    if id.is_empty() {
        return Err(bad);
    }
    let named = IDENTITY.iter().chain(&["server_label"]).filter_map(|k| m.get(*k));
    let mut long = false;
    for v in named {
        match v {
            Value::Null => {}
            Value::String(s) => long |= s.len() > MAX_IDENTIFIER_BYTES,
            _ => return Err(bad),
        }
    }
    if long {
        return Err(Deviation("long_identifier"));
    }
    Ok((m, ty, id))
}

/// The subset rule (§ 6.4, A2): every field of `added`, at any depth, except the top-level
/// `status`, is `null`, `""`, `[]` or equal to the same field of `done`, and `done` has every
/// field that `added` has. An object field follows the rule inside; an array is `[]` or equal.
fn subset(added: &Map<String, Value>, done: &Map<String, Value>, top: bool) -> bool {
    added.iter().all(|(k, a)| {
        if top && k == "status" {
            return true;
        }
        let Some(d) = done.get(k) else { return false };
        match a {
            Value::Null => true,
            Value::String(s) if s.is_empty() => true,
            Value::Array(x) if x.is_empty() => true,
            Value::Object(ao) => matches!(d, Value::Object(dobj) if subset(ao, dobj, false)),
            _ => a == d,
        }
    })
}

fn index_of(v: Option<&Value>) -> Option<u32> {
    v.and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok())
}

/// `action.commands` of a `shell_call` item: `None` when absent or `null`.
fn shell_commands(item: &Map<String, Value>) -> Result<Option<&Vec<Value>>, Deviation> {
    let bad = Deviation("shell_command_mismatch");
    match item.get("action") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(a)) => match a.get("commands") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(c)) => Ok(Some(c)),
            Some(_) => Err(bad),
        },
        Some(_) => Err(bad),
    }
}

/// R13 per command event. Indexes appear in order, and a command ends before the next starts.
fn shell_event(commands: &mut Vec<Command>, ty: &str, ev: &Map<String, Value>) -> Result<(), Deviation> {
    let bad = Deviation("shell_command_mismatch");
    let i = index_of(ev.get("command_index")).ok_or(bad)? as usize;
    let text = |k: &str| match ev.get(k) {
        Some(Value::String(s)) => Ok(s.as_str()),
        _ => Err(bad),
    };
    if ty == "response.shell_call_command.added" {
        if i != commands.len() || commands.last().is_some_and(|c| c.done.is_none()) {
            return Err(bad);
        }
        let mut h = Sha256::new();
        h.update(text("command")?.as_bytes());
        commands.push(Command { text: h, done: None });
        return Ok(());
    }
    if i + 1 != commands.len() {
        return Err(bad);
    }
    let c = commands.last_mut().filter(|c| c.done.is_none()).ok_or(bad)?;
    if ty == "response.shell_call_command.delta" {
        c.text.update(text("delta")?.as_bytes());
    } else {
        let h = hash_str(text("command")?);
        if finish(&c.text) != h {
            return Err(bad);
        }
        c.done = Some(h);
    }
    Ok(())
}

/// R15: whether a client-run item has finished. A `status` other than `"completed"` is a
/// truncation. So is an absent `status`, except for an item type whose model has no `status`
/// field (T4b). Shared by the stream (`done`) and the body path (an `output[]` item).
pub(crate) fn is_completed(item: &Map<String, Value>, item_type: &str) -> bool {
    match item.get("status") {
        None => !STATUS_TYPES.contains(&item_type),
        Some(Value::String(s)) => s == "completed",
        Some(_) => false,
    }
}

/// R11: the arguments of a completed function call are `""` or one complete JSON value.
pub(crate) fn arguments_complete(text: &str) -> bool {
    text.is_empty() || parse_value_strict(text.as_bytes()).is_ok()
}

/// The per-kind rules of a client-run `done` item (R11–R14).
fn check_client(kind: ToolCallKind, open: &Open, added: &Map<String, Value>, done: &Map<String, Value>, completed: bool) -> Result<(), Deviation> {
    match kind {
        ToolCallKind::Function | ToolCallKind::Custom => {
            // R15 takes precedence over R11 (A3), and R12 is the same rule with `input`.
            if !completed {
                return Ok(());
            }
            let function = kind == ToolCallKind::Function;
            let (key, mismatch, added_label) = if function {
                ("arguments", "arguments_mismatch", "added_arguments")
            } else {
                ("input", "input_mismatch", "input_mismatch")
            };
            if !matches!(added.get(key), Some(Value::String(s)) if s.is_empty()) {
                return Err(Deviation(added_label));
            }
            let Some(Value::String(text)) = done.get(key) else { return Err(Deviation(mismatch)) };
            if !open.text.agrees(text) {
                return Err(Deviation(mismatch));
            }
            if function && !arguments_complete(text) {
                return Err(Deviation("incomplete_arguments"));
            }
        }
        ToolCallKind::Shell => {
            let bad = Deviation("shell_command_mismatch");
            let done_commands = shell_commands(done)?.map_or(&[][..], Vec::as_slice);
            // Ruling F2: `added` commands are `[]` or the `done` commands.
            if let Some(a) = shell_commands(added)?
                && !a.is_empty()
                && a.as_slice() != done_commands
            {
                return Err(bad);
            }
            if open.commands.len() != done_commands.len() {
                return Err(bad);
            }
            for (c, d) in open.commands.iter().zip(done_commands) {
                let Value::String(d) = d else { return Err(bad) };
                if c.done != Some(hash_str(d)) {
                    return Err(bad);
                }
            }
        }
        ToolCallKind::ApplyPatch => {
            let diff = match done.get("operation") {
                Some(Value::Object(op)) => op.get("diff"),
                _ => None,
            };
            let ok = match diff {
                Some(Value::String(d)) => open.text.agrees(d),
                None | Some(Value::Null) => !open.text.touched,
                Some(_) => false,
            };
            if !ok {
                return Err(Deviation("diff_mismatch"));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The assembly of one Responses stream (spec § 6.4).
pub(crate) struct Assembly {
    cap: usize,
    failed: Option<Deviation>,
    started: bool,
    ended: bool,
    response_id: Option<String>,
    /// R7: whether events carry `sequence_number`, fixed by the first non-exempt event.
    numbered: Option<bool>,
    last_number: Option<u64>,
    items: Vec<Seen>,
    open: Option<Open>,
    ids: HashSet<String>,
    call_ids: HashSet<String>,
    /// The `output_index` of every client-run item, in order: a call's slot is its place here.
    client: Vec<u32>,
    /// `(output_index, display_name)` of every client-run item so far.
    names: Vec<(u32, String)>,
    calls: Vec<ToolCall>,
    hold_open: bool,
    truncated: bool,
    overflowed: bool,
    /// The bytes of the open `added` item and of the calls' arguments.
    held: usize,
}

impl Assembly {
    /// `cap` is the stream's `max_held_bytes`: the calls of a response that hold more are dropped.
    pub(crate) fn with_cap(cap: usize) -> Self {
        Assembly {
            cap,
            failed: None,
            started: false,
            ended: false,
            response_id: None,
            numbered: None,
            last_number: None,
            items: Vec::new(),
            open: None,
            ids: HashSet::new(),
            call_ids: HashSet::new(),
            client: Vec::new(),
            names: Vec::new(),
            calls: Vec::new(),
            hold_open: false,
            truncated: false,
            overflowed: false,
            held: 0,
        }
    }

    /// Apply one classified event (`ty` is its `data.type`). `keepalive` and `error` are exempt
    /// (§ 6.4). A record after the terminal event changes nothing: the stream drops and counts it
    /// (R3, ruling F8).
    pub(crate) fn accept(&mut self, ty: &'static str, v: &Value) -> Result<Effect, Deviation> {
        if let Some(d) = self.failed {
            return Err(d);
        }
        let r = self.step(ty, v);
        if let Err(d) = r {
            self.failed = Some(d);
        }
        r
    }

    fn step(&mut self, ty: &'static str, v: &Value) -> Result<Effect, Deviation> {
        let scope = scope_of(ty);
        if matches!(scope, Scope::Keepalive | Scope::Error) || self.ended {
            return Ok(Effect::other());
        }
        let Value::Object(ev) = v else { return Err(Deviation("not_object")) };
        if !self.started && ty != "response.created" {
            return Err(Deviation("first_not_created"));
        }
        if self.started && ty == "response.created" {
            return Err(Deviation("repeated_created"));
        }
        self.sequence(ev)?;
        match scope {
            Scope::Lifecycle => {
                let resp = self.response_of(ev, ty == "response.created")?;
                if !matches!(resp.get("output"), Some(Value::Array(a)) if a.is_empty()) {
                    return Err(Deviation("early_output"));
                }
                self.started = true;
                Ok(Effect::other())
            }
            Scope::Terminal => self.terminal(ev),
            Scope::ItemAdded => self.added(ev),
            Scope::ItemDone => self.done(ev),
            Scope::Item(item_type) => self.middle(ty, item_type, v, ev),
            Scope::ResponseLevel | Scope::Keepalive | Scope::Error => Ok(Effect::other()),
        }
    }

    /// R7: an integer that strictly increases, on every event or on none.
    fn sequence(&mut self, ev: &Map<String, Value>) -> Result<(), Deviation> {
        let bad = Deviation("sequence_number");
        let n = match ev.get("sequence_number") {
            None => None,
            Some(Value::Number(n)) if n.is_u64() => n.as_u64(),
            Some(_) => return Err(bad),
        };
        match (*self.numbered.get_or_insert(n.is_some()), n) {
            (true, Some(n)) if self.last_number.is_none_or(|l| n > l) => {
                self.last_number = Some(n);
                Ok(())
            }
            (false, None) => Ok(()),
            _ => Err(bad),
        }
    }

    /// R1: the `response` object, whose `id` is the same on every lifecycle and terminal event.
    fn response_of<'a>(&mut self, ev: &'a Map<String, Value>, created: bool) -> Result<&'a Map<String, Value>, Deviation> {
        let bad = Deviation(if created && self.response_id.is_none() { "first_not_created" } else { "response_id_changed" });
        let Some(Value::Object(resp)) = ev.get("response") else { return Err(bad) };
        let Some(Value::String(id)) = resp.get("id") else { return Err(bad) };
        match &self.response_id {
            None => self.response_id = Some(id.clone()),
            Some(prev) if prev == id => {}
            Some(_) => return Err(Deviation("response_id_changed")),
        }
        Ok(resp)
    }

    /// R3, R9, R10, and R15 for a client-run item with no `done`.
    fn terminal(&mut self, ev: &Map<String, Value>) -> Result<Effect, Deviation> {
        let resp = self.response_of(ev, false)?;
        let Some(Value::Array(output)) = resp.get("output") else {
            return Err(Deviation("terminal_output_missing"));
        };
        let mismatch = Deviation("terminal_output_mismatch");
        if output.len() != self.items.len() {
            return Err(mismatch);
        }
        for (seen, v) in self.items.iter().zip(output) {
            let Value::Object(item) = v else { return Err(mismatch) };
            if identity(item) != seen.identity {
                return Err(Deviation("item_identity_changed"));
            }
            if seen.done_hash.is_some_and(|h| h != hash_value(v)) {
                return Err(mismatch);
            }
        }
        if let Some(open) = self.open.take() {
            if matches!(open.class, ItemClass::ClientRun(_)) {
                self.truncated = true;
            }
            self.held -= open.added.as_ref().map_or(0, raw_len);
        }
        self.ended = true;
        Ok(Effect { terminal: true, ..Effect::other() })
    }

    /// R4, R5, R8 at `response.output_item.added`.
    fn added(&mut self, ev: &Map<String, Value>) -> Result<Effect, Deviation> {
        let value = ev.get("item");
        let (item, ty, id) = item_shape(value)?;
        let next = u32::try_from(self.items.len()).unwrap_or(u32::MAX);
        let index = index_of(ev.get("output_index")).ok_or(Deviation("index_order"))?;
        if index < next {
            return Err(Deviation("index_reused"));
        }
        if index > next {
            return Err(Deviation("index_order"));
        }
        if self.open.is_some() {
            return Err(Deviation("item_interleaved"));
        }
        if id.starts_with(RESERVED_ID_PREFIX) {
            return Err(Deviation("reserved_id"));
        }
        if self.ids.contains(id) {
            return Err(Deviation("duplicate_id"));
        }
        let call_id = match item.get("call_id") {
            Some(Value::String(c)) => Some(c),
            _ => None,
        };
        if call_id.is_some_and(|c| self.call_ids.contains(c)) {
            return Err(Deviation("duplicate_call_id"));
        }
        self.ids.insert(id.to_owned());
        if let Some(c) = call_id {
            self.call_ids.insert(c.clone());
        }
        let class = item_class(ty);
        let mut effect = Effect {
            frame: ItemFrame::Added,
            ..Effect::other()
        };
        let mut added = None;
        match class {
            ItemClass::ClientRun(kind) => {
                effect.item = Some(index);
                if !self.hold_open {
                    self.hold_open = true;
                    effect.opened = true;
                }
                self.client.push(index);
                // The name the calls not assembled yet are known by (a `BlockAll` report).
                let name = match name_of(item, kind) {
                    Ok((Some(ns), name)) => format!("{ns}.{name}"),
                    Ok((None, name)) => name,
                    Err(_) => kind.as_str().to_owned(),
                };
                self.names.push((index, name));
                let v = value.cloned().expect("item_shape found the item");
                self.held += raw_len(&v);
                added = Some(v);
            }
            ItemClass::ProviderRun => {
                effect.report = Some(ReportEntry {
                    choice: 0,
                    position: index,
                    name: ty.to_owned(),
                    decision: ReportDecision::Unenforceable,
                    basis: None,
                    code: None,
                    enforced: None,
                    reason: None,
                });
            }
            ItemClass::Content => {}
        }
        self.items.push(Seen {
            identity: identity(item),
            done: false,
            done_hash: None,
        });
        self.open = Some(Open {
            index,
            ty: ty.to_owned(),
            id: id.to_owned(),
            class,
            added,
            text: Text::default(),
            commands: Vec::new(),
        });
        Ok(effect)
    }

    /// R6 and R16: the event's `output_index` is the open item's.
    fn locate(&self, ev: &Map<String, Value>) -> Result<u32, Deviation> {
        let index = index_of(ev.get("output_index")).ok_or(Deviation("wrong_item_index"))?;
        if self.open.as_ref().is_some_and(|o| o.index == index) {
            return Ok(index);
        }
        if self.items.get(index as usize).is_some_and(|s| s.done) {
            return Err(Deviation("event_after_done"));
        }
        Err(Deviation("wrong_item_index"))
    }

    /// An item-scoped event between `added` and `done` (R6, R11–R14 ordering, R16).
    fn middle(&mut self, ty: &'static str, item_type: &'static str, v: &Value, ev: &Map<String, Value>) -> Result<Effect, Deviation> {
        let index = self.locate(ev)?;
        let open = self.open.as_mut().expect("locate found the open item");
        // Ruling T3a: a content part belongs to the item type its part names.
        let expected = if ty.starts_with("response.content_part.") {
            content_part_item(v).map_err(|_| Deviation("event_item_type"))?
        } else {
            item_type
        };
        if open.ty != expected {
            return Err(Deviation("event_item_type"));
        }
        // `response.shell_call_command.*` is the one item-scoped family with no `item_id`.
        let no_item_id = ty.starts_with("response.shell_call_command.");
        match ev.get("item_id") {
            Some(Value::String(s)) if *s == open.id => {}
            None if no_item_id => {}
            _ => return Err(Deviation("item_id_mismatch")),
        }
        // Function and custom streams are judged at `done`, where R15 can take precedence (A3).
        // An apply-patch diff and a shell command are judged here too.
        let diff_ok = match ty {
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                open.text.delta(ev.get("delta"));
                true
            }
            "response.function_call_arguments.done" => {
                open.text.done(ev.get("arguments"));
                true
            }
            "response.custom_tool_call_input.done" => {
                open.text.done(ev.get("input"));
                true
            }
            "response.apply_patch_call_operation_diff.delta" => open.text.delta(ev.get("delta")),
            "response.apply_patch_call_operation_diff.done" => open.text.done(ev.get("diff")),
            _ if no_item_id => {
                shell_event(&mut open.commands, ty, ev)?;
                true
            }
            _ => true,
        };
        if !diff_ok {
            return Err(Deviation("diff_mismatch"));
        }
        let client = matches!(open.class, ItemClass::ClientRun(_));
        Ok(Effect {
            frame: ItemFrame::Middle,
            item: client.then_some(index),
            ..Effect::other()
        })
    }

    /// R9, R11–R15 and the subset rule at `response.output_item.done`.
    fn done(&mut self, ev: &Map<String, Value>) -> Result<Effect, Deviation> {
        let value = ev.get("item");
        let (item, item_type, _) = item_shape(value)?;
        let index = self.locate(ev)?;
        if identity(item) != self.items[index as usize].identity {
            return Err(Deviation("item_identity_changed"));
        }
        let open = self.open.take().expect("locate found the open item");
        let mut client = false;
        if let ItemClass::ClientRun(kind) = open.class {
            client = true;
            let added_value = open.added.as_ref().expect("a client-run item keeps its added item");
            let added = added_value.as_object().expect("item_shape checked the added item");
            // R15: a `done` whose `status` is not `"completed"` is a truncation. An absent `status`
            // is one too, except for an item type whose model has no `status` field (T4b).
            let completed = is_completed(item, item_type);
            check_client(kind, &open, added, item, completed)?;
            if !subset(added, item, true) {
                return Err(Deviation("added_done_differ"));
            }
            let call = tool_call(index, item, kind).map_err(|m| Deviation(if m.0 == "lossy_number" { "lossy_number" } else { "item_shape" }))?;
            if !completed {
                self.truncated = true;
            }
            self.held -= raw_len(added_value);
            if let Some(n) = self.names.last_mut() {
                *n = (index, call.display_name());
            }
            if !self.overflowed {
                self.held += call.arguments.len();
                self.calls.push(call);
                if self.held > self.cap {
                    // 749 D13: an outgrown hold keeps nothing and is never decided.
                    self.overflowed = true;
                    self.held -= std::mem::take(&mut self.calls).iter().map(|c| c.arguments.len()).sum::<usize>();
                }
            }
            self.items[index as usize].done_hash = value.map(hash_value);
        }
        self.items[index as usize].done = true;
        Ok(Effect {
            frame: ItemFrame::Done,
            item: client.then_some(index),
            ..Effect::other()
        })
    }

    /// Whether the response hold opened: a client-run item was added (D10). It stays `true`.
    pub(crate) fn hold_open(&self) -> bool {
        self.hold_open
    }

    /// R15: a client-run `done` was not `"completed"`, or the terminal event arrived while a
    /// client-run item had no `done`. (`[DONE]` and EOF before the terminal are the stream's.)
    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    /// The calls outgrew the cap and were dropped: the hold must never be decided as allowed on
    /// the strength of [`Assembly::hold_calls`], which is then empty (749 D13).
    pub(crate) fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// The terminal event arrived (R3). Every later record but `[DONE]` is dropped and counted.
    pub(crate) fn ended(&self) -> bool {
        self.ended
    }

    /// The end-of-stream rule of the response hold, read at the terminal event, at `[DONE]` and at
    /// EOF: the hold opened and cannot be decided as complete. It is cut when no terminal event
    /// arrived (R3; also R15 when a client-run item has no `done`), when the stream is truncated
    /// (R15), or when the calls outgrew the cap (749 D13, ruling T4d). A cut hold is never decided
    /// on [`Assembly::hold_calls`]: the interceptor applies `on_failure` to it.
    pub(crate) fn hold_cut(&self) -> bool {
        self.hold_open && (!self.ended || self.truncated() || self.overflowed())
    }

    /// The completed calls, in `output_index` order. Moves them out. Empty after an overflow.
    pub(crate) fn hold_calls(&mut self) -> Vec<ToolCall> {
        let calls = std::mem::take(&mut self.calls);
        self.held -= calls.iter().map(|c| c.arguments.len()).sum::<usize>();
        calls
    }

    /// The slot (place in the hold's call order) of the client-run item at `output_index`.
    pub(crate) fn slot_of(&self, output_index: u32) -> Option<usize> {
        self.client.iter().position(|i| *i == output_index)
    }

    /// `(output_index, display_name)` of every client-run item so far, done or not.
    pub(crate) fn known(&self) -> Vec<(u32, String)> {
        self.names.clone()
    }

    /// R7: the stream's events carry `sequence_number`. Tests only: the rewrite copies the key
    /// from the frame it replaces, so it needs no flag.
    #[cfg(test)]
    pub(crate) fn sequence_numbers(&self) -> bool {
        self.numbered == Some(true)
    }

    /// The bytes held in the open `added` item and the calls' arguments, measured exactly.
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        let added = self.open.as_ref().and_then(|o| o.added.as_ref()).map_or(0, raw_len);
        let held = added + self.calls.iter().map(|c| c.arguments.len()).sum::<usize>();
        assert_eq!(held, self.held, "the held-byte count is exact");
        held
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::protocol::MAX_IDENTIFIER_BYTES;
    use crate::report::ReportDecision;
    use crate::responses::classify::{Event, STREAM_EVENT_TYPES, classify};

    const CAP: usize = 1 << 20;

    fn ty(v: &Value) -> &'static str {
        let t = v["type"].as_str().expect("an event has a type");
        STREAM_EVENT_TYPES.iter().find(|x| **x == t).copied().unwrap_or_else(|| panic!("unknown type {t}"))
    }

    /// Give every event except `keepalive` the `sequence_number` 0, 1, 2, … (R7).
    fn numbered(evs: Vec<Value>) -> Vec<Value> {
        evs.into_iter()
            .enumerate()
            .map(|(i, mut e)| {
                if e["type"] != "keepalive" {
                    e["sequence_number"] = json!(i);
                }
                e
            })
            .collect()
    }

    fn run_with(a: &mut Assembly, evs: &[Value]) -> Result<Vec<Effect>, Deviation> {
        evs.iter().map(|e| a.accept(ty(e), e)).collect()
    }

    /// Number the events and run them on a fresh assembly.
    fn run(evs: Vec<Value>) -> (Assembly, Result<Vec<Effect>, Deviation>) {
        let mut a = Assembly::with_cap(CAP);
        let r = run_with(&mut a, &numbered(evs));
        (a, r)
    }

    fn deviation(evs: Vec<Value>) -> &'static str {
        match run(evs).1 {
            Err(Deviation(d)) => d,
            Ok(_) => panic!("expected a deviation"),
        }
    }

    fn passes(evs: Vec<Value>) -> Assembly {
        let (a, r) = run(evs);
        if let Err(d) = r {
            panic!("unexpected deviation {d:?}");
        }
        a
    }

    fn lifecycle(t: &str, output: Value) -> Value {
        json!({"type": t, "response": {"id": "resp_1", "status": "in_progress", "output": output}})
    }

    fn created() -> Value {
        lifecycle("response.created", json!([]))
    }

    fn in_progress() -> Value {
        lifecycle("response.in_progress", json!([]))
    }

    fn completed(output: Vec<Value>) -> Value {
        json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "output": output}})
    }

    fn added(i: u32, item: Value) -> Value {
        json!({"type": "response.output_item.added", "output_index": i, "item": item})
    }

    fn item_done(i: u32, item: Value) -> Value {
        json!({"type": "response.output_item.done", "output_index": i, "item": item})
    }

    fn scoped(t: &str, i: u32, id: &str, extra: Value) -> Value {
        let mut e = json!({"type": t, "output_index": i, "item_id": id});
        for (k, v) in extra.as_object().expect("extra is an object") {
            e[k] = v.clone();
        }
        e
    }

    fn fc(id: &str, call_id: &str, name: &str, args: &str, status: &str) -> Value {
        json!({"id": id, "type": "function_call", "status": status, "arguments": args, "call_id": call_id, "name": name})
    }

    /// One item: its events and its `done` item (for the terminal `output`).
    type Item = (Vec<Value>, Value);

    /// Added, deltas, `.done`, item done. The `.done` and the `done` item hold the concatenation.
    fn function_item(i: u32, id: &str, call_id: &str, name: &str, deltas: &[&str]) -> Item {
        let args = deltas.concat();
        let mut evs = vec![added(i, fc(id, call_id, name, "", "in_progress"))];
        evs.extend(deltas.iter().map(|d| scoped("response.function_call_arguments.delta", i, id, json!({"delta": d}))));
        evs.push(scoped("response.function_call_arguments.done", i, id, json!({"arguments": args})));
        let done = fc(id, call_id, name, &args, "completed");
        evs.push(item_done(i, done.clone()));
        (evs, done)
    }

    fn message_item(i: u32, id: &str, text: &str) -> Item {
        let part = json!({"type": "output_text", "text": "", "annotations": []});
        let full = json!({"type": "output_text", "text": text, "annotations": []});
        let done = json!({"id": id, "type": "message", "status": "completed", "role": "assistant", "content": [full.clone()]});
        let evs = vec![
            added(i, json!({"id": id, "type": "message", "status": "in_progress", "role": "assistant", "content": []})),
            scoped("response.content_part.added", i, id, json!({"content_index": 0, "part": part})),
            scoped("response.output_text.delta", i, id, json!({"content_index": 0, "delta": text})),
            scoped("response.output_text.done", i, id, json!({"content_index": 0, "text": text})),
            scoped("response.content_part.done", i, id, json!({"content_index": 0, "part": full})),
            item_done(i, done.clone()),
        ];
        (evs, done)
    }

    fn reasoning_item(i: u32, id: &str, encrypted: &str) -> Item {
        let done = json!({"id": id, "type": "reasoning", "summary": [], "encrypted_content": encrypted});
        (vec![added(i, json!({"id": id, "type": "reasoning", "summary": []})), item_done(i, done.clone())], done)
    }

    /// An item with no middle events: the same item at `added` and `done`, except `status`.
    fn plain_item(i: u32, added_item: Value, done_item: Value) -> Item {
        (vec![added(i, added_item), item_done(i, done_item.clone())], done_item)
    }

    fn stream(items: Vec<Item>) -> Vec<Value> {
        let mut evs = vec![created(), in_progress()];
        let mut output = Vec::new();
        for (e, done) in items {
            evs.extend(e);
            output.push(done);
        }
        evs.push(completed(output));
        evs
    }

    fn call(position: u32, id: &str, namespace: Option<&str>, name: &str, kind: ToolCallKind, arguments: &str) -> ToolCall {
        ToolCall {
            choice: 0,
            position,
            id: Some(id.to_owned()),
            name: name.to_owned(),
            namespace: namespace.map(str::to_owned),
            kind,
            arguments: arguments.to_owned(),
        }
    }

    #[test]
    fn a_function_call_yields_its_call() {
        let evs = stream(vec![
            message_item(0, "msg_1", "hi"),
            function_item(1, "fc_1", "call_1", "get_weather", &["{\"city\":", "\"Paris\"}"]),
            function_item(2, "fc_2", "call_2", "now", &[]),
        ]);
        let (mut a, r) = run(evs);
        let effects = r.unwrap();
        assert_eq!(
            a.hold_calls(),
            [
                call(1, "call_1", None, "get_weather", ToolCallKind::Function, "{\"city\":\"Paris\"}"),
                call(2, "call_2", None, "now", ToolCallKind::Function, "{}"),
            ]
        );
        assert!(a.hold_open());
        assert!(!a.truncated());
        assert!(a.sequence_numbers());
        assert_eq!((a.slot_of(0), a.slot_of(1), a.slot_of(2), a.slot_of(3)), (None, Some(0), Some(1), None));
        assert_eq!(a.known(), [(1, "get_weather".to_owned()), (2, "now".to_owned())]);
        // The message frames belong to no client-run item; the function frames belong to item 1.
        let frames: Vec<(ItemFrame, Option<u32>)> = effects.iter().map(|e| (e.frame, e.item)).collect();
        assert_eq!(frames[2..8].iter().map(|f| f.1).collect::<Vec<_>>(), [None; 6]);
        assert_eq!(
            frames[8..13],
            [
                (ItemFrame::Added, Some(1)),
                (ItemFrame::Middle, Some(1)),
                (ItemFrame::Middle, Some(1)),
                (ItemFrame::Middle, Some(1)),
                (ItemFrame::Done, Some(1)),
            ]
        );
        let last = effects.last().unwrap();
        assert!(last.terminal);
        assert_eq!((last.frame, last.item), (ItemFrame::Other, None));
        assert_eq!(a.retained(), 0, "the calls moved out, and no item is open");
    }

    #[test]
    fn a_custom_call_uses_input() {
        let id = "ctc_1";
        let item = |input: &str, status: &str| json!({"id": id, "type": "custom_tool_call", "status": status, "call_id": "call_c", "input": input, "name": "run_shell"});
        let done = item("ls -la\n", "completed");
        let evs = vec![
            added(0, item("", "in_progress")),
            scoped("response.custom_tool_call_input.delta", 0, id, json!({"delta": "ls"})),
            scoped("response.custom_tool_call_input.delta", 0, id, json!({"delta": " -la\n"})),
            scoped("response.custom_tool_call_input.done", 0, id, json!({"input": "ls -la\n"})),
            item_done(0, done.clone()),
        ];
        let mut a = passes(stream(vec![(evs, done)]));
        assert_eq!(a.hold_calls(), [call(0, "call_c", None, "run_shell", ToolCallKind::Custom, "ls -la\n")]);
    }

    #[test]
    fn a_namespaced_custom_call_keeps_its_namespace() {
        // Ruling T4a: a custom call's `namespace` is part of its identity, as a function's is.
        let item = |ns: &str, input: &str, status: &str| json!({"id": "ctc_1", "type": "custom_tool_call", "status": status, "call_id": "call_c", "namespace": ns, "input": input, "name": "run"});
        let build = |done_ns: &str| {
            let done = item(done_ns, "ls", "completed");
            let evs = vec![
                added(0, item("tools", "", "in_progress")),
                scoped("response.custom_tool_call_input.delta", 0, "ctc_1", json!({"delta": "ls"})),
                item_done(0, done.clone()),
            ];
            stream(vec![(evs, done)])
        };
        let mut a = passes(build("tools"));
        assert_eq!(a.known(), [(0, "tools.run".to_owned())]);
        let calls = a.hold_calls();
        assert_eq!(calls, [call(0, "call_c", Some("tools"), "run", ToolCallKind::Custom, "ls")]);
        assert_eq!(calls[0].display_name(), "tools.run");
        assert_eq!(deviation(build("other")), "item_identity_changed");
    }

    #[test]
    fn a_namespaced_function_keeps_its_namespace() {
        let mut item = function_item(0, "fc_1", "call_1", "read", &["{}"]);
        for e in item.0.iter_mut().filter(|e| e.get("item").is_some()) {
            e["item"]["namespace"] = json!("files");
        }
        item.1["namespace"] = json!("files");
        let mut a = passes(stream(vec![item]));
        let calls = a.hold_calls();
        assert_eq!(calls, [call(0, "call_1", Some("files"), "read", ToolCallKind::Function, "{}")]);
        assert_eq!(calls[0].display_name(), "files.read");
        assert_eq!(a.known(), [(0, "files.read".to_owned())]);
    }

    #[test]
    fn an_mcp_approval_request_uses_the_item_id_and_server_label() {
        let item = json!({"id": "mcpr_1", "type": "mcp_approval_request", "server_label": "git", "name": "push", "arguments": "{\"force\":true}"});
        let mut a = passes(stream(vec![plain_item(0, item.clone(), item)]));
        let compact = r#"{"id":"mcpr_1","type":"mcp_approval_request","server_label":"git","name":"push","arguments":"{\"force\":true}"}"#;
        assert_eq!(a.hold_calls(), [call(0, "mcpr_1", Some("git"), "push", ToolCallKind::McpApprovalRequest, compact)]);
        assert!(!a.truncated(), "an item type with no status is not a truncation");
    }

    #[test]
    fn built_in_calls_use_the_done_item_without_status() {
        let computer = (
            json!({"id": "cu_1", "type": "computer_call", "status": "in_progress", "call_id": "call_cu"}),
            json!({"id": "cu_1", "type": "computer_call", "status": "completed", "actions": [{"type": "screenshot"}], "call_id": "call_cu"}),
            r#"{"id":"cu_1","type":"computer_call","actions":[{"type":"screenshot"}],"call_id":"call_cu"}"#,
        );
        let local = (
            json!({"id": "lsh_1", "type": "local_shell_call", "status": "in_progress", "call_id": "call_l", "action": {"type": "exec", "command": []}}),
            json!({"id": "lsh_1", "type": "local_shell_call", "status": "completed", "call_id": "call_l", "action": {"type": "exec", "command": ["ls"]}}),
            r#"{"id":"lsh_1","type":"local_shell_call","call_id":"call_l","action":{"type":"exec","command":["ls"]}}"#,
        );
        let search = (
            json!({"id": "ts_1", "type": "tool_search_call", "status": "in_progress", "call_id": "call_t", "execution": "client", "arguments": {}}),
            json!({"id": "ts_1", "type": "tool_search_call", "status": "completed", "call_id": "call_t", "execution": "client", "arguments": {"q": "x"}}),
            r#"{"id":"ts_1","type":"tool_search_call","call_id":"call_t","execution":"client","arguments":{"q":"x"}}"#,
        );
        let other = (
            json!({"id": "fco_1", "type": "function_call_output", "status": "in_progress", "call_id": "call_o", "output": ""}),
            json!({"id": "fco_1", "type": "function_call_output", "status": "completed", "call_id": "call_o", "output": "42"}),
            r#"{"id":"fco_1","type":"function_call_output","call_id":"call_o","output":"42"}"#,
        );
        let patch_added = json!({"id": "apc_1", "type": "apply_patch_call", "status": "in_progress", "call_id": "call_p", "operation": {"type": "create_file", "diff": "", "path": "a.txt"}});
        let patch_done = json!({"id": "apc_1", "type": "apply_patch_call", "status": "completed", "call_id": "call_p", "operation": {"type": "create_file", "diff": "+hi\n", "path": "a.txt"}});
        let patch = (
            vec![
                added(4, patch_added),
                scoped("response.apply_patch_call_operation_diff.delta", 4, "apc_1", json!({"delta": "+hi"})),
                scoped("response.apply_patch_call_operation_diff.delta", 4, "apc_1", json!({"delta": "\n"})),
                scoped("response.apply_patch_call_operation_diff.done", 4, "apc_1", json!({"diff": "+hi\n"})),
                item_done(4, patch_done.clone()),
            ],
            patch_done,
        );
        let mut a = passes(stream(vec![
            plain_item(0, computer.0, computer.1),
            plain_item(1, local.0, local.1),
            plain_item(2, search.0, search.1),
            plain_item(3, other.0, other.1),
            patch,
        ]));
        let got: Vec<(u32, Option<String>, String, ToolCallKind, String)> = a.hold_calls().into_iter().map(|c| (c.position, c.id, c.name, c.kind, c.arguments)).collect();
        let s = |x: &str| x.to_owned();
        assert_eq!(
            got,
            [
                (0, Some(s("call_cu")), s("computer"), ToolCallKind::Computer, s(computer.2)),
                (1, Some(s("call_l")), s("local_shell"), ToolCallKind::LocalShell, s(local.2)),
                (2, Some(s("call_t")), s("tool_search"), ToolCallKind::ToolSearch, s(search.2)),
                (3, Some(s("call_o")), s("function_call_output"), ToolCallKind::Other, s(other.2)),
                (
                    4,
                    Some(s("call_p")),
                    s("apply_patch"),
                    ToolCallKind::ApplyPatch,
                    s(r#"{"id":"apc_1","type":"apply_patch_call","call_id":"call_p","operation":{"type":"create_file","diff":"+hi\n","path":"a.txt"}}"#)
                ),
            ]
        );
    }

    fn shell_item(cmds: &[&[&str]], done_cmds: Value) -> Item {
        let id = "sh_1";
        let item = |commands: Value, max: Value, status: &str| json!({"id": id, "type": "shell_call", "status": status, "action": {"commands": commands, "max_output_length": max, "timeout_ms": null}, "call_id": "call_s", "environment": null});
        let mut evs = vec![added(0, item(json!([]), json!(null), "in_progress"))];
        for (i, parts) in cmds.iter().enumerate() {
            let c = |t: &str, extra: Value| {
                let mut e = json!({"type": t, "command_index": i, "output_index": 0});
                for (k, v) in extra.as_object().unwrap() {
                    e[k] = v.clone();
                }
                e
            };
            evs.push(c("response.shell_call_command.added", json!({"command": parts[0]})));
            evs.extend(parts[1..].iter().map(|d| c("response.shell_call_command.delta", json!({"delta": d}))));
            evs.push(c("response.shell_call_command.done", json!({"command": parts.concat()})));
        }
        let done = item(done_cmds, json!(20000), "completed");
        evs.push(item_done(0, done.clone()));
        (evs, done)
    }

    #[test]
    fn shell_commands_must_agree() {
        // The measured shape: `commands` `[]` and `max_output_length` null at `added` (A2).
        let mut a = passes(stream(vec![shell_item(&[&["", "ls", " -la"], &["da", "te"]], json!(["ls -la", "date"]))]));
        let calls = a.hold_calls();
        assert_eq!((calls[0].name.as_str(), calls[0].kind, calls[0].id.as_deref()), ("shell", ToolCallKind::Shell, Some("call_s")));
        // A done command that differs from its deltas, and a `done` item that differs from both.
        let mut evs = stream(vec![shell_item(&[&["", "ls"]], json!(["ls"]))]);
        evs[5]["command"] = json!("rm");
        assert_eq!(deviation(evs), "shell_command_mismatch");
        assert_eq!(deviation(stream(vec![shell_item(&[&["", "ls"]], json!(["rm"]))])), "shell_command_mismatch");
        // A command of the `done` item that never streamed, and one that streamed but is not there.
        assert_eq!(deviation(stream(vec![shell_item(&[&["", "ls"]], json!(["ls", "date"]))])), "shell_command_mismatch");
        assert_eq!(deviation(stream(vec![shell_item(&[&["", "ls"], &["date"]], json!(["ls"]))])), "shell_command_mismatch");
        // Indexes in order: a first command at index 1.
        let mut evs = stream(vec![shell_item(&[&["", "ls"]], json!(["ls"]))]);
        for e in &mut evs[3..6] {
            e["command_index"] = json!(1);
        }
        assert_eq!(deviation(evs), "shell_command_mismatch");
        // A delta after its command's `done`.
        let mut evs = stream(vec![shell_item(&[&["", "ls"]], json!(["ls"]))]);
        evs.swap(4, 5);
        assert_eq!(deviation(evs), "shell_command_mismatch");
        // Ruling F2: `added` commands are `[]` or the `done` commands.
        let mut evs = stream(vec![shell_item(&[&["", "ls"]], json!(["ls"]))]);
        evs[2]["item"]["action"]["commands"] = json!(["l"]);
        assert_eq!(deviation(evs), "shell_command_mismatch");
        let mut evs = stream(vec![shell_item(&[&["", "ls"]], json!(["ls"]))]);
        evs[2]["item"]["action"]["commands"] = json!(["ls"]);
        passes(evs);
    }

    #[test]
    fn measured_partial_added_items_pass() {
        // Shapes from the Task 1 recordings (spec § 15, A2): `actions` absent at `added`, and an
        // `operation.diff` of `""` at `added`.
        let computer = plain_item(
            0,
            json!({"id": "cu_1", "type": "computer_call", "status": "in_progress", "call_id": "call_cu"}),
            json!({"id": "cu_1", "type": "computer_call", "status": "completed", "actions": [{"type": "screenshot"}], "call_id": "call_cu"}),
        );
        passes(stream(vec![computer]));
    }

    #[test]
    fn keepalive_and_errors_are_exempt() {
        let mut evs = numbered(stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]));
        evs.insert(0, json!({"type": "keepalive"}));
        evs.insert(4, json!({"type": "keepalive", "sequence_number": 0}));
        evs.insert(0, json!({"type": "error", "code": "x"}));
        let mut a = Assembly::with_cap(CAP);
        let effects = run_with(&mut a, &evs).unwrap();
        assert_eq!((effects[0].frame, effects[1].frame, effects[5].frame), (ItemFrame::Other, ItemFrame::Other, ItemFrame::Other));
        assert_eq!(a.hold_calls().len(), 1);
    }

    #[test]
    fn records_after_the_terminal_change_nothing() {
        // Ruling F8: a second terminal is a record after the terminal; the stream drops it.
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs.push(completed(vec![]));
        evs.push(added(5, json!({"id": "x", "type": "function_call"})));
        let (mut a, r) = run(evs);
        let effects = r.unwrap();
        let n = effects.len();
        assert!(effects[n - 3].terminal);
        assert_eq!(effects[n - 2].clone(), effects[n - 1].clone());
        assert!(!effects[n - 1].terminal && effects[n - 1].frame == ItemFrame::Other && effects[n - 1].item.is_none());
        assert_eq!(a.hold_calls().len(), 1);
    }

    #[test]
    fn r1_first_record_and_response_id() {
        assert_eq!(deviation(vec![in_progress(), created()]), "first_not_created");
        assert_eq!(deviation(vec![json!({"type": "response.output_text.delta"})]), "first_not_created");
        // Ruling T4c: a `response.created` after the first record, with the same id.
        let mut evs = stream(vec![message_item(0, "msg_1", "x")]);
        evs.insert(4, created());
        assert_eq!(deviation(evs), "repeated_created");
        assert_eq!(deviation(vec![created(), created()]), "repeated_created");
        let mut evs = stream(vec![]);
        evs[1]["response"]["id"] = json!("resp_2");
        assert_eq!(deviation(evs), "response_id_changed");
        let mut evs = stream(vec![message_item(0, "msg_1", "x")]);
        let last = evs.len() - 1;
        evs[last]["response"]["id"] = json!("resp_2");
        assert_eq!(deviation(evs), "response_id_changed");
        let mut evs = stream(vec![]);
        evs[1]["response"].as_object_mut().unwrap().remove("id");
        assert_eq!(deviation(evs), "response_id_changed");
    }

    #[test]
    fn r2_lifecycle_events_carry_no_output() {
        let mut evs = stream(vec![]);
        evs[1]["response"]["output"] = json!([{"id": "msg_1", "type": "message"}]);
        assert_eq!(deviation(evs), "early_output");
        let mut evs = stream(vec![]);
        evs[0]["response"].as_object_mut().unwrap().remove("output");
        assert_eq!(deviation(evs), "early_output");
    }

    #[test]
    fn r3_the_terminal_carries_an_output_array() {
        let mut evs = stream(vec![]);
        evs[2]["response"]["output"] = json!(null);
        assert_eq!(deviation(evs), "terminal_output_missing");
        let mut evs = stream(vec![]);
        evs[2]["response"].as_object_mut().unwrap().remove("output");
        assert_eq!(deviation(evs), "terminal_output_missing");
    }

    #[test]
    fn r4_indexes_appear_in_order_once() {
        assert_eq!(deviation(stream(vec![function_item(1, "fc_1", "call_1", "f", &["{}"])])), "index_order");
        let mut evs = stream(vec![message_item(0, "msg_1", "x"), function_item(1, "fc_1", "call_1", "f", &["{}"])]);
        evs[8]["output_index"] = json!(0);
        assert_eq!(deviation(evs), "index_reused");
    }

    #[test]
    fn r5_items_are_contiguous() {
        let (first, _) = function_item(0, "fc_1", "call_1", "f", &["{}"]);
        let evs = vec![created(), first[0].clone(), first[1].clone(), added(1, fc("fc_2", "call_2", "g", "", "in_progress"))];
        assert_eq!(deviation(evs), "item_interleaved");
    }

    #[test]
    fn r6_events_match_the_open_item() {
        let base = || stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        let mut evs = base();
        evs[3]["output_index"] = json!(1);
        assert_eq!(deviation(evs), "wrong_item_index");
        let mut evs = base();
        evs[3]["item_id"] = json!("fc_x");
        assert_eq!(deviation(evs), "item_id_mismatch");
        let mut evs = base();
        evs[3].as_object_mut().unwrap().remove("item_id");
        assert_eq!(deviation(evs), "item_id_mismatch");
        let mut evs = base();
        evs[3]["type"] = json!("response.custom_tool_call_input.delta");
        assert_eq!(deviation(evs), "event_item_type");
        let mut evs = base();
        evs[3] = scoped("response.output_text.delta", 0, "fc_1", json!({"delta": "x", "content_index": 0}));
        assert_eq!(deviation(evs), "event_item_type");
    }

    #[test]
    fn r6_a_content_part_event_takes_its_item_type_from_the_part() {
        // Ruling T3a: `scope_of` says `message` for every `response.content_part.*`; the part type
        // decides. A `reasoning_text` part on a `function_call` item is a mismatch.
        let part = |t: &str| json!({"content_index": 0, "part": {"type": t, "text": ""}});
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs.insert(3, scoped("response.content_part.added", 0, "fc_1", part("reasoning_text")));
        assert_eq!(deviation(evs), "event_item_type");
        // On a `reasoning` item, `reasoning_text` belongs and `output_text` does not.
        let reasoning = |t: &str| {
            let (mut evs, done) = reasoning_item(0, "rs_1", "e");
            evs.insert(1, scoped("response.content_part.added", 0, "rs_1", part(t)));
            stream(vec![(evs, done)])
        };
        passes(reasoning("reasoning_text"));
        assert_eq!(deviation(reasoning("output_text")), "event_item_type");
        assert_eq!(deviation(reasoning("refusal")), "event_item_type");
    }

    #[test]
    fn r7_sequence_numbers() {
        let base = || numbered(stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]));
        let check = |evs: Vec<Value>| run_with(&mut Assembly::with_cap(CAP), &evs).unwrap_err().0;
        let mut evs = base();
        evs[3]["sequence_number"] = json!(1);
        assert_eq!(check(evs), "sequence_number");
        let mut evs = base();
        evs[3].as_object_mut().unwrap().remove("sequence_number");
        assert_eq!(check(evs), "sequence_number");
        let mut evs = base();
        evs[3]["sequence_number"] = json!(3.5);
        assert_eq!(check(evs), "sequence_number");
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs[3]["sequence_number"] = json!(3);
        assert_eq!(check(evs), "sequence_number");
        // None at all is conforming.
        let mut a = Assembly::with_cap(CAP);
        run_with(&mut a, &stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])])).unwrap();
        assert!(!a.sequence_numbers());
    }

    #[test]
    fn r8_item_shape_and_unique_ids() {
        let base = || stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        for edit in [json!({"id": null}), json!({"id": ""}), json!({"id": 7}), json!({"type": 1}), json!({"name": 3}), json!({"call_id": []})] {
            let mut evs = base();
            for (k, v) in edit.as_object().unwrap() {
                evs[2]["item"][k] = v.clone();
            }
            assert_eq!(deviation(evs), "item_shape", "{edit}");
        }
        let mut evs = base();
        evs[2]["item"] = json!("fc_1");
        assert_eq!(deviation(evs), "item_shape");
        let dup = stream(vec![message_item(0, "fc_1", "x"), function_item(1, "fc_1", "call_1", "f", &["{}"])]);
        assert_eq!(deviation(dup), "duplicate_id");
        let dup = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"]), function_item(1, "fc_2", "call_1", "f", &["{}"])]);
        assert_eq!(deviation(dup), "duplicate_call_id");
        assert_eq!(deviation(stream(vec![message_item(0, "msg_paigasus_0", "x")])), "reserved_id");
    }

    #[test]
    fn long_identifiers_are_a_deviation() {
        let long = "n".repeat(MAX_IDENTIFIER_BYTES + 1);
        let max = "n".repeat(MAX_IDENTIFIER_BYTES);
        let mut a = passes(stream(vec![function_item(0, "fc_1", "call_1", &max, &["{}"])]));
        assert_eq!(a.hold_calls()[0].name, max);
        assert_eq!(deviation(stream(vec![function_item(0, "fc_1", "call_1", &long, &["{}"])])), "long_identifier");
        assert_eq!(deviation(stream(vec![function_item(0, "fc_1", &long, "f", &["{}"])])), "long_identifier");
        assert_eq!(deviation(stream(vec![message_item(0, &format!("msg_{long}"), "x")])), "long_identifier");
    }

    #[test]
    fn r9_identity_is_equal_in_every_view() {
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs[5]["item"]["name"] = json!("g");
        assert_eq!(deviation(evs), "item_identity_changed");
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs[6]["response"]["output"][0]["call_id"] = json!("call_2");
        assert_eq!(deviation(evs), "item_identity_changed");
        let mut evs = stream(vec![message_item(0, "msg_1", "x")]);
        evs[7]["item"]["id"] = json!("msg_2");
        assert_eq!(deviation(evs), "item_identity_changed");
        let mut evs = stream(vec![reasoning_item(0, "rs_1", "e")]);
        evs[4]["response"]["output"][0]["type"] = json!("message");
        assert_eq!(deviation(evs), "item_identity_changed");
    }

    #[test]
    fn r10_the_terminal_output_matches_the_items() {
        let base = || stream(vec![message_item(0, "msg_1", "x"), function_item(1, "fc_1", "call_1", "f", &["{}"])]);
        let last = base().len() - 1;
        let mut evs = base();
        evs[last]["response"]["output"].as_array_mut().unwrap().pop();
        assert_eq!(deviation(evs), "terminal_output_mismatch");
        let mut evs = base();
        evs[last]["response"]["output"].as_array_mut().unwrap().push(json!({"id": "x", "type": "message"}));
        assert_eq!(deviation(evs), "terminal_output_mismatch");
        let mut evs = base();
        evs[last]["response"]["output"][1]["arguments"] = json!("{\"a\":1}");
        assert_eq!(deviation(evs), "terminal_output_mismatch");
        let mut evs = base();
        evs[last]["response"]["output"][1] = json!("fc_1");
        assert_eq!(deviation(evs), "terminal_output_mismatch");
        // Key order is not a difference: the terminal item is equal as a JSON value.
        let mut evs = base();
        let item = evs[last]["response"]["output"][1].as_object().unwrap().clone();
        evs[last]["response"]["output"][1] = Value::Object(item.into_iter().rev().collect());
        passes(evs);
        // Amendment A1: a tool-free item needs only R9 in the terminal output.
        let mut evs = stream(vec![reasoning_item(0, "rs_1", "a"), function_item(1, "fc_1", "call_1", "f", &["{}"])]);
        let last = evs.len() - 1;
        evs[last]["response"]["output"][0]["encrypted_content"] = json!("b");
        passes(evs);
    }

    #[test]
    fn r11_function_arguments() {
        let base = || stream(vec![function_item(0, "fc_1", "call_1", "f", &["{\"a\":", "1}"])]);
        let mut evs = base();
        evs[2]["item"]["arguments"] = json!("{");
        assert_eq!(deviation(evs), "added_arguments");
        // The deltas, the `.done` event and the `done` item must agree.
        let mut evs = base();
        evs[4]["delta"] = json!("2}");
        assert_eq!(deviation(evs), "arguments_mismatch");
        let mut evs = base();
        evs[5]["arguments"] = json!("{\"a\":2}");
        assert_eq!(deviation(evs), "arguments_mismatch");
        let mut evs = base();
        evs[6]["item"]["arguments"] = json!("{\"a\":2}");
        evs[7]["response"]["output"][0]["arguments"] = json!("{\"a\":2}");
        assert_eq!(deviation(evs), "arguments_mismatch");
        // A delta after the `.done` event, and a second `.done` event.
        let mut evs = base();
        evs.swap(4, 5);
        assert_eq!(deviation(evs), "arguments_mismatch");
        let mut evs = base();
        evs.insert(6, evs[5].clone());
        assert_eq!(deviation(evs), "arguments_mismatch");
        // A completed call holds one complete JSON value.
        assert_eq!(deviation(stream(vec![function_item(0, "fc_1", "call_1", "f", &["{\"a\":"])])), "incomplete_arguments");
        assert_eq!(deviation(stream(vec![function_item(0, "fc_1", "call_1", "f", &["{} {}"])])), "incomplete_arguments");
    }

    #[test]
    fn r12_custom_input() {
        let item = |input: &str, status: &str| json!({"id": "ctc_1", "type": "custom_tool_call", "status": status, "call_id": "call_c", "input": input, "name": "run"});
        let build = |added_input: &str, delta: &str, done_event: &str, done_input: &str| {
            let done = item(done_input, "completed");
            let evs = vec![
                added(0, item(added_input, "in_progress")),
                scoped("response.custom_tool_call_input.delta", 0, "ctc_1", json!({"delta": delta})),
                scoped("response.custom_tool_call_input.done", 0, "ctc_1", json!({"input": done_event})),
                item_done(0, done.clone()),
            ];
            stream(vec![(evs, done)])
        };
        passes(build("", "not json {", "not json {", "not json {"));
        assert_eq!(deviation(build("", "ls", "ls -la", "ls -la")), "input_mismatch");
        assert_eq!(deviation(build("", "ls", "ls", "rm")), "input_mismatch");
        assert_eq!(deviation(build("l", "ls", "ls", "ls")), "input_mismatch");
    }

    #[test]
    fn r14_the_subset_rule_and_the_diff() {
        let computer = |added_extra: Value| {
            let mut a = json!({"id": "cu_1", "type": "computer_call", "status": "in_progress", "call_id": "call_cu"});
            for (k, v) in added_extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            stream(vec![plain_item(
                0,
                a,
                json!({"id": "cu_1", "type": "computer_call", "status": "completed", "actions": [{"type": "click", "x": 1}], "call_id": "call_cu"}),
            )])
        };
        passes(computer(json!({"actions": []})));
        passes(computer(json!({"actions": null})));
        passes(computer(json!({"actions": [{"type": "click", "x": 1}]})));
        assert_eq!(deviation(computer(json!({"actions": [{"type": "click", "x": 2}]}))), "added_done_differ");
        assert_eq!(deviation(computer(json!({"actions": [{"type": "click"}]}))), "added_done_differ");
        assert_eq!(deviation(computer(json!({"extra": null}))), "added_done_differ");
        assert_eq!(deviation(computer(json!({"actions": {}}))), "added_done_differ");
        // Nested objects follow the rule at every depth.
        let patch = |added_op: Value, deltas: &[&str], done_event: &str, done_diff: &str| {
            let done = json!({"id": "apc_1", "type": "apply_patch_call", "status": "completed", "call_id": "call_p", "operation": {"type": "create_file", "diff": done_diff, "path": "a"}});
            let mut evs = vec![added(
                0,
                json!({"id": "apc_1", "type": "apply_patch_call", "status": "in_progress", "call_id": "call_p", "operation": added_op}),
            )];
            evs.extend(deltas.iter().map(|d| scoped("response.apply_patch_call_operation_diff.delta", 0, "apc_1", json!({"delta": d}))));
            evs.push(scoped("response.apply_patch_call_operation_diff.done", 0, "apc_1", json!({"diff": done_event})));
            evs.push(item_done(0, done.clone()));
            stream(vec![(evs, done)])
        };
        passes(patch(json!({"type": "create_file", "diff": "", "path": "a"}), &["+a"], "+a", "+a"));
        passes(patch(json!({"type": "create_file", "path": null}), &["+a"], "+a", "+a"));
        assert_eq!(deviation(patch(json!({"type": "create_file", "diff": "", "path": "b"}), &["+a"], "+a", "+a")), "added_done_differ");
        assert_eq!(deviation(patch(json!({"type": "create_file", "diff": "", "path": "a"}), &["+a"], "+b", "+b")), "diff_mismatch");
        assert_eq!(deviation(patch(json!({"type": "create_file", "diff": "", "path": "a"}), &["+a"], "+a", "+b")), "diff_mismatch");
    }

    #[test]
    fn r16_no_event_after_done() {
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs.insert(6, scoped("response.function_call_arguments.delta", 0, "fc_1", json!({"delta": "x"})));
        assert_eq!(deviation(evs), "event_after_done");
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs.insert(6, evs[5].clone());
        assert_eq!(deviation(evs), "event_after_done");
    }

    #[test]
    fn truncation_is_reported_not_a_deviation() {
        // R15 takes precedence over R11 (A3): an incomplete call with partial JSON arguments.
        let (mut evs, mut done) = function_item(0, "fc_1", "call_1", "f", &["{\"a\":"]);
        done["status"] = json!("incomplete");
        evs[3]["item"]["status"] = json!("incomplete");
        let mut evs = stream(vec![(evs, done)]);
        let last = evs.len() - 1;
        evs[last]["type"] = json!("response.incomplete");
        let a = passes(evs);
        assert!(a.truncated());
        // A terminal event while a client-run item has no `done`.
        let (first, _) = function_item(0, "fc_1", "call_1", "f", &["{\"a\":1}"]);
        let evs = vec![created(), first[0].clone(), first[1].clone(), completed(vec![fc("fc_1", "call_1", "f", "", "in_progress")])];
        let (a, r) = run(evs);
        assert!(r.unwrap().last().unwrap().terminal);
        assert!(a.truncated());
        // A tool-free item with no `done` is no truncation.
        let (msg, _) = message_item(0, "msg_1", "x");
        let a = passes(vec![created(), msg[0].clone(), completed(vec![msg[0]["item"].clone()])]);
        assert!(!a.truncated());
        assert!(!passes(stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])])).truncated());
    }

    /// The end-of-stream rule (R3, R15, T4d): a hold is cut unless a terminal event completes it.
    #[test]
    fn the_hold_is_cut_unless_a_terminal_event_completes_it() {
        let call = || function_item(0, "fc_1", "call_1", "f", &["{}"]);
        let whole = passes(stream(vec![call()]));
        assert!(whole.ended() && !whole.hold_cut(), "a terminal event with every call done completes the hold");
        let text = passes(stream(vec![message_item(0, "msg_1", "x")]));
        assert!(text.ended() && !text.hold_cut(), "no hold, nothing to cut");
        // R3: every item done, but no terminal event.
        let mut evs = stream(vec![call()]);
        evs.pop();
        let a = passes(evs);
        assert!(!a.ended() && a.hold_cut());
        // R15: the stream stops inside a client-run item.
        let (item, _) = call();
        let a = passes(vec![created(), item[0].clone(), item[1].clone()]);
        assert!(!a.ended() && a.hold_cut());
        // R15 at the terminal event: a `done` that is not completed.
        let (mut evs, mut done) = call();
        done["status"] = json!("incomplete");
        let last = evs.len() - 1;
        evs[last]["item"] = done.clone();
        let a = passes(stream(vec![(evs, done)]));
        assert!(a.ended() && a.hold_cut());
        // T4d: calls over the cap are never decided.
        let mut a = Assembly::with_cap(1);
        run_with(&mut a, &numbered(stream(vec![call()]))).unwrap();
        assert!(a.ended() && a.overflowed() && a.hold_cut());
    }

    #[test]
    fn an_absent_done_status_is_a_truncation_where_the_model_has_one() {
        // Ruling T4b: `function_call` and `computer_call` have a `status` field in the SDK model.
        let without_status = |mut item: Item| {
            item.1.as_object_mut().unwrap().remove("status");
            let last = item.0.len() - 1;
            item.0[last]["item"].as_object_mut().unwrap().remove("status");
            stream(vec![item])
        };
        assert!(passes(without_status(function_item(0, "fc_1", "call_1", "f", &["{}"]))).truncated());
        let computer = plain_item(
            0,
            json!({"id": "cu_1", "type": "computer_call", "status": "in_progress", "call_id": "call_cu"}),
            json!({"id": "cu_1", "type": "computer_call", "status": "completed", "actions": [], "call_id": "call_cu"}),
        );
        assert!(passes(without_status(computer)).truncated());
        // `custom_tool_call_output` declares `status` in the output union (fix round 1).
        let output = |ty: &str| {
            let item = json!({"id": "out_1", "type": ty, "call_id": "call_o", "output": "42"});
            stream(vec![plain_item(0, item.clone(), item)])
        };
        assert!(passes(output("custom_tool_call_output")).truncated());
        assert!(passes(output("function_call_output")).truncated());
        // `custom_tool_call`, `mcp_approval_request` and `mcp_approval_response` have none: an
        // absent status is complete.
        let response = json!({"id": "mcpa_1", "type": "mcp_approval_response", "approval_request_id": "mcpr_0", "approve": true});
        assert!(!passes(stream(vec![plain_item(0, response.clone(), response)])).truncated());
        let custom = plain_item(
            0,
            json!({"id": "ctc_1", "type": "custom_tool_call", "call_id": "call_c", "input": "", "name": "run"}),
            json!({"id": "ctc_1", "type": "custom_tool_call", "call_id": "call_c", "input": "", "name": "run"}),
        );
        assert!(!passes(stream(vec![custom])).truncated());
        let mcp = json!({"id": "mcpr_1", "type": "mcp_approval_request", "server_label": "git", "name": "push", "arguments": "{}"});
        assert!(!passes(stream(vec![plain_item(0, mcp.clone(), mcp)])).truncated());
        // A `null` status is never complete.
        let mut evs = stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]);
        evs[5]["item"]["status"] = json!(null);
        evs[6]["response"]["output"][0]["status"] = json!(null);
        assert!(passes(evs).truncated());
    }

    #[test]
    fn provider_run_items_report_unenforceable() {
        let ws_added = json!({"id": "ws_1", "type": "web_search_call", "status": "in_progress"});
        let ws_done = json!({"id": "ws_1", "type": "web_search_call", "status": "completed", "action": {"type": "search", "query": "q"}});
        let ws = (
            vec![
                added(0, ws_added),
                scoped("response.web_search_call.in_progress", 0, "ws_1", json!({})),
                scoped("response.web_search_call.searching", 0, "ws_1", json!({})),
                scoped("response.web_search_call.completed", 0, "ws_1", json!({})),
                item_done(0, ws_done.clone()),
            ],
            ws_done,
        );
        let (a, r) = run(stream(vec![ws.clone()]));
        let effects = r.unwrap();
        assert!(!a.hold_open(), "a provider-run item opens no hold");
        assert_eq!(
            effects[2].report,
            Some(ReportEntry {
                choice: 0,
                position: 0,
                name: "web_search_call".to_owned(),
                decision: ReportDecision::Unenforceable,
                basis: None,
                code: None,
                enforced: None,
                reason: None,
            })
        );
        assert!(effects.iter().all(|e| e.item.is_none() && !e.opened));
        assert_eq!(effects.iter().filter(|e| e.report.is_some()).count(), 1);
        assert_eq!(
            effects[2..7].iter().map(|e| e.frame).collect::<Vec<_>>(),
            [ItemFrame::Added, ItemFrame::Middle, ItemFrame::Middle, ItemFrame::Middle, ItemFrame::Done]
        );
        let mut a = passes(stream(vec![ws, function_item(1, "fc_1", "call_1", "f", &["{}"])]));
        assert_eq!(a.slot_of(0), None);
        assert_eq!(a.hold_calls().len(), 1);
    }

    #[test]
    fn the_hold_opens_at_the_first_client_run_added() {
        let (a, r) = run(stream(vec![
            message_item(0, "msg_1", "x"),
            reasoning_item(1, "rs_1", "e"),
            function_item(2, "fc_1", "call_1", "f", &["{}"]),
            function_item(3, "fc_2", "call_2", "g", &["{}"]),
        ]));
        let effects = r.unwrap();
        let opened: Vec<usize> = effects.iter().enumerate().filter(|(_, e)| e.opened).map(|(i, _)| i).collect();
        assert_eq!(opened, [10]);
        assert_eq!((effects[10].frame, effects[10].item), (ItemFrame::Added, Some(2)));
        assert!(a.hold_open());
        assert!(!passes(stream(vec![message_item(0, "msg_1", "x"), reasoning_item(1, "rs_1", "e")])).hold_open());
    }

    #[test]
    fn retained_bytes_are_bounded() {
        let cap = 256;
        let chunk = "x".repeat(100);
        let mut deltas = vec!["{\"a\":\""];
        deltas.extend(std::iter::repeat_n(chunk.as_str(), 100));
        deltas.push("\"}");
        let evs = numbered(stream(vec![function_item(0, "fc_1", "call_1", "f", &deltas)]));
        let mut a = Assembly::with_cap(cap);
        let mut record = 0;
        for e in &evs {
            record = record.max(serde_json::to_vec(e).unwrap().len());
            a.accept(ty(e), e).unwrap();
            assert!(a.retained() <= cap + record, "retained {} over {cap} + {record}", a.retained());
        }
        assert!(a.overflowed(), "a call over the cap is never decided");
        assert!(a.hold_calls().is_empty());
        assert_eq!(a.retained(), 0);
        // A short call under the same cap is kept.
        let mut a = Assembly::with_cap(cap);
        run_with(&mut a, &numbered(stream(vec![function_item(0, "fc_1", "call_1", "f", &["{}"])]))).unwrap();
        assert!(!a.overflowed());
        assert!(a.retained() > 0);
        assert_eq!(a.hold_calls().len(), 1);
        assert_eq!(a.retained(), 0);
    }

    #[test]
    fn tool_call_maps_the_kinds_of_section_5() {
        let obj = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(tool_call(3, &obj(fc("fc_1", "call_1", "f", "", "completed")), ToolCallKind::Function).unwrap().arguments, "{}");
        assert_eq!(
            tool_call(3, &obj(json!({"type": "function_call", "name": "f", "arguments": "{}"})), ToolCallKind::Function),
            Err(Malformed("call_id"))
        );
        assert_eq!(
            tool_call(0, &obj(json!({"type": "function_call", "call_id": "c", "name": "f", "arguments": 1})), ToolCallKind::Function),
            Err(Malformed("arguments"))
        );
        let search = tool_call(
            0,
            &obj(json!({"id": "ts_1", "type": "tool_search_call", "execution": "server", "arguments": {}})),
            ToolCallKind::ToolSearch,
        )
        .unwrap();
        assert_eq!(search.id, None, "a server tool search has no call_id");
        let big = json!({"id": "x", "type": "computer_call", "call_id": "c", "n": 1e300});
        assert_eq!(tool_call(0, &obj(big), ToolCallKind::Computer), Err(Malformed("lossy_number")));
        assert_eq!(tool_call(0, &obj(json!({"type": "x"})), ToolCallKind::AnthropicToolUse), Err(Malformed("kind")));
    }

    fn assemble_file(path: &std::path::Path) -> Result<Assembly, String> {
        let mut splitter = crate::sse::SseSplitter::new(16 * 1024 * 1024);
        let bytes = std::fs::read(path).unwrap();
        let mut items = splitter.push(&bytes);
        items.extend(splitter.finish());
        let mut a = Assembly::with_cap(crate::limits::Limits::defaults().max_held_bytes);
        let mut terminal = false;
        for item in items {
            let crate::sse::SseItem::Record(r) = item else { return Err("oversized".to_owned()) };
            match classify(&r).map_err(|e| e.to_string())? {
                Event::Known { ty, value } => terminal |= a.accept(ty, &value).map_err(|d| d.0.to_owned())?.terminal,
                Event::Done => {}
                Event::UpstreamError(_) => return Err("upstream error".to_owned()),
            }
        }
        if terminal { Ok(a) } else { Err("no terminal".to_owned()) }
    }

    #[test]
    fn recorded_streams_assemble_without_a_deviation() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/responses/recorded");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "sse") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut a = assemble_file(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            // The `max_output_tokens_in_call` streams end with an incomplete call: R15 (A3).
            assert_eq!(a.truncated(), name.contains("max_output_tokens_in_call"), "{name}");
            assert!(a.sequence_numbers(), "{name}");
            let calls = a.hold_calls();
            assert_eq!(a.hold_open(), !calls.is_empty(), "{name}");
            assert_eq!(calls.is_empty(), name.contains("text_only") || name.contains("structured_output"), "{name}");
            seen += 1;
        }
        assert_eq!(seen, 28);
    }
}
