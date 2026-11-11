// SPDX-License-Identifier: Apache-2.0
//! P1 (invariant I1) and P2 (bounded memory) over generated hostile streams (spec § 8.1, AC 3, AC 4).

mod support;

use paigasus_gateway_core::{Format, OnFailure};
use proptest::prelude::*;
use support::driver::{DECIDERS, Outcome, allow_all, block_named, cut, drive, limits};
use support::oracle::{check_i1, runners};
use support::streams::{Quirk, anthropic_scenario, openai_scenario, render_anthropic, render_openai};

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// P1 (invariant I1), through every client model; see `oracle::check_i1`.
fn check_p1(format: Format, input: &[u8], o: &Outcome) -> Result<(), TestCaseError> {
    check_i1(format, input, &o.out, &o.decided, o.core_failed()).map_err(TestCaseError::fail)
}

fn check_canary(out: &[u8]) -> Result<(), TestCaseError> {
    prop_assert!(!show(out).contains("CANARY"), "a blocked call's canary leaked:\n{}", show(out));
    Ok(())
}

/// Every decider of spec § 8.1, `on_failure = block`: no canary under a blocking decider, P1, P2.
fn check_all_deciders(format: Format, bytes: &[u8], cuts: &[usize]) -> Result<(), TestCaseError> {
    let l = limits(OnFailure::Block);
    for (name, decide) in DECIDERS {
        let o = drive(format, l, &cut(bytes, cuts), decide);
        prop_assert!(o.finished, "decider {name}: the interceptor never ended:\n{}", show(&o.out));
        if *name != "allow-all" {
            check_canary(&o.out).map_err(|e| TestCaseError::fail(format!("decider {name}: {e}")))?;
        }
        check_p1(format, bytes, &o).map_err(|e| TestCaseError::fail(format!("decider {name}: {e}")))?;
        prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes, "decider {name}: queue {} over the bound", o.max_queued);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn openai_p1_and_p2_hold(s in openai_scenario()) {
        check_all_deciders(Format::OpenAiChat, &render_openai(&s), &s.cuts)?;
    }

    #[test]
    fn anthropic_p1_and_p2_hold(s in anthropic_scenario()) {
        check_all_deciders(Format::AnthropicMessages, &render_anthropic(&s), &s.cuts)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// Observe mode is not a security mode, but it must never panic and must stay bounded. Its
    /// output must parse for every client when the scenario has at most one quirk: pass-through
    /// forwards later records unclassified (§ 5.3, P-4), so a second quirk reaches the client as
    /// the upstream sent it (ruling D9 (a)).
    #[test]
    fn observe_mode_is_bounded_and_parseable(s in openai_scenario(), a in anthropic_scenario()) {
        for (format, bytes, cuts, quirks) in [
            (Format::OpenAiChat, render_openai(&s), &s.cuts, s.quirks.len()),
            (Format::AnthropicMessages, render_anthropic(&a), &a.cuts, a.quirks.len()),
        ] {
            let l = limits(OnFailure::Allow);
            let o = drive(format, l, &cut(&bytes, cuts), &block_named);
            prop_assert!(o.finished, "observe mode never ended:\n{}", show(&o.out));
            prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes);
            if quirks <= 1 {
                for (model, run) in runners(format) {
                    let r = run(&o.out);
                    prop_assert!(r.error.is_none() || o.core_failed() || run(&bytes).error.is_some(), "{model} failed in observe mode: {:?}\n{}", r.error, show(&o.out));
                }
            }
        }
    }

    /// Allow-all with only framing quirks: the decider sees exactly the calls every model derives.
    #[test]
    fn allow_all_models_agree_with_the_decider(mut s in openai_scenario()) {
        s.quirks.retain(|(q, _)| matches!(q, Quirk::Interleave | Quirk::Comments | Quirk::UnknownFields | Quirk::Bom));
        let bytes = render_openai(&s);
        let o = drive(Format::OpenAiChat, limits(OnFailure::Block), &cut(&bytes, &s.cuts), &allow_all);
        prop_assert!(o.finished);
        check_p1(Format::OpenAiChat, &bytes, &o)?;
        if !o.core_failed() {
            for (model, run) in runners(Format::OpenAiChat) {
                let derived = run(&o.out);
                if derived.error.is_some() {
                    // LangChain raises on any custom tool call, upstream bytes included.
                    prop_assert!(run(&bytes).error.is_some(), "{} raised: {:?}", model, derived.error);
                    continue;
                }
                prop_assert_eq!(derived.calls.len(), o.decided.len(), "{}: every decided call reaches the client", model);
            }
        }
    }
}
