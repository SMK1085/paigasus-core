// SPDX-License-Identifier: Apache-2.0
//! Compile the floor and tenant policy sets (§ 6.1). The floor holds `forbid` policies only. Every
//! policy needs an `@id`. A policy may not be a template. An action literal must be in the
//! normal form of a tool name (the group ids `InvokeTool` and `Unregistered` are exempt).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use cedar_policy::{Effect, Entities, Entity, EntityId, EntityTypeName, EntityUid, Policy, PolicyId, PolicySet};

use super::normalise::normalise;
use super::optimistic::{DROP_READS, optimistic, screen_dependent};

pub(crate) const ACTION_TYPE: &str = "Pgs::Gateway::Action";
pub(crate) const GROUP_INVOKE: &str = "InvokeTool";
pub(crate) const GROUP_UNREGISTERED: &str = "Unregistered";
pub(crate) const RESOURCE_TYPE: &str = "Pgs::Iam::Organization";

/// The set that a policy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyOrigin {
    /// The floor set, which the operator ships.
    Floor,
    /// The tenant set.
    Tenant,
}

/// The set and the `@id` of one policy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PolicyRef {
    /// The set of the policy.
    pub set: PolicyOrigin,
    /// The `@id` annotation of the policy.
    pub id: String,
}

impl PolicyRef {
    /// Make a policy reference.
    #[must_use]
    pub fn new(set: PolicyOrigin, id: impl Into<String>) -> Self {
        Self { set, id: id.into() }
    }
}

/// Why a policy set does not compile.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompileError {
    /// The text is not valid Cedar.
    #[error("policy text does not parse: {0}")]
    Parse(String),
    /// A policy has no `@id` annotation.
    #[error("a policy in the {origin:?} set has no @id annotation")]
    MissingId {
        /// The set of the policy.
        origin: PolicyOrigin,
    },
    /// Two policies in one set have the same `@id`.
    #[error("duplicate policy id {0}")]
    DuplicateId(String),
    /// A policy is a template.
    #[error("policy {0} is a template")]
    Template(String),
    /// A floor policy is a `permit`.
    #[error("floor policy {0} is a permit")]
    FloorPermit(String),
    /// An action literal is not in the normal form of a tool name.
    #[error("action id {0} is not in the normal form")]
    ActionNotNormal(String),
    /// Two catalog names have the same normal form.
    #[error("catalog names collide after normalisation: {0}")]
    CatalogCollision(String),
    /// A catalog name has the normal form of a group id.
    #[error("catalog name {0} is reserved")]
    ReservedCatalogName(String),
}

fn action_uid(id: &str) -> EntityUid {
    let ty = EntityTypeName::from_str(ACTION_TYPE).expect("ACTION_TYPE is a valid entity type name");
    EntityUid::from_type_name_and_id(ty, EntityId::new(id))
}

/// The compiled floor: `forbid` policies only.
#[derive(Debug, Clone)]
pub struct FloorPolicies {
    /// Policies keyed by `floor/<id>`.
    set: PolicySet,
    ids: Vec<String>,
}

impl FloorPolicies {
    /// The number of floor policies.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the floor has no policy.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// The floor and the tenant set, merged, with the action entities.
#[derive(Debug, Clone)]
pub struct ToolPolicies {
    floor: Arc<FloorPolicies>,
    set: PolicySet,
    origins: HashMap<PolicyId, PolicyRef>,
    catalog: HashSet<String>,
    entities: Entities,
    /// The optimistic set of § 6.1.6, built from `set`.
    optimistic: PolicySet,
    /// Whether a policy of `set` reads a screen-dependent part of the context (§ 6.8.0).
    reads_screen_dependent: bool,
}

/// Parse one set. Return each policy with its `@id`, after the checks that both sets share.
fn parse_set(text: &str, origin: PolicyOrigin) -> Result<Vec<(String, Policy)>, CompileError> {
    let set = PolicySet::from_str(text).map_err(|e| CompileError::Parse(e.to_string()))?;
    if let Some(t) = set.templates().next() {
        return Err(CompileError::Template(t.annotation("id").map_or_else(|| t.id().to_string(), str::to_owned)));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in set.policies() {
        let id = p.annotation("id").ok_or(CompileError::MissingId { origin })?.to_owned();
        if !seen.insert(id.clone()) {
            return Err(CompileError::DuplicateId(id));
        }
        check_actions(p)?;
        out.push((id, p.clone()));
    }
    Ok(out)
}

fn check_actions(p: &Policy) -> Result<(), CompileError> {
    let json = p.to_json().map_err(|e| CompileError::Parse(e.to_string()))?;
    let mut bad = None;
    walk(&json, &mut |id| {
        if bad.is_none() && id != GROUP_INVOKE && id != GROUP_UNREGISTERED && normalise(id) != id {
            bad = Some(id.to_owned());
        }
    });
    bad.map_or(Ok(()), |id| Err(CompileError::ActionNotNormal(id)))
}

/// Call `f` with the id of every entity literal of type `Pgs::Gateway::Action`.
fn walk(v: &serde_json::Value, f: &mut impl FnMut(&str)) {
    match v {
        serde_json::Value::Object(m) => {
            if let (Some(serde_json::Value::String(t)), Some(serde_json::Value::String(id))) = (m.get("type"), m.get("id"))
                && t == ACTION_TYPE
            {
                f(id);
            }
            m.values().for_each(|x| walk(x, f));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, f)),
        _ => {}
    }
}

fn rekey(prefix: &str, id: &str, p: &Policy) -> Policy {
    p.new_id(PolicyId::new(format!("{prefix}/{id}")))
}

impl ToolPolicies {
    /// Compile the floor. A `permit` in the floor is an error.
    ///
    /// # Errors
    /// Returns a [`CompileError`] for text that is not Cedar, a missing or duplicate `@id`, a
    /// template, a `permit` or an action literal that is not in the normal form.
    pub fn compile_floor(floor: &str) -> Result<FloorPolicies, CompileError> {
        let mut set = PolicySet::new();
        let mut ids = Vec::new();
        for (id, p) in parse_set(floor, PolicyOrigin::Floor)? {
            if p.effect() == Effect::Permit {
                return Err(CompileError::FloorPermit(id));
            }
            set.add(rekey("floor", &id, &p)).map_err(|e| CompileError::Parse(e.to_string()))?;
            ids.push(id);
        }
        Ok(FloorPolicies { set, ids })
    }

    /// Merge the floor and the tenant set, and build the action entities of the catalog.
    ///
    /// # Errors
    /// Returns a [`CompileError`] for a bad tenant set or a bad catalog.
    pub fn compile(floor: Arc<FloorPolicies>, tenant: &str, catalog: &[String]) -> Result<ToolPolicies, CompileError> {
        let mut set = floor.set.clone();
        let mut origins: HashMap<PolicyId, PolicyRef> = floor
            .ids
            .iter()
            .map(|id| {
                (
                    PolicyId::new(format!("floor/{id}")),
                    PolicyRef {
                        set: PolicyOrigin::Floor,
                        id: id.clone(),
                    },
                )
            })
            .collect();
        for (id, p) in parse_set(tenant, PolicyOrigin::Tenant)? {
            let keyed = rekey("tenant", &id, &p);
            origins.insert(keyed.id().clone(), PolicyRef { set: PolicyOrigin::Tenant, id });
            set.add(keyed).map_err(|e| CompileError::Parse(e.to_string()))?;
        }

        let mut by_norm: HashMap<String, &str> = HashMap::new();
        for raw in catalog {
            let n = normalise(raw);
            if n == normalise(GROUP_INVOKE) || n == normalise(GROUP_UNREGISTERED) {
                return Err(CompileError::ReservedCatalogName(raw.clone()));
            }
            if let Some(prev) = by_norm.insert(n, raw.as_str())
                && prev != raw
            {
                return Err(CompileError::CatalogCollision(raw.clone()));
            }
        }

        let invoke = action_uid(GROUP_INVOKE);
        let mut ents = vec![
            Entity::new_no_attrs(invoke.clone(), HashSet::new()),
            Entity::new_no_attrs(action_uid(GROUP_UNREGISTERED), HashSet::new()),
        ];
        let names: HashSet<String> = by_norm.into_keys().collect();
        ents.extend(names.iter().map(|n| Entity::new_no_attrs(action_uid(n), HashSet::from([invoke.clone()]))));
        let entities = Entities::from_entities(ents, None).map_err(|e| CompileError::Parse(e.to_string()))?;
        let optimistic = optimistic(&set, DROP_READS, true);
        // A policy without a JSON form counts as dependent: the safe side.
        let reads_screen_dependent = set.policies().any(|p| p.to_json().map_or(true, |json| screen_dependent(&json)));
        Ok(ToolPolicies {
            floor,
            set,
            optimistic,
            reads_screen_dependent,
            origins,
            catalog: names,
            entities,
        })
    }

    /// The compiled floor that this set uses.
    #[must_use]
    pub fn floor(&self) -> &Arc<FloorPolicies> {
        &self.floor
    }

    /// The merged policy set.
    pub(crate) fn set(&self) -> &PolicySet {
        &self.set
    }

    /// Whether a policy of the merged set reads a screen-dependent part of the context (§ 6.8.0):
    /// `max_severity`, `call`, or `findings` in a form other than `contains` of a literal in
    /// `SCREEN_INDEPENDENT_KINDS`. The request side screens a floor-only set only when this is true.
    #[must_use]
    pub fn reads_screen_dependent(&self) -> bool {
        self.reads_screen_dependent
    }

    /// The action group of a tool name (the raw name: it is normalised here), with no Cedar
    /// decision. `decide` gives the same group in `Verdict.action_group`.
    #[must_use]
    pub fn action_group(&self, name: &str) -> ActionGroup {
        self.group_of(&normalise(name))
    }

    fn group_of(&self, normalised: &str) -> ActionGroup {
        if self.catalog.contains(normalised) { ActionGroup::Catalog } else { ActionGroup::Unregistered }
    }

    /// The optimistic set of § 6.1.6.
    pub(crate) fn optimistic_set(&self) -> &PolicySet {
        &self.optimistic
    }

    /// The set and `@id` of a merged policy id (`floor/<id>` or `tenant/<id>`).
    #[must_use]
    pub(crate) fn origin(&self, merged_id: &PolicyId) -> Option<&PolicyRef> {
        self.origins.get(merged_id)
    }
}

/// The action group of a tool name: the parent of its Cedar action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActionGroup {
    /// The name is in the catalog (parent `InvokeTool`).
    Catalog,
    /// The name is not in the catalog (parent `Unregistered`).
    Unregistered,
}

impl ActionGroup {
    /// The wire name of the group.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::Unregistered => "unregistered",
        }
    }
}

/// The action of a call, with parent `InvokeTool` (a catalog name) or `Unregistered` (D22).
pub(crate) fn action_entity(policies: &ToolPolicies, normalised: &str) -> (EntityUid, Entities, ActionGroup) {
    let uid = action_uid(normalised);
    if policies.group_of(normalised) == ActionGroup::Catalog {
        return (uid, policies.entities.clone(), ActionGroup::Catalog);
    }
    let entity = Entity::new_no_attrs(uid.clone(), HashSet::from([action_uid(GROUP_UNREGISTERED)]));
    let entities = policies.entities.clone().add_entities([entity], None).expect("a new action entity adds to the action entities");
    (uid, entities, ActionGroup::Unregistered)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORBID: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"read_file", resource);"#;

    fn floor() -> Arc<FloorPolicies> {
        Arc::new(ToolPolicies::compile_floor(FORBID).unwrap())
    }

    fn compile_tenant(t: &str) -> Result<ToolPolicies, CompileError> {
        ToolPolicies::compile(floor(), t, &[])
    }

    fn compile_with_catalog(c: &[&str]) -> Result<ToolPolicies, CompileError> {
        let c: Vec<String> = c.iter().map(|s| (*s).to_owned()).collect();
        ToolPolicies::compile(floor(), "", &c)
    }

    /// The floor that the gateway ships.
    const SHIPPED_FLOOR: &str = include_str!("../../../../services/paigasus-gateway/tool-policies/floor.cedar");

    fn policies(floor: &str, tenant: &str, catalog: &[&str]) -> ToolPolicies {
        let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
        let catalog: Vec<String> = catalog.iter().map(|s| (*s).to_owned()).collect();
        ToolPolicies::compile(floor, tenant, &catalog).expect("tenant")
    }

    fn parents(e: &Entities, uid: &EntityUid) -> Vec<String> {
        e.ancestors(uid).expect("entity present").map(|a| a.id().unescaped().to_owned()).collect()
    }

    #[test]
    fn a_floor_permit_is_a_compile_error() {
        assert!(matches!(ToolPolicies::compile_floor(r#"@id("p") permit(principal, action, resource);"#), Err(CompileError::FloorPermit(id)) if id == "p"));
    }

    #[test]
    fn a_policy_without_id_is_a_compile_error() {
        assert_eq!(
            ToolPolicies::compile_floor("forbid(principal, action, resource);").unwrap_err(),
            CompileError::MissingId { origin: PolicyOrigin::Floor }
        );
        assert_eq!(
            compile_tenant("permit(principal, action, resource);").unwrap_err(),
            CompileError::MissingId { origin: PolicyOrigin::Tenant }
        );
    }

    #[test]
    fn a_duplicate_id_in_one_set_is_a_compile_error() {
        let two = r#"@id("x") permit(principal, action, resource); @id("x") forbid(principal, action, resource);"#;
        assert_eq!(compile_tenant(two).unwrap_err(), CompileError::DuplicateId("x".into()));
        assert_eq!(ToolPolicies::compile_floor(&two.replace("permit", "forbid")).unwrap_err(), CompileError::DuplicateId("x".into()));
        // The same id in the floor and the tenant set is allowed: floor/x and tenant/x.
        let fl = Arc::new(ToolPolicies::compile_floor(r#"@id("x") forbid(principal, action, resource);"#).unwrap());
        assert!(ToolPolicies::compile(fl, r#"@id("x") permit(principal, action, resource);"#, &[]).is_ok());
    }

    #[test]
    fn a_template_is_a_compile_error() {
        let t = r#"@id("t") permit(principal == ?principal, action, resource);"#;
        assert!(matches!(compile_tenant(t), Err(CompileError::Template(_))));
        assert!(matches!(ToolPolicies::compile_floor(&t.replace("permit", "forbid")), Err(CompileError::Template(_))));
    }

    #[test]
    fn a_parse_error_is_a_compile_error() {
        assert!(matches!(compile_tenant("permit(("), Err(CompileError::Parse(_))));
    }

    #[test]
    fn a_non_normal_action_literal_is_a_compile_error() {
        let scope = r#"@id("a") permit(principal, action == Pgs::Gateway::Action::"Send_Email", resource);"#;
        assert!(matches!(compile_tenant(scope), Err(CompileError::ActionNotNormal(s)) if s == "Send_Email"));
        let cond = r#"@id("a") permit(principal, action, resource) when { action == Pgs::Gateway::Action::"Send_Email" };"#;
        assert!(matches!(compile_tenant(cond), Err(CompileError::ActionNotNormal(s)) if s == "Send_Email"));
        let set = r#"@id("a") permit(principal, action in [Pgs::Gateway::Action::"ok", Pgs::Gateway::Action::"Bad"], resource);"#;
        assert!(matches!(compile_tenant(set), Err(CompileError::ActionNotNormal(s)) if s == "Bad"));
        let floor = r#"@id("a") forbid(principal, action == Pgs::Gateway::Action::"Send_Email", resource);"#;
        assert!(matches!(ToolPolicies::compile_floor(floor), Err(CompileError::ActionNotNormal(_))));
    }

    #[test]
    fn the_group_ids_are_exempt_from_the_normal_form_rule() {
        let t = r#"@id("a") permit(principal, action in Pgs::Gateway::Action::"InvokeTool", resource);
                   @id("b") permit(principal, action in Pgs::Gateway::Action::"Unregistered", resource);"#;
        assert!(compile_tenant(t).is_ok());
    }

    #[test]
    fn catalog_rules() {
        assert!(matches!(compile_with_catalog(&["Read_File", "read_file"]), Err(CompileError::CatalogCollision(_))));
        assert!(matches!(compile_with_catalog(&["InvokeTool"]), Err(CompileError::ReservedCatalogName(_))));
        assert!(matches!(compile_with_catalog(&[" unregistered"]), Err(CompileError::ReservedCatalogName(_))));
        assert!(compile_with_catalog(&["read_file", "read_file", "Write_File"]).is_ok());
    }

    #[test]
    fn catalog_and_unregistered_parents() {
        let p = compile_with_catalog(&["Read_File"]).unwrap();
        let (uid, ents, _) = action_entity(&p, "read_file");
        assert_eq!(parents(&ents, &uid), [GROUP_INVOKE]);
        let (uid, ents, _) = action_entity(&p, "x");
        assert_eq!(uid.id().unescaped(), "x");
        assert_eq!(parents(&ents, &uid), [GROUP_UNREGISTERED]);
        // The catalog entities stay unchanged for the next call.
        let (uid, ents, _) = action_entity(&p, "read_file");
        assert_eq!(parents(&ents, &uid), [GROUP_INVOKE]);
    }

    #[test]
    fn merged_ids_carry_their_origin() {
        let fl = Arc::new(ToolPolicies::compile_floor(r#"@id("x") forbid(principal, action, resource);"#).unwrap());
        assert_eq!((fl.len(), fl.is_empty()), (1, false));
        let p = ToolPolicies::compile(fl.clone(), r#"@id("x") permit(principal, action, resource);"#, &[]).unwrap();
        assert!(Arc::ptr_eq(p.floor(), &fl));
        let f = p.origin(&PolicyId::new("floor/x")).unwrap();
        assert_eq!((f.set, f.id.as_str()), (PolicyOrigin::Floor, "x"));
        let t = p.origin(&PolicyId::new("tenant/x")).unwrap();
        assert_eq!((t.set, t.id.as_str()), (PolicyOrigin::Tenant, "x"));
        assert_eq!(p.set.policies().count(), 2);
        assert!(p.set.policy(&PolicyId::new("floor/x")).is_some() && p.set.policy(&PolicyId::new("tenant/x")).is_some());
    }

    #[test]
    fn screen_dependent_forms() {
        let dep = |c: &str| policies("", &format!(r#"@id("f") forbid(principal, action, resource) when {{ {c} }};"#), &[]).reads_screen_dependent();
        assert!(dep(r#"context.findings.contains("unseen_host")"#));
        assert!(!dep(r#"context.findings.contains("secret_in_args")"#));
        assert!(dep("context == {}"));
        assert!(dep("context.findings.contains(context.request.model)"));
        // The walk compares the decoded literal, so an escape cannot hide a dependent kind.
        assert!(dep(r#"context.findings.contains("unseen\u{5f}host")"#));
        assert!(!dep(r#"context.findings.contains("secret\u{5f}in_args")"#));
        assert!(dep("context.max_severity >= 3") && dep(r#"context.call.declared == "no""#) && dep("context.findings.isEmpty()"));
        assert!(dep("context has findings") && dep(r#"context.findings.containsAny(["secret_in_args"])"#));
        // A read of other parts only is independent.
        assert!(!dep(r#"context.request.model == "m" && context.input.x == 1"#));
        // A permit counts too, and one dependent policy makes the set dependent.
        assert!(policies("", r#"@id("p") permit(principal, action, resource) when { context.max_severity < 3 };"#, &[]).reads_screen_dependent());
        let tenant = r#"@id("p") permit(principal, action, resource) when { context.findings.contains("injection_markers") };"#;
        assert!(policies(SHIPPED_FLOOR, tenant, &[]).reads_screen_dependent());
        assert!(!policies(SHIPPED_FLOOR, "", &[]).reads_screen_dependent());
        assert!(!policies("", "", &[]).reads_screen_dependent());
    }
}
