// SPDX-License-Identifier: Apache-2.0
//! The request rewrite (§ 6.2, § 6.5.1 steps 4-6, § 6.5.2, P-8, P-9): apply the enforced items of
//! a [`RequestPlan`] to the body bytes. The body is edited through `RawValue` slices. Only the
//! members that change are written again (`tools`, `functions`, `tool_choice`, `function_call`,
//! `parallel_tool_calls`, and the replaced result items); every other member keeps its original
//! bytes, so every untouched number keeps its original text. No I/O.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use paigasus_kernel::tool_policy::normalise;

use super::plan::{ChoiceRewrite, REMOVABLE_BUILT_INS, RequestPlan, ResultTarget};
use super::raw::{ObjectEdit, RawArray, RawObject, top_level};
use crate::report::{RequestAction, RequestReason, sanitize_name};
use crate::turn::{ResultShape, responses_result_shape};
use crate::types::Format;

/// The fixed notice that replaces a tool result (§ 6.5.2, ADR-0027 (c)). It is built only from the
/// reason and the cleaned tool name, never from the result content (SMA-749 D21).
#[must_use]
pub fn result_notice(tool: &str, reason: RequestReason) -> String {
    format!("[paigasus] tool result for \"{}\" was replaced ({}).", sanitize_name(tool), reason.as_str())
}

/// A JSON string literal.
fn json_str(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

/// The enforced items of the plan, ready to match against the body.
struct Enforced<'p> {
    /// Normalised display names of the definitions to remove.
    removed: BTreeSet<&'p str>,
    /// The notice (as a JSON string) for each call id to replace.
    by_id: BTreeMap<&'p str, String>,
    /// The notice for each legacy chat result name to replace. The name is matched exactly as the
    /// plan keyed it: raw, not normalised (`x` and `X` are two items).
    by_name: BTreeMap<&'p str, String>,
    /// The notice for each `local_shell_call_output` `id` to replace.
    by_shell_id: BTreeMap<&'p str, String>,
}

impl<'p> Enforced<'p> {
    fn new(plan: &'p RequestPlan) -> Self {
        let removed = plan.removed.iter().filter(|i| i.enforced).map(|i| i.target.as_str()).collect();
        // The plan pushes one `replace-result` entry for each replaced item, in the same order.
        let entries = plan.entries.iter().filter(|e| e.action == RequestAction::ReplaceResult);
        let mut by_id = BTreeMap::new();
        let mut by_name = BTreeMap::new();
        let mut by_shell_id = BTreeMap::new();
        for (item, entry) in plan.replaced.iter().zip(entries) {
            if !item.enforced {
                continue;
            }
            let notice = json_str(&result_notice(&entry.tool, entry.reason));
            match &item.target {
                ResultTarget::CallId(id) => by_id.insert(id.as_str(), notice),
                ResultTarget::Name(n) => by_name.insert(n.as_str(), notice),
                ResultTarget::LocalShellId(id) => by_shell_id.insert(id.as_str(), notice),
            };
        }
        Enforced { removed, by_id, by_name, by_shell_id }
    }

    fn gone(&self, display_name: &str) -> bool {
        self.removed.contains(normalise(display_name).as_str())
    }

    /// The notice for a result with this call id, if the plan replaces it.
    fn id_notice(&self, obj: &RawObject, key: &str) -> Option<&String> {
        obj.str(key).and_then(|id| self.by_id.get(id.as_str()))
    }

    /// The notice for a `local_shell_call_output` by its `id`, if the plan replaces it.
    fn shell_notice(&self, obj: &RawObject) -> Option<&String> {
        obj.str("id").and_then(|id| self.by_shell_id.get(id.as_str()))
    }
}

/// Apply the enforced items of `plan` to `body` (§ 6.5.1 step 6). `None` when the plan changes
/// nothing (`is_noop`), and also when `body` is not one JSON object; the caller plans only from a
/// body that `parse_request` read, so the second case does not occur. The caller handles
/// `plan.refusal()` before it calls this function. `None` also when an enforced target is a
/// `computer_call_output`: it has no replacement (SMA-776 § 6.3), so the caller must not send the
/// body.
#[must_use]
pub fn rewrite_request(format: Format, body: &[u8], plan: &RequestPlan) -> Option<Bytes> {
    if plan.is_noop() {
        return None;
    }
    let text = std::str::from_utf8(body).ok()?;
    let (at, top) = top_level(text)?;
    let obj = RawObject::parse(top.get())?;
    let enforced = Enforced::new(plan);
    let mut edit = obj.edit();
    match format {
        Format::OpenAiChat => chat(&obj, &enforced, &plan.choice, &mut edit),
        Format::OpenAiResponses => responses(&obj, &enforced, &plan.choice, &mut edit)?,
        Format::AnthropicMessages => anthropic(&obj, &enforced, &plan.choice, &mut edit),
    }
    let end = at + top.get().len();
    let mut out = String::with_capacity(body.len());
    out.push_str(&text[..at]);
    out.push_str(&edit.render());
    out.push_str(&text[end..]);
    Some(Bytes::from(out))
}

/// What a definition array became.
enum Defs {
    /// Nothing was removed.
    Same,
    /// Some entries were removed; this is the new array.
    Fewer(String),
    /// Every entry was removed (§ 6.5.1 step 5).
    Empty,
}

/// Remove the entries of `arr` for which `gone` is true.
fn filter_defs(arr: &RawArray, gone: impl Fn(&RawObject) -> bool) -> Defs {
    let mut e = arr.edit();
    for (i, el) in arr.elements.iter().enumerate() {
        if RawObject::parse(el.value.get()).is_some_and(|o| gone(&o)) {
            e.drop(i);
        }
    }
    match (e.changed(), e.kept()) {
        (false, _) => Defs::Same,
        (true, 0) => Defs::Empty,
        (true, _) => Defs::Fewer(e.render()),
    }
}

/// Apply a [`Defs`] to the member `key`. `true` when the array is now empty and was dropped.
fn apply_defs(edit: &mut ObjectEdit, key: &str, defs: Defs) -> bool {
    match defs {
        Defs::Same => false,
        Defs::Fewer(json) => {
            edit.replace(key, json);
            false
        }
        Defs::Empty => {
            edit.drop(key);
            true
        }
    }
}

/// Keep only the elements of an `allowed_tools` list whose name is in `names`.
fn narrow(arr: &RawArray, names: &[String], name_of: impl Fn(&RawObject) -> Option<String>) -> String {
    let mut e = arr.edit();
    for (i, el) in arr.elements.iter().enumerate() {
        let name = RawObject::parse(el.value.get()).and_then(|o| name_of(&o));
        if !name.is_some_and(|n| names.contains(&n)) {
            e.drop(i);
        }
    }
    e.render()
}

/// The name of a chat `{"type":"function","function":{"name":..}}` or `custom` entry.
fn chat_entry_name(o: &RawObject) -> Option<String> {
    let ty = o.str("type")?;
    if !matches!(ty.as_str(), "function" | "custom") {
        return None;
    }
    o.object(&ty)?.str("name")
}

/// A Responses `function` or `custom` entry: its display name (`namespace.name` with a namespace).
fn responses_entry_name(o: &RawObject) -> Option<String> {
    let name = o.str("name")?;
    Some(match o.str("namespace") {
        Some(ns) => format!("{ns}.{name}"),
        None => name,
    })
}

fn chat(top: &RawObject, en: &Enforced, choice: &ChoiceRewrite, edit: &mut ObjectEdit) {
    chat_messages(top, en, edit);
    let tools_gone = top
        .array("tools")
        .is_some_and(|a| apply_defs(edit, "tools", filter_defs(&a, |t| chat_entry_name(t).is_some_and(|n| en.gone(&n)))));
    let functions_gone = top
        .array("functions")
        .is_some_and(|a| apply_defs(edit, "functions", filter_defs(&a, |f| f.str("name").is_some_and(|n| en.gone(&n)))));
    if tools_gone {
        edit.drop("tool_choice");
        edit.drop("parallel_tool_calls");
    }
    if functions_gone {
        edit.drop("functions");
        edit.drop("function_call");
    }
    // The choice lives in `tool_choice`, or in the legacy `function_call` when `tool_choice` is
    // absent or null (as `turn.rs` reads it).
    let in_tool_choice = top.get("tool_choice").is_some_and(|v| v.get() != "null");
    let (key, dropped) = if in_tool_choice { ("tool_choice", tools_gone) } else { ("function_call", functions_gone) };
    if dropped {
        return;
    }
    match choice {
        ChoiceRewrite::Keep => {}
        ChoiceRewrite::Auto => {
            edit.replace(key, json_str("auto"));
        }
        ChoiceRewrite::Allowed(names) => {
            // `{"type":"allowed_tools","allowed_tools":{"mode":..,"tools":[..]}}`: only `tools`
            // changes; `mode` and every other member keep their bytes.
            let Some(tc) = top.object("tool_choice") else { return };
            let Some(inner) = tc.object("allowed_tools") else { return };
            let Some(list) = inner.array("tools") else { return };
            let mut ie = inner.edit();
            ie.replace("tools", narrow(&list, names, chat_entry_name));
            let mut te = tc.edit();
            te.replace("allowed_tools", ie.render());
            edit.replace("tool_choice", te.render());
        }
    }
}

/// Edit the chat messages: replace the content of each enforced `role:"tool"` and legacy
/// `role:"function"` result.
fn chat_messages(top: &RawObject, en: &Enforced, edit: &mut ObjectEdit) {
    let Some(msgs) = top.array("messages") else { return };
    let mut me = msgs.edit();
    for (i, el) in msgs.elements.iter().enumerate() {
        let Some(m) = RawObject::parse(el.value.get()) else { continue };
        let notice = match m.str("role").as_deref() {
            Some("tool") => en.id_notice(&m, "tool_call_id"),
            Some("function") => m.str("name").and_then(|n| en.by_name.get(n.as_str())),
            _ => None,
        };
        if let Some(notice) = notice {
            let mut e = m.edit();
            e.set("content", notice.clone());
            me.replace(i, e.render());
        }
    }
    if me.changed() {
        edit.replace("messages", me.render());
    }
}

/// The one notice chunk of a blocked `shell_call_output`. `notice` is a JSON string literal.
fn shell_chunk(notice: &str) -> String {
    format!(r#"[{{"stdout":"","stderr":{notice},"outcome":{{"type":"exit","exit_code":1}}}}]"#)
}

/// `None` when an enforced target is a `computer_call_output` (fail closed).
fn responses(top: &RawObject, en: &Enforced, choice: &ChoiceRewrite, edit: &mut ObjectEdit) -> Option<()> {
    let tools_gone = top.array("tools").is_some_and(|a| {
        let mut e = a.edit();
        for (i, el) in a.elements.iter().enumerate() {
            let Some(t) = RawObject::parse(el.value.get()) else { continue };
            let ty = t.str("type").unwrap_or_default();
            match ty.as_str() {
                "function" | "custom" => {
                    if t.str("name").is_some_and(|n| en.gone(&n)) {
                        e.drop(i);
                    }
                }
                "namespace" => {
                    let ns = t.str("name").unwrap_or_default();
                    let Some(inner) = t.array("tools") else { continue };
                    match filter_defs(&inner, |d| d.str("name").is_some_and(|n| en.gone(&format!("{ns}.{n}")))) {
                        Defs::Same => {}
                        Defs::Fewer(json) => {
                            let mut te = t.edit();
                            te.replace("tools", json);
                            e.replace(i, te.render());
                        }
                        // A namespace with no inner tools left is removed (§ 6.5.1 step 5).
                        Defs::Empty => e.drop(i),
                    }
                }
                // A built-in client-run tool is named by its type (P-15); a hosted tool stays.
                other => {
                    if REMOVABLE_BUILT_INS.contains(&other) && en.gone(other) {
                        e.drop(i);
                    }
                }
            }
        }
        match (e.changed(), e.kept()) {
            (false, _) => false,
            (true, 0) => {
                edit.drop("tools");
                true
            }
            (true, _) => {
                edit.replace("tools", e.render());
                false
            }
        }
    });
    if tools_gone {
        edit.drop("tool_choice");
        edit.drop("parallel_tool_calls");
    } else {
        match choice {
            ChoiceRewrite::Keep => {}
            ChoiceRewrite::Auto => {
                edit.replace("tool_choice", json_str("auto"));
            }
            ChoiceRewrite::Allowed(names) => {
                // `{"type":"allowed_tools","mode":..,"tools":[..]}`: only `tools` changes.
                if let Some(tc) = top.object("tool_choice")
                    && let Some(list) = tc.array("tools")
                {
                    let mut te = tc.edit();
                    te.replace("tools", narrow(&list, names, responses_entry_name));
                    edit.replace("tool_choice", te.render());
                }
            }
        }
    }
    let Some(items) = top.array("input") else { return Some(()) };
    let mut ie = items.edit();
    for (i, el) in items.elements.iter().enumerate() {
        let Some(item) = RawObject::parse(el.value.get()) else { continue };
        let ty = item.str("type").unwrap_or_default();
        let mut e = item.edit();
        // One arm per shape: a new shape does not compile until it has an arm here.
        match responses_result_shape(&ty) {
            None => continue,
            Some(ResultShape::Text) => {
                let shell = || if ty == "local_shell_call_output" { en.shell_notice(&item) } else { None };
                let Some(notice) = en.id_notice(&item, "call_id").or_else(shell) else { continue };
                e.set("output", notice.clone());
            }
            Some(ResultShape::ShellChunks) => {
                let Some(notice) = en.id_notice(&item, "call_id") else { continue };
                e.set("output", shell_chunk(notice));
            }
            // A screenshot has no text member, so it has no replacement (SMA-776 § 6.3).
            Some(ResultShape::Screenshot) => {
                if en.id_notice(&item, "call_id").is_some() {
                    return None;
                }
                continue;
            }
            Some(ResultShape::Approval) => {
                // The approval becomes a refusal, with the notice as its reason.
                let Some(notice) = en.id_notice(&item, "approval_request_id") else { continue };
                e.set("approve", "false".to_owned());
                e.set("reason", notice.clone());
            }
        }
        ie.replace(i, e.render());
    }
    if ie.changed() {
        edit.replace("input", ie.render());
    }
    Some(())
}

fn anthropic(top: &RawObject, en: &Enforced, choice: &ChoiceRewrite, edit: &mut ObjectEdit) {
    // A custom tool has no `type` or `type: "custom"`; a server tool is never removed. The plan
    // already keeps the last definition when blocks refer to tools (its item is not enforced).
    let custom = |t: &RawObject| matches!(t.str("type").as_deref(), None | Some("custom"));
    let tools_gone = top
        .array("tools")
        .is_some_and(|a| apply_defs(edit, "tools", filter_defs(&a, |t| custom(t) && t.str("name").is_some_and(|n| en.gone(&n)))));
    if tools_gone {
        edit.drop("tool_choice");
    } else if *choice == ChoiceRewrite::Auto
        && let Some(tc) = top.object("tool_choice")
    {
        // `{"type":"tool","name":..}` becomes `{"type":"auto"}`; other members (for example
        // `disable_parallel_tool_use`) keep their bytes.
        let mut te = tc.edit();
        te.replace("type", json_str("auto"));
        te.drop("name");
        edit.replace("tool_choice", te.render());
    }
    let Some(msgs) = top.array("messages") else { return };
    let mut me = msgs.edit();
    for (i, el) in msgs.elements.iter().enumerate() {
        let Some(m) = RawObject::parse(el.value.get()) else { continue };
        let Some(blocks) = m.array("content") else { continue };
        let mut be = blocks.edit();
        for (j, b) in blocks.elements.iter().enumerate() {
            let Some(block) = RawObject::parse(b.value.get()) else { continue };
            if block.str("type").as_deref() != Some("tool_result") {
                continue;
            }
            let Some(notice) = en.id_notice(&block, "tool_use_id") else { continue };
            let mut e = block.edit();
            e.set("content", notice.clone());
            e.set("is_error", "true".to_owned());
            be.replace(j, e.render());
        }
        if be.changed() {
            let mut e = m.edit();
            e.replace("content", be.render());
            me.replace(i, e.render());
        }
    }
    if me.changed() {
        edit.replace("messages", me.render());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::policy::plan::PlanItem;
    use crate::report::RequestEntry;

    /// A hand-made plan: the service refuses this plan before the rewrite, so only a direct call
    /// can give the rewrite an enforced computer target (SMA-776 § 6.3, fail closed).
    #[test]
    fn an_enforced_computer_output_target_fails_closed() {
        let body = br#"{"model":"m","input":[{"type":"computer_call","call_id":"c3","actions":[{"type":"screenshot"}]},{"type":"computer_call_output","call_id":"c3","output":{"type":"computer_screenshot","image_url":"SECRET"}},{"type":"function_call_output","call_id":"c3","output":"x"}]}"#;
        let mut plan = RequestPlan {
            removed: Vec::new(),
            replaced: vec![PlanItem {
                target: ResultTarget::CallId("c3".to_owned()),
                enforced: true,
            }],
            choice: ChoiceRewrite::Keep,
            refusals: BTreeMap::new(),
            entries: vec![RequestEntry::new("computer".to_owned(), RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)],
        };
        assert!(!plan.is_noop());
        assert_eq!(rewrite_request(Format::OpenAiResponses, body, &plan), None);
        plan.replaced[0].enforced = false;
        assert_eq!(rewrite_request(Format::OpenAiResponses, body, &plan), None, "a no-op plan");
    }
}
