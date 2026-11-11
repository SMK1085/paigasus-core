// SPDX-License-Identifier: Apache-2.0
//! Canonical tool-call assembly for one OpenAI choice (spec § 4.3, D9).
//!
//! The client SDKs disagree on everything outside this form. openai-python concatenates every
//! string except `index` and `type`, so a second `name` fragment turns `safe_` into `safe_rm`.
//! openai-node replaces the name, and LangChain merges by index value. Inside the canonical form
//! they all agree, so the decider sees exactly what every client will run.
#![allow(dead_code)] // Removed in Task 8, when the interceptor uses every item.

use serde_json::Value;

use crate::openai::classify::{Fragment, NameArgs};
use crate::protocol::Deviation;
use crate::types::{ToolCall, ToolCallKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Function,
    Custom,
}

#[derive(Default)]
struct CallBuild {
    id: Option<String>,
    ty: Option<String>,
    shape: Option<Shape>,
    name: Option<String>,
    args: String,
    raws: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct ChoiceAssembly {
    calls: Vec<CallBuild>,
    legacy: Option<CallBuild>,
}

fn set_once(slot: &mut Option<String>, v: &Option<String>, why: &'static str) -> Result<(), Deviation> {
    if let Some(v) = v {
        if slot.is_some() {
            return Err(Deviation(why));
        }
        *slot = Some(v.clone());
    }
    Ok(())
}

impl ChoiceAssembly {
    pub(crate) fn apply(&mut self, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<(), Deviation> {
        let mixes = (legacy.is_some() && (!frags.is_empty() || !self.calls.is_empty())) || (!frags.is_empty() && self.legacy.is_some());
        if mixes {
            return Err(Deviation("legacy_and_tool_calls"));
        }
        if let Some(l) = legacy {
            let b = self.legacy.get_or_insert_with(CallBuild::default);
            set_once(&mut b.name, &l.name, "repeated_name")?;
            if let Some(a) = &l.args {
                b.args.push_str(a);
            }
        }
        for f in frags {
            let pos = f.index as usize;
            if pos == self.calls.len() {
                self.calls.push(CallBuild::default());
            } else if pos > self.calls.len() {
                return Err(Deviation("index_gap"));
            }
            let b = &mut self.calls[pos];
            set_once(&mut b.id, &f.id, "repeated_id")?;
            set_once(&mut b.ty, &f.ty, "repeated_type")?;
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
                set_once(&mut b.name, &na.name, "repeated_name")?;
                if let Some(a) = &na.args {
                    b.args.push_str(a);
                }
            }
            b.raws.push(f.raw.clone());
        }
        Ok(())
    }

    pub(crate) fn finish(&self, choice: u32) -> Result<Vec<ToolCall>, Deviation> {
        if let Some(b) = &self.legacy {
            let name = b.name.clone().ok_or(Deviation("missing_name"))?;
            return Ok(vec![ToolCall {
                choice,
                position: 0,
                id: None,
                name,
                kind: ToolCallKind::LegacyFunctionCall,
                arguments: b.args.clone(),
            }]);
        }
        let mut out = Vec::with_capacity(self.calls.len());
        for (i, b) in self.calls.iter().enumerate() {
            let kind = match (b.ty.as_deref(), b.shape) {
                (Some("function") | None, Some(Shape::Function)) => ToolCallKind::Function,
                (Some("custom") | None, Some(Shape::Custom)) => ToolCallKind::Custom,
                (Some("function"), Some(Shape::Custom)) | (Some("custom"), Some(Shape::Function)) => return Err(Deviation("type_shape_mismatch")),
                (Some("function" | "custom"), None) => return Err(Deviation("missing_body")),
                _ => ToolCallKind::Other,
            };
            let (name, arguments) = if kind == ToolCallKind::Other {
                let name = b.name.clone().or_else(|| b.ty.clone()).unwrap_or_else(|| "unknown".to_owned());
                (name, serde_json::to_string(&b.raws).expect("a Value array always serializes"))
            } else {
                (b.name.clone().ok_or(Deviation("missing_name"))?, b.args.clone())
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
        let Chunk::Choices(mut v) = chunk(&parse_strict(s.as_bytes()).unwrap()).unwrap() else { panic!() };
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
}
