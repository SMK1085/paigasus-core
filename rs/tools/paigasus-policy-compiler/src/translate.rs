// SPDX-License-Identifier: Apache-2.0
//! Translate one lowered Dogwood temporal block into one history condition (spec 2026-11-15
//! iam-tool-policy-sets, section 5.3 step 5).
//!
//! The input is a `TemporalField` of `LoweredPolicySet::nonrelativized_temporal_fields()`. Only
//! the nodes below are accepted. Every other node or term is rejected with
//! `tool-policy-shape-unsupported`, and the `ShapeError` names the node.
//!
//! # The Dogwood node inventory
//!
//! The file is `dogwood-language/src/extension/temporal/ast.rs` at Dogwood commit
//! `ff158506cfd9c7871385ed3a87d7435505fd24b1`. The line is the line of the variant or the type.
//!
//! | Node (ast.rs line) | Maps to |
//! |---|---|
//! | `ConditionKind::Formerly` (53) with `WithinSpec::Concrete` (260) | `Shape::Formerly`, `window_ms` from `Interval::checked_seconds` (244) |
//! | `ConditionKind::Comparison` (84) of `Term::Agg` (391) with `AggExprKind::Count` (171) and `Term::Integer` (353) | `Shape::CountWithin` |
//! | The `bind` form: `ConditionKind::Exists` (94) over `And` (28) of `Comparison { Eq, Agg, Term::Var (382) }` and `Comparison { op, Var, Integer }` | `Shape::CountWithin` or `Shape::SumWithin` |
//! | `AggExprKind::Count` (171) with `for_vars` = one `TypedBinder` (468) of `Type::Timepoint` (444), body `Formerly { W, And(P, ConditionKind::Tp (102)) }` | `Shape::CountWithin` |
//! | `AggExprKind::Sum` (163) with `for_vars` = `[a: Long, t: Timepoint]`, the same body, and exactly one `NamedArg` (200) `input.<path>: Var(a)` in `P` | `Shape::SumWithin { field_path }` |
//! | `ConditionKind::Predicate` (77), `Predicate` (183) with namespace `Pgs::Gateway::Action` | `AllOf(EventKind, Tool or AnyCatalogTool, Field..)` |
//! | `Predicate.kind` `request` / `response` | `EventKind`; `error` and other kinds are rejected |
//! | `Predicate.action` `InvokeTool` / a normal-form tool name | `AnyCatalogTool` / `Tool { name_key }` |
//! | `NamedArg` `input.<path>` with `Term::Integer` (353), `Term::String` (356), `Term::Bool` (357), `Term::Decimal` (355) | `Field { op: Eq }` |
//! | `Exists { v: Long, And(Predicate { input.<p>: Var(v) }, Comparison { op, Var(v), Integer(c) }) }` in a predicate position | `Field { op }` |
//! | `ConditionKind::And` (28), `Or` (43), `Not` (50) in a predicate position | `AllOf`, `AnyOf`, `Negation` |
//!
//! The surface grammar has no `||`. Dogwood makes `Or` only in its pin rewrite
//! (`event_schema::relativize`), and the compiler uses no pins (D2). So no source gives `AnyOf`
//! today. The mapping stays for a later Dogwood with a surface `||`.
//!
//! Rejected, among others: `ConditionKind::Previous` (59), `Since` (68), `Tp` (102) outside an
//! aggregate body, `Call` (106), `SigilRef` (111), `Refine` (125); `WithinSpec::ParamRef` (261);
//! `AggExprKind::Call` (177); a `count` with other binders (the `count_distinct_within` macro);
//! `Term::Entity` (349), `ContextField` (366), `ScopeField` (380), `Wildcard` (384), `Array`
//! (385), `Arith` (398), `ParamRef` (406), `BinderRef` (410), and a `Var` that no accepted form
//! binds. A root that combines two shapes (for example `And(Formerly, CountWithin)`) is rejected
//! as `two-shapes`.

use dogwood_language::TemporalField;
use dogwood_language::temporal_ast::{self as ast, AggExprKind, Condition, ConditionKind, NamedArg, Term, Type, WithinSpec};

/// How the gateway partitions the history of a condition (`@scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Session,
    Principal,
    Key,
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
    Ne,
}

impl From<ast::CmpOp> for CmpOp {
    fn from(op: ast::CmpOp) -> Self {
        match op {
            ast::CmpOp::Lt => CmpOp::Lt,
            ast::CmpOp::Le => CmpOp::Le,
            ast::CmpOp::Eq => CmpOp::Eq,
            ast::CmpOp::Ge => CmpOp::Ge,
            ast::CmpOp::Gt => CmpOp::Gt,
            ast::CmpOp::NotEq => CmpOp::Ne,
        }
    }
}

/// A constant that an argument field is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Long(i64),
    String(String),
    Bool(bool),
    /// The decimal as written in `decimal("…")`.
    Decimal(String),
}

/// The kind of a recorded event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Request,
    Response,
}

/// A predicate on one recorded event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    EventKind(EventKind),
    /// The event is a call of the tool with this match key.
    Tool {
        name_key: String,
    },
    /// The event is a call of any tool in the catalog (`InvokeTool`).
    AnyCatalogTool,
    /// The argument field at `path` (below `input`) compares with `value`.
    Field {
        path: Vec<String>,
        op: CmpOp,
        value: Literal,
    },
    AllOf(Box<Predicate>, Box<Predicate>),
    AnyOf(Box<Predicate>, Box<Predicate>),
    Negation(Box<Predicate>),
}

/// The shape of a history condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Formerly {
        predicate: Predicate,
    },
    CountWithin {
        predicate: Predicate,
        op: CmpOp,
        threshold: i64,
    },
    SumWithin {
        predicate: Predicate,
        field_path: Vec<String>,
        op: CmpOp,
        threshold: i64,
    },
}

/// One temporal slot of the lowered set, as the gateway evaluates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCondition {
    /// The slot attribute in the lowered Cedar context.
    pub slot: String,
    /// The `@id` of the policy the slot came from.
    pub policy_id: String,
    pub scope: Scope,
    pub window_ms: u64,
    pub shape: Shape,
}

/// A temporal block outside the accepted shapes. `shape` names the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeError {
    pub shape: &'static str,
    pub detail: String,
}

fn unsupported<T>(shape: &'static str, detail: impl Into<String>) -> Result<T, ShapeError> {
    Err(ShapeError { shape, detail: detail.into() })
}

/// Translate one lowered temporal block.
pub fn translate_field(field: &TemporalField, policy_id: &str, scope: Scope) -> Result<HistoryCondition, ShapeError> {
    let (window_ms, shape) = translate_root(&field.condition.condition)?;
    Ok(HistoryCondition {
        slot: field.id.clone(),
        policy_id: policy_id.to_string(),
        scope,
        window_ms,
        shape,
    })
}

fn translate_root(root: &Condition) -> Result<(u64, Shape), ShapeError> {
    match &root.kind {
        ConditionKind::Formerly { within, body } => {
            let window_ms = window_ms(within)?;
            let predicate = predicate(body, &mut None)?;
            Ok((window_ms, Shape::Formerly { predicate }))
        }
        ConditionKind::Comparison {
            op,
            left: Term::Agg(agg),
            right: Term::Integer(threshold),
        } => aggregate(agg, (*op).into(), *threshold),
        ConditionKind::Exists { var, body } => match bind_form(var, body) {
            Some((agg, op, threshold)) => aggregate(agg, op, threshold),
            None => unsupported("Exists", "an `exists` at the root is accepted only as the `bind` form of an aggregate"),
        },
        ConditionKind::And { .. } | ConditionKind::Or { .. } | ConditionKind::Not { .. } if shape_count(root) >= 2 => {
            unsupported("two-shapes", "the block combines more than one shape; write one shape for each temporal block")
        }
        other => unsupported(condition_name(other), "the root of a temporal block must be `formerly within`, `count_within` or `sum_within`"),
    }
}

/// The `bind(n, A, n <op> c)` form: `exists (n: Long). (A == n && n <op> c)`.
fn bind_form<'a>(var: &ast::TypedBinder, body: &'a Condition) -> Option<(&'a ast::AggExpr, CmpOp, i64)> {
    if !is_named(&var.ty, "Long") {
        return None;
    }
    let name = binder_name(&var.slot)?;
    let ConditionKind::And { left, right } = &body.kind else { return None };
    let ConditionKind::Comparison {
        op: ast::CmpOp::Eq,
        left: Term::Agg(agg),
        right: Term::Var(bound),
    } = &left.kind
    else {
        return None;
    };
    let ConditionKind::Comparison {
        op,
        left: Term::Var(used),
        right: Term::Integer(threshold),
    } = &right.kind
    else {
        return None;
    };
    (bound == name && used == name).then_some((agg.as_ref(), (*op).into(), *threshold))
}

/// The number of shapes (temporal operators and aggregates) below a root.
fn shape_count(condition: &Condition) -> usize {
    match &condition.kind {
        ConditionKind::Formerly { .. } | ConditionKind::Previous { .. } | ConditionKind::Since { .. } => 1,
        ConditionKind::Comparison { left, right, .. } => [left, right].iter().filter(|term| matches!(term, Term::Agg(_))).count(),
        ConditionKind::Exists { body, .. } => shape_count(body),
        ConditionKind::And { left, right } | ConditionKind::Or { left, right } => shape_count(left) + shape_count(right),
        ConditionKind::Not { inner } => shape_count(inner),
        _ => 0,
    }
}

fn aggregate(agg: &ast::AggExpr, op: CmpOp, threshold: i64) -> Result<(u64, Shape), ShapeError> {
    match &agg.kind {
        AggExprKind::Count { for_vars, body } => {
            let [timepoint] = for_vars.as_slice() else {
                return unsupported("count_distinct_within", "a `count` must range over exactly one Timepoint binder");
            };
            if timepoint.ty != Type::Timepoint {
                return unsupported("count_distinct_within", "a `count` must range over exactly one Timepoint binder");
            }
            let (window_ms, p) = aggregate_body(body, binder(timepoint)?, &mut None)?;
            Ok((window_ms, Shape::CountWithin { predicate: p, op, threshold }))
        }
        AggExprKind::Sum { bound_var, for_vars, body } => {
            let [value, timepoint] = for_vars.as_slice() else {
                return unsupported("AggExprKind::Sum", "a `sum` must range over one Long binder and one Timepoint binder");
            };
            let value_name = binder(value)?;
            if !is_named(&value.ty, "Long") || timepoint.ty != Type::Timepoint || binder_name(bound_var) != Some(value_name) {
                return unsupported("AggExprKind::Sum", "a `sum` must sum its Long binder and range over one Timepoint binder");
            }
            let mut capture = Some(SumCapture { var: value_name, path: None });
            let (window_ms, p) = aggregate_body(body, binder(timepoint)?, &mut capture)?;
            let Some(field_path) = capture.and_then(|c| c.path) else {
                return unsupported("AggExprKind::Sum", "the summed binder must be bound by exactly one `input.<path>` argument");
            };
            Ok((
                window_ms,
                Shape::SumWithin {
                    predicate: p,
                    field_path,
                    op,
                    threshold,
                },
            ))
        }
        AggExprKind::Call(call) => unsupported("AggExprKind::Call", format!("the macro call `{}` was not expanded", call.name)),
    }
}

/// The aggregate body `formerly within W (P && tp(t))`.
fn aggregate_body(body: &Condition, timepoint: &str, capture: &mut Option<SumCapture<'_>>) -> Result<(u64, Predicate), ShapeError> {
    let ConditionKind::Formerly { within, body } = &body.kind else {
        return unsupported(condition_name(&body.kind), "an aggregate body must be `formerly within W (P && tp(t))`");
    };
    let ConditionKind::And { left, right } = &body.kind else {
        return unsupported(condition_name(&body.kind), "an aggregate body must be `formerly within W (P && tp(t))`");
    };
    match &right.kind {
        ConditionKind::Tp { var } if binder_name(var) == Some(timepoint) => {}
        other => return unsupported(condition_name(other), "an aggregate body must end with `tp(t)` of its Timepoint binder"),
    }
    Ok((window_ms(within)?, predicate(left, capture)?))
}

/// The summed binder of a `sum`, and the field path that binds it.
struct SumCapture<'a> {
    var: &'a str,
    path: Option<Vec<String>>,
}

/// A field comparison of the `exists` form: the binder, and the comparison with a constant.
struct FieldBinding<'a> {
    var: &'a str,
    op: CmpOp,
    value: i64,
    used: bool,
}

/// Translate a predicate position. `capture` is passed only through `And`, so the summed
/// binder is accepted only where every match of the predicate binds it.
fn predicate(condition: &Condition, capture: &mut Option<SumCapture<'_>>) -> Result<Predicate, ShapeError> {
    match &condition.kind {
        ConditionKind::Predicate(p) => event_predicate(p, capture, &mut None),
        ConditionKind::And { left, right } => Ok(Predicate::AllOf(Box::new(predicate(left, capture)?), Box::new(predicate(right, capture)?))),
        ConditionKind::Or { left, right } => Ok(Predicate::AnyOf(Box::new(predicate(left, &mut None)?), Box::new(predicate(right, &mut None)?))),
        ConditionKind::Not { inner } => Ok(Predicate::Negation(Box::new(predicate(inner, &mut None)?))),
        ConditionKind::Exists { var, body } => field_exists(var, body, capture),
        other => unsupported(condition_name(other), "a predicate may use only event patterns, field comparisons, `&&`, `||` and `!`"),
    }
}

/// `exists (v: Long). (P{ input.<p>: v } && v <op> c)`.
fn field_exists(var: &ast::TypedBinder, body: &Condition, capture: &mut Option<SumCapture<'_>>) -> Result<Predicate, ShapeError> {
    const FORM: &str = "an `exists` in a predicate must be `exists (v: Long). (P{ input.<field>: v } && v <op> <integer>)`";
    let name = binder(var)?;
    if !is_named(&var.ty, "Long") {
        return unsupported("Exists", FORM);
    }
    let ConditionKind::And { left, right } = &body.kind else { return unsupported("Exists", FORM) };
    let ConditionKind::Predicate(p) = &left.kind else { return unsupported("Exists", FORM) };
    let ConditionKind::Comparison {
        op,
        left: Term::Var(used),
        right: Term::Integer(value),
    } = &right.kind
    else {
        return unsupported("Exists", FORM);
    };
    if used != name {
        return unsupported("Exists", FORM);
    }
    let mut binding = Some(FieldBinding {
        var: name,
        op: (*op).into(),
        value: *value,
        used: false,
    });
    let translated = event_predicate(p, capture, &mut binding)?;
    if !binding.is_some_and(|b| b.used) {
        return unsupported("Exists", FORM);
    }
    Ok(translated)
}

/// `Pgs::Gateway::Action::"<tool>"::<kind>{ args }`.
fn event_predicate(p: &ast::Predicate, capture: &mut Option<SumCapture<'_>>, binding: &mut Option<FieldBinding<'_>>) -> Result<Predicate, ShapeError> {
    if p.namespace != ["Pgs", "Gateway", "Action"] {
        return unsupported("Predicate", format!("the event pattern namespace `{}` is not `Pgs::Gateway::Action`", p.namespace.join("::")));
    }
    let kind = match p.kind.as_str() {
        "request" => EventKind::Request,
        "response" => EventKind::Response,
        "error" => return unsupported("error-event", "an `error` event pattern is not supported in v1"),
        other => return unsupported("Predicate", format!("the event kind `{other}` is not `request` or `response`")),
    };
    let tool = match p.action.as_str() {
        "InvokeTool" => Predicate::AnyCatalogTool,
        "Unregistered" => return unsupported("Predicate", "a history condition cannot match `Unregistered` tools"),
        name => Predicate::Tool { name_key: name.to_string() },
    };
    let mut result = Predicate::AllOf(Box::new(Predicate::EventKind(kind)), Box::new(tool));
    for arg in &p.args {
        if let Some(field) = argument(arg, capture, binding)? {
            result = Predicate::AllOf(Box::new(result), Box::new(field));
        }
    }
    Ok(result)
}

/// One `input.<path>: term` argument. A binder of a sum gives no predicate: it records the path.
fn argument(arg: &NamedArg, capture: &mut Option<SumCapture<'_>>, binding: &mut Option<FieldBinding<'_>>) -> Result<Option<Predicate>, ShapeError> {
    let path = match arg.name.strip_prefix("input.") {
        Some(rest) if !rest.is_empty() => rest.split('.').map(str::to_string).collect::<Vec<_>>(),
        _ => return unsupported("NamedArg", format!("the argument `{}` is not an `input.<field>` path", arg.name)),
    };
    let value = match &arg.value {
        Term::Integer(n) => Literal::Long(*n),
        Term::String(s) => Literal::String(s.clone()),
        Term::Bool(b) => Literal::Bool(*b),
        Term::Decimal(d) => Literal::Decimal(d.clone()),
        Term::Var(var) => {
            if let Some(b) = binding.as_mut().filter(|b| b.var == var && !b.used) {
                b.used = true;
                return Ok(Some(Predicate::Field {
                    path,
                    op: b.op,
                    value: Literal::Long(b.value),
                }));
            }
            if let Some(c) = capture.as_mut().filter(|c| c.var == var && c.path.is_none()) {
                c.path = Some(path);
                return Ok(None);
            }
            return unsupported("Term::Var", format!("the variable `{var}` in `{}` is not bound by an accepted form", arg.name));
        }
        other => return unsupported(term_name(other), format!("the argument `{}` must be a constant", arg.name)),
    };
    Ok(Some(Predicate::Field { path, op: CmpOp::Eq, value }))
}

/// The window in milliseconds. An overflow saturates, so the window check rejects it.
fn window_ms(within: &WithinSpec) -> Result<u64, ShapeError> {
    match within {
        WithinSpec::Concrete(interval) => Ok(interval
            .checked_seconds()
            .and_then(|seconds| u64::try_from(seconds).ok())
            .and_then(|seconds| seconds.checked_mul(1000))
            .unwrap_or(u64::MAX)),
        WithinSpec::ParamRef(name) => unsupported("WithinSpec::ParamRef", format!("the window `?{name}` was not expanded")),
    }
}

fn binder(var: &ast::TypedBinder) -> Result<&str, ShapeError> {
    match binder_name(&var.slot) {
        Some(name) => Ok(name),
        None => unsupported("BinderSlot", "a binder is a macro sigil that was not expanded"),
    }
}

fn binder_name(slot: &ast::BinderSlot) -> Option<&str> {
    match slot {
        ast::BinderSlot::Name(name) => Some(name),
        ast::BinderSlot::ParamRef(_) | ast::BinderSlot::BinderRef(_) => None,
    }
}

fn is_named(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Named(path) if path.len() == 1 && path[0] == name)
}

fn condition_name(kind: &ConditionKind) -> &'static str {
    match kind {
        ConditionKind::And { .. } => "And",
        ConditionKind::Or { .. } => "Or",
        ConditionKind::Not { .. } => "Not",
        ConditionKind::Formerly { .. } => "Formerly",
        ConditionKind::Previous { .. } => "Previous",
        ConditionKind::Since { .. } => "Since",
        ConditionKind::Predicate(_) => "Predicate",
        ConditionKind::Comparison { .. } => "Comparison",
        ConditionKind::Exists { .. } => "Exists",
        ConditionKind::Tp { .. } => "Tp",
        ConditionKind::Call(_) => "Call",
        ConditionKind::SigilRef { .. } => "SigilRef",
        ConditionKind::Refine { .. } => "Refine",
    }
}

fn term_name(term: &Term) -> &'static str {
    match term {
        Term::Entity { .. } => "Term::Entity",
        Term::Integer(_) => "Term::Integer",
        Term::Decimal(_) => "Term::Decimal",
        Term::String(_) => "Term::String",
        Term::Bool(_) => "Term::Bool",
        Term::ContextField(_) => "Term::ContextField",
        Term::ScopeField(_) => "Term::ScopeField",
        Term::Var(_) => "Term::Var",
        Term::Wildcard => "Term::Wildcard",
        Term::Array(_) => "Term::Array",
        Term::Agg(_) => "Term::Agg",
        Term::Arith { .. } => "Term::Arith",
        Term::ParamRef(_) => "Term::ParamRef",
        Term::BinderRef(_) => "Term::BinderRef",
    }
}
