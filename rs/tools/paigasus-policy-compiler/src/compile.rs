// SPDX-License-Identifier: Apache-2.0
//! Compile one tool policy set: the source rules, the Dogwood lowering, the translation of each
//! temporal block, and the checks after lowering (spec 2026-11-15 iam-tool-policy-sets, section
//! 5.3, and the compile order D7 of the PR 1 plan).
//!
//! The compiler does not check the source size. IAM checks it before it starts the compiler.
//!
//! # D3: what Dogwood uses `rhai` for
//!
//! `cargo tree --locked -i rhai` at Dogwood commit `ff158506cfd9c7871385ed3a87d7435505fd24b1`:
//!
//! ```text
//! rhai v1.27.0
//! └── dogwood-language v1.1.1 (https://github.com/dogwood-policy/dogwood?rev=ff158506cfd9c7871385ed3a87d7435505fd24b1#ff158506)
//!     └── paigasus-policy-compiler v0.0.0 (...)
//! ```
//!
//! Dogwood uses `rhai` only to run information-provider scripts
//! (`dogwood-language/src/extension/provider/eval.rs`: `evaluate` at line 244 and
//! `run_methods` at line 314). Their only callers are `eval_provider` (`src/api.rs`, line 1462),
//! which `build_context` (line 1407) calls at line 1443. That is authorize time. Parsing,
//! lowering and validation do not run a script. The compiler never authorizes, its service
//! schema declares no providers, and it rejects every provider invocation before lowering. So
//! tenant source cannot reach `rhai`, and no operation limit is necessary.

use std::collections::BTreeSet;

use cedar_policy::{Effect, PolicyId, SchemaFragment, ValidationMode, ValidationWarning};
use dogwood_language::temporal_ast::{Condition, ConditionKind, Term};
use dogwood_language::{Error, LoweredPolicySet, ParsedPolicySet, PolicySchema, SourceLoc, Validator};
use serde::{Deserialize, Serialize};

use crate::lower::{PolicyUnit, lower_units, service_schema, slot_prefix, split_policies};
use crate::translate::{HistoryCondition, NON_NORMAL_ACTION, Scope, is_normal_action, translate_field};

/// Which set a source is. On the wire: `"tenant"` or `"floor"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetKind {
    Tenant,
    Floor,
}

/// The machine-readable reason of a rejection (spec section 7). It serializes as `as_wire`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonCode {
    ToolPolicyInvalid,
    ToolPolicyShapeUnsupported,
    ToolPolicyLoweringFailed,
    ToolPolicyFloorPermit,
    ToolPolicyIdInvalid,
    ToolPolicyWindowTooLong,
}

impl ReasonCode {
    /// The reason code on the wire.
    pub fn as_wire(&self) -> &'static str {
        match self {
            ReasonCode::ToolPolicyInvalid => "tool-policy-invalid",
            ReasonCode::ToolPolicyShapeUnsupported => "tool-policy-shape-unsupported",
            ReasonCode::ToolPolicyLoweringFailed => "tool-policy-lowering-failed",
            ReasonCode::ToolPolicyFloorPermit => "tool-policy-floor-permit",
            ReasonCode::ToolPolicyIdInvalid => "tool-policy-id-invalid",
            ReasonCode::ToolPolicyWindowTooLong => "tool-policy-window-too-long",
        }
    }
}

/// One reason why a source does not compile. On the wire every field is present, and an
/// absent value is `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rejection {
    /// The `@id` of the policy, when the rejection belongs to one policy that has an id.
    pub policy_id: Option<String>,
    /// The 1-based line in the source, when the error has a location in it.
    pub line: Option<u32>,
    /// The 1-based column (in characters), when the error has a location in the source.
    pub column: Option<u32>,
    pub reason: ReasonCode,
    pub message: String,
}

impl Rejection {
    pub(crate) fn new(policy_id: Option<&str>, reason: ReasonCode, message: impl Into<String>) -> Self {
        Rejection {
            policy_id: policy_id.map(str::to_string),
            line: None,
            column: None,
            reason,
            message: message.into(),
        }
    }
}

/// A compiled set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Compiled {
    /// The lowered Cedar text of the whole set.
    pub cedar_policies: String,
    /// The lowered schema in the Cedar JSON schema form, with the slot attributes.
    pub cedar_schema_json: serde_json::Value,
    /// One condition for each slot, sorted by `slot`.
    pub conditions: Vec<HistoryCondition>,
}

/// A unit that passed the source rules.
struct Checked<'a> {
    unit: &'a PolicyUnit,
    id: &'a str,
    scope: Scope,
}

/// Compile `source` against the action schema `cedarschema` (Cedar schema syntax).
pub fn compile_cedarschema(kind: SetKind, source: &str, cedarschema: &str, max_window_ms: u64) -> Result<Compiled, Vec<Rejection>> {
    let svc = service_schema();
    let units = split_policies(source, &svc).map_err(|error| vec![located(&error, source)])?;

    let mut rejections = Vec::new();
    let mut checked = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for unit in &units {
        let before = rejections.len();
        let id = check_id(kind, unit, &mut seen_ids, &mut rejections);
        let scope = check_scope(unit, id, &mut rejections);
        check_parse_tree(unit, id, &svc, &mut rejections);
        if let (Some(id), Some(scope), true) = (id, scope, rejections.len() == before) {
            checked.push(Checked { unit, id, scope });
        }
    }
    if !rejections.is_empty() {
        return Err(rejections);
    }

    if units.is_empty() {
        return empty_set(cedarschema);
    }
    let policy_schema = PolicySchema::from_cedarschema_str(cedarschema).map_err(|error| vec![Rejection::new(None, ReasonCode::ToolPolicyInvalid, format!("the action schema is invalid: {error}"))])?;
    let lowered = lower_units(&units, &svc, &policy_schema).map_err(|error| vec![Rejection::new(None, ReasonCode::ToolPolicyLoweringFailed, format!("Dogwood lowering failed: {error}"))])?;

    let validation = Validator::new().validate(&lowered);
    for error in validation.validation_errors() {
        let message = error.to_string();
        let owner = owner_by_text(&checked, &message).or_else(|| owner_by_source(&checked, error));
        rejections.push(Rejection::new(owner, ReasonCode::ToolPolicyInvalid, format!("Dogwood validation: {message}")));
    }
    if !rejections.is_empty() {
        return Err(rejections);
    }

    let conditions = translate_all(&lowered, &checked, &mut rejections);
    cedar_checks(kind, &lowered, &checked, &mut rejections);
    for condition in &conditions {
        if condition.window_ms > max_window_ms {
            rejections.push(Rejection::new(
                Some(&condition.policy_id),
                ReasonCode::ToolPolicyWindowTooLong,
                format!("the window of {} is {} ms, more than the limit of {max_window_ms} ms", condition.slot, condition.window_ms),
            ));
        }
    }
    if !rejections.is_empty() {
        return Err(rejections);
    }

    let schema_json = lowered
        .cedar_schema_json()
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string()))
        .map_err(|error| vec![Rejection::new(None, ReasonCode::ToolPolicyLoweringFailed, format!("the lowered schema has no JSON form: {error}"))])?;
    Ok(Compiled {
        cedar_policies: lowered.as_cedar().to_string(),
        cedar_schema_json: schema_json,
        conditions,
    })
}

/// The `@id` rules: present, in the grammar, unique, and the `floor-` prefix rule.
fn check_id<'a>(kind: SetKind, unit: &'a PolicyUnit, seen: &mut BTreeSet<&'a str>, rejections: &mut Vec<Rejection>) -> Option<&'a str> {
    // The rejection names the policy only when its id is in the grammar.
    let reject = |rejections: &mut Vec<Rejection>, id: Option<&str>, message: String| rejections.push(Rejection::new(id, ReasonCode::ToolPolicyIdInvalid, message));
    // Dogwood keeps every annotation, but the emitted Cedar annotation map keeps the LAST `@id`,
    // and `unit.id` is the first. A second `@id` could make the checks and the slots use one id
    // and the Cedar text another.
    if unit.annotations.iter().filter(|(key, _)| key == "id").count() > 1 {
        reject(rejections, None, format!("policy {} has more than one @id", unit.index));
        return None;
    }
    let Some(id) = unit.id.as_deref() else {
        reject(rejections, None, format!("policy {} has no @id value", unit.index));
        return None;
    };
    if !is_valid_id(id) {
        reject(rejections, None, format!("policy {}: the @id {id:?} does not match ^[a-z0-9][a-z0-9_-]{{0,63}}$", unit.index));
        return None;
    }
    if !seen.insert(id) {
        reject(rejections, Some(id), format!("policy {}: the @id {id:?} is not unique", unit.index));
        return None;
    }
    let has_prefix = id.starts_with("floor-");
    match (kind, has_prefix) {
        (SetKind::Floor, false) => reject(rejections, Some(id), format!("policy {}: a floor @id must start with \"floor-\", but {id:?} does not", unit.index)),
        (SetKind::Tenant, true) => reject(rejections, Some(id), format!("policy {}: a tenant @id must not start with \"floor-\", but {id:?} does", unit.index)),
        _ => return Some(id),
    }
    None
}

fn is_valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let tail_ok = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-';
    matches!(bytes.first(), Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit()) && bytes.len() <= 64 && bytes.iter().all(tail_ok)
}

/// The `@scope` rule: absent (session), or one of `session`, `principal`, `key`.
fn check_scope(unit: &PolicyUnit, id: Option<&str>, rejections: &mut Vec<Rejection>) -> Option<Scope> {
    // As for `@id`: the emitted Cedar annotation map keeps the LAST `@scope`, and this check reads
    // the first. Two values could make the history condition and the Cedar text disagree.
    if unit.annotations.iter().filter(|(key, _)| key == "scope").count() > 1 {
        rejections.push(Rejection::new(id, ReasonCode::ToolPolicyIdInvalid, format!("policy {} has more than one @scope", unit.index)));
        return None;
    }
    let Some((_, value)) = unit.annotations.iter().find(|(key, _)| key == "scope") else {
        return Some(Scope::Session);
    };
    match value.as_deref() {
        Some("session") => Some(Scope::Session),
        Some("principal") => Some(Scope::Principal),
        Some("key") => Some(Scope::Key),
        other => {
            let shown = other.map_or_else(|| "no value".to_string(), |v| format!("{v:?}"));
            rejections.push(Rejection::new(
                id,
                ReasonCode::ToolPolicyIdInvalid,
                format!("policy {}: the @scope has {shown}; use \"session\", \"principal\" or \"key\"", unit.index),
            ));
            None
        }
    }
}

/// The rules on the Dogwood parse tree, before any lowering: no provider invocation, and every
/// action literal in the Cedar scope and in each temporal predicate in normal form.
fn check_parse_tree(unit: &PolicyUnit, id: Option<&str>, svc: &dogwood_language::ServiceSchema, rejections: &mut Vec<Rejection>) {
    let parsed = match ParsedPolicySet::parse(&unit.source, svc) {
        Ok(parsed) => parsed,
        Err(error) => {
            rejections.push(Rejection::new(
                id,
                ReasonCode::ToolPolicyInvalid,
                format!("policy {}: the rendering does not parse again: {error}", unit.index),
            ));
            return;
        }
    };
    for policy in parsed.policies() {
        let providers: Vec<String> = policy.provider_invocations().collect();
        if !providers.is_empty() {
            rejections.push(Rejection::new(
                id,
                ReasonCode::ToolPolicyInvalid,
                format!("policy {}: information providers are not supported ({})", unit.index, providers.join(", ")),
            ));
        }
        let mut literals: Vec<String> = match policy.scope().action() {
            dogwood_language::ActionConstraint::Any => Vec::new(),
            dogwood_language::ActionConstraint::Eq(uid) => vec![uid.id().unescaped().to_string()],
            dogwood_language::ActionConstraint::In(uids) => uids.iter().map(|uid| uid.id().unescaped().to_string()).collect(),
        };
        for condition in policy.temporal_conditions() {
            collect_actions(condition, &mut literals);
        }
        for literal in literals.iter().filter(|literal| !is_normal_action(literal)) {
            rejections.push(Rejection::new(
                id,
                ReasonCode::ToolPolicyInvalid,
                format!("policy {}: the action {literal:?} is not `InvokeTool`, `Unregistered` or a tool name in normal form", unit.index),
            ));
        }
    }
}

/// Every `Predicate.action` in a temporal condition, at any depth.
fn collect_actions(condition: &Condition, out: &mut Vec<String>) {
    match &condition.kind {
        ConditionKind::Predicate(p) => {
            out.push(p.action.clone());
            p.args.iter().for_each(|arg| collect_term_actions(&arg.value, out));
        }
        ConditionKind::And { left, right } | ConditionKind::Or { left, right } | ConditionKind::Since { left, right, .. } => {
            collect_actions(left, out);
            collect_actions(right, out);
        }
        ConditionKind::Not { inner } => collect_actions(inner, out),
        ConditionKind::Formerly { body, .. } | ConditionKind::Previous { body, .. } | ConditionKind::Exists { body, .. } => collect_actions(body, out),
        ConditionKind::Comparison { left, right, .. } => {
            collect_term_actions(left, out);
            collect_term_actions(right, out);
        }
        ConditionKind::Refine { base, fields, .. } => {
            collect_actions(base, out);
            fields.iter().for_each(|arg| collect_term_actions(&arg.value, out));
        }
        ConditionKind::Tp { .. } | ConditionKind::Call(_) | ConditionKind::SigilRef { .. } => {}
    }
}

fn collect_term_actions(term: &Term, out: &mut Vec<String>) {
    match term {
        Term::Agg(agg) => match &agg.kind {
            dogwood_language::temporal_ast::AggExprKind::Sum { body, .. } | dogwood_language::temporal_ast::AggExprKind::Count { body, .. } => collect_actions(body, out),
            dogwood_language::temporal_ast::AggExprKind::Call(_) => {}
        },
        Term::Arith { left, right, .. } => {
            collect_term_actions(left, out);
            collect_term_actions(right, out);
        }
        Term::Array(items) => items.iter().for_each(|item| collect_term_actions(item, out)),
        _ => {}
    }
}

/// A set with no policy: no lowering, and the input schema in its JSON form.
fn empty_set(cedarschema: &str) -> Result<Compiled, Vec<Rejection>> {
    let schema_json = SchemaFragment::from_cedarschema_str(cedarschema)
        .map_err(|error| error.to_string())
        .and_then(|(fragment, _warnings)| fragment.to_json_value().map_err(|error| error.to_string()))
        .map_err(|error| vec![Rejection::new(None, ReasonCode::ToolPolicyInvalid, format!("the action schema is invalid: {error}"))])?;
    Ok(Compiled {
        cedar_policies: String::new(),
        cedar_schema_json: schema_json,
        conditions: Vec::new(),
    })
}

/// Translate every temporal field. Under D2 the non-relativized fields equal `temporal_fields()`.
fn translate_all(lowered: &LoweredPolicySet, checked: &[Checked<'_>], rejections: &mut Vec<Rejection>) -> Vec<HistoryCondition> {
    if !lowered.nonrelativized_temporal_fields().eq(lowered.temporal_fields()) {
        rejections.push(Rejection::new(
            None,
            ReasonCode::ToolPolicyLoweringFailed,
            "the lowered temporal fields depend on the event schema pins, which the compiler does not support",
        ));
        return Vec::new();
    }
    let mut conditions = Vec::new();
    for field in lowered.nonrelativized_temporal_fields() {
        let Some(owner) = checked.iter().find(|c| field.id.starts_with(&format!("{}__temporal_", slot_prefix(c.id)))) else {
            rejections.push(Rejection::new(None, ReasonCode::ToolPolicyLoweringFailed, format!("the slot {} belongs to no policy", field.id)));
            continue;
        };
        match translate_field(field, owner.id, owner.scope) {
            Ok(condition) => conditions.push(condition),
            Err(error) if error.shape == NON_NORMAL_ACTION => rejections.push(Rejection::new(
                Some(owner.id),
                ReasonCode::ToolPolicyInvalid,
                format!("policy {:?}: the temporal block {}: {}", owner.id, field.id, error.detail),
            )),
            Err(error) => rejections.push(Rejection::new(
                Some(owner.id),
                ReasonCode::ToolPolicyShapeUnsupported,
                format!("policy {:?}: the temporal block {} uses the unsupported shape `{}`: {}", owner.id, field.id, error.shape, error.detail),
            )),
        }
    }
    conditions.sort_by(|a, b| a.slot.cmp(&b.slot));
    conditions
}

/// The strict `cedar-policy` 4.11 validation of the lowered Cedar (errors, and the
/// impossible-policy warning), and the floor effect rule.
fn cedar_checks(kind: SetKind, lowered: &LoweredPolicySet, checked: &[Checked<'_>], rejections: &mut Vec<Rejection>) {
    let validator = cedar_policy::Validator::new(lowered.cedar_schema().clone());
    let result = validator.validate(lowered.as_cedar(), ValidationMode::Strict);
    for error in result.validation_errors() {
        rejections.push(Rejection::new(
            owner_by_policy_id(checked, error.policy_id()),
            ReasonCode::ToolPolicyInvalid,
            format!("Cedar validation: {error}"),
        ));
    }
    // A strict validation types a `has` guard on a field that the schema does not declare as
    // `False`. Then the policy is never satisfied, and cedar gives only an "impossible policy"
    // warning. Such a policy reads a field that is not in the schema, so it is rejected.
    for warning in result.validation_warnings() {
        if let ValidationWarning::ImpossiblePolicy(_) = warning {
            let owner = owner_by_policy_id(checked, warning.policy_id());
            rejections.push(Rejection::new(
                owner,
                ReasonCode::ToolPolicyInvalid,
                format!("Cedar validation: {warning}; a guarded field may be missing from the schema"),
            ));
        }
    }
    if kind == SetKind::Floor {
        for policy in lowered.as_cedar().policies().filter(|policy| policy.effect() == Effect::Permit) {
            let owner = owner_by_policy_id(checked, policy.id());
            rejections.push(Rejection::new(
                owner,
                ReasonCode::ToolPolicyFloorPermit,
                format!("the floor policy {:?} is a permit; a floor policy must be a forbid", owner.unwrap_or(AsRef::<str>::as_ref(policy.id()))),
            ));
        }
    }
}

fn owner_by_policy_id<'a>(checked: &[Checked<'a>], policy_id: &PolicyId) -> Option<&'a str> {
    checked.iter().find(|c| slot_prefix(c.id) == AsRef::<str>::as_ref(policy_id)).map(|c| c.id)
}

/// The policy whose Cedar policy id (the slot prefix) is a word of `text`.
fn owner_by_text<'a>(checked: &[Checked<'a>], text: &str) -> Option<&'a str> {
    let words: BTreeSet<&str> = text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).collect();
    checked
        .iter()
        .find(|c| {
            let prefix = slot_prefix(c.id);
            words.iter().any(|word| *word == prefix || word.starts_with(&format!("{prefix}__temporal_")))
        })
        .map(|c| c.id)
}

/// The policy whose rendering the finding is located in.
fn owner_by_source<'a>(checked: &[Checked<'a>], error: &dogwood_language::ValidationError) -> Option<&'a str> {
    let src = match error {
        dogwood_language::ValidationError::Cedar { src, .. } | dogwood_language::ValidationError::Extension { src, .. } => src,
        _ => return None,
    };
    checked.iter().find(|c| c.unit.source == **src).map(|c| c.id)
}

/// A parse error of the whole source, with its location.
fn located(error: &Error, source: &str) -> Rejection {
    let location: Option<&SourceLoc> = match error {
        Error::Parse(errors) => errors.iter().next().map(|e| e.location()),
        Error::Macro(e) => Some(e.location()),
        Error::Cedarify(e) => e.location(),
        _ => None,
    };
    let mut rejection = Rejection::new(None, ReasonCode::ToolPolicyInvalid, format!("the source does not parse: {error}"));
    if let Some(loc) = location.filter(|loc| loc.source() == source) {
        let (line, column) = line_column(source, loc.span().offset());
        rejection.line = Some(line);
        rejection.column = Some(column);
    }
    rejection
}

/// The 1-based line and column (in characters) of a byte offset.
fn line_column(source: &str, offset: usize) -> (u32, u32) {
    let before = source.get(..offset.min(source.len())).unwrap_or(source);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (u32::try_from(line).unwrap_or(u32::MAX), u32::try_from(column).unwrap_or(u32::MAX))
}
