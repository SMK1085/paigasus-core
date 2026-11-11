// SPDX-License-Identifier: Apache-2.0
//! Canonical tool-call assembly for one OpenAI choice (spec § 4.3, D9).
//!
//! Only choice 0 may carry tool calls, and only at array position 0 of its chunk, because LangChain
//! reads `choices[0]` of every chunk and ignores `choice.index`. A `type` that does not match the body's shape is a deviation, because
//! clients ignore `type` and run the body. Kind Other is only for a call with no body at all.
//!
//! The client SDKs disagree on everything outside this form. openai-python concatenates every
//! string except `index` and `type`, so a second `name` fragment turns `safe_` into `safe_rm`.
//! openai-node replaces the name, and LangChain merges by index value. Inside the canonical form
//! they all agree, so the decider sees exactly what every client will run.
//!
//! Every check runs on a small copy of the state ([`Check`]) before anything is changed, so a
//! deviation leaves the assembly exactly as it was.
//!
//! A function call's arguments are what clients parse, so at completion they must be one complete
//! JSON value ([`complete_arguments`]), as an Anthropic `tool_use` input must. LangChain
//! partial-parses a truncated string (`{"cmd":"ls` runs as `{"cmd":"ls"}`) and openai-node can
//! partial-parse it too, so the decider would see a string the client reads differently.

use serde_json::Value;

use crate::json::parse_value_strict;
use crate::openai::classify::{Fragment, NameArgs};
use crate::protocol::Deviation;
use crate::types::{ToolCall, ToolCallKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Function,
    Custom,
}

/// The class of a `type` value: all that the canonical checks read of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TyClass {
    Function,
    Custom,
    Other,
}

/// What the canonical checks read of one call. It is small, so a whole record can be checked
/// against a copy before anything is changed: a deviation must never leave half-applied state, for
/// example a hold marked open that the interceptor never registered.
#[derive(Clone, Copy, Default)]
struct Shadow {
    id: bool,
    ty: Option<TyClass>,
    shape: Option<Shape>,
    name: bool,
}

/// The canonical-form state of one choice (spec § 4.3), without the strings.
#[derive(Clone, Default)]
pub(crate) struct Check {
    calls: Vec<Shadow>,
    /// `Some(has_name)` once a legacy `function_call` fragment arrived.
    legacy: Option<bool>,
}

fn once(slot: &mut bool, present: bool, why: &'static str) -> Result<(), Deviation> {
    if present {
        if *slot {
            return Err(Deviation(why));
        }
        *slot = true;
    }
    Ok(())
}

impl Check {
    fn apply(&mut self, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<(), Deviation> {
        let mixes = (legacy.is_some() && (!frags.is_empty() || !self.calls.is_empty())) || (!frags.is_empty() && self.legacy.is_some());
        if mixes {
            return Err(Deviation("legacy_and_tool_calls"));
        }
        if let Some(l) = legacy {
            once(self.legacy.get_or_insert(false), l.name.is_some(), "repeated_name")?;
        }
        for f in frags {
            let pos = f.index as usize;
            if pos == self.calls.len() {
                self.calls.push(Shadow::default());
            } else if pos > self.calls.len() {
                return Err(Deviation("index_gap"));
            }
            let b = &mut self.calls[pos];
            once(&mut b.id, f.id.is_some(), "repeated_id")?;
            if let Some(t) = &f.ty {
                if b.ty.is_some() {
                    return Err(Deviation("repeated_type"));
                }
                b.ty = Some(match t.as_str() {
                    "function" => TyClass::Function,
                    "custom" => TyClass::Custom,
                    _ => TyClass::Other,
                });
            }
            let (shape, na) = match (&f.function, &f.custom) {
                (Some(na), None) => (Some(Shape::Function), Some(na)),
                (None, Some(na)) => (Some(Shape::Custom), Some(na)),
                _ => (None, None),
            };
            if let Some(s) = shape {
                match b.shape {
                    None => b.shape = Some(s),
                    Some(prev) if prev != s => return Err(Deviation("shape_changed")),
                    Some(_) => {}
                }
            }
            if let Some(na) = na {
                once(&mut b.name, na.name.is_some(), "repeated_name")?;
            }
        }
        Ok(())
    }

    /// The kind of every call once the choice finishes, or the deviation that finishing is.
    pub(crate) fn finish(&self) -> Result<Vec<ToolCallKind>, Deviation> {
        if let Some(has_name) = self.legacy {
            if !has_name {
                return Err(Deviation("missing_name"));
            }
            return Ok(vec![ToolCallKind::LegacyFunctionCall]);
        }
        self.calls
            .iter()
            .map(|b| {
                // Clients ignore `type` and run the body, so any type other than the one the
                // body's shape expects is a deviation, never kind Other (spec I1).
                let kind = match (b.shape, b.ty) {
                    (Some(Shape::Function), Some(TyClass::Function) | None) => ToolCallKind::Function,
                    (Some(Shape::Custom), Some(TyClass::Custom) | None) => ToolCallKind::Custom,
                    (Some(_), Some(_)) => return Err(Deviation("type_shape_mismatch")),
                    (None, Some(TyClass::Function | TyClass::Custom)) => return Err(Deviation("missing_body")),
                    (None, _) => ToolCallKind::Other,
                };
                if kind != ToolCallKind::Other && !b.name {
                    return Err(Deviation("missing_name"));
                }
                Ok(kind)
            })
            .collect()
    }
}

#[derive(Default)]
struct CallBuild {
    id: Option<String>,
    ty: Option<String>,
    name: Option<String>,
    args: String,
    raws: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct ChoiceAssembly {
    calls: Vec<CallBuild>,
    legacy: Option<CallBuild>,
    check: Check,
}

/// Tool data is allowed only in choice 0, and only at array position 0 of its chunk (spec § 4.3).
/// LangChain reads `choices[0]` of every chunk and ignores `choice.index`. So a call split across
/// choices 0 and 1 would be merged by it but assembled separately here (`multi_choice_tool_calls`),
/// and a choice-0 fragment behind another choice's entry would never reach it
/// (`choice_position`). Text in other choices and positions stays allowed.
pub(crate) fn check_tool_choice(choice: u32, position: usize, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<(), Deviation> {
    if frags.is_empty() && legacy.is_none() {
        return Ok(());
    }
    if choice != 0 {
        return Err(Deviation("multi_choice_tool_calls"));
    }
    if position != 0 {
        return Err(Deviation("choice_position"));
    }
    Ok(())
}

/// The arguments the decider sees for a completed call of `kind`. A function or legacy call's
/// arguments must be one complete JSON value; an empty string is presented as `{}`, as an empty
/// Anthropic input is. A custom tool's input is free-form text and is not checked; kind Other
/// carries its raw entry.
pub(crate) fn complete_arguments(kind: ToolCallKind, args: String) -> Result<String, Deviation> {
    match kind {
        ToolCallKind::Function | ToolCallKind::LegacyFunctionCall => {
            if args.is_empty() {
                Ok("{}".to_owned())
            } else if parse_value_strict(args.as_bytes()).is_ok() {
                Ok(args)
            } else {
                Err(Deviation("incomplete_arguments"))
            }
        }
        _ => Ok(args),
    }
}

impl ChoiceAssembly {
    /// The canonical-form state after these fragments, or the deviation they are. Changes nothing.
    pub(crate) fn check(&self, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<Check, Deviation> {
        let mut c = self.check.clone();
        c.apply(frags, legacy)?;
        Ok(c)
    }

    /// The current canonical-form state.
    pub(crate) fn state(&self) -> &Check {
        &self.check
    }

    /// Apply fragments that [`ChoiceAssembly::check`] accepted, with `after` its result. Cannot fail.
    pub(crate) fn commit(&mut self, frags: &[Fragment], legacy: Option<&NameArgs>, after: Check) {
        if let Some(l) = legacy {
            let b = self.legacy.get_or_insert_with(CallBuild::default);
            if let Some(n) = &l.name {
                b.name = Some(n.clone());
            }
            if let Some(a) = &l.args {
                b.args.push_str(a);
            }
        }
        for f in frags {
            let pos = f.index as usize;
            while self.calls.len() <= pos {
                self.calls.push(CallBuild::default());
            }
            let b = &mut self.calls[pos];
            if let Some(id) = &f.id {
                b.id = Some(id.clone());
            }
            if let Some(t) = &f.ty {
                b.ty = Some(t.clone());
            }
            if let Some(na) = f.function.as_ref().or(f.custom.as_ref()) {
                if let Some(n) = &na.name {
                    b.name = Some(n.clone());
                }
                if let Some(a) = &na.args {
                    b.args.push_str(a);
                }
            }
            b.raws.push(f.raw.clone());
        }
        self.check = after;
    }

    /// Check, then commit. On a deviation nothing is changed.
    pub(crate) fn apply(&mut self, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<(), Deviation> {
        let after = self.check(frags, legacy)?;
        self.commit(frags, legacy, after);
        Ok(())
    }

    /// Phase-1 form of [`ChoiceAssembly::finish`]: whether finishing would succeed once `frags` and
    /// `legacy` (already accepted by [`ChoiceAssembly::check`], with result `after`) are applied.
    /// Changes nothing.
    pub(crate) fn check_finish(&self, frags: &[Fragment], legacy: Option<&NameArgs>, after: &Check) -> Result<(), Deviation> {
        let kinds = after.finish()?;
        let pending = |own: Option<&CallBuild>, more: &mut dyn Iterator<Item = &str>| -> String {
            let mut a = own.map(|b| b.args.clone()).unwrap_or_default();
            more.for_each(|x| a.push_str(x));
            a
        };
        if after.legacy.is_some() {
            let args = pending(self.legacy.as_ref(), &mut legacy.and_then(|l| l.args.as_deref()).into_iter());
            return complete_arguments(ToolCallKind::LegacyFunctionCall, args).map(drop);
        }
        for (i, kind) in kinds.into_iter().enumerate() {
            let mut more = frags
                .iter()
                .filter(|f| f.index as usize == i)
                .filter_map(|f| f.function.as_ref().or(f.custom.as_ref()).and_then(|na| na.args.as_deref()));
            complete_arguments(kind, pending(self.calls.get(i), &mut more))?;
        }
        Ok(())
    }

    pub(crate) fn finish(&self, choice: u32) -> Result<Vec<ToolCall>, Deviation> {
        let kinds = self.check.finish()?;
        if let Some(b) = &self.legacy {
            return Ok(vec![ToolCall {
                choice,
                position: 0,
                id: None,
                name: b.name.clone().unwrap_or_default(),
                kind: ToolCallKind::LegacyFunctionCall,
                arguments: complete_arguments(ToolCallKind::LegacyFunctionCall, b.args.clone())?,
            }]);
        }
        let mut out = Vec::with_capacity(self.calls.len());
        for ((i, b), kind) in self.calls.iter().enumerate().zip(kinds) {
            let (name, arguments) = if kind == ToolCallKind::Other {
                let name = b.name.clone().or_else(|| b.ty.clone()).unwrap_or_else(|| "unknown".to_owned());
                (name, serde_json::to_string(&b.raws).expect("a Value array always serializes"))
            } else {
                (b.name.clone().unwrap_or_default(), complete_arguments(kind, b.args.clone())?)
            };
            let position = u32::try_from(i).expect("at most 128 calls per choice");
            out.push(ToolCall {
                choice,
                position,
                id: b.id.clone(),
                name,
                kind,
                arguments,
            });
        }
        Ok(out)
    }

    pub(crate) fn names(&self) -> Vec<(u32, String)> {
        let name = |b: &CallBuild| b.name.clone().unwrap_or_else(|| "unknown".to_owned());
        if let Some(b) = &self.legacy {
            return vec![(0, name(b))];
        }
        self.calls.iter().enumerate().map(|(i, b)| (u32::try_from(i).expect("at most 128 calls per choice"), name(b))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_strict;
    use crate::openai::classify::{Chunk, chunk};
    use crate::types::ToolCallKind;

    /// The fragments and legacy part of choice 0 of one chunk.
    fn frags(s: &str) -> (Vec<Fragment>, Option<NameArgs>) {
        let mut v = parse_strict(s.as_bytes()).unwrap();
        v["object"] = crate::openai::classify::CHUNK_OBJECT.into();
        let Chunk::Choices(mut v) = chunk(&v).unwrap() else { panic!() };
        let c = v.remove(0);
        (c.fragments, c.legacy)
    }
    fn tc(entries: &str) -> String {
        format!(r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{entries}]}}}}]}}"#)
    }
    fn feed(a: &mut ChoiceAssembly, s: &str) -> Result<(), Deviation> {
        let (f, l) = frags(s);
        a.apply(&f, l.as_ref())
    }

    #[test]
    fn two_interleaved_calls_assemble() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c0","type":"function","function":{"name":"read","arguments":"{\"p\":"}}"#)).unwrap();
        feed(&mut a, &tc(r#"{"index":1,"id":"c1","type":"function","function":{"name":"rm","arguments":"{"}}"#)).unwrap();
        feed(&mut a, &tc(r#"{"index":0,"function":{"arguments":"1}"}},{"index":1,"function":{"arguments":"}"}}"#)).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].name.as_str(), calls[0].arguments.as_str(), calls[0].kind), ("read", "{\"p\":1}", ToolCallKind::Function));
        assert_eq!((calls[1].position, calls[1].id.as_deref(), calls[1].arguments.as_str()), (1, Some("c1"), "{}"));
    }

    #[test]
    fn a_custom_call_assembles_from_input() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"custom","custom":{"name":"sql","input":"SELECT"}}"#)).unwrap();
        feed(&mut a, &tc(r#"{"index":0,"custom":{"input":" 1"}}"#)).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!((calls[0].kind, calls[0].arguments.as_str()), (ToolCallKind::Custom, "SELECT 1"));
    }

    #[test]
    fn a_legacy_call_assembles() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f","arguments":"{"}}}]}"#).unwrap();
        feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"arguments":"}"}}}]}"#).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!((calls[0].kind, calls[0].id.clone(), calls[0].arguments.as_str()), (ToolCallKind::LegacyFunctionCall, None, "{}"));
    }

    /// Argument keys are application data (I-1): PascalCase and `constructor` keys pass, while a
    /// duplicate key or `__proto__` is still a deviation.
    #[test]
    fn argument_keys_are_application_data() {
        for kind in [ToolCallKind::Function, ToolCallKind::LegacyFunctionCall] {
            for ok in [r#"{"Id":1,"Name":"Alice","Type":"pdf"}"#, r#"{"constructor":"x","prototype":{"Content":1}}"#] {
                assert_eq!(complete_arguments(kind, ok.to_owned()).as_deref(), Ok(ok), "{ok}");
            }
            for bad in [r#"{"a":1,"a":2}"#, r#"{"x":{"__proto__":1}}"#] {
                assert_eq!(complete_arguments(kind, bad.to_owned()), Err(Deviation("incomplete_arguments")), "{bad}");
            }
        }
    }

    #[test]
    fn an_unknown_type_is_other_with_its_raw_fragments() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"shell","shell":{"cmd":"ls"}}"#)).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!((calls[0].kind, calls[0].name.as_str()), (ToolCallKind::Other, "shell"));
        let raw: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(raw[0]["shell"]["cmd"], "ls");
    }

    #[test]
    fn every_canonical_violation_is_a_deviation() {
        let cases: &[(&[&str], &str)] = &[
            (&[r#"{"index":1,"id":"c","type":"function","function":{"name":"x"}}"#], "index_gap"),
            (
                &[r#"{"index":0,"id":"c","type":"function","function":{"name":"safe_"}}"#, r#"{"index":0,"function":{"name":"rm"}}"#],
                "repeated_name",
            ),
            (&[r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#, r#"{"index":0,"id":"d"}"#], "repeated_id"),
            (
                &[r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#, r#"{"index":0,"type":"function"}"#],
                "repeated_type",
            ),
            (
                &[r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#, r#"{"index":0,"custom":{"input":"y"}}"#],
                "shape_changed",
            ),
        ];
        for (seq, want) in cases {
            let mut a = ChoiceAssembly::default();
            let mut got = None;
            for s in *seq {
                if let Err(d) = feed(&mut a, &tc(s)) {
                    got = Some(d.0);
                    break;
                }
            }
            assert_eq!(got, Some(*want), "{seq:?}");
        }
    }

    #[test]
    fn finish_rejects_incomplete_or_contradictory_calls() {
        let cases = [
            (r#"{"index":0,"id":"c","type":"function","function":{"arguments":"{}"}}"#, "missing_name"),
            (r#"{"index":0,"id":"c","type":"function","custom":{"name":"x"}}"#, "type_shape_mismatch"),
            (r#"{"index":0,"id":"c","type":"function"}"#, "missing_body"),
        ];
        for (s, want) in cases {
            let mut a = ChoiceAssembly::default();
            feed(&mut a, &tc(s)).unwrap();
            assert_eq!(a.finish(0).unwrap_err().0, want, "{s}");
        }
    }

    #[test]
    fn legacy_and_tool_calls_never_share_a_choice() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#)).unwrap();
        assert_eq!(
            feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f"}}}]}"#).unwrap_err().0,
            "legacy_and_tool_calls"
        );
    }

    #[test]
    fn names_report_unknown_until_a_name_arrives() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"function"}"#)).unwrap();
        assert_eq!(a.names(), vec![(0, "unknown".to_owned())]);
        feed(&mut a, &tc(r#"{"index":0,"function":{"name":"rm"}}"#)).unwrap();
        assert_eq!(a.names(), vec![(0, "rm".to_owned())]);
    }

    #[test]
    fn an_unexpected_type_with_a_body_is_a_deviation() {
        let cases = [
            r#"{"index":0,"id":"c","type":"shell","function":{"name":"x","arguments":"{}"}}"#,
            r#"{"index":0,"id":"c","type":"Function","function":{"name":"x","arguments":"{}"}}"#,
            r#"{"index":0,"id":"c","type":"shell","custom":{"name":"x","input":"y"}}"#,
        ];
        for s in cases {
            let mut a = ChoiceAssembly::default();
            feed(&mut a, &tc(s)).unwrap();
            assert_eq!(a.finish(0).unwrap_err().0, "type_shape_mismatch", "{s}");
        }
    }

    #[test]
    fn tool_fragments_outside_choice_zero_are_a_deviation() {
        let (f, l) = frags(&tc(r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#));
        assert_eq!(check_tool_choice(1, 0, &f, l.as_ref()).unwrap_err().0, "multi_choice_tool_calls");
        assert!(check_tool_choice(0, 0, &f, l.as_ref()).is_ok());
        let (f, l) = frags(r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f"}}}]}"#);
        assert_eq!(check_tool_choice(2, 0, &f, l.as_ref()).unwrap_err().0, "multi_choice_tool_calls");
        let (f, l) = frags(r#"{"choices":[{"index":0,"delta":{"content":"hi"}}]}"#);
        assert!(check_tool_choice(1, 1, &f, l.as_ref()).is_ok());
    }

    #[test]
    fn tool_fragments_behind_another_choice_entry_are_a_deviation() {
        // LangChain reads choices[0] only, so choice 0 at array position 1 never reaches it.
        let (f, l) = frags(&tc(r#"{"index":0,"function":{"arguments":"x"}}"#));
        assert_eq!(check_tool_choice(0, 1, &f, l.as_ref()).unwrap_err().0, "choice_position");
        let (f, l) = frags(r#"{"choices":[{"index":0,"delta":{"function_call":{"arguments":"x"}}}]}"#);
        assert_eq!(check_tool_choice(0, 2, &f, l.as_ref()).unwrap_err().0, "choice_position");
        // Both rules compose: a later choice at a later position is still a multi-choice call.
        assert_eq!(check_tool_choice(1, 1, &f, l.as_ref()).unwrap_err().0, "multi_choice_tool_calls");
    }

    #[test]
    fn function_arguments_must_be_one_complete_json_value() {
        let cases: &[(&[&str], Result<&str, &str>)] = &[
            (&[""], Ok("{}")),
            (&[r#"{\"p\":"#, "1}"], Ok(r#"{"p":1}"#)),
            (&[r#"{\"p\":"#, "1"], Err("incomplete_arguments")),
            (&[r#"{\"a\":1}"#, r#"{\"b\":2}"#], Err("incomplete_arguments")),
            (&[r#"{\"a\":1,\"a\":2}"#], Err("incomplete_arguments")),
        ];
        for (pieces, want) in cases {
            let mut a = ChoiceAssembly::default();
            for (k, p) in pieces.iter().enumerate() {
                let e = if k == 0 {
                    format!(r#"{{"index":0,"id":"c","type":"function","function":{{"name":"x","arguments":"{p}"}}}}"#)
                } else {
                    format!(r#"{{"index":0,"function":{{"arguments":"{p}"}}}}"#)
                };
                feed(&mut a, &tc(&e)).unwrap();
            }
            let got = a.finish(0).map(|c| c[0].arguments.clone()).map_err(|d| d.0);
            assert_eq!(got.as_deref().map_err(|e| *e), *want, "{pieces:?}");
        }
        // The legacy call follows the same rule; a custom tool's free-form input does not.
        let mut a = ChoiceAssembly::default();
        feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f","arguments":"{"}}}]}"#).unwrap();
        assert_eq!(a.finish(0).unwrap_err().0, "incomplete_arguments");
        let mut a = ChoiceAssembly::default();
        feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f"}}}]}"#).unwrap();
        assert_eq!(a.finish(0).unwrap()[0].arguments, "{}");
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"custom","custom":{"name":"sql","input":"SELECT ("}}"#)).unwrap();
        assert_eq!(a.finish(0).unwrap()[0].arguments, "SELECT (");
    }

    #[test]
    fn check_finish_agrees_with_finish() {
        // Phase 1 of the stream step must reject exactly what finish rejects.
        for (first, last) in [(r#"{\"p\":"#, "1}"), (r#"{\"p\":"#, "1"), ("", ""), ("{}", "{}")] {
            let mut a = ChoiceAssembly::default();
            feed(&mut a, &tc(&format!(r#"{{"index":0,"id":"c","type":"function","function":{{"name":"x","arguments":"{first}"}}}}"#))).unwrap();
            let (f, l) = frags(&tc(&format!(r#"{{"index":0,"function":{{"arguments":"{last}"}}}}"#)));
            let after = a.check(&f, l.as_ref()).unwrap();
            let phase1 = a.check_finish(&f, l.as_ref(), &after).map_err(|d| d.0);
            a.commit(&f, l.as_ref(), after);
            assert_eq!(phase1, a.finish(0).map(drop).map_err(|d| d.0), "{first:?} + {last:?}");
        }
    }
}
