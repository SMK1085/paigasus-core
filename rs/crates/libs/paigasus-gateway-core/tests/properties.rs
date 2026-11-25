// SPDX-License-Identifier: Apache-2.0
//! P1 (invariant I1) and P2 (bounded memory) over generated hostile streams (spec § 8.1, AC 3, AC 4).

mod support;

use paigasus_gateway_core::{Decision, Format, Metric, OnFailure, ToolCall, ToolCallKind};
use proptest::prelude::*;
use support::driver::{DECIDERS, Outcome, allow_all, block_named, cut, drive, limits};
use support::oracle::{Run, check_i1, runners};
use support::responses_oracle::{run_node, run_python_stream};
use support::streams::{
    AQuirk, AnthropicScenario, Block, Quirk, RKind, RQUIRKS, RQuirk, anthropic_scenario, anthropic_scenario_with_quirks, every_kind_scenario, openai_scenario, render_anthropic, render_openai,
    render_responses, responses_scenario,
};

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// P1 (invariant I1), through every client model; see `oracle::check_i1`. A Responses abort on an
/// upstream error record inside the hold (749 D17, SMA-769 § 6.3) is a failure path too: it ends
/// the stream with the D13 frame, so openai-python, which passes an `error` event through, raises
/// at EOF. The chat and Anthropic checks are unchanged.
fn check_p1(format: Format, input: &[u8], o: &Outcome) -> Result<(), TestCaseError> {
    let failed = o.core_failed() || (format == Format::OpenAiResponses && o.aborted);
    check_i1(format, input, &o.out, &o.decided, failed).map_err(TestCaseError::fail)
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

    /// SMA-769 AC 4 and AC 5: P1 and P2 over generated Responses streams (spec § 10.3).
    #[test]
    fn responses_p1_and_p2_hold(s in responses_scenario()) {
        check_all_deciders(Format::OpenAiResponses, &render_responses(&s), &s.cuts)?;
    }
}

/// Whether a Responses model shows a call of this kind at all. LangChain drops `shell_call` and
/// `local_shell_call`, and it has no surface for an item of kind `Other` (spec § 10.1).
fn shown_by(model: &str, c: &ToolCall) -> bool {
    model != "langchain-responses" || !matches!(c.kind, ToolCallKind::Shell | ToolCallKind::LocalShell | ToolCallKind::Other)
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

    /// SMA-758: observe mode over the wide Anthropic generator with two or three quirks. It ends,
    /// stays bounded, never forwards a blocked call's arguments, and keeps the order of the
    /// upstream data payloads it forwards.
    #[test]
    fn anthropic_observe_mode_holds_for_many_quirks(a in anthropic_scenario_with_quirks(2..4)) {
        let bytes = render_anthropic(&a);
        let l = limits(OnFailure::Allow);
        let o = drive(Format::AnthropicMessages, l, &cut(&bytes, &a.cuts), &block_named);
        prop_assert!(o.finished, "observe mode never ended:\n{}", show(&o.out));
        prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes, "queue {} over the bound", o.max_queued);
        let out = show(&o.out);
        // A malformed record switches the stream to pass-through, which releases the hold and
        // forwards unclassified bytes (§ 5.3, ruling D9 (a)), so the check holds without it only.
        let passed_through = out.contains("paigasus-interception pass-through");
        for (call, d) in o.decided.iter().filter(|_| !passed_through) {
            if matches!(d, Decision::Block { .. }) && !call.arguments.is_empty() {
                let escaped = serde_json::to_string(&call.arguments).expect("a string serialises");
                prop_assert!(!out.contains(&call.arguments), "blocked arguments forwarded:\n{out}");
                prop_assert!(!out.contains(&escaped[1..escaped.len() - 1]), "blocked arguments forwarded (escaped):\n{out}");
            }
        }
        let payloads = |b: &str| b.lines().filter_map(|l| l.strip_prefix("data: ").map(str::to_owned)).collect::<Vec<_>>();
        let input = payloads(&show(&bytes));
        // Each forwarded payload maps to the first input index after the previous one. A record
        // the interceptor dropped can leave an identical later record, so the first unused index
        // would map it too early.
        let mut next = 0;
        for p in payloads(&out) {
            if !input.contains(&p) {
                continue;
            }
            match input[next..].iter().position(|i| *i == p) {
                Some(k) => next += k + 1,
                None => prop_assert!(false, "payload {p} out of order:\n{out}"),
            }
        }
    }

    /// The Responses variant of `observe_mode_is_bounded_and_parseable` (spec § 10.3).
    #[test]
    fn responses_observe_mode_is_bounded_and_parseable(s in responses_scenario()) {
        let bytes = render_responses(&s);
        let l = limits(OnFailure::Allow);
        let o = drive(Format::OpenAiResponses, l, &cut(&bytes, &s.cuts), &block_named);
        prop_assert!(o.finished, "observe mode never ended:\n{}", show(&o.out));
        prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes);
        if s.quirks.len() <= 1 {
            for (model, run) in runners(Format::OpenAiResponses) {
                let r = run(&o.out);
                prop_assert!(r.error.is_none() || o.core_failed() || o.aborted || run(&bytes).error.is_some(), "{model} failed in observe mode: {:?}\n{}", r.error, show(&o.out));
            }
        }
    }

    /// The Responses variant of `allow_all_models_agree_with_the_decider`: with only framing
    /// quirks, every model shows exactly the decided calls of the kinds it has a surface for.
    #[test]
    fn responses_allow_all_models_agree_with_the_decider(mut s in responses_scenario()) {
        s.quirks.retain(|(q, _)| matches!(q, RQuirk::KeepaliveWithoutSequence | RQuirk::Bom));
        let bytes = render_responses(&s);
        let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &cut(&bytes, &s.cuts), &allow_all);
        prop_assert!(o.finished);
        check_p1(Format::OpenAiResponses, &bytes, &o)?;
        if !o.core_failed() && !o.aborted {
            for (model, run) in runners(Format::OpenAiResponses) {
                let derived = run(&o.out);
                if derived.error.is_some() {
                    // openai-node raises on the apply_patch diff events, upstream bytes included.
                    prop_assert!(run(&bytes).error.is_some(), "{} raised: {:?}", model, derived.error);
                    continue;
                }
                let shown = o.decided.iter().filter(|(c, _)| shown_by(model, c)).count();
                prop_assert_eq!(derived.calls.len(), shown, "{}: every decided call reaches the client", model);
            }
        }
    }

    /// Allow-all with only framing quirks: the decider sees exactly the calls every model derives.
    #[test]
    fn allow_all_models_agree_with_the_decider(mut s in openai_scenario()) {
        s.quirks.retain(|(q, _)| matches!(q, Quirk::Interleave | Quirk::Comments | Quirk::UnknownFields | Quirk::Bom | Quirk::CapitalisedArgKeys));
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    /// Argument keys are application data (I-1): PascalCase, `constructor` and `prototype` keys
    /// in arguments never make the core fail, and every model derives exactly the decided calls.
    #[test]
    fn capitalised_argument_keys_are_conforming(mut s in openai_scenario(), mut a in anthropic_scenario()) {
        s.quirks = vec![(Quirk::CapitalisedArgKeys, 0)];
        for c in s.choices.iter_mut().skip(1) {
            c.calls.clear(); // later choices' tool calls are unsupported while interception is on
        }
        a.quirks = vec![(AQuirk::CapitalisedArgKeys, 0)];
        for (format, bytes, cuts) in [(Format::OpenAiChat, render_openai(&s), &s.cuts), (Format::AnthropicMessages, render_anthropic(&a), &a.cuts)] {
            let o = drive(format, limits(OnFailure::Block), &cut(&bytes, cuts), &allow_all);
            prop_assert!(o.finished);
            prop_assert!(!o.core_failed(), "the core failed on capitalised argument keys:\n{}", show(&bytes));
            check_p1(format, &bytes, &o)?;
            for (model, run) in runners(format) {
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

/// SMA-758: the P1 oracle sees a pass-through that forwards the hidden call.
#[test]
fn a_pass_through_interceptor_fails_anthropic_p1() {
    let s = AnthropicScenario {
        blocks: vec![Block::Tool {
            name: "blocked_rm",
            args: "{\"v\":\"CANARY-x\",\"n\":10.5}".to_owned(),
            pieces: 2,
        }],
        quirks: vec![],
        cuts: vec![],
    };
    let input = render_anthropic(&s);
    let o = Outcome {
        out: input.clone(),
        decided: vec![],
        max_queued: 0,
        finished: true,
        metrics: vec![],
        aborted: false,
    };
    assert!(check_p1(Format::AnthropicMessages, &input, &o).is_err());
}

/// What the core does with a stream that has one quirk (allow-all, `on_failure = block`).
#[derive(Debug, PartialEq, Eq)]
enum Reach {
    /// A malformed record or a § 6.4 deviation: the record is dropped as `malformed`.
    Malformed,
    /// R15: `on_failure` with code `truncated`, and no malformed record.
    Truncated,
    /// An upstream error record: no failure metric; the core aborts with D13 inside the hold.
    Upstream,
    /// R3: the record after the terminal event is dropped and counted, nothing else.
    AfterEnd,
    /// D16: a record over `max_record_bytes` is dropped as `oversized`.
    Oversized,
    /// D13: the held bytes outgrew `max_held_bytes`: `on_failure` with code `hold_limit`.
    HoldLimit,
    /// No failure at all.
    Clean,
}

fn reach(q: RQuirk, at: usize) -> Reach {
    use RQuirk::*;
    match q {
        TopLevelError | ErrorEventMidItem => Reach::Upstream,
        RecordAfterTerminal => Reach::AfterEnd,
        Oversized => Reach::Oversized,
        HoldLimit => Reach::HoldLimit,
        Bom | NoFinalBlankLine | KeepaliveWithoutSequence => Reach::Clean,
        AbsentStatus if at % 2 == 1 => Reach::Clean,
        FailedWithOpenItem if at.is_multiple_of(2) => Reach::Truncated,
        DoneIncomplete | NoTerminal | AbsentStatus | EofInItem => Reach::Truncated,
        _ => Reach::Malformed,
    }
}

fn reached(o: &Outcome) -> Reach {
    let has = |m: Metric| o.metrics.contains(&m);
    if has(Metric::OnFailure("hold_limit")) {
        Reach::HoldLimit
    } else if has(Metric::RecordDropped("oversized")) {
        Reach::Oversized
    } else if has(Metric::RecordDropped("malformed")) {
        Reach::Malformed
    } else if has(Metric::OnFailure("truncated")) {
        Reach::Truncated
    } else if has(Metric::RecordDropped("after_done")) && !o.metrics.iter().any(|m| matches!(m, Metric::OnFailure(_))) {
        Reach::AfterEnd
    } else if !o.core_failed() && (o.aborted || show(&o.out).contains("\"boom\"")) {
        Reach::Upstream
    } else if !o.core_failed() && !o.aborted {
        Reach::Clean
    } else {
        panic!("an unexpected outcome: {:?}", o.metrics)
    }
}

/// Ruling F3 and F12: every quirk, alone on a stream with every item kind, reaches the outcome of
/// its rule, and P1 holds. (The deviation label is crate-private. The label each quirk reaches
/// first was measured once with a print in `Assembly::accept`; see the SMA-769 Task 8 report.)
/// The client surfaces of the R-d fill and of ruling T7e see the hidden call on the upstream
/// bytes, so the generated cases exercise them.
#[test]
fn each_responses_quirk_reaches_its_rule() {
    for q in RQUIRKS {
        for at in 0..4usize {
            for second in [false, true] {
                let mut s = every_kind_scenario(second);
                s.quirks = vec![(q, at)];
                s.ensure_targets();
                let bytes = render_responses(&s);
                let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&bytes], &allow_all);
                assert!(o.finished, "{q:?} {at}");
                assert_eq!(reached(&o), reach(q, at), "{q:?} at {at}: {:?}\n{}", o.metrics, show(&bytes));
                check_p1(Format::OpenAiResponses, &bytes, &o).unwrap_or_else(|e| panic!("{q:?} at {at}: {e}"));
                let ids = |r: Run| r.calls.into_iter().filter_map(|c| c.id).collect::<Vec<_>>();
                match q {
                    // R-d: openai-python fills the absent `response.failed` output from its items.
                    RQuirk::FailedWithOpenItem if at % 2 == 1 => {
                        let last = s.items.iter().rposition(|i| i.kind.client_run()).expect("ensured");
                        let open = s.items[last].kind;
                        if open != RKind::McpApproval {
                            assert!(ids(run_python_stream(&bytes)).contains(&format!("call_{last}")), "{q:?} {at}");
                        }
                    }
                    // T7e: openai-node counts the output of the terminal event it emitted, then raises.
                    // (It raises earlier at an apply_patch diff event, § 6.3.)
                    RQuirk::TerminalItemNeverStreamed if at % 2 == 1 && !s.items.iter().any(|i| i.kind == RKind::ApplyPatch) => {
                        let r = run_node(&bytes);
                        assert!(r.error.is_some() && ids(r).contains(&"call_hidden".to_owned()), "{q:?} {at}");
                    }
                    RQuirk::TerminalItemNeverStreamed => assert!(ids(run_python_stream(&bytes)).contains(&"call_hidden".to_owned()), "{q:?} {at}"),
                    _ => {}
                }
            }
        }
    }
}

/// `ItemInterleaved` changes the stream when the last client-run item has no item after it: with
/// two function items, every position picks the first one, so the quirk reaches R5.
#[test]
fn item_interleaved_reaches_r5_when_the_last_item_is_client_run() {
    for at in 0..4usize {
        let mut s = every_kind_scenario(false);
        let function = s.items.iter().find(|i| i.kind == RKind::Function).cloned().expect("a function item");
        s.items = vec![function.clone(), function];
        s.quirks = vec![(RQuirk::ItemInterleaved, at)];
        s.ensure_targets();
        let bytes = render_responses(&s);
        let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&bytes], &allow_all);
        assert!(o.finished, "at {at}");
        assert_eq!(reached(&o), Reach::Malformed, "at {at}: {:?}\n{}", o.metrics, show(&bytes));
        check_p1(Format::OpenAiResponses, &bytes, &o).unwrap_or_else(|e| panic!("at {at}: {e}"));
    }
}

/// Two quirks in one stream never break the generator, and P1 holds (two `FirstNotCreated`
/// quirks once emptied the lifecycle records before a swap).
#[test]
fn every_pair_of_responses_quirks_renders_and_holds_p1() {
    for (k, a) in RQUIRKS.iter().enumerate() {
        for b in &RQUIRKS[k..] {
            for at in 0..2usize {
                let mut s = every_kind_scenario(at == 1);
                s.quirks = vec![(*a, at), (*b, at + 1)];
                s.ensure_targets();
                let bytes = render_responses(&s);
                let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&bytes], &allow_all);
                assert!(o.finished, "{a:?} {b:?} {at}");
                check_p1(Format::OpenAiResponses, &bytes, &o).unwrap_or_else(|e| panic!("{a:?} {b:?} {at}: {e}"));
            }
        }
    }
}
