// SPDX-License-Identifier: Apache-2.0
//! The request-side plan of one request (§ 6.5.1 steps 1-4, P-7, P-9, P-15): which definitions to
//! remove, which tool results to replace, and how `tool_choice` changes. Each item carries its own
//! `enforced` flag; `rewrite_request` applies only the enforced items. No I/O.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use paigasus_kernel::detect::{DETECTOR_BUILT_IN, DETECTOR_ERROR, Finding, Severity, call_hosts};
use paigasus_kernel::tool_policy::{
    Declared, Effect, Facts, Findings, NAME_COLLISION, PolicyOrigin, PolicyRef, ToolInput, ToolInvocation, ToolPolicies, ToolVerdict, Verdict, decide, decide_tool, normalise,
};
use sha2::{Digest, Sha256};

use super::calls::{cedar_detail, kernel_findings};
use super::mapping::{definition_call_name, input_for, name_collisions};
use super::snapshot::{PolicySet, PolicySnapshot, TenantMode, TenantPart};
use crate::decision_log::CedarDetail;
use crate::report::{RequestAction, RequestEntry, RequestReason};
use crate::turn::{PriorCall, ResultShape, ToolChoice, ToolDefinition, TurnFacts};
use crate::types::{Format, ToolCallKind};

/// The Responses built-in client-run tools that the plan may remove (P-15). Every other
/// definition of kind `Other` is a hosted tool that the provider runs, and is never removed.
pub(crate) const REMOVABLE_BUILT_INS: [&str; 5] = ["local_shell", "shell", "apply_patch", "computer", "computer_use_preview"];

/// The tool result to replace (P-7).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultTarget {
    /// Every result that refers to this call id.
    CallId(String),
    /// A legacy chat `role:"function"` result with this name (it has no call id).
    Name(String),
    /// Every Responses `local_shell_call_output` whose `id` is this. The id matched the `call_id`
    /// or the item `id` of an earlier `local_shell_call`.
    LocalShellId(String),
}

/// The `tool_choice` after the enforced removals (§ 6.5.1 step 4).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceRewrite {
    /// Leave `tool_choice` as it is.
    Keep,
    /// Set `tool_choice` to `auto`.
    Auto,
    /// Keep only these allowed tools (display names, as the caller spelled them).
    Allowed(Vec<String>),
}

/// One planned change. Only an enforced item changes the body (P-9).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem<T> {
    pub target: T,
    pub enforced: bool,
}

/// Why the plan refuses the request (SMA-776 § 6.2). The order is the order of the plan steps.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusalCause {
    /// A blocked result has a shape that the gateway cannot replace with a notice.
    UnreplaceableResult,
    /// The `tool_choice` has an unknown shape and a removal exists (D17).
    UnknownToolChoice,
}

/// The request-side plan.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPlan {
    /// The normalised display names of the definitions to remove. Every definition with that
    /// name is removed (P-7).
    pub removed: Vec<PlanItem<String>>,
    pub replaced: Vec<PlanItem<ResultTarget>>,
    /// Computed from the enforced removals only.
    pub choice: ChoiceRewrite,
    /// Each cause that the plan found, and whether it is enforced (SMA-776 § 6.2).
    pub refusals: BTreeMap<RefusalCause, bool>,
    /// The report entries, in the order: removals, replacements, the `tool_choice` reset.
    pub entries: Vec<RequestEntry>,
    /// The Cedar detail of each entry, in the order of `entries` (SMA-855 § 6.11.2). Always as
    /// long as `entries`. `Some` for a replacement of a target with findings.
    pub entry_details: Vec<Option<CedarDetail>>,
    /// One record per target with findings that the plan does not replace, and one per result
    /// with findings that names no call (P-6). Not report entries.
    pub screen_records: Vec<ScreenRecord>,
}

/// The decision of a tool result with findings that the plan does not replace (§ 6.11.2). It is
/// not a report entry: it is written as a `SCREEN_RESULT` record.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRecord {
    /// The name of the first earlier call of the target, else the result's name, else empty.
    pub tool: String,
    /// Verdict A (the decision with the result's findings) of that call.
    pub cedar: CedarDetail,
}

impl RequestPlan {
    fn empty() -> Self {
        RequestPlan {
            removed: Vec::new(),
            replaced: Vec::new(),
            choice: ChoiceRewrite::Keep,
            refusals: BTreeMap::new(),
            entries: Vec::new(),
            entry_details: Vec::new(),
            screen_records: Vec::new(),
        }
    }

    /// Add one report entry and its detail, so that the two lists stay in step.
    fn push_entry(&mut self, entry: RequestEntry, detail: Option<CedarDetail>) {
        self.entries.push(entry);
        self.entry_details.push(detail);
    }

    /// Whether the result at `index` of `turn.tool_results` is in a target that the plan
    /// replaces with `enforced = true` (D14: its hosts are then not seen).
    #[must_use]
    pub fn replaced_enforced(&self, turn: &TurnFacts, index: usize) -> bool {
        self.enforced_results(turn).get(index).copied().unwrap_or(false)
    }

    /// For each entry of `turn.tool_results`, whether it is in a target that the plan replaces
    /// with `enforced = true` (D14). The enforced targets are indexed once, so the cost is
    /// `O((results + targets) log targets)`, not one scan of the targets per result.
    #[must_use]
    pub fn enforced_results(&self, turn: &TurnFacts) -> Vec<bool> {
        let (mut ids, mut names, mut shells) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for item in self.replaced.iter().filter(|i| i.enforced) {
            match &item.target {
                ResultTarget::CallId(id) => ids.insert(id.as_str()),
                ResultTarget::Name(n) => names.insert(n.as_str()),
                ResultTarget::LocalShellId(id) => shells.insert(id.as_str()),
            };
        }
        turn.tool_results
            .iter()
            .map(|r| {
                let by_call = match r.call_id.as_deref() {
                    Some(id) => ids.contains(id),
                    None => r.name.as_deref().is_some_and(|n| names.contains(n)),
                };
                by_call || r.item_id.as_deref().is_some_and(|id| shells.contains(id))
            })
            .collect()
    }

    /// The first enforced cause in map order, or `None`.
    #[must_use]
    pub fn refusal(&self) -> Option<RefusalCause> {
        self.refusals.iter().find_map(|(c, e)| e.then_some(*c))
    }

    /// Add a cause; `enforced` is ORed into the value.
    fn add_refusal(&mut self, cause: RefusalCause, enforced: bool) {
        *self.refusals.entry(cause).or_insert(false) |= enforced;
    }

    /// Whether the plan changes nothing that is enforced: the body goes out as it came in.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.refusal().is_none() && self.choice == ChoiceRewrite::Keep && !self.removed.iter().any(|i| i.enforced) && !self.replaced.iter().any(|i| i.enforced)
    }
}

/// The policies by which the findings change the verdict (§ 6.8.2, D12), or `None` when they do
/// not. A is the verdict with the findings, B the one without. F(v) holds only forbids:
/// `v.determining_forbids ∪ v.errored_forbids` (controller ruling on D12). The kernel keeps Cedar's
/// satisfied permits in `determining` for a deny by an errored forbid, and a permit must not make
/// a difference. The findings change the verdict when `A` denies and `B` allows (all of F(A)), or
/// when F(A) is not a subset of F(B) (the policies of F(A) \ F(B)). The list can be empty: a
/// default deny in A against an allow in B.
fn difference<'v>(a: &'v Verdict, b: &Verdict) -> Option<Vec<&'v PolicyRef>> {
    if a.decision != Effect::Deny {
        return None;
    }
    let f_a = a.determining_forbids.iter().chain(&a.errored_forbids);
    if b.decision == Effect::Allow {
        return Some(f_a.collect());
    }
    let in_b = |p: &PolicyRef| b.determining_forbids.contains(p) || b.errored_forbids.contains(p);
    let more: Vec<&PolicyRef> = f_a.filter(|p| !in_b(p)).collect();
    (!more.is_empty()).then_some(more)
}

/// The findings of each result. `given` is empty (no detector ran) or has one list per result.
/// Any other count is a defect of the caller: no list can be matched to its result, so every
/// result gets one `detector_error` (detector `built_in`, high) in place of the lists (A3).
fn findings_per_result(results: usize, given: &[Vec<Finding>]) -> Cow<'_, [Vec<Finding>]> {
    if given.is_empty() || given.len() == results {
        Cow::Borrowed(given)
    } else {
        Cow::Owned(vec![vec![Finding::new(DETECTOR_BUILT_IN, DETECTOR_ERROR, Severity::High, "")]; results])
    }
}

/// The most distinct differential pairs (§ 6.8.2) that one request decides. Each pair is two
/// Cedar decisions, and the client controls how many targets and earlier calls a request has.
/// Equal pairs are decided once (the pair cache), so only distinct pairs count. Past the budget,
/// every target with findings that is not yet decided is decided by one pseudo pair for its tool
/// name: the D32 pseudo call (input `{}`), with one `detector_error` (detector `built_in`, high)
/// added to A. The D12 predicate and the normal accounting decide it, so a finding never
/// replaces a result without a policy that reads it (A1). Spec A13.
pub(crate) const PAIR_BUDGET: usize = 64;

/// A target's findings with the `detector_error` (detector `built_in`, high) that says that the
/// request went past `PAIR_BUDGET`.
fn with_budget_error(given: &[Finding]) -> Vec<Finding> {
    let mut given = given.to_vec();
    given.push(Finding::new(DETECTOR_BUILT_IN, DETECTOR_ERROR, Severity::High, ""));
    given
}

/// A SHA-256 digest. A pair cache key holds digests, never the call content (A2).
type Digest32 = [u8; 32];

fn finish(h: Sha256) -> Digest32 {
    let mut out = [0; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// The digest of a call as the pair decides it: the raw name, whether the input is JSON or text,
/// and the arguments. The hosts come from the input, and `declared` is always `Unknown`.
fn call_digest(name: &str, kind: ToolCallKind, arguments: &str) -> Digest32 {
    #[cfg(test)]
    tests::CALL_DIGESTS.with(|n| n.set(n.get() + 1));
    let mut h = Sha256::new();
    h.update([u8::from(matches!(input_for(kind, ""), ToolInput::Text(_)))]);
    h.update((name.len() as u64).to_le_bytes());
    h.update(name.as_bytes());
    h.update(arguments.as_bytes());
    finish(h)
}

/// The digest of a kernel finding set: each kind with its severity, in kind order.
fn findings_digest(set: &Findings) -> Digest32 {
    let mut h = Sha256::new();
    for kind in set.iter() {
        h.update((kind.len() as u64).to_le_bytes());
        h.update(kind.as_bytes());
        h.update(set.severity(kind).map_or(-1, Severity::as_long).to_le_bytes());
    }
    finish(h)
}

/// The earlier calls with this id in an index, or none.
fn calls_of<'m, 'k>(index: &'m BTreeMap<&str, Vec<EarlierCall<'k>>>, id: &str) -> &'m [EarlierCall<'k>] {
    index.get(id).map_or(&[], Vec::as_slice)
}

/// An earlier call of a result target, or the pseudo call of D32 (input `{}`).
#[derive(Clone, Copy)]
struct EarlierCall<'k> {
    name: &'k str,
    kind: ToolCallKind,
    arguments: &'k str,
    /// The index of the call in `TurnFacts.prior_calls`, so that its `call_digest` is computed
    /// at most once, and only when a pair needs it. `None` for the pseudo call.
    slot: Option<usize>,
}

impl<'k> EarlierCall<'k> {
    fn of(slot: usize, c: &'k PriorCall) -> Self {
        Self {
            name: &c.name,
            kind: c.kind,
            arguments: &c.arguments,
            slot: Some(slot),
        }
    }

    /// D32: a result with no earlier call is decided with its own name (or the empty name).
    fn pseudo(name: &'k str) -> Self {
        Self {
            name,
            kind: ToolCallKind::Function,
            arguments: "{}",
            slot: None,
        }
    }
}

/// The pair that a record cites: a call and the finding set of its A.
#[derive(Clone, Copy)]
struct Cite<'k> {
    call: EarlierCall<'k>,
    findings: Digest32,
    /// A had the budget `detector_error` added (the past-budget pseudo pair).
    past_budget: bool,
}

/// The decided pair of one distinct (call, finding set).
struct PairOutcome {
    /// Verdict A, with the findings.
    a: Verdict,
    /// The time of decision A.
    duration: Duration,
    /// `Some(enforced)` when the findings change the verdict in a way that counts (§ 6.8.2).
    counts: Option<bool>,
}

/// The differential pairs of every earlier call of one target (§ 6.8.2).
struct Pairs {
    /// The `findings_digest` of the target's finding set.
    findings: Digest32,
    /// The call that the record cites, and whether the item is enforced (the OR over every call
    /// whose pair differs in a way that counts). The cited call is the first one whose difference
    /// is enforced; when no difference is enforced, it is the first one that differs (spec A14).
    hit: Option<(usize, bool)>,
    /// Every call of the target has its pair. `false` when the budget stopped the loop: `hit`
    /// then holds what the pairs before the stop found, and it is kept.
    complete: bool,
}

/// Whether the plan may remove a definition of this kind and name (P-15).
fn removable(d: &ToolDefinition) -> bool {
    d.kind != ToolCallKind::Other || REMOVABLE_BUILT_INS.contains(&d.name.as_str())
}

/// The kernel answers for one snapshot, one per normalised tool name.
struct Judge<'a> {
    policies: &'a ToolPolicies,
    facts: &'a Facts,
    collisions: BTreeSet<String>,
    /// Only a floor deny counts: the tenant mode is `off`, or the set is the floor alone because
    /// the tenant part is `Off` or `Unavailable` (P-4).
    floor_only: bool,
    mode: TenantMode,
    seen: BTreeMap<String, Option<bool>>,
}

impl Judge<'_> {
    /// `Some(enforced)` when the tool is denied for every input and the deny counts, else `None`.
    fn denied(&mut self, name: &str) -> Option<bool> {
        let n = normalise(name);
        if let Some(v) = self.seen.get(&n) {
            return *v;
        }
        let mut findings = Findings::new();
        if self.collisions.contains(&n) {
            findings.add(NAME_COLLISION, Severity::High);
        }
        let v = match decide_tool(&n, self.facts, &findings, self.policies) {
            ToolVerdict::DeniedForAllInputs { by_floor } if by_floor || !self.floor_only => Some(by_floor || self.mode == TenantMode::Enforce),
            _ => None,
        };
        self.seen.insert(n, v);
        v
    }
}

/// Step 3: decides each result target once (P-7).
struct ResultJudge<'j, 'p, 'k> {
    judge: &'j mut Judge<'p>,
    plan: &'j mut RequestPlan,
    /// Every target decided so far, and whether it is replaced. A repeated target has the same
    /// outcome, so it is not decided again.
    seen: BTreeMap<(u8, &'k str), Option<bool>>,
    /// The union of the findings of the results of each target (§ 6.8.2). A target with no
    /// finding has no entry and keeps the SMA-752 path.
    unions: BTreeMap<(u8, &'k str), Vec<Finding>>,
    now: &'j dyn Fn() -> Instant,
    /// The pair cache of this request, keyed by (`call_digest`, `findings_digest`). The policies
    /// are the snapshot's for the whole `plan_request` call, so they need no part in the key.
    cache: HashMap<(Digest32, Digest32), PairOutcome>,
    /// The `call_digest` of each earlier call (by `EarlierCall.slot`), computed on first use. A
    /// request with no findings hashes no arguments.
    digests: Vec<Option<Digest32>>,
    /// The pairs decided so far (each one two Cedar decisions). At most `PAIR_BUDGET`.
    decided: usize,
    /// A pair past `PAIR_BUDGET` was needed: every later target with findings goes to
    /// `past_budget`.
    exhausted: bool,
}

impl<'k> ResultJudge<'_, '_, 'k> {
    /// `Some(enforced)` when the target is replaced, else `None`. The optimistic path (SMA-752)
    /// replaces it when the name of any earlier call (for a legacy target: its `legacy` name) is
    /// denied for every input. A target with findings is also decided by the differential pair of
    /// each earlier call, or of the pseudo call named `pseudo` when there is none (§ 6.8.2, D32).
    /// A new replaced target adds one plan item and one report entry; a target with findings that
    /// is not replaced adds one screen record.
    fn decide(&mut self, key: (u8, &'k str), target: impl FnOnce() -> ResultTarget, calls: &[EarlierCall<'k>], legacy: Option<&'k str>, pseudo: &'k str) -> Option<bool> {
        if let Some(replaced) = self.seen.get(&key) {
            return *replaced;
        }
        let names: Vec<&'k str> = match legacy {
            Some(name) => vec![name],
            None => calls.iter().map(|c| c.name).collect(),
        };
        let mut hit: Option<(&'k str, bool)> = None;
        for name in names {
            if let Some(enforced) = self.judge.denied(name) {
                hit = Some(match hit {
                    Some((first, e)) => (first, e || enforced),
                    None => (name, enforced),
                });
            }
        }
        // A target is decided once (the `seen` cache), so its union is taken out here.
        let outcome = match self.unions.remove(&key) {
            None => hit.map(|(name, enforced)| (name, enforced, None)),
            Some(given) => {
                let calls: Cow<'_, [EarlierCall<'k>]> = if calls.is_empty() { Cow::Owned(vec![EarlierCall::pseudo(pseudo)]) } else { Cow::Borrowed(calls) };
                self.screened(&calls, &given, hit)
            }
        };
        self.seen.insert(key, outcome.as_ref().map(|(_, enforced, _)| *enforced));
        let (name, enforced, detail) = outcome?;
        self.plan
            .push_entry(RequestEntry::new(name.to_owned(), RequestAction::ReplaceResult, enforced, RequestReason::ToolResultBlocked), detail);
        self.plan.replaced.push(PlanItem { target: target(), enforced });
        Some(enforced)
    }

    /// The outcome of a target with findings: the differential pairs merged with the optimistic
    /// `hit` into one item (`enforced` is the OR), with A's detail. Not replaced: a screen record.
    fn screened(&mut self, calls: &[EarlierCall<'k>], given: &[Finding], hit: Option<(&'k str, bool)>) -> Option<(&'k str, bool, Option<CedarDetail>)> {
        let first = *calls.first()?;
        let own = self.pairs(calls, given);
        let cite = |index: usize| -> Option<Cite<'k>> {
            Some(Cite {
                call: *calls.get(index)?,
                findings: own.findings,
                past_budget: false,
            })
        };
        // The pair that a record cites when no pair differs.
        let mut base = Cite {
            call: first,
            findings: own.findings,
            past_budget: false,
        };
        let mut differs = own.hit.and_then(|(index, enforced)| Some((cite(index)?, enforced)));
        let with_error = if own.complete { Vec::new() } else { with_budget_error(given) };
        if !own.complete {
            // Past the pair budget (spec A13): one pseudo pair for the target's tool name, with a
            // `detector_error` added to A. The policies decide, as for any other pair. It only
            // adds to what the target's own pairs found before the stop: a hit already found is
            // kept, its `enforced` is ORed, and the cited pair moves only to an enforced
            // difference that the kept hit lacks (spec A14).
            let pseudo = EarlierCall::pseudo(first.name);
            let past = self.past_budget(&pseudo, &with_error);
            base = Cite {
                call: pseudo,
                findings: past.findings,
                past_budget: true,
            };
            differs = match (differs, past.hit) {
                (Some((kept, e)), Some((_, pe))) => Some((if pe && !e { base } else { kept }, e || pe)),
                (Some(kept), None) => Some(kept),
                (None, Some((_, pe))) => Some((base, pe)),
                (None, None) => None,
            };
        }
        let detail = |judge: &Self, c: &Cite<'k>| judge.detail(c, if c.past_budget { &with_error } else { given });
        match (differs, hit) {
            // Replaced by the findings: the cited pair names the entry and gives the detail.
            (Some((c, enforced)), optimistic) => Some((c.call.name, enforced || optimistic.is_some_and(|(_, e)| e), detail(self, &c))),
            // Replaced by the optimistic path only: A's detail of the call that it denied, so that
            // a floor deny shows its floor policy (§ 6.8.2). A call with no pair (past the budget)
            // gives the base detail.
            (None, Some((name, enforced))) => {
                let n = normalise(name);
                let denied = calls.iter().position(|c| normalise(c.name) == n).and_then(cite);
                let cedar = denied.and_then(|c| detail(self, &c)).or_else(|| detail(self, &base));
                Some((name, enforced, cedar))
            }
            (None, None) => {
                if let Some(cedar) = detail(self, &base) {
                    self.plan.screen_records.push(ScreenRecord {
                        tool: base.call.name.to_owned(),
                        cedar,
                    });
                }
                None
            }
        }
    }

    /// A result that names no call (P-6): the pair of the pseudo call, and a screen record only.
    /// Past the pair budget, A also gets the `detector_error`; the result is still not replaced.
    fn orphan(&mut self, given: &[Finding]) {
        let pseudo = EarlierCall::pseudo("");
        let own = self.pairs(&[pseudo], given);
        let cedar = if own.complete {
            let c = Cite {
                call: pseudo,
                findings: own.findings,
                past_budget: false,
            };
            self.detail(&c, given)
        } else {
            let given = with_budget_error(given);
            let past = self.past_budget(&pseudo, &given);
            let c = Cite {
                call: pseudo,
                findings: past.findings,
                past_budget: true,
            };
            self.detail(&c, &given)
        };
        if let Some(cedar) = cedar {
            self.plan.screen_records.push(ScreenRecord { tool: pseudo.name.to_owned(), cedar });
        }
    }

    /// A's Cedar detail of a cited pair, with the findings that A had.
    fn detail(&self, c: &Cite<'k>, given: &[Finding]) -> Option<CedarDetail> {
        let outcome = self.cache.get(&(self.known_digest(&c.call)?, c.findings))?;
        Some(cedar_detail(&outcome.a, given, Declared::Unknown, self.judge.facts, outcome.duration))
    }

    /// The `call_digest` of a call: from the memo for an earlier call (computed on first use),
    /// computed here for the pseudo call.
    fn digest(&mut self, c: &EarlierCall<'k>) -> Digest32 {
        let Some(slot) = c.slot else {
            return call_digest(c.name, c.kind, c.arguments);
        };
        if let Some(Some(d)) = self.digests.get(slot) {
            return *d;
        }
        let d = call_digest(c.name, c.kind, c.arguments);
        if let Some(entry) = self.digests.get_mut(slot) {
            *entry = Some(d);
        }
        d
    }

    /// The digest of a call whose pair is decided: the memo of an earlier call (`None` when it
    /// was never needed), or the pseudo call's digest.
    fn known_digest(&self, c: &EarlierCall<'k>) -> Option<Digest32> {
        match c.slot {
            Some(slot) => self.digests.get(slot).copied().flatten(),
            None => Some(call_digest(c.name, c.kind, c.arguments)),
        }
    }

    /// The pair of a target past `PAIR_BUDGET` (spec A13): the D32 pseudo call of its tool name,
    /// with `given` (the target's findings and the budget `detector_error`). It is in the same
    /// cache, keyed by the pseudo call (so by the tool name) and the finding set, and it does not
    /// count against the budget. Past the budget, a request therefore decides at most one pair per
    /// distinct (tool name, finding set). That is at most one pair per target, never one per
    /// (target, earlier call). A finding never decides alone (A1): the policies decide the pair.
    fn past_budget(&mut self, pseudo: &EarlierCall<'k>, given: &[Finding]) -> Pairs {
        let with = kernel_findings(given);
        let findings = findings_digest(&with);
        let key = (self.digest(pseudo), findings);
        let counts = match self.cache.get(&key) {
            Some(outcome) => outcome.counts,
            None => {
                let outcome = self.decide_pair(pseudo, &with);
                let counts = outcome.counts;
                self.cache.insert(key, outcome);
                counts
            }
        };
        Pairs {
            findings,
            hit: counts.map(|enforced| (0, enforced)),
            complete: true,
        }
    }

    /// The differential pairs of a target (§ 6.8.2), from the pair cache. When a pair that is not
    /// in the cache would go past `PAIR_BUDGET`, the loop stops: the result has `complete` false
    /// and keeps the `hit` of the pairs before the stop. The target, and every later target with
    /// findings, then also gets `past_budget`. Before the budget, a request decides at most
    /// `PAIR_BUDGET` pairs (`2 * PAIR_BUDGET` Cedar decisions), plus one cache look-up for each
    /// (target, earlier call).
    fn pairs(&mut self, calls: &[EarlierCall<'k>], given: &[Finding]) -> Pairs {
        let with = kernel_findings(given);
        let mut pairs = Pairs {
            findings: findings_digest(&with),
            hit: None,
            complete: !self.exhausted,
        };
        if self.exhausted {
            return pairs;
        }
        for (index, c) in calls.iter().enumerate() {
            let key = (self.digest(c), pairs.findings);
            let counts = match self.cache.get(&key) {
                Some(outcome) => outcome.counts,
                None => {
                    if self.decided >= PAIR_BUDGET {
                        self.exhausted = true;
                        pairs.complete = false;
                        return pairs;
                    }
                    self.decided += 1;
                    let outcome = self.decide_pair(c, &with);
                    let counts = outcome.counts;
                    self.cache.insert(key, outcome);
                    counts
                }
            };
            if let Some(enforced) = counts {
                // The cited call: the first enforced differing pair, else the first differing pair
                // (spec A14). The verdict (`enforced`, the OR) does not depend on the choice.
                pairs.hit = Some(match pairs.hit {
                    None => (index, enforced),
                    Some((cited, e)) => (if enforced && !e { index } else { cited }, e || enforced),
                });
            }
        }
        pairs
    }

    /// One differential pair (§ 6.8.2): A with the target's findings only, B with no findings,
    /// both with `Declared::Unknown`. The request-level `name_collision` is in neither (A9). Two
    /// kernel decisions; the clock is read around A only.
    fn decide_pair(&self, c: &EarlierCall<'k>, with: &Findings) -> PairOutcome {
        let judge = &*self.judge;
        let input = input_for(c.kind, c.arguments);
        // One host pass, shared by A and B (S8).
        let hosts = call_hosts(&input);
        let inv = ToolInvocation::new(c.name, input, Declared::Unknown).with_hosts(hosts);
        let started = (self.now)();
        let a = decide(&inv, judge.facts, with, judge.policies);
        let duration = (self.now)().saturating_duration_since(started);
        let b = decide(&inv, judge.facts, &Findings::new(), judge.policies);
        let counts = difference(&a, &b).and_then(|policies| {
            // A floor policy in the difference is enforced in every mode (SMA-752 D33). Else a
            // floor-only snapshot ignores the difference, and the mode decides.
            let by_floor = policies.iter().any(|p| p.set == PolicyOrigin::Floor);
            (by_floor || !judge.floor_only).then_some(by_floor || judge.mode == TenantMode::Enforce)
        });
        PairOutcome { a, duration, counts }
    }
}

/// The `tool_choice` after removing `removed` (normalised names): the rewrite, whether to refuse,
/// and the name of the tool that caused a reset.
fn choice_after(choice: &ToolChoice, removed: &BTreeSet<&str>) -> (ChoiceRewrite, bool, Option<String>) {
    match choice {
        ToolChoice::Named(n) if removed.contains(normalise(n).as_str()) => (ChoiceRewrite::Auto, false, Some(n.clone())),
        ToolChoice::AllowedTools(names) => {
            let (gone, left): (Vec<&String>, Vec<&String>) = names.iter().partition(|n| removed.contains(normalise(n).as_str()));
            match (gone.first(), left.is_empty()) {
                (None, _) => (ChoiceRewrite::Keep, false, None),
                (Some(g), true) => (ChoiceRewrite::Auto, false, Some((*g).clone())),
                (Some(g), false) => (ChoiceRewrite::Allowed(left.into_iter().cloned().collect()), false, Some((*g).clone())),
            }
        }
        // With only log-only removals, `removed` is empty here: no refuse and no entry (§ 6.5.3 has
        // no refuse action).
        ToolChoice::Unknown => (ChoiceRewrite::Keep, !removed.is_empty(), None),
        _ => (ChoiceRewrite::Keep, false, None),
    }
}

/// Plan the request-side actions (§ 6.5.1 steps 1-4, and the Anthropic part of step 5). With the
/// policies unavailable, the plan is empty (§ 6.4). With the tenant part unavailable, only floor
/// denies are planned, in every tenant mode, as in mode `off` (controller ruling I3: D33 says the
/// floor is always enforced, which outranks the § 6.4 sentence "the request side changes
/// nothing"). An item is enforced when a floor policy decided its deny (D33) or the tenant mode is
/// `enforce`; in log-only mode a tenant deny is only reported (D25).
///
/// Anthropic (step 5, controller ruling): when the messages hold `tool_use` or `tool_result`
/// blocks and the enforced removals would leave `tools[]` empty (server tools count), the last
/// definition in array order stays. Its removal is reported with `enforced: false`, and it does not
/// count in the `tool_choice` rewrite. Its calls are still blocked at call time. The other parts of
/// step 5 (dropping `tools`, `tool_choice` and their companions) belong to the rewrite.
#[must_use]
pub fn plan_request(snapshot: &PolicySnapshot, format: Format, turn: &TurnFacts, result_findings: &[Vec<Finding>], now: &dyn Fn() -> Instant) -> RequestPlan {
    let PolicySet::Available { policies, tenant, facts } = &snapshot.set else {
        return RequestPlan::empty();
    };
    // Only the floor counts when the mode is `off` (controller ruling, Task 8), when the set is the
    // floor alone, and when the tenant part is unavailable (controller ruling, I3: D33, the floor
    // is always enforced). In each case `policies` is the floor alone, or the tenant is ignored.
    let floor_only = snapshot.mode == TenantMode::Off || *tenant != TenantPart::Evaluated;
    debug_assert!(result_findings.is_empty() || result_findings.len() == turn.tool_results.len(), "one finding list per result, or none");
    let mut judge = Judge {
        policies,
        facts,
        // Step 1: the request-level findings.
        collisions: name_collisions(&turn.definitions),
        floor_only,
        mode: snapshot.mode,
        seen: BTreeMap::new(),
    };
    let mut plan = RequestPlan::empty();

    // Step 2: definitions, one item per normalised display name (P-7). A definition is decided by
    // the name its calls arrive under (`computer_use_preview` gives `computer` calls).
    let mut done: BTreeSet<String> = BTreeSet::new();
    for d in turn.definitions.iter().filter(|d| removable(d)) {
        let target = normalise(&d.display_name());
        if !done.insert(target.clone()) {
            continue;
        }
        if let Some(enforced) = judge.denied(&definition_call_name(d)) {
            plan.push_entry(RequestEntry::new(d.display_name(), RequestAction::RemoveDefinition, enforced, RequestReason::ToolCallDenied), None);
            plan.removed.push(PlanItem { target, enforced });
        }
    }

    // Step 5, Anthropic: keep the last definition when blocks refer to tools and nothing else would
    // stay in `tools[]`. Removal entries come first and in item order, so index `i` matches.
    let mut kept: Option<String> = None;
    let blocks = !turn.prior_calls.is_empty() || !turn.tool_results.is_empty();
    if format == Format::AnthropicMessages && blocks && !turn.server_tools {
        let gone: BTreeSet<&str> = plan.removed.iter().filter(|i| i.enforced).map(|i| i.target.as_str()).collect();
        let empties = turn.definitions.iter().all(|d| gone.contains(normalise(&d.display_name()).as_str()));
        if let Some(last) = turn.definitions.last().filter(|_| empties) {
            let t = normalise(&last.display_name());
            if let Some(i) = plan.removed.iter().position(|item| item.target == t) {
                plan.removed[i].enforced = false; // ANTHROPIC-KEEP-LAST
                plan.entries[i].enforced = false;
            }
            kept = Some(t);
        }
    }

    // Step 3: results. A result with a call id takes the names of the earlier calls with that id;
    // it is replaced if any of them is denied. A legacy result carries its own name.
    // A result with findings is also decided by the differential pair (§ 6.8.2).
    // One index of the prior calls by id, built once. A duplicate id keeps every call in order.
    // Each earlier call is held with its slot; its `call_digest` is computed only when a pair needs it.
    let mut by_id: BTreeMap<&str, Vec<EarlierCall>> = BTreeMap::new();
    // A `local_shell_call_output` can name its call by `id` alone. The provider schema does not say
    // if that is the `call_id` or the item `id` of the call, so it is matched against both ids of
    // every earlier `local_shell_call` (fail-safe).
    let mut shell_ids: BTreeMap<&str, Vec<EarlierCall>> = BTreeMap::new();
    // The legacy chat `function_call`s (no id) by normalised name, for the pair of a `Name` target.
    let mut legacy: BTreeMap<String, Vec<EarlierCall>> = BTreeMap::new();
    for (slot, c) in turn.prior_calls.iter().enumerate() {
        let earlier = EarlierCall::of(slot, c);
        if let Some(id) = c.id.as_deref() {
            by_id.entry(id).or_default().push(earlier);
        } else if c.kind == ToolCallKind::LegacyFunctionCall {
            legacy.entry(normalise(&c.name)).or_default().push(earlier);
        }
        if c.local_shell {
            let ids: BTreeSet<&str> = c.id.iter().chain(&c.item_id).map(String::as_str).collect();
            for id in ids {
                shell_ids.entry(id).or_default().push(earlier);
            }
        }
    }
    let findings = findings_per_result(turn.tool_results.len(), result_findings);
    // The union of the findings of each target. A result with both a call id and an item id is
    // in both targets, so its findings go into both unions.
    let mut unions: BTreeMap<(u8, &str), Vec<Finding>> = BTreeMap::new();
    let mut orphans: Vec<&[Finding]> = Vec::new();
    for (r, given) in turn.tool_results.iter().zip(findings.iter()).filter(|(_, g)| !g.is_empty()) {
        let mut keys: Vec<(u8, &str)> = Vec::with_capacity(2);
        match (&r.call_id, &r.name) {
            (Some(id), _) => keys.push((0, id)),
            (None, Some(name)) => keys.push((1, name)),
            (None, None) => {}
        }
        if let Some(id) = &r.item_id {
            keys.push((2, id));
        }
        if keys.is_empty() {
            orphans.push(given);
        }
        for key in keys {
            let union = unions.entry(key).or_default();
            for f in given {
                if !union.contains(f) {
                    union.push(f.clone());
                }
            }
        }
    }
    let mut results = ResultJudge {
        judge: &mut judge,
        plan: &mut plan,
        seen: BTreeMap::new(),
        unions,
        now,
        cache: HashMap::new(),
        digests: vec![None; turn.prior_calls.len()],
        decided: 0,
        exhausted: false,
    };
    // The enforced flags of the unreplaceable results, added after the loop (`results` holds `plan`).
    let mut unreplaceable: Vec<bool> = Vec::new();
    for r in &turn.tool_results {
        // D32: with no earlier call, the pair uses the result's own name, or the empty name.
        let pseudo = r.name.as_deref().unwrap_or("");
        let replaced = match (&r.call_id, &r.name) {
            (Some(id), _) => results.decide((0, id.as_str()), || ResultTarget::CallId(id.clone()), calls_of(&by_id, id), None, pseudo),
            (None, Some(name)) => {
                let earlier = legacy.get(&normalise(name)).map_or(&[][..], Vec::as_slice);
                results.decide((1, name.as_str()), || ResultTarget::Name(name.clone()), earlier, Some(name.as_str()), pseudo)
            }
            (None, None) => None,
        };
        if r.shape == ResultShape::Screenshot
            && let Some(enforced) = replaced
        {
            unreplaceable.push(enforced);
        }
        // The `id` form of a `local_shell_call_output`, unless its `call_id` already replaced it
        // with an enforced item. An unenforced `call_id` item changes nothing, so a floor-denied
        // `id` must still be planned (fail-safe: enforced wins).
        if replaced != Some(true)
            && let Some(id) = &r.item_id
        {
            results.decide((2, id.as_str()), || ResultTarget::LocalShellId(id.clone()), calls_of(&shell_ids, id), None, pseudo);
        }
    }
    // P-6: a result with findings that names no call gets a screen record, and is never replaced.
    for given in orphans {
        results.orphan(given);
    }

    for enforced in unreplaceable {
        plan.add_refusal(RefusalCause::UnreplaceableResult, enforced);
    }

    // Step 4: `tool_choice`, from the enforced removals. A reset that only all removals together
    // would make is reported once, not enforced (P-9).
    let enforced: BTreeSet<&str> = plan.removed.iter().filter(|i| i.enforced).map(|i| i.target.as_str()).collect();
    // The kept Anthropic definition stays in the request, so it does not count here either.
    let all: BTreeSet<&str> = plan.removed.iter().map(|i| i.target.as_str()).filter(|t| kept.as_deref() != Some(*t)).collect();
    let (choice, refuse, reset) = choice_after(&turn.tool_choice, &enforced);
    let (_, refuse_all, reset_all) = choice_after(&turn.tool_choice, &all);
    let reset_entry = match reset {
        Some(tool) => Some((tool, true)),
        None => reset_all.map(|tool| (tool, false)),
    };
    if let Some((tool, enforced)) = reset_entry {
        plan.push_entry(RequestEntry::new(tool, RequestAction::ResetToolChoice, enforced, RequestReason::ToolCallDenied), None);
    }
    plan.choice = choice;
    if refuse {
        plan.add_refusal(RefusalCause::UnknownToolChoice, true);
    } else if refuse_all {
        plan.add_refusal(RefusalCause::UnknownToolChoice, false);
    }
    plan
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::tool_policy::{RequestInfo, SessionInfo};
    use paigasus_kernel::{Prn, to_cedar_uid};
    use uuid::Uuid;

    use paigasus_kernel::detect::{INJECTION_MARKERS, INJECTION_MARKERS_VERSION};
    use paigasus_kernel::tool_policy::{ToolInput, ToolInvocation};

    use super::*;
    use crate::policy::SHIPPED_FLOOR;
    use crate::policy::snapshot::OnUnavailable;
    use crate::turn::{ToolResultRef, ToolTurn, parse_request};

    thread_local! {
        /// The number of `call_digest` calls on this thread (nextest runs each test in its own
        /// process).
        pub(super) static CALL_DIGESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn a_request_with_no_findings_hashes_no_arguments() {
        let s = tenant(PERMIT_ALL_STARTER, TenantMode::Enforce);
        let t = distinct(10);
        CALL_DIGESTS.with(|n| n.set(0));
        let _ = plan_request(&s, Format::OpenAiChat, &t, &[], &Instant::now);
        let _ = plan_request(&s, Format::OpenAiChat, &t, &vec![vec![]; 10], &Instant::now);
        assert_eq!(CALL_DIGESTS.with(std::cell::Cell::get), 0, "no result has findings");
        // The control: one result with findings hashes its own call only.
        let mut findings = vec![vec![]; 10];
        findings[3] = vec![high()];
        let _ = plan_request(&s, Format::OpenAiChat, &t, &findings, &Instant::now);
        assert_eq!(CALL_DIGESTS.with(std::cell::Cell::get), 1);
    }

    const NO_FLOOR: &str = "";
    const RM_FLOOR: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
    const PERMIT_ALL: &str = r#"@id("all") permit(principal, action, resource);"#;
    const PERMIT_OK: &str = r#"@id("ok") permit(principal, action == Pgs::Gateway::Action::"ok", resource);"#;

    fn facts() -> Facts {
        let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("prn");
        Facts::new(
            to_cedar_uid(&prn),
            Uuid::nil(),
            RequestInfo::new("prn:pgs:gateway:::scope/x", crate::policy::ingress(Format::OpenAiChat), "m"),
            SessionInfo::placeholder(),
        )
    }

    /// A snapshot whose set is the floor and, when `tenant` is `Evaluated`, the tenant text (P-4).
    fn snap(floor: &str, tenant_text: &str, tenant: TenantPart, mode: TenantMode) -> PolicySnapshot {
        let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
        let text = if tenant == TenantPart::Evaluated { tenant_text } else { "" };
        let set = PolicySet::Available {
            policies: Arc::new(ToolPolicies::compile(floor, text, &[]).expect("compile")),
            tenant,
            facts: facts(),
        };
        PolicySnapshot::new(set, mode, OnUnavailable::Closed)
    }

    fn enforce(tenant_text: &str) -> PolicySnapshot {
        snap(NO_FLOOR, tenant_text, TenantPart::Evaluated, TenantMode::Enforce)
    }

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Function,
            schema: None,
        }
    }

    fn other(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Other,
            schema: None,
        }
    }

    fn prior(id: &str, name: &str) -> PriorCall {
        PriorCall {
            id: Some(id.to_owned()),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
            kind: ToolCallKind::Function,
            local_shell: false,
            item_id: None,
        }
    }

    fn by_id(id: &str) -> ToolResultRef {
        ToolResultRef {
            call_id: Some(id.to_owned()),
            name: None,
            item_id: None,
            shape: ResultShape::Text,
        }
    }

    fn turn(definitions: Vec<ToolDefinition>) -> TurnFacts {
        TurnFacts { definitions, ..TurnFacts::default() }
    }

    fn item<T>(target: T, enforced: bool) -> PlanItem<T> {
        PlanItem { target, enforced }
    }

    fn removed(p: &RequestPlan) -> Vec<(&str, bool)> {
        p.removed.iter().map(|i| (i.target.as_str(), i.enforced)).collect()
    }

    fn actions(p: &RequestPlan) -> Vec<(RequestAction, bool, RequestReason)> {
        p.entries.iter().map(|e| (e.action, e.enforced, e.reason)).collect()
    }

    #[test]
    fn an_unavailable_set_plans_nothing() {
        let mut t = turn(vec![def("rm"), def("x")]);
        t.prior_calls = vec![prior("c1", "rm")];
        t.tool_results = vec![by_id("c1")];
        t.tool_choice = ToolChoice::Unknown;
        let unavailable = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
        let p = plan_request(&unavailable, Format::OpenAiChat, &t, &[], &Instant::now);
        assert_eq!(p, RequestPlan::empty());
        assert!(p.is_noop());
    }

    /// Controller ruling (I3, D33): an unavailable tenant part plans the floor alone in every
    /// mode. A floor-denied definition is removed and a floor-denied result is replaced, both
    /// enforced; the floor-alone default deny of "x" does not count (P-4).
    #[test]
    fn an_unavailable_tenant_part_plans_the_floor_in_every_mode() {
        let mut t = turn(vec![def("rm"), def("x")]);
        t.prior_calls = vec![prior("c1", "rm"), prior("c2", "x")];
        t.tool_results = vec![by_id("c1"), by_id("c2")];
        for mode in [TenantMode::Enforce, TenantMode::LogOnly, TenantMode::Off] {
            let p = plan_request(&snap(RM_FLOOR, "", TenantPart::Unavailable, mode), Format::OpenAiChat, &t, &[], &Instant::now);
            assert_eq!(removed(&p), [("rm", true)], "{mode:?}");
            assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)], "{mode:?}");
            assert_eq!(
                actions(&p),
                [
                    (RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied),
                    (RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked),
                ],
                "{mode:?}"
            );
            assert!(!p.is_noop());
        }
        // With no floor deny, an unavailable tenant part plans nothing.
        let p = plan_request(&snap(NO_FLOOR, "", TenantPart::Unavailable, TenantMode::Enforce), Format::OpenAiChat, &t, &[], &Instant::now);
        assert!(p.is_noop() && p.entries.is_empty());
    }

    #[test]
    fn a_tool_denied_for_every_input_is_removed() {
        let p = plan_request(&enforce(PERMIT_OK), Format::OpenAiChat, &turn(vec![def("X "), def("ok")]), &[], &Instant::now);
        assert_eq!(p.removed, [item("x".to_owned(), true)]);
        assert_eq!(actions(&p), [(RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied)]);
        assert_eq!(p.entries[0].tool, "X ");
        assert!(!p.is_noop());
    }

    #[test]
    fn every_definition_of_a_removed_name_goes_together() {
        // P-7: one item per normalised display name.
        let p = plan_request(&enforce(PERMIT_OK), Format::OpenAiChat, &turn(vec![def("x"), def("ok"), def("X")]), &[], &Instant::now);
        assert_eq!(removed(&p), [("x", true)]);
        assert_eq!(p.entries.len(), 1);
    }

    #[test]
    fn an_input_reading_permit_keeps_the_definition() {
        let permit = r#"@id("p") permit(principal, action == Pgs::Gateway::Action::"read_file", resource) when { context.input.path like "/tmp/*" };"#;
        let p = plan_request(&enforce(permit), Format::OpenAiChat, &turn(vec![def("read_file")]), &[], &Instant::now);
        assert!(p.removed.is_empty());
        assert!(p.entries.is_empty());
        assert!(p.is_noop());
    }

    #[test]
    fn log_only_plans_but_does_not_enforce() {
        let mut t = turn(vec![def("x"), def("ok")]);
        t.prior_calls = vec![prior("c1", "x")];
        t.tool_results = vec![by_id("c1")];
        t.tool_choice = ToolChoice::Named("x".to_owned());
        let p = plan_request(&snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiChat, &t, &[], &Instant::now);
        assert_eq!(removed(&p), [("x", false)]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), false)]);
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert_eq!(p.refusal(), None);
        assert_eq!(p.entries.len(), 3);
        assert!(p.entries.iter().all(|e| !e.enforced));
        assert!(p.is_noop());
    }

    #[test]
    fn a_floor_removal_is_enforced_in_every_mode() {
        let cases = [
            snap(RM_FLOOR, "", TenantPart::Off, TenantMode::Off),
            snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Off),
            snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly),
            snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Enforce),
            // An unavailable tenant part plans the floor alone in every mode (I3 ruling).
            snap(RM_FLOOR, "", TenantPart::Unavailable, TenantMode::Off),
            snap(RM_FLOOR, "", TenantPart::Unavailable, TenantMode::LogOnly),
            snap(RM_FLOOR, "", TenantPart::Unavailable, TenantMode::Enforce),
        ];
        for s in cases {
            let p = plan_request(&s, Format::OpenAiChat, &turn(vec![def("rm"), def("ok")]), &[], &Instant::now);
            // The floor-alone default deny of "ok" does not count (P-4).
            assert_eq!(removed(&p), [("rm", true)], "{:?}", s.mode);
            assert!(!p.is_noop());
        }
        // Mode `off` with a tenant set: a tenant-only deny is not planned.
        let p = plan_request(
            &snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::Off),
            Format::OpenAiChat,
            &turn(vec![def("x")]),
            &[],
            &Instant::now,
        );
        assert!(p.removed.is_empty());
    }

    #[test]
    fn an_empty_catalog_in_enforce_removes_every_definition() {
        let invoke = r#"@id("i") permit(principal, action in Pgs::Gateway::Action::"InvokeTool", resource);"#;
        let p = plan_request(&enforce(invoke), Format::OpenAiChat, &turn(vec![def("a"), def("b")]), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
        let unregistered = r#"@id("u") permit(principal, action in Pgs::Gateway::Action::"Unregistered", resource);"#;
        assert!(
            plan_request(&enforce(unregistered), Format::OpenAiChat, &turn(vec![def("a"), def("b")]), &[], &Instant::now)
                .removed
                .is_empty()
        );
    }

    #[test]
    fn colliding_definitions_are_not_removed() {
        // The collision floor reads `context.findings`, so it cannot deny every input.
        let s = snap(SHIPPED_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Enforce);
        let p = plan_request(&s, Format::OpenAiChat, &turn(vec![def("send"), def("Send")]), &[], &Instant::now);
        assert!(p.removed.is_empty());
        assert!(p.is_noop());
    }

    #[test]
    fn hosted_responses_tools_are_never_removed() {
        let defs = vec![
            other("web_search"),
            other("mcp"),
            other("mcp"),
            other("tool_search"),
            other("local_shell"),
            other("shell"),
            other("apply_patch"),
            other("computer"),
            other("computer_use_preview"),
        ];
        let p = plan_request(&enforce(PERMIT_OK), Format::OpenAiResponses, &turn(defs), &[], &Instant::now);
        // `mcp` collides with itself, and is still kept (P-15).
        assert_eq!(
            removed(&p),
            [("local_shell", true), ("shell", true), ("apply_patch", true), ("computer", true), ("computer_use_preview", true)]
        );
        // A `computer_use_preview` definition is decided by its call name `computer`.
        let permit_computer = r#"@id("c") permit(principal, action == Pgs::Gateway::Action::"computer", resource);"#;
        let p = plan_request(&enforce(permit_computer), Format::OpenAiResponses, &turn(vec![other("computer_use_preview")]), &[], &Instant::now);
        assert!(p.removed.is_empty());
    }

    #[test]
    fn an_id_only_local_shell_result_matches_both_ids_of_the_call() {
        let s = enforce(PERMIT_OK);
        let shell = |name: &str| PriorCall {
            id: Some("c1".to_owned()),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
            kind: ToolCallKind::LocalShell,
            local_shell: true,
            item_id: Some("ls_1".to_owned()),
        };
        let out = |id: &str| ToolResultRef {
            call_id: None,
            name: None,
            item_id: Some(id.to_owned()),
            shape: ResultShape::Text,
        };
        let run = |prior_calls: Vec<PriorCall>, tool_results: Vec<ToolResultRef>| {
            let mut t = turn(vec![]);
            t.prior_calls = prior_calls;
            t.tool_results = tool_results;
            plan_request(&s, Format::OpenAiResponses, &t, &[], &Instant::now)
        };
        // The output `id` is the call's `call_id` or its item `id`: replaced either way.
        for id in ["c1", "ls_1"] {
            let p = run(vec![shell("x")], vec![out(id)]);
            assert_eq!(p.replaced, [item(ResultTarget::LocalShellId(id.to_owned()), true)], "{id}");
            assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)], "{id}");
        }
        // An allowed call, or an `id` that matches no call: not replaced.
        assert!(run(vec![shell("ok")], vec![out("ls_1")]).replaced.is_empty());
        assert!(run(vec![shell("x")], vec![out("ls_2")]).replaced.is_empty());
        // Only a `local_shell_call` is matched by `id`.
        let mut function = shell("x");
        function.local_shell = false;
        function.item_id = None;
        assert!(run(vec![function], vec![out("c1")]).replaced.is_empty());
        // A result with both ids: its `call_id` replaces it, with one entry.
        let both = ToolResultRef {
            call_id: Some("c1".to_owned()),
            name: None,
            item_id: Some("ls_1".to_owned()),
            shape: ResultShape::Text,
        };
        let p = run(vec![shell("x")], vec![both]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        assert_eq!(p.entries.len(), 1);
    }

    #[test]
    fn an_enforced_local_shell_id_is_planned_after_an_unenforced_call_id() {
        // The floor denies "rm" (enforced); the tenant denies "x" in log-only (not enforced).
        let s = snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let shell = |id: &str, item: &str, name: &str| PriorCall {
            id: Some(id.to_owned()),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
            kind: ToolCallKind::LocalShell,
            local_shell: true,
            item_id: Some(item.to_owned()),
        };
        let mut t = turn(vec![]);
        t.prior_calls = vec![shell("c1", "ls_1", "x"), shell("c2", "ls_2", "rm")];
        // `call_id` matches the unenforced call, `id` matches the floor-denied call.
        t.tool_results = vec![ToolResultRef {
            call_id: Some("c1".to_owned()),
            name: None,
            item_id: Some("ls_2".to_owned()),
            shape: ResultShape::Text,
        }];
        let p = plan_request(&s, Format::OpenAiResponses, &t, &[], &Instant::now);
        assert_eq!(
            p.replaced,
            [item(ResultTarget::CallId("c1".to_owned()), false), item(ResultTarget::LocalShellId("ls_2".to_owned()), true)]
        );
        assert_eq!(
            actions(&p),
            [
                (RequestAction::ReplaceResult, false, RequestReason::ToolResultBlocked),
                (RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)
            ]
        );
        assert!(!p.is_noop());
    }

    #[test]
    fn result_replacement_by_call_id_and_by_earlier_call() {
        let s = enforce(PERMIT_OK);
        let replaced = |prior_calls: Vec<PriorCall>, tool_results: Vec<ToolResultRef>| {
            let mut t = turn(vec![]);
            t.prior_calls = prior_calls;
            t.tool_results = tool_results;
            plan_request(&s, Format::OpenAiChat, &t, &[], &Instant::now)
        };
        // A chat `role:"tool"` result named by its earlier call (Anthropic `tool_use_id`, Responses
        // `call_id` and `approval_request_id` reach the plan in the same shape).
        let p = replaced(vec![prior("c1", "x")], vec![by_id("c1")]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)]);
        assert_eq!(p.entries[0].tool, "x");
        // An allowed earlier call: not replaced.
        assert!(replaced(vec![prior("c1", "ok")], vec![by_id("c1")]).replaced.is_empty());
        // Two earlier calls with one id: replaced if any is denied.
        let p = replaced(vec![prior("c1", "ok"), prior("c1", "x")], vec![by_id("c1")]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        // No earlier call with that id: not replaced.
        assert!(replaced(vec![prior("c2", "x")], vec![by_id("c1")]).replaced.is_empty());
        // A legacy `role:"function"` result carries its name.
        let legacy = ToolResultRef {
            call_id: None,
            name: Some("x".to_owned()),
            item_id: None,
            shape: ResultShape::Text,
        };
        let p = replaced(vec![], vec![legacy]);
        assert_eq!(p.replaced, [item(ResultTarget::Name("x".to_owned()), true)]);
        // Two results for one call id: one item.
        assert_eq!(replaced(vec![prior("c1", "x")], vec![by_id("c1"), by_id("c1")]).replaced.len(), 1);
        // A result with neither id nor name: not replaced.
        let none = ToolResultRef {
            call_id: None,
            name: None,
            item_id: None,
            shape: ResultShape::Text,
        };
        assert!(replaced(vec![], vec![none]).replaced.is_empty());
    }

    #[test]
    fn result_replacement_follows_the_floor_and_the_mode() {
        let mut t = turn(vec![]);
        t.prior_calls = vec![prior("c1", "rm"), prior("c2", "x")];
        t.tool_results = vec![by_id("c1"), by_id("c2")];
        let p = plan_request(&snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiChat, &t, &[], &Instant::now);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true), item(ResultTarget::CallId("c2".to_owned()), false)]);
        // Mixed floor and tenant denies on one id: enforced.
        t.prior_calls = vec![prior("c1", "x"), prior("c1", "rm")];
        t.tool_results = vec![by_id("c1")];
        let p = plan_request(&snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiChat, &t, &[], &Instant::now);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
    }

    fn with_choice(defs: &[&str], choice: ToolChoice) -> TurnFacts {
        let mut t = turn(defs.iter().map(|n| def(n)).collect());
        t.tool_choice = choice;
        t
    }

    fn resets(p: &RequestPlan) -> Vec<(String, bool)> {
        p.entries.iter().filter(|e| e.action == RequestAction::ResetToolChoice).map(|e| (e.tool.clone(), e.enforced)).collect()
    }

    #[test]
    fn tool_choice_after_removals() {
        let s = enforce(PERMIT_OK);
        let named = |n: &str| ToolChoice::Named(n.to_owned());
        let allowed = |n: &[&str]| ToolChoice::AllowedTools(n.iter().map(|x| (*x).to_owned()).collect());

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "ok"], named("A")), &[], &Instant::now);
        assert_eq!((p.choice.clone(), p.refusal()), (ChoiceRewrite::Auto, None));
        assert_eq!(resets(&p), [("A".to_owned(), true)]);

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "ok"], named("ok")), &[], &Instant::now);
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(resets(&p).is_empty());

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "ok"], allowed(&["a", "ok"])), &[], &Instant::now);
        assert_eq!(p.choice, ChoiceRewrite::Allowed(vec!["ok".to_owned()]));
        assert_eq!(resets(&p), [("a".to_owned(), true)]);

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "b", "ok"], allowed(&["a", "b"])), &[], &Instant::now);
        assert_eq!(p.choice, ChoiceRewrite::Auto);
        assert_eq!(resets(&p).len(), 1);

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["ok"], allowed(&["ok"])), &[], &Instant::now);
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(p.is_noop());

        for c in [ToolChoice::Auto, ToolChoice::None, ToolChoice::Required] {
            let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a"], c), &[], &Instant::now);
            assert_eq!((p.choice.clone(), p.refusal()), (ChoiceRewrite::Keep, None));
            assert!(resets(&p).is_empty());
        }

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a"], ToolChoice::Unknown), &[], &Instant::now);
        assert_eq!(p.refusal(), Some(RefusalCause::UnknownToolChoice));
        assert!(!p.is_noop());
        assert!(resets(&p).is_empty());
        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["ok"], ToolChoice::Unknown), &[], &Instant::now);
        assert_eq!(p.refusal(), None);

        let log_only = snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let p = plan_request(&log_only, Format::OpenAiChat, &with_choice(&["a"], ToolChoice::Unknown), &[], &Instant::now);
        assert_eq!(p.refusal(), None);
        assert!(p.is_noop());
    }

    #[test]
    fn a_log_only_reset_is_reported_once() {
        // P-9: the floor removes "a" (enforced); the log-only tenant would remove "b".
        let s = snap(
            r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"a", resource);"#,
            PERMIT_OK,
            TenantPart::Evaluated,
            TenantMode::LogOnly,
        );
        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "b", "ok"], ToolChoice::Named("b".to_owned())), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true), ("b", false)]);
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert_eq!(resets(&p), [("b".to_owned(), false)]);
        // An enforced reset is reported once, as enforced.
        let p = plan_request(
            &s,
            Format::OpenAiChat,
            &with_choice(&["a", "b", "ok"], ToolChoice::AllowedTools(vec!["a".to_owned(), "b".to_owned()])),
            &[],
            &Instant::now,
        );
        assert_eq!(p.choice, ChoiceRewrite::Allowed(vec!["b".to_owned()]));
        assert_eq!(resets(&p), [("a".to_owned(), true)]);
    }

    fn parsed(format: Format, body: serde_json::Value) -> TurnFacts {
        match parse_request(format, body.to_string().as_bytes()) {
            ToolTurn::Parsed(f) => f,
            ToolTurn::Unavailable(r) => panic!("unavailable: {r}"),
        }
    }

    /// A tenant that permits everything and forbids one tool for every input.
    fn forbid(tool: &str) -> PolicySnapshot {
        enforce(&format!(r#"{PERMIT_ALL} @id("x") forbid(principal, action == Pgs::Gateway::Action::"{tool}", resource);"#))
    }

    #[test]
    fn a_parsed_result_is_decided_by_the_full_call_name() {
        // A Responses namespace call: the result is decided as `db.query`, not `query`.
        let responses = serde_json::json!({"model":"m",
            "tools":[{"type":"namespace","name":"db","tools":[{"type":"function","name":"query","parameters":{}}]}],
            "input":[{"type":"function_call","call_id":"c1","namespace":"db","name":"query","arguments":"{}"},
                     {"type":"function_call_output","call_id":"c1","output":"x"}]});
        let p = plan_request(&forbid("db.query"), Format::OpenAiResponses, &parsed(Format::OpenAiResponses, responses), &[], &Instant::now);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        assert!(p.entries.iter().any(|e| (e.action, e.tool.as_str()) == (RequestAction::ReplaceResult, "db.query")));

        // An MCP approval: the response is decided as `server_label.name`.
        let mcp = serde_json::json!({"model":"m",
            "input":[{"type":"mcp_approval_request","id":"mcpr_1","server_label":"srv","name":"search","arguments":"{}"},
                     {"type":"mcp_approval_response","approval_request_id":"mcpr_1","approve":true}]});
        let p = plan_request(&forbid("srv.search"), Format::OpenAiResponses, &parsed(Format::OpenAiResponses, mcp), &[], &Instant::now);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("mcpr_1".to_owned()), true)]);

        // An Anthropic `tool_result` by `tool_use_id`.
        let anthropic = serde_json::json!({"model":"c","tools":[{"name":"rm","input_schema":{}}, {"name":"ok","input_schema":{}}],
            "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"rm","input":{}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}]});
        let p = plan_request(&forbid("rm"), Format::AnthropicMessages, &parsed(Format::AnthropicMessages, anthropic), &[], &Instant::now);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("t1".to_owned()), true)]);
    }

    fn anthropic(tools: serde_json::Value, blocks: bool, choice: Option<serde_json::Value>) -> TurnFacts {
        let messages = if blocks {
            serde_json::json!([{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"a","input":{}}]},
                               {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}])
        } else {
            serde_json::json!([{"role":"user","content":"hi"}])
        };
        let mut body = serde_json::json!({"model":"c","tools":tools,"messages":messages});
        if let Some(c) = choice {
            body["tool_choice"] = c;
        }
        parsed(Format::AnthropicMessages, body)
    }

    #[test]
    fn anthropic_keeps_the_last_definition_when_blocks_refer_to_tools() {
        let s = enforce(PERMIT_OK);
        let two = serde_json::json!([{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}]);
        // Blocks, every definition denied: `b` (the last) stays, reported not enforced.
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two.clone(), true, None), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true), ("b", false)]);
        assert_eq!(
            actions(&p),
            [
                (RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied),
                (RequestAction::RemoveDefinition, false, RequestReason::ToolCallDenied),
                (RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked),
            ]
        );
        // A named choice of the kept tool stays named, with no reset entry.
        let p = plan_request(
            &s,
            Format::AnthropicMessages,
            &anthropic(two.clone(), true, Some(serde_json::json!({"type":"tool","name":"b"}))),
            &[],
            &Instant::now,
        );
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(resets(&p).is_empty());
        // A named choice of a removed tool is still reset.
        let p = plan_request(
            &s,
            Format::AnthropicMessages,
            &anthropic(two.clone(), true, Some(serde_json::json!({"type":"tool","name":"a"}))),
            &[],
            &Instant::now,
        );
        assert_eq!((p.choice.clone(), resets(&p)), (ChoiceRewrite::Auto, vec![("a".to_owned(), true)]));
        // No blocks: every definition goes.
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two.clone(), false, None), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
        // A server tool stays in `tools[]`, so the rule does not apply.
        let with_server = serde_json::json!([{"name":"a","input_schema":{}},{"name":"b","input_schema":{}},{"type":"web_search_20250305","name":"web_search"}]);
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(with_server, true, None), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
        // A definition that stays anyway: the rule does not apply.
        let with_ok = serde_json::json!([{"name":"a","input_schema":{}},{"name":"ok","input_schema":{}}]);
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(with_ok, true, None), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true)]);
        // Another format with the same facts: every definition goes.
        let p = plan_request(&s, Format::OpenAiChat, &anthropic(two, true, None), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
    }

    #[test]
    fn anthropic_keeps_the_last_definition_only_against_enforced_removals() {
        // Log-only: nothing is enforced, so nothing needs to be kept; the plan reports both.
        let s = snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let two = serde_json::json!([{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}]);
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two, true, None), &[], &Instant::now);
        assert_eq!(removed(&p), [("a", false), ("b", false)]);
    }

    const COMPUTER_FLOOR: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;

    fn shot(id: &str) -> ToolResultRef {
        ToolResultRef {
            call_id: Some(id.to_owned()),
            name: None,
            item_id: None,
            shape: ResultShape::Screenshot,
        }
    }

    fn computer_turn() -> TurnFacts {
        let mut t = turn(vec![]);
        t.prior_calls = vec![prior("c1", "computer")];
        t.tool_results = vec![shot("c1")];
        t
    }

    fn causes(p: &RequestPlan) -> Vec<(RefusalCause, bool)> {
        p.refusals.iter().map(|(c, e)| (*c, *e)).collect()
    }

    #[test]
    fn an_enforced_block_of_a_screenshot_result_is_an_enforced_refusal() {
        let p = plan_request(&forbid("computer"), Format::OpenAiResponses, &computer_turn(), &[], &Instant::now);
        assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        assert_eq!(p.refusal(), Some(RefusalCause::UnreplaceableResult));
        assert!(!p.is_noop());
        // The report entry stays, so the caller sees which tool caused the refusal.
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)]);
        // An allowed computer call gives no cause.
        let p = plan_request(&forbid("other"), Format::OpenAiResponses, &computer_turn(), &[], &Instant::now);
        assert!(p.refusals.is_empty() && p.is_noop());
    }

    #[test]
    fn a_log_only_block_of_a_screenshot_result_is_an_unenforced_cause() {
        let p = plan_request(
            &snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly),
            Format::OpenAiResponses,
            &computer_turn(),
            &[],
            &Instant::now,
        );
        assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, false)]);
        assert_eq!(p.refusal(), None);
        assert!(p.is_noop());
    }

    #[test]
    fn a_floor_block_of_a_screenshot_result_is_enforced_in_log_only_and_without_a_tenant_part() {
        for s in [
            snap(COMPUTER_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly),
            snap(COMPUTER_FLOOR, "", TenantPart::Unavailable, TenantMode::LogOnly),
            snap(COMPUTER_FLOOR, "", TenantPart::Unavailable, TenantMode::Enforce),
        ] {
            let p = plan_request(&s, Format::OpenAiResponses, &computer_turn(), &[], &Instant::now);
            assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        }
    }

    #[test]
    fn a_screenshot_result_gives_the_cause_in_both_orders_of_one_call_id() {
        for results in [vec![shot("c1"), by_id("c1")], vec![by_id("c1"), shot("c1")]] {
            let mut t = computer_turn();
            t.tool_results = results;
            let p = plan_request(&forbid("computer"), Format::OpenAiResponses, &t, &[], &Instant::now);
            assert_eq!(p.replaced.len(), 1, "one target");
            assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        }
    }

    /// Review Focus 1: the value is enforced when any finding of the cause is enforced.
    #[test]
    fn two_findings_of_one_cause_are_enforced_when_any_is() {
        let s = snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        for results in [vec![shot("c1"), shot("c2")], vec![shot("c2"), shot("c1")]] {
            let mut t = turn(vec![]);
            // "computer" is a log-only tenant deny; "rm" is a floor deny.
            t.prior_calls = vec![prior("c1", "computer"), prior("c2", "rm")];
            t.tool_results = results;
            let p = plan_request(&s, Format::OpenAiResponses, &t, &[], &Instant::now);
            assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        }
    }

    #[test]
    fn an_unknown_tool_choice_with_only_log_only_removals_is_an_unenforced_cause() {
        let log_only = snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let p = plan_request(&log_only, Format::OpenAiChat, &with_choice(&["a"], ToolChoice::Unknown), &[], &Instant::now);
        assert_eq!(causes(&p), [(RefusalCause::UnknownToolChoice, false)]);
        assert_eq!(p.refusal(), None);
        assert!(p.is_noop());
        // No removal at all: no cause.
        let p = plan_request(&log_only, Format::OpenAiChat, &with_choice(&["ok"], ToolChoice::Unknown), &[], &Instant::now);
        assert!(p.refusals.is_empty());
    }

    #[test]
    fn the_refusal_is_the_first_enforced_cause_in_plan_order() {
        let mut t = with_choice(&["computer"], ToolChoice::Unknown);
        t.definitions = vec![other("computer")];
        t.prior_calls = vec![prior("c1", "computer")];
        t.tool_results = vec![shot("c1")];
        let p = plan_request(&forbid("computer"), Format::OpenAiResponses, &t, &[], &Instant::now);
        assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true), (RefusalCause::UnknownToolChoice, true)]);
        assert_eq!(p.refusal(), Some(RefusalCause::UnreplaceableResult));
    }

    // SMA-855 § 6.8.2: the differential result decision.

    fn finding(severity: Severity) -> Finding {
        Finding::new(INJECTION_MARKERS, INJECTION_MARKERS, severity, INJECTION_MARKERS_VERSION)
    }

    fn high() -> Finding {
        finding(Severity::High)
    }

    /// A chat turn: one earlier call `c0` named `fetch` with arguments `{}`, and its result.
    fn chat_one() -> serde_json::Value {
        serde_json::json!({"model":"m","messages":[
            {"role":"assistant","tool_calls":[{"id":"c0","type":"function","function":{"name":"fetch","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c0","content":"x"}]})
    }

    fn plan_with(s: &PolicySnapshot, format: Format, body: serde_json::Value, findings: &[Vec<Finding>]) -> RequestPlan {
        plan_request(s, format, &parsed(format, body), findings, &Instant::now)
    }

    fn tenant(text: &str, mode: TenantMode) -> PolicySnapshot {
        snap(NO_FLOOR, text, TenantPart::Evaluated, mode)
    }

    fn c0(enforced: bool) -> PlanItem<ResultTarget> {
        item(ResultTarget::CallId("c0".to_owned()), enforced)
    }

    fn ids(refs: &[PolicyRef]) -> Vec<&str> {
        refs.iter().map(|r| r.id.as_str()).collect()
    }

    fn detail(p: &RequestPlan, i: usize) -> &CedarDetail {
        p.entry_details[i].as_ref().expect("detail")
    }

    const PERMIT_ALL_STARTER: &str = r#"@id("all") permit(principal, action, resource); @id("detectors-block-high") forbid (principal, action, resource) when { context.max_severity >= 3 };"#;

    #[test]
    fn starter_policy_replaces() {
        let s = tenant(PERMIT_ALL_STARTER, TenantMode::Enforce);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert_eq!(p.replaced, [c0(true)]);
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)]);
        assert_eq!(p.entries[0].tool, "fetch");
        let d = detail(&p, 0);
        assert_eq!((d.effect, ids(&d.determining), d.max_severity), (Effect::Deny, vec!["detectors-block-high"], Some(Severity::High)));
        assert_eq!(d.declared, Declared::Unknown);
        assert!(!d.detection_disabled);
        assert!(p.screen_records.is_empty());
        // The same turn with no findings: nothing (SMA-752 path).
        assert!(plan_with(&s, Format::OpenAiChat, chat_one(), &[]).entries.is_empty());
    }

    #[test]
    fn a_detail_states_that_the_detectors_were_off() {
        let PolicySet::Available { policies, tenant: part, facts } = tenant(PERMIT_ALL_STARTER, TenantMode::Enforce).set else {
            unreachable!("snap() is always Available")
        };
        let set = PolicySet::Available {
            policies,
            tenant: part,
            facts: facts.with_detection(false),
        };
        let s = PolicySnapshot::new(set, TenantMode::Enforce, OnUnavailable::Closed);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        // With the detectors off, `max_severity` is -1, so the starter policy does not deny.
        assert!(p.replaced.is_empty());
        assert!(p.screen_records[0].cedar.detection_disabled);
    }

    #[test]
    fn a_permit_below_high_replaces() {
        let s = tenant(r#"@id("p") permit(principal, action, resource) when { context.max_severity < 3 };"#, TenantMode::Enforce);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert_eq!(p.replaced, [c0(true)]);
        assert_eq!(detail(&p, 0).effect, Effect::Deny);
    }

    #[test]
    fn a_permit_on_findings_does_not_replace() {
        let s = tenant(
            r#"@id("p") permit(principal, action, resource) when { context.findings.contains("injection_markers") };"#,
            TenantMode::Enforce,
        );
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert!(p.replaced.is_empty() && p.entries.is_empty());
        assert_eq!(p.screen_records.len(), 1);
        assert_eq!(p.screen_records[0].cedar.effect, Effect::Allow);
    }

    /// The two verdicts of the call `fetch` with input `{}`, with a `high` finding and without.
    fn pair(s: &PolicySnapshot) -> (Verdict, Verdict) {
        let PolicySet::Available { policies, facts, .. } = &s.set else {
            unreachable!("snap() is always Available")
        };
        let inv = ToolInvocation::new("fetch", ToolInput::Json("{}".to_owned()), Declared::Unknown);
        let a = decide(&inv, facts, &kernel_findings(&[high()]), policies);
        let b = decide(&inv, facts, &Findings::new(), policies);
        (a, b)
    }

    /// F(v) holds only forbids (controller ruling on D12). The kernel keeps Cedar's satisfied
    /// permit `all` in A's `determining`, because a forbid errored. That permit is not in F(A), so
    /// F(A) = {e} is a subset of F(B) = {e}. B denies too, so the findings did not cause the block.
    #[test]
    fn an_is_empty_or_input_forbid_does_not_replace() {
        let s = tenant(
            &format!(r#"{PERMIT_ALL} @id("e") forbid(principal, action, resource) when {{ context.findings.isEmpty() || context.input.x == 1 }};"#),
            TenantMode::Enforce,
        );
        // `e` is an errored forbid in A and a determining forbid in B.
        let (a, b) = pair(&s);
        assert_eq!((a.decision, ids(&a.errored_forbids), ids(&a.determining)), (Effect::Deny, vec!["e"], vec!["all"]));
        assert!(a.determining_forbids.is_empty());
        assert_eq!((b.decision, ids(&b.determining_forbids), b.errored_forbids.len()), (Effect::Deny, vec!["e"], 0));
        assert!(difference(&a, &b).is_none(), "F(A) is a subset of F(B) and B denies");
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert!(p.replaced.is_empty() && p.entries.is_empty());
        assert_eq!(p.screen_records.len(), 1);
    }

    #[test]
    fn two_denies_by_different_forbids_replace() {
        // A is denied by a forbid on the finding; B by another forbid, which needs no finding.
        let s = tenant(
            &format!(
                r#"{PERMIT_ALL} @id("fa") forbid(principal, action, resource) when {{ context.findings.contains("injection_markers") }};
                   @id("fb") forbid(principal, action, resource) when {{ context.findings.isEmpty() }};"#
            ),
            TenantMode::Enforce,
        );
        let (a, b) = pair(&s);
        assert_eq!((a.decision, ids(&a.determining_forbids)), (Effect::Deny, vec!["fa"]));
        assert_eq!((b.decision, ids(&b.determining_forbids)), (Effect::Deny, vec!["fb"]));
        let more = difference(&a, &b).expect("F(A) is not a subset of F(B)");
        assert_eq!(more.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["fa"]);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert_eq!(p.replaced, [c0(true)]);
    }

    #[test]
    fn a_contains_and_input_forbid_replaces() {
        let s = tenant(
            &format!(r#"{PERMIT_ALL} @id("c") forbid(principal, action, resource) when {{ context.findings.contains("injection_markers") && context.input.path like "/etc/*" }};"#),
            TenantMode::Enforce,
        );
        // Short circuit: the forbid errors in A only.
        let (a, b) = pair(&s);
        assert_eq!((a.decision, ids(&a.errored_forbids)), (Effect::Deny, vec!["c"]));
        assert_eq!((b.decision, b.errored_forbids.len()), (Effect::Allow, 0));
        // B allows, so the difference is all of F(A): the forbid `c` (the permit `all` is not in F).
        let more = difference(&a, &b).expect("B allows");
        assert_eq!(more.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["c"]);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert_eq!(p.replaced, [c0(true)]);
    }

    #[test]
    fn the_shipped_floor_with_colliding_names_and_a_medium_finding_does_not_replace() {
        let s = snap(SHIPPED_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Enforce);
        let mut body = chat_one();
        body["tools"] = serde_json::json!([
            {"type":"function","function":{"name":"fetch","parameters":{}}},
            {"type":"function","function":{"name":"Fetch","parameters":{}}}]);
        let p = plan_with(&s, Format::OpenAiChat, body, &[vec![finding(Severity::Medium)]]);
        // A9: `name_collision` is in neither A nor B.
        assert!(p.replaced.is_empty() && p.entries.is_empty());
        let d = &p.screen_records[0].cedar;
        assert_eq!(d.effect, Effect::Allow);
        assert!(d.findings.iter().all(|f| f.kind != NAME_COLLISION));
    }

    #[test]
    fn an_unguarded_input_forbid_with_no_earlier_call_does_not_replace() {
        let s = tenant(
            &format!(r#"{PERMIT_ALL} @id("x") forbid(principal, action, resource) when {{ context.input.x == 1 }};"#),
            TenantMode::Enforce,
        );
        let body = serde_json::json!({"model":"m","messages":[{"role":"tool","tool_call_id":"c9","content":"x"}]});
        let p = plan_with(&s, Format::OpenAiChat, body, &[vec![high()]]);
        // D32: the pseudo call errors in A and in B.
        assert!(p.replaced.is_empty());
        assert_eq!(p.screen_records.len(), 1);
        assert_eq!(p.screen_records[0].tool, "");
        assert_eq!(ids(&p.screen_records[0].cedar.errored), ["x"]);
    }

    #[test]
    fn a_floor_only_snapshot_ignores_a_tenant_difference() {
        // Mode `off` with a tenant set: only the floor counts (P-4).
        let s = snap(NO_FLOOR, PERMIT_ALL_STARTER, TenantPart::Evaluated, TenantMode::Off);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert!(p.replaced.is_empty() && p.entries.is_empty());
        assert_eq!(p.screen_records.len(), 1);
        assert_eq!(p.screen_records[0].cedar.effect, Effect::Deny);
    }

    #[test]
    fn a_floor_difference_is_enforced_in_every_mode() {
        let floor = r#"@id("fh") forbid(principal, action, resource) when { context.max_severity >= 3 };"#;
        for s in [
            snap(floor, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly),
            snap(floor, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Off),
            snap(floor, "", TenantPart::Off, TenantMode::Off),
            snap(floor, "", TenantPart::Unavailable, TenantMode::Enforce),
        ] {
            let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
            assert_eq!(p.replaced, [c0(true)], "{:?}", s.mode);
            assert!(detail(&p, 0).floor_denied);
        }
    }

    #[test]
    fn log_only_gives_an_unenforced_item() {
        let t = parsed(Format::OpenAiChat, chat_one());
        let p = plan_request(&tenant(PERMIT_ALL_STARTER, TenantMode::LogOnly), Format::OpenAiChat, &t, &[vec![high()]], &Instant::now);
        assert_eq!(p.replaced, [c0(false)]);
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, false, RequestReason::ToolResultBlocked)]);
        assert!(p.is_noop());
        // D14: an unenforced replacement does not count as replaced.
        assert!(!p.replaced_enforced(&t, 0));
        let enforced = plan_request(&tenant(PERMIT_ALL_STARTER, TenantMode::Enforce), Format::OpenAiChat, &t, &[vec![high()]], &Instant::now);
        assert!(enforced.replaced_enforced(&t, 0));
        assert!(!enforced.replaced_enforced(&t, 1), "no such result");
    }

    #[test]
    fn two_results_of_one_target_are_decided_once_on_the_union() {
        use std::cell::Cell;
        let s = tenant(PERMIT_ALL_STARTER, TenantMode::Enforce);
        let body = serde_json::json!({"model":"m","messages":[
            {"role":"assistant","tool_calls":[{"id":"c0","type":"function","function":{"name":"fetch","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c0","content":"x"},
            {"role":"tool","tool_call_id":"c0","content":"y"}]});
        let reads = Cell::new(0_usize);
        let clock = || {
            reads.set(reads.get() + 1);
            Instant::now()
        };
        let p = plan_request(&s, Format::OpenAiChat, &parsed(Format::OpenAiChat, body), &[vec![finding(Severity::Low)], vec![high()]], &clock);
        assert_eq!(p.replaced, [c0(true)]);
        assert_eq!(p.entries.len(), 1);
        let d = detail(&p, 0);
        assert_eq!(d.max_severity, Some(Severity::High));
        assert_eq!(d.findings.iter().map(|f| f.severity).collect::<Vec<_>>(), [Some(Severity::Low), Some(Severity::High)]);
        assert_eq!(reads.get(), 2, "one A decision (two clock reads) for the one target");
    }

    #[test]
    fn a_local_shell_output_with_both_ids_is_in_both_unions() {
        let s = tenant(
            &format!(r#"{PERMIT_ALL} @id("r") forbid(principal, action == Pgs::Gateway::Action::"rm", resource) when {{ context.findings.contains("injection_markers") }};"#),
            TenantMode::Enforce,
        );
        let shell = |id: &str, item_id: &str, name: &str| PriorCall {
            id: Some(id.to_owned()),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
            kind: ToolCallKind::LocalShell,
            local_shell: true,
            item_id: Some(item_id.to_owned()),
        };
        let mut t = turn(vec![]);
        t.prior_calls = vec![shell("c1", "ls_1", "x"), shell("c2", "ls_2", "rm")];
        t.tool_results = vec![ToolResultRef {
            call_id: Some("c1".to_owned()),
            name: None,
            item_id: Some("ls_2".to_owned()),
            shape: ResultShape::Text,
        }];
        let p = plan_request(&s, Format::OpenAiResponses, &t, &[vec![high()]], &Instant::now);
        // The `id` target sees the finding: its call `rm` is denied by it.
        assert_eq!(p.replaced, [item(ResultTarget::LocalShellId("ls_2".to_owned()), true)]);
        assert_eq!(p.entries[0].tool, "rm");
        // The `call_id` target sees it too: its screen record holds it.
        assert_eq!(p.screen_records.len(), 1);
        assert_eq!(p.screen_records[0].tool, "x");
        assert!(p.screen_records[0].cedar.findings.iter().any(|f| f.kind == INJECTION_MARKERS));
        assert!(p.replaced_enforced(&t, 0));
    }

    #[test]
    fn optimistic_and_differential_on_one_target_give_one_item() {
        // The floor denies `rm` for every input (enforced); the log-only tenant differs by the finding.
        let s = snap(RM_FLOOR, PERMIT_ALL_STARTER, TenantPart::Evaluated, TenantMode::LogOnly);
        let mut body = chat_one();
        body["messages"][0]["tool_calls"][0]["function"]["name"] = "rm".into();
        let p = plan_with(&s, Format::OpenAiChat, body, &[vec![high()]]);
        assert_eq!(p.replaced, [c0(true)]);
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)]);
        assert_eq!(p.entries[0].tool, "rm");
        let d = detail(&p, 0);
        assert!(d.floor_denied);
        assert!(ids(&d.determining).contains(&"f"), "the record shows the enforcing floor policy");
        assert!(ids(&d.determining).contains(&"detectors-block-high"));
    }

    #[test]
    fn an_optimistic_floor_hit_on_a_later_call_keeps_its_own_detail() {
        // Two earlier calls with one id: `ok` first, then `rm` (floor-denied). No policy reads findings.
        let s = snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly);
        let mut t = turn(vec![]);
        t.prior_calls = vec![prior("c0", "ok"), prior("c0", "rm")];
        t.tool_results = vec![by_id("c0")];
        let p = plan_request(&s, Format::OpenAiChat, &t, &[vec![high()]], &Instant::now);
        assert_eq!(p.replaced, [c0(true)]);
        assert_eq!(p.entries[0].tool, "rm");
        assert_eq!(ids(&detail(&p, 0).determining), ["f"]);
    }

    #[test]
    fn a_not_replaced_target_gets_a_screen_record() {
        let s = tenant(PERMIT_ALL, TenantMode::Enforce);
        let p = plan_with(&s, Format::OpenAiChat, chat_one(), &[vec![high()]]);
        assert!(p.replaced.is_empty() && p.entries.is_empty() && p.entry_details.is_empty());
        assert_eq!(p.screen_records.len(), 1);
        let r = &p.screen_records[0];
        assert_eq!(r.tool, "fetch");
        assert_eq!(r.cedar.effect, Effect::Allow);
        assert_eq!(r.cedar.findings.iter().map(|f| f.kind.as_str()).collect::<Vec<_>>(), [INJECTION_MARKERS]);
    }

    #[test]
    fn entry_details_line_up_with_entries() {
        let s = tenant(
            r#"@id("ok") permit(principal, action == Pgs::Gateway::Action::"ok", resource);
               @id("p") permit(principal, action == Pgs::Gateway::Action::"fetch", resource) when { context.max_severity < 3 };"#,
            TenantMode::Enforce,
        );
        let mut body = chat_one();
        body["tools"] = serde_json::json!([
            {"type":"function","function":{"name":"x","parameters":{}}},
            {"type":"function","function":{"name":"ok","parameters":{}}}]);
        body["tool_choice"] = serde_json::json!({"type":"function","function":{"name":"x"}});
        let p = plan_with(&s, Format::OpenAiChat, body, &[vec![high()]]);
        assert_eq!(
            p.entries.iter().map(|e| e.action).collect::<Vec<_>>(),
            [RequestAction::RemoveDefinition, RequestAction::ReplaceResult, RequestAction::ResetToolChoice]
        );
        assert_eq!(p.entry_details.iter().map(Option::is_some).collect::<Vec<_>>(), [false, true, false]);
    }

    #[test]
    fn a_result_without_ids_gets_a_screen_record_and_is_not_replaced() {
        // P-6: an orphan result is decided by the D32 pair, and never replaced.
        let s = tenant(PERMIT_ALL_STARTER, TenantMode::Enforce);
        let mut t = turn(vec![]);
        t.tool_results = vec![ToolResultRef {
            call_id: None,
            name: None,
            item_id: None,
            shape: ResultShape::Text,
        }];
        let p = plan_request(&s, Format::OpenAiChat, &t, &[vec![high()]], &Instant::now);
        assert!(p.replaced.is_empty() && p.entries.is_empty());
        assert_eq!(p.screen_records.len(), 1);
        let r = &p.screen_records[0];
        assert_eq!((r.tool.as_str(), r.cedar.effect), ("", Effect::Deny));
        assert_eq!(ids(&r.cedar.determining), ["detectors-block-high"]);
        // No findings: no record.
        assert!(plan_request(&s, Format::OpenAiChat, &t, &[vec![]], &Instant::now).screen_records.is_empty());
    }

    #[test]
    fn no_findings_keeps_the_sma_752_path() {
        let s = snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let mut t = turn(vec![def("x"), def("ok")]);
        t.prior_calls = vec![prior("c1", "rm"), prior("c2", "x")];
        t.tool_results = vec![by_id("c1"), by_id("c2")];
        let none = plan_request(&s, Format::OpenAiChat, &t, &[], &Instant::now);
        let empty = plan_request(&s, Format::OpenAiChat, &t, &[vec![], vec![]], &Instant::now);
        assert_eq!(none, empty);
        assert!(none.screen_records.is_empty());
        assert_eq!(none.entry_details, [None, None, None]);
    }

    #[test]
    fn a_wrong_findings_count_gives_every_result_a_detector_error() {
        assert!(matches!(findings_per_result(2, &[]), Cow::Borrowed(_)));
        let one = [vec![high()]];
        assert!(matches!(findings_per_result(1, &one), Cow::Borrowed(_)));
        let wrong = findings_per_result(3, &one);
        assert_eq!(wrong.len(), 3);
        for f in wrong.iter() {
            let got: Vec<(&str, Severity, &str)> = f.iter().map(|f| (f.kind.as_str(), f.severity, f.detector.as_str())).collect();
            assert_eq!(got, [(DETECTOR_ERROR, Severity::High, DETECTOR_BUILT_IN)]);
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "one finding list per result, or none")]
    fn a_wrong_findings_count_panics_in_a_debug_build() {
        let _ = plan_with(&tenant(PERMIT_ALL, TenantMode::Enforce), Format::OpenAiChat, chat_one(), &[vec![], vec![]]);
    }

    // Spec A13: the pair cache and the pair budget. The counts come from the clock: it is read
    // twice for each decided pair (around A), so `reads / 2` is the number of distinct pairs.

    /// The plan with a clock that counts its reads and always gives one instant (duration zero).
    fn counted(s: &PolicySnapshot, t: &TurnFacts, findings: &[Vec<Finding>]) -> (RequestPlan, usize) {
        use std::cell::Cell;
        let t0 = Instant::now();
        let reads = Cell::new(0_usize);
        let clock = || {
            reads.set(reads.get() + 1);
            t0
        };
        let p = plan_request(s, Format::OpenAiChat, t, findings, &clock);
        (p, reads.get())
    }

    /// `n` legacy `function_call`s named `fetch` with input `{}`, and `n` legacy results whose raw
    /// names all normalise to `fetch`. Each result is its own target, and each target has all `n`
    /// earlier calls: `n * n` (target, call) pairs.
    fn legacy_n_by_n(n: usize) -> TurnFacts {
        let mut t = turn(vec![]);
        t.prior_calls = (0..n)
            .map(|_| PriorCall {
                id: None,
                name: "fetch".to_owned(),
                arguments: "{}".to_owned(),
                kind: ToolCallKind::LegacyFunctionCall,
                local_shell: false,
                item_id: None,
            })
            .collect();
        t.tool_results = (0..n)
            .map(|i| ToolResultRef {
                call_id: None,
                name: Some(format!("fetch{}", " ".repeat(i))),
                item_id: None,
                shape: ResultShape::Text,
            })
            .collect();
        t
    }

    #[test]
    fn equal_pairs_of_many_targets_are_decided_once() {
        let s = tenant(PERMIT_ALL_STARTER, TenantMode::Enforce);
        let (control, control_reads) = counted(&s, &legacy_n_by_n(2), &vec![vec![high()]; 2]);
        assert_eq!(control_reads, 2, "one distinct pair");
        assert_eq!(control.replaced.len(), 2);
        let want = control.entry_details[0].clone().expect("detail");
        assert!(want.findings.iter().all(|f| f.kind != DETECTOR_ERROR));

        let n = 2000;
        let (p, reads) = counted(&s, &legacy_n_by_n(n), &vec![vec![high()]; n]);
        // 2000 x 2000 (target, call) pairs, one Cedar pair: the cache decided it once.
        assert_eq!(reads, 2, "one distinct pair for {} (target, call) pairs", n * n);
        assert_eq!(p.replaced.len(), n);
        assert!(p.replaced.iter().all(|i| i.enforced));
        assert!(p.entries.iter().all(|e| e.tool == "fetch"));
        assert!(p.entry_details.iter().all(|d| d.as_ref() == Some(&want)), "the same outcome as the control");
    }

    /// `n` results, each with its own earlier call with distinct arguments: `n` distinct pairs.
    fn distinct(n: usize) -> TurnFacts {
        let mut t = turn(vec![]);
        t.prior_calls = (0..n)
            .map(|i| {
                let mut c = prior(&format!("c{i}"), "fetch");
                c.arguments = format!(r#"{{"i":{i}}}"#);
                c
            })
            .collect();
        t.tool_results = (0..n).map(|i| by_id(&format!("c{i}"))).collect();
        t
    }

    /// The finding records of a detail as (kind, severity, detector).
    fn records(d: &CedarDetail) -> Vec<(&str, Option<Severity>, &str)> {
        d.findings.iter().map(|f| (f.kind.as_str(), f.severity, f.detector.as_str())).collect()
    }

    const BUDGET_RECORDS: [(&str, Option<Severity>, &str); 2] = [(DETECTOR_ERROR, Some(Severity::High), DETECTOR_BUILT_IN), (INJECTION_MARKERS, Some(Severity::High), INJECTION_MARKERS)];

    #[test]
    fn past_the_pair_budget_an_allow_all_tenant_replaces_nothing() {
        // A1: a tenant with no detector policy ("signals only") keeps every result.
        let n = PAIR_BUDGET + 6;
        for mode in [TenantMode::Enforce, TenantMode::LogOnly] {
            let (p, reads) = counted(&tenant(PERMIT_ALL, mode), &distinct(n), &vec![vec![high()]; n]);
            // 64 budget pairs, then one pseudo pair for the one tool name `fetch`.
            assert_eq!(reads, 2 * (PAIR_BUDGET + 1), "{mode:?}");
            assert!(p.replaced.is_empty() && p.entries.is_empty(), "{mode:?}");
            assert_eq!(p.screen_records.len(), n, "{mode:?}");
            assert!(p.screen_records.iter().all(|r| r.cedar.effect == Effect::Allow));
            // The records past the budget hold the error; the ones before do not.
            for (i, r) in p.screen_records.iter().enumerate() {
                let past = i >= PAIR_BUDGET;
                assert_eq!(r.cedar.findings.iter().any(|f| f.kind == DETECTOR_ERROR), past, "{mode:?} {i}");
                if past {
                    assert_eq!(records(&r.cedar), BUDGET_RECORDS);
                }
            }
        }
        // At the budget exactly, no pseudo pair runs.
        let (p, reads) = counted(&tenant(PERMIT_ALL, TenantMode::Enforce), &distinct(PAIR_BUDGET), &vec![vec![high()]; PAIR_BUDGET]);
        assert_eq!((reads, p.screen_records.len(), p.replaced.len()), (2 * PAIR_BUDGET, PAIR_BUDGET, 0));
        assert!(p.screen_records.iter().all(|r| r.cedar.findings.iter().all(|f| f.kind != DETECTOR_ERROR)));
    }

    #[test]
    fn past_the_pair_budget_the_starter_policy_replaces() {
        let n = PAIR_BUDGET + 6;
        for (mode, enforced) in [(TenantMode::Enforce, true), (TenantMode::LogOnly, false)] {
            let (p, _) = counted(&tenant(PERMIT_ALL_STARTER, mode), &distinct(n), &vec![vec![high()]; n]);
            // Every target is replaced by the policy: before the budget by its own pair, past it by
            // the pseudo pair. `log_only` gives Observe (an unenforced item).
            assert_eq!(p.replaced.len(), n, "{mode:?}");
            assert!(p.replaced.iter().all(|i| i.enforced == enforced), "{mode:?}");
            for d in &p.entry_details[PAIR_BUDGET..] {
                let d = d.as_ref().expect("detail");
                assert_eq!((d.effect, ids(&d.determining)), (Effect::Deny, vec!["detectors-block-high"]));
                assert_eq!(records(d), BUDGET_RECORDS);
            }
        }
    }

    #[test]
    fn past_the_pair_budget_one_tool_name_costs_one_pair() {
        let n = PAIR_BUDGET + 2000;
        let (p, reads) = counted(&tenant(PERMIT_ALL_STARTER, TenantMode::Enforce), &distinct(n), &vec![vec![high()]; n]);
        assert_eq!(reads, 2 * (PAIR_BUDGET + 1), "2000 targets past the budget, one tool name: one pseudo pair");
        assert_eq!(p.replaced.len(), n);
    }

    #[test]
    fn an_enforced_hit_is_kept_when_a_later_pair_of_the_target_is_past_the_budget() {
        // 63 targets use 63 pairs. Target `cx` has two calls: `rm` uses pair 64 and finds a floor
        // difference (enforced in log_only); `b` needs pair 65, past the budget. The pseudo pair
        // (`rm`, input `{}`) does not differ, because the floor forbid reads `input.p`.
        let floor = r#"@id("fp") forbid(principal, action == Pgs::Gateway::Action::"rm", resource)
            when { context.findings.contains("injection_markers") && context.input has p };"#;
        let s = snap(floor, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly);
        let mut t = distinct(PAIR_BUDGET - 1);
        let mut rm = prior("cx", "rm");
        rm.arguments = r#"{"p":1}"#.to_owned();
        let mut b = prior("cx", "b");
        b.arguments = r#"{"z":1}"#.to_owned();
        t.prior_calls.extend([rm, b]);
        t.tool_results.push(by_id("cx"));
        let (p, reads) = counted(&s, &t, &vec![vec![high()]; PAIR_BUDGET]);
        assert_eq!(reads, 2 * (PAIR_BUDGET + 1), "64 budget pairs and one pseudo pair");
        assert_eq!(p.replaced, [item(ResultTarget::CallId("cx".to_owned()), true)]);
        assert_eq!(p.entries[0].tool, "rm");
        let d = detail(&p, 0);
        assert!(d.floor_denied);
        assert_eq!(ids(&d.determining), ["fp"]);
    }

    #[test]
    fn an_orphan_past_the_pair_budget_gets_a_screen_record_only() {
        let mut t = distinct(PAIR_BUDGET);
        // A result with no id is decided by the pseudo pair, which is past the budget.
        t.tool_results.push(ToolResultRef {
            call_id: None,
            name: None,
            item_id: None,
            shape: ResultShape::Text,
        });
        for (text, effect) in [(PERMIT_ALL, Effect::Allow), (PERMIT_ALL_STARTER, Effect::Deny)] {
            let (p, _) = counted(&tenant(text, TenantMode::Enforce), &t, &vec![vec![high()]; PAIR_BUDGET + 1]);
            // P-6: never replaced, even when the policy denies. Only the 64 call targets can be.
            let calls_replaced = if effect == Effect::Deny { PAIR_BUDGET } else { 0 };
            assert_eq!((p.replaced.len(), p.screen_records.len()), (calls_replaced, PAIR_BUDGET + 1 - calls_replaced));
            let last = p.screen_records.last().expect("record");
            assert_eq!((last.tool.as_str(), last.cedar.effect), ("", effect));
            assert_eq!(records(&last.cedar), BUDGET_RECORDS);
        }
    }

    #[test]
    fn a_call_digest_separates_the_name_the_input_kind_and_the_arguments() {
        let d = call_digest("fetch", ToolCallKind::Function, "{}");
        assert_eq!(d, call_digest("fetch", ToolCallKind::Function, "{}"));
        assert_ne!(d, call_digest("Fetch", ToolCallKind::Function, "{}"));
        assert_ne!(d, call_digest("fetch", ToolCallKind::Custom, "{}"));
        assert_ne!(d, call_digest("fetch", ToolCallKind::Function, "{ }"));
        // The name is length-prefixed: a name/argument split cannot collide.
        assert_ne!(call_digest("ab", ToolCallKind::Function, "c"), call_digest("a", ToolCallKind::Function, "bc"));
        let mut medium = Findings::new();
        medium.add(INJECTION_MARKERS, Severity::Medium);
        assert_ne!(findings_digest(&medium), findings_digest(&kernel_findings(&[high()])), "the severity is in the key");
    }

    /// Spec A14: one target, two earlier calls `a` then `rm`, both differ by the finding.
    fn two_differing_calls(s: &PolicySnapshot) -> RequestPlan {
        let mut t = turn(vec![]);
        t.prior_calls = vec![prior("c0", "a"), prior("c0", "rm")];
        t.tool_results = vec![by_id("c0")];
        plan_request(s, Format::OpenAiChat, &t, &[vec![high()]], &Instant::now)
    }

    const TENANT_A: &str = r#"@id("all") permit(principal, action, resource);
        @id("ta") forbid(principal, action == Pgs::Gateway::Action::"a", resource) when { context.max_severity >= 3 };"#;

    #[test]
    fn the_record_cites_the_first_enforced_differing_pair() {
        // `a` differs only by a log-only tenant forbid; `rm` differs by a floor forbid.
        let floor = r#"@id("fr") forbid(principal, action == Pgs::Gateway::Action::"rm", resource) when { context.findings.contains("injection_markers") };"#;
        let p = two_differing_calls(&snap(floor, TENANT_A, TenantPart::Evaluated, TenantMode::LogOnly));
        assert_eq!(p.replaced, [c0(true)]);
        assert_eq!(p.entries[0].tool, "rm");
        let d = detail(&p, 0);
        assert!(d.floor_denied);
        assert_eq!(ids(&d.determining), ["fr"]);
    }

    #[test]
    fn with_no_enforced_pair_the_record_cites_the_first_differing_pair() {
        // The control: both differences are log-only tenant forbids.
        let tenant = format!(r#"{TENANT_A} @id("tr") forbid(principal, action == Pgs::Gateway::Action::"rm", resource) when {{ context.findings.contains("injection_markers") }};"#);
        let p = two_differing_calls(&snap(NO_FLOOR, &tenant, TenantPart::Evaluated, TenantMode::LogOnly));
        assert_eq!(p.replaced, [c0(false)]);
        assert_eq!(p.entries[0].tool, "a");
        assert_eq!(ids(&detail(&p, 0).determining), ["ta"]);
    }
}
