// SPDX-License-Identifier: Apache-2.0

//! `ToolPolicyService`: the tool catalog and the tool policy sets, with compile on save and the
//! catalog coupling (SMA-774, spec sections 4.2 to 4.6, 7, 9.5 and 9.6).
//!
//! **Authorization.** Every method authorizes first. The tenant methods look up the organization
//! (`NotFound` when it does not exist) and authorize at its stored PRN: `ManageToolCatalog`,
//! `ReadToolCatalog`, `ManageToolPolicies` or `ReadToolPolicies`. The floor methods authorize
//! `ManageToolPolicyFloor` or `ReadToolPolicyFloor` at `root_prn()`, and never at a PRN from the
//! request. The Cedar `appliesTo` of the floor actions also lists Organization, Team and Project,
//! so the schema alone does not keep the floor at Root: this service does.
//!
//! **A write in four steps (spec section 4.5).** (1) Read the state row and the inputs. An
//! expected value that is not current is `tool-policy-version-conflict` at once. (2) Compile with
//! no transaction open. (3) Open a transaction, lock the state row and check that `live_version`
//! and `catalog_revision` did not change; a change drops the transaction and is a version
//! conflict. (4) Write, record one audit entry, commit. PR 1 writes no outbox event (D9).
//!
//! **A rejection is a successful result** ([`WriteResult::Rejected`]). It stores nothing and
//! writes no audit entry. A compiler that answers with a reason that is not one of its six codes
//! broke the protocol: `tool-policy-compiler-unavailable`.
//!
//! **Capability.** The service needs a working [`PolicyCompiler`]. When the boot probe
//! (`adapters::policy_compiler::probe`) finds no compiler, or the feature is off, the wiring does
//! not build this service, and each tool policy RPC answers `capability-disabled` (spec section
//! 6.2, Task 13).

pub mod catalog;
pub mod rejection;
pub mod sets;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::tool_policy::{
    CatalogLimits, CompileOutcome, CompileRequest, CompiledOutput, CompilerFailure, ImportFormat, PROTOCOL_VERSION, PolicyCompiler, SetKind, StoredTool, TOOL_SCHEMA_REVISION, ToolEntry, ToolKind,
    ToolPolicyRepository, ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus, base_schema_json, build_manual, catalog_names, check_compiled, outputs_identical, parse_import,
    tenant_schema_json,
};
use paigasus_iam_core::{Action, AuditEntry, AuditLog, AuditOutcome, Clock, IdGenerator, OrganizationRepository, UnitOfWork};
use paigasus_kernel::Prn;
use serde_json::json;
use uuid::Uuid;

use self::catalog::{CatalogPlan, PlanStop, apply_plan, check_tool_limit, plan_delete, plan_import, plan_put, problems_to_stop, stored_upserts};
pub use self::rejection::{RejectionReason, ToolPolicyRejection};
use self::sets::{CompiledMeta, add_dropped_field_reasons, decode_compiled, encode_compiled};
use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;

/// The limits of the tool policy feature, from `[tool_policies]` in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolPolicyLimits {
    pub catalog: CatalogLimits,
    /// The largest source that a save accepts. A larger one is a `tool-policy-invalid`
    /// rejection, and no compile runs.
    pub max_source_bytes: usize,
    /// The longest window that a temporal block may use.
    pub max_window_ms: u64,
}

/// The ports and values that [`ToolPolicyService::new`] takes.
pub struct ToolPolicyServiceDeps<I, C> {
    pub repo: Arc<dyn ToolPolicyRepository>,
    pub orgs: Arc<dyn OrganizationRepository>,
    pub compiler: Arc<dyn PolicyCompiler>,
    pub authorize: Authorize,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub limits: ToolPolicyLimits,
    pub ids: I,
    pub clock: C,
}

/// The result of a write: the new value, or every rejection (spec section 7).
#[derive(Debug, Clone, PartialEq)]
pub enum WriteResult<T> {
    Accepted(T),
    Rejected(Vec<ToolPolicyRejection>),
}

/// An accepted catalog write.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogAccepted {
    pub catalog_revision: u64,
    /// The new live version of the tenant set. 0 when the set did not change.
    pub set_version: u64,
    /// The tools that this write added or replaced, as stored.
    pub tools: Vec<StoredTool>,
    /// Import entries of another type: the index in the document and the `type` value.
    pub skipped: Vec<(usize, String)>,
}

/// The tool catalog and tool policy set use cases.
#[derive(Clone)]
pub struct ToolPolicyService<I, C> {
    repo: Arc<dyn ToolPolicyRepository>,
    orgs: Arc<dyn OrganizationRepository>,
    compiler: Arc<dyn PolicyCompiler>,
    authorize: Authorize,
    uow: Arc<dyn UnitOfWork>,
    audit: Arc<dyn AuditLog>,
    limits: ToolPolicyLimits,
    ids: Arc<I>,
    clock: Arc<C>,
}

/// One set of the two kinds, with the PRN that its methods authorize at.
struct SetTarget {
    key: ToolPolicySetKey,
    resource: Prn,
}

impl SetTarget {
    fn kind(&self) -> SetKind {
        match self.key {
            ToolPolicySetKey::Tenant(_) => SetKind::Tenant,
            ToolPolicySetKey::Floor => SetKind::Floor,
        }
    }

    fn audit_set(&self) -> &'static str {
        match self.key {
            ToolPolicySetKey::Tenant(_) => "tenant",
            ToolPolicySetKey::Floor => "floor",
        }
    }

    fn audit_action(&self) -> &'static str {
        match self.key {
            ToolPolicySetKey::Tenant(_) => "PutToolPolicySet",
            ToolPolicySetKey::Floor => "PutFloorPolicySet",
        }
    }
}

fn origin_str(origin: VersionOrigin) -> &'static str {
    match origin {
        VersionOrigin::Save => "save",
        VersionOrigin::CatalogChange => "catalog_change",
    }
}

/// The result of one compile: an accepted output that passed the cedar 4.14 checks, or the
/// rejections.
enum Compiled {
    Accepted(CompiledOutput),
    Rejected(Vec<ToolPolicyRejection>),
}

impl<I: IdGenerator, C: Clock> ToolPolicyService<I, C> {
    pub fn new(deps: ToolPolicyServiceDeps<I, C>) -> Self {
        Self {
            repo: deps.repo,
            orgs: deps.orgs,
            compiler: deps.compiler,
            authorize: deps.authorize,
            uow: deps.uow,
            audit: deps.audit,
            limits: deps.limits,
            ids: Arc::new(deps.ids),
            clock: Arc::new(deps.clock),
        }
    }

    /// Adds or replaces one tool (spec section 4.2).
    pub async fn put_tool(
        &self,
        actor: &Prn,
        org: Uuid,
        name: &str,
        kind: ToolKind,
        input_schema: Option<serde_json::Value>,
        expected_catalog_revision: u64,
    ) -> Result<WriteResult<CatalogAccepted>, TenancyError> {
        let org_prn = self.authorize_org(actor, org, Action::ManageToolCatalog).await?;
        let built = build_manual(name, kind, input_schema, &self.limits.catalog);
        self.catalog_write(actor, org, &org_prn, "PutTool", expected_catalog_revision, move |current| {
            let entry = built.map_err(problems_to_stop)?;
            plan_put(current, entry)
        })
        .await
    }

    /// Removes one tool. `NotFound` when no tool has the match key of `name`.
    pub async fn delete_tool(&self, actor: &Prn, org: Uuid, name: &str, expected_catalog_revision: u64) -> Result<WriteResult<CatalogAccepted>, TenancyError> {
        let org_prn = self.authorize_org(actor, org, Action::ManageToolCatalog).await?;
        self.catalog_write(actor, org, &org_prn, "DeleteTool", expected_catalog_revision, |current| plan_delete(current, name))
            .await
    }

    /// One page of the catalog, sorted by match key, and the current catalog revision.
    pub async fn list_tools(&self, actor: &Prn, org: Uuid, page: Page) -> Result<(Vec<StoredTool>, u64), TenancyError> {
        self.authorize_org(actor, org, Action::ReadToolCatalog).await?;
        let state = self.repo.state(&ToolPolicySetKey::Tenant(org)).await?;
        let tools = self.repo.catalog(org).await?;
        let offset = usize::try_from(page.offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(page.limit).unwrap_or(usize::MAX);
        Ok((tools.into_iter().skip(offset).take(limit).collect(), state.catalog_revision))
    }

    /// Imports a batch of tools, all or nothing (spec section 4.2). With `replace`, the tools that
    /// are not in the batch are removed.
    #[allow(clippy::too_many_arguments)]
    pub async fn import_tools(
        &self,
        actor: &Prn,
        org: Uuid,
        format: ImportFormat,
        document: &serde_json::Value,
        namespace: Option<&str>,
        replace: bool,
        expected_catalog_revision: u64,
    ) -> Result<WriteResult<CatalogAccepted>, TenancyError> {
        let org_prn = self.authorize_org(actor, org, Action::ManageToolCatalog).await?;
        let parsed = parse_import(format, document, namespace, &self.limits.catalog);
        self.catalog_write(actor, org, &org_prn, "ImportTools", expected_catalog_revision, move |current| {
            let imported = parsed.map_err(problems_to_stop)?;
            plan_import(current, imported, replace)
        })
        .await
    }

    /// Saves the tenant set of `org` (spec sections 4.4 to 4.6).
    pub async fn put_set(&self, actor: &Prn, org: Uuid, source: &str, expected_version: u64) -> Result<WriteResult<ToolPolicyVersion>, TenancyError> {
        let resource = self.authorize_org(actor, org, Action::ManageToolPolicies).await?;
        let target = SetTarget {
            key: ToolPolicySetKey::Tenant(org),
            resource,
        };
        self.save(actor, &target, source, expected_version).await
    }

    /// The live tenant version (`version = None`) or one stored version. `Ok(None)` when the set
    /// has no live version; `tool-policy-version-not-found` for an absent version number.
    pub async fn get_set(&self, actor: &Prn, org: Uuid, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError> {
        self.authorize_org(actor, org, Action::ReadToolPolicies).await?;
        self.read_version(&ToolPolicySetKey::Tenant(org), version).await
    }

    /// Saves the platform floor. Authorized at `root_prn()` only.
    pub async fn put_floor(&self, actor: &Prn, source: &str, expected_version: u64) -> Result<WriteResult<ToolPolicyVersion>, TenancyError> {
        let target = SetTarget {
            key: ToolPolicySetKey::Floor,
            resource: root_prn(),
        };
        self.authorize.check(actor, Action::ManageToolPolicyFloor, &target.resource).await?;
        self.save(actor, &target, source, expected_version).await
    }

    /// The live floor version or one stored version. Authorized at `root_prn()` only.
    pub async fn get_floor(&self, actor: &Prn, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError> {
        self.authorize.check(actor, Action::ReadToolPolicyFloor, &root_prn()).await?;
        self.read_version(&ToolPolicySetKey::Floor, version).await
    }

    /// Looks up the organization (`NotFound` when it does not exist) and authorizes `action` at
    /// its stored PRN. Returns that PRN.
    async fn authorize_org(&self, actor: &Prn, org: Uuid, action: Action) -> Result<Prn, TenancyError> {
        let view = self.orgs.find(org).await?.ok_or(TenancyError::NotFound)?;
        let resource = view.node.id.prn().clone();
        self.authorize.check(actor, action, &resource).await?;
        Ok(resource)
    }

    async fn read_version(&self, key: &ToolPolicySetKey, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError> {
        match version {
            None => Ok(self.repo.live(key).await?),
            Some(n) => self.repo.version(key, n).await?.map(Some).ok_or(TenancyError::ToolPolicyVersionNotFound),
        }
    }

    /// A live version from another compiler generation or schema revision must be saved again
    /// before IAM compiles anything else against it (spec section 4.6).
    fn is_current(&self, v: &ToolPolicyVersion) -> bool {
        v.compiler_generation == self.compiler.generation() && v.schema_revision == TOOL_SCHEMA_REVISION
    }

    /// Compiles `source` with no transaction open, then runs the cedar 4.14 checks on an accepted
    /// output. Each rejection gets the `dropped_fields` reason of a path that its message names.
    async fn compile(&self, kind: SetKind, source: &str, catalog: &[ToolEntry]) -> Result<Compiled, TenancyError> {
        let schema_json = match kind {
            SetKind::Tenant => tenant_schema_json(catalog),
            SetKind::Floor => base_schema_json(),
        };
        let request = CompileRequest {
            protocol_version: PROTOCOL_VERSION,
            kind,
            source: source.to_owned(),
            schema_json,
            max_window_ms: self.limits.max_window_ms,
        };
        let raw = match self.compiler.compile(&request).await {
            Ok(CompileOutcome::Accepted(out)) => match check_compiled(kind, &out) {
                Ok(()) => return Ok(Compiled::Accepted(out)),
                // IAM's own checks: their reasons are `tool-policy-invalid` and
                // `tool-policy-floor-permit`, and they pass through unchanged.
                Err(rejections) => rejections,
            },
            Ok(CompileOutcome::Rejected(rejections)) => rejections,
            Err(CompilerFailure::Busy) => return Err(TenancyError::ToolPolicyCompilerBusy),
            Err(CompilerFailure::Unavailable(reason)) => {
                tracing::warn!(reason = %reason, "a tool policy compile failed: the compiler is unavailable");
                return Err(TenancyError::ToolPolicyCompilerUnavailable);
            }
        };
        let mut rejections = Vec::with_capacity(raw.len());
        for r in &raw {
            let Some(mut rejection) = ToolPolicyRejection::from_compiler(r) else {
                tracing::warn!(reason = %r.reason, "the policy compiler sent an unknown rejection reason");
                return Err(TenancyError::ToolPolicyCompilerUnavailable);
            };
            add_dropped_field_reasons(&mut rejection, catalog);
            rejections.push(rejection);
        }
        Ok(Compiled::Rejected(rejections))
    }

    async fn save(&self, actor: &Prn, target: &SetTarget, source: &str, expected_version: u64) -> Result<WriteResult<ToolPolicyVersion>, TenancyError> {
        if source.len() > self.limits.max_source_bytes {
            return Ok(WriteResult::Rejected(vec![ToolPolicyRejection::new(
                RejectionReason::PolicyInvalid,
                format!("the source is {} bytes; the limit is {} bytes", source.len(), self.limits.max_source_bytes),
            )]));
        }

        // Step 1: the state and the inputs.
        let state = self.repo.state(&target.key).await?;
        if expected_version != state.live_version {
            return Err(TenancyError::ToolPolicyVersionConflict);
        }
        if let Some(live) = self.repo.live(&target.key).await?
            && live.source == source
            && self.is_current(&live)
        {
            // A byte-identical save at the running compiler and schema makes no new version. A
            // save from another generation compiles again: that clears recompile-pending (D10).
            return Ok(WriteResult::Accepted(live));
        }
        let catalog: Vec<ToolEntry> = match target.key {
            ToolPolicySetKey::Tenant(org) => self.repo.catalog(org).await?.into_iter().map(|s| s.entry).collect(),
            ToolPolicySetKey::Floor => Vec::new(),
        };

        // Step 2: compile, with no transaction open.
        let out = match self.compile(target.kind(), source, &catalog).await? {
            Compiled::Accepted(out) => out,
            Compiled::Rejected(rejections) => return Ok(WriteResult::Rejected(rejections)),
        };

        // Step 3: lock and recheck.
        let tx = self.uow.begin().await?;
        let locked = self.repo.lock_state_in(&*tx, &target.key).await?;
        if locked != state {
            drop(tx);
            return Err(TenancyError::ToolPolicyVersionConflict);
        }

        // Step 4: write, audit, commit.
        let catalog_revision = match target.key {
            ToolPolicySetKey::Tenant(_) => Some(state.catalog_revision),
            ToolPolicySetKey::Floor => None,
        };
        let now = self.clock.now();
        let version = self.new_version(actor, &target.key, state.live_version + 1, source, &out, catalog_revision, &catalog, VersionOrigin::Save, now);
        self.repo.insert_live_version_in(&*tx, &version).await?;
        let entry = self.audit_entry(actor, target.audit_action(), &target.resource, target.audit_set(), Some(&version), catalog_revision, now);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(WriteResult::Accepted(version))
    }

    /// A catalog write (spec sections 4.2, 4.5 and 4.6). `plan` turns the current catalog into
    /// the changes, or stops the write.
    async fn catalog_write<F>(&self, actor: &Prn, org: Uuid, org_prn: &Prn, action: &'static str, expected_catalog_revision: u64, plan: F) -> Result<WriteResult<CatalogAccepted>, TenancyError>
    where
        F: FnOnce(&[StoredTool]) -> Result<CatalogPlan, PlanStop>,
    {
        let key = ToolPolicySetKey::Tenant(org);

        // Step 1: the state and the inputs.
        let state = self.repo.state(&key).await?;
        if expected_catalog_revision != state.catalog_revision {
            return Err(TenancyError::ToolPolicyVersionConflict);
        }
        let current = self.repo.catalog(org).await?;
        let stop = |s: PlanStop| match s {
            PlanStop::Rejected(rejections) => Ok(WriteResult::Rejected(rejections)),
            PlanStop::Error(error) => Err(error),
        };
        let plan = match plan(&current) {
            Ok(plan) => plan,
            Err(s) => return stop(s),
        };
        let next = apply_plan(&current, &plan);
        if let Err(s) = check_tool_limit(&next, self.limits.catalog.max_tools) {
            return stop(s);
        }

        // Step 2: the coupling compile of the live tenant set against the new catalog.
        let mut coupled: Option<(ToolPolicyVersion, CompiledOutput)> = None;
        if let Some(live) = self.repo.live(&key).await? {
            if !self.is_current(&live) {
                return Err(TenancyError::ToolPolicyRecompilePending);
            }
            match self.compile(SetKind::Tenant, &live.source, &next).await? {
                Compiled::Rejected(rejections) => {
                    let rejections = rejections
                        .into_iter()
                        .map(|r| ToolPolicyRejection {
                            reason: RejectionReason::CatalogBreaksPolicy,
                            message: format!("{} ({})", r.message, r.reason.code()),
                            ..r
                        })
                        .collect();
                    return Ok(WriteResult::Rejected(rejections));
                }
                Compiled::Accepted(out) => {
                    if !outputs_identical(&decode_compiled(&live.compiled)?, &out) {
                        coupled = Some((live, out));
                    }
                }
            }
        }

        // Step 3: lock and recheck.
        let tx = self.uow.begin().await?;
        let locked = self.repo.lock_state_in(&*tx, &key).await?;
        if locked != state {
            drop(tx);
            return Err(TenancyError::ToolPolicyVersionConflict);
        }

        // Step 4: write, audit, commit.
        let now = self.clock.now();
        let catalog_revision = self.repo.apply_catalog_in(&*tx, org, &plan.upserts, &plan.deletes, now).await?;
        let version = match coupled {
            Some((live, out)) => {
                let v = self.new_version(
                    actor,
                    &key,
                    state.live_version + 1,
                    &live.source,
                    &out,
                    Some(catalog_revision),
                    &next,
                    VersionOrigin::CatalogChange,
                    now,
                );
                self.repo.insert_live_version_in(&*tx, &v).await?;
                Some(v)
            }
            None => None,
        };
        let entry = self.audit_entry(actor, action, org_prn, "tenant", version.as_ref(), Some(catalog_revision), now);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(WriteResult::Accepted(CatalogAccepted {
            catalog_revision,
            set_version: version.map_or(0, |v| v.version),
            tools: stored_upserts(&current, &plan, now),
            skipped: plan.skipped,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn new_version(
        &self,
        actor: &Prn,
        key: &ToolPolicySetKey,
        version: u64,
        source: &str,
        out: &CompiledOutput,
        catalog_revision: Option<u64>,
        catalog: &[ToolEntry],
        origin: VersionOrigin,
        now: DateTime<Utc>,
    ) -> ToolPolicyVersion {
        let compiler_generation = self.compiler.generation();
        let meta = CompiledMeta {
            key: *key,
            version,
            compiler_generation,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision,
            max_window_ms: self.limits.max_window_ms,
            catalog_names: catalog_names(catalog),
        };
        ToolPolicyVersion {
            key: *key,
            version,
            source: source.to_owned(),
            compiled: encode_compiled(&meta, out),
            compiler_generation,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision,
            status: VersionStatus::Live,
            origin,
            created_by: actor.canonical(),
            created_at: now,
        }
    }

    /// One audit entry (spec section 9.6): the RPC name, the org PRN or `root_prn()`, and the set,
    /// the version, the origin and the catalog revision.
    #[allow(clippy::too_many_arguments)]
    fn audit_entry(&self, actor: &Prn, action: &str, resource: &Prn, set: &str, version: Option<&ToolPolicyVersion>, catalog_revision: Option<u64>, now: DateTime<Utc>) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: action.to_owned(),
            resource_prn: Some(resource.canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: json!({
                "set": set,
                "version": version.map(|v| v.version),
                "origin": version.map(|v| origin_str(v.origin)),
                "catalog_revision": catalog_revision,
            }),
            correlation_id: Some(self.ids.new_correlation_id()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{ConcurrentChange, FakeAuditLog, FakeAuthorizer, FakePolicyCompiler, FakeUnitOfWork, FixedClock, InMemoryOrgs, InMemoryToolPolicies, SeqIds, seed_org};
    use crate::application::tool_policies::sets::{CompiledMeta, decode_compiled, encode_compiled};
    use paigasus_iam_core::tool_policy::{CompileOutcome, CompiledOutput, CompilerFailure, Rejection, STARTER_TOOL_POLICY_SOURCE, ToolPolicyRepository, VersionStatus};
    use paigasus_iam_core::tool_policy::{SetKind, base_schema_json, tenant_schema_json};
    use serde_json::json;

    const ORG: u128 = 100;
    const MAX_SOURCE: usize = 4096;
    const MAX_WINDOW_MS: u64 = 3_600_000;
    const SEARCH_POLICY: &str = "@id(\"search-q\")\nforbid(principal, action == Pgs::Gateway::Action::\"search\", resource) when { context.input.q == \"x\" };\n";
    const FLOOR_DENY: &str = "@id(\"floor-deny\")\nforbid(principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n";

    fn principal(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn search_schema() -> serde_json::Value {
        json!({"type": "object", "properties": {"q": {"type": "string"}, "price": {"type": "number"}}, "required": ["q"]})
    }

    struct Harness {
        svc: ToolPolicyService<SeqIds, FixedClock>,
        repo: InMemoryToolPolicies,
        compiler: FakePolicyCompiler,
        authz: FakeAuthorizer,
        audit: FakeAuditLog,
        uow: FakeUnitOfWork,
        org: Uuid,
        org_prn: Prn,
        actor: Prn,
    }

    /// A harness whose actor holds every tenant action at the org and every floor action at Root.
    fn harness() -> Harness {
        let h = bare_harness();
        for action in [Action::ManageToolCatalog, Action::ReadToolCatalog, Action::ManageToolPolicies, Action::ReadToolPolicies] {
            h.authz.allow(action, &h.org_prn);
        }
        for action in [Action::ManageToolPolicyFloor, Action::ReadToolPolicyFloor] {
            h.authz.allow(action, &root_prn());
        }
        h
    }

    /// A harness whose actor holds nothing.
    fn bare_harness() -> Harness {
        let orgs = InMemoryOrgs::default();
        let org_id = seed_org(&orgs.0, ORG);
        let repo = InMemoryToolPolicies::default();
        let compiler = FakePolicyCompiler::default();
        let authz = FakeAuthorizer::default();
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let svc = ToolPolicyService::new(ToolPolicyServiceDeps {
            repo: Arc::new(repo.clone()),
            orgs: Arc::new(orgs),
            compiler: Arc::new(compiler.clone()),
            authorize: Authorize::new(Arc::new(authz.clone())),
            uow: Arc::new(uow.clone()),
            audit: Arc::new(audit.clone()),
            limits: ToolPolicyLimits {
                catalog: CatalogLimits::default(),
                max_source_bytes: MAX_SOURCE,
                max_window_ms: MAX_WINDOW_MS,
            },
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        Harness {
            svc,
            repo,
            compiler,
            authz,
            audit,
            uow,
            org: org_id.uuid(),
            org_prn: org_id.prn().clone(),
            actor: principal(1),
        }
    }

    fn tenant(h: &Harness) -> ToolPolicySetKey {
        ToolPolicySetKey::Tenant(h.org)
    }

    fn accepted<T: std::fmt::Debug>(r: Result<WriteResult<T>, TenancyError>) -> T {
        match r {
            Ok(WriteResult::Accepted(v)) => v,
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    fn rejected<T: std::fmt::Debug>(r: Result<WriteResult<T>, TenancyError>) -> Vec<ToolPolicyRejection> {
        match r {
            Ok(WriteResult::Rejected(v)) => v,
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    fn audit_actions(h: &Harness) -> Vec<String> {
        h.audit.0.lock().unwrap().iter().map(|e| e.action.clone()).collect()
    }

    fn compiler_rejection(policy_id: Option<&str>, reason: &str, message: &str) -> Rejection {
        Rejection {
            policy_id: policy_id.map(str::to_owned),
            line: Some(2),
            column: Some(5),
            reason: reason.to_owned(),
            message: message.to_owned(),
        }
    }

    async fn put_search(h: &Harness, expected: u64) -> CatalogAccepted {
        accepted(h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, Some(search_schema()), expected).await)
    }

    /// A live tenant version stored with no service call, at `generation` and `schema_revision`.
    fn seed_live_tenant(h: &Harness, source: &str, generation: u32, schema_revision: u32) {
        let out = CompiledOutput {
            cedar_policies: source.to_owned(),
            cedar_schema_json: tenant_schema_json(&[]),
            conditions: Vec::new(),
        };
        let meta = CompiledMeta {
            key: tenant(h),
            version: 1,
            compiler_generation: generation,
            schema_revision,
            catalog_revision: Some(0),
            max_window_ms: MAX_WINDOW_MS,
            catalog_names: Vec::new(),
        };
        h.repo.seed_live(ToolPolicyVersion {
            key: tenant(h),
            version: 1,
            source: source.to_owned(),
            compiled: encode_compiled(&meta, &out),
            compiler_generation: generation,
            schema_revision,
            catalog_revision: Some(0),
            status: VersionStatus::Live,
            origin: VersionOrigin::Save,
            created_by: h.actor.canonical(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        });
    }

    #[tokio::test]
    async fn put_set_writes_version_1_and_one_audit_entry() {
        let h = harness();
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        assert_eq!((v.version, v.origin, v.status), (1, VersionOrigin::Save, VersionStatus::Live));
        assert_eq!((v.compiler_generation, v.schema_revision, v.catalog_revision), (1, TOOL_SCHEMA_REVISION, Some(0)));
        assert_eq!(v.created_by, h.actor.canonical());
        assert_eq!(decode_compiled(&v.compiled).unwrap().cedar_policies, STARTER_TOOL_POLICY_SOURCE);
        let request = &h.compiler.requests()[0];
        assert_eq!((request.kind, request.max_window_ms), (SetKind::Tenant, MAX_WINDOW_MS));
        assert_eq!(request.schema_json, tenant_schema_json(&[]));
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "PutToolPolicySet");
        assert_eq!(entries[0].resource_prn.as_deref(), Some(h.org_prn.canonical().as_str()));
        assert_eq!(entries[0].actor_prn.as_deref(), Some(h.actor.canonical().as_str()));
        assert_eq!(entries[0].detail, json!({"set": "tenant", "version": 1, "origin": "save", "catalog_revision": 0}));
        assert_eq!(h.uow.commits(), 1);
        assert_eq!(h.repo.state(&tenant(&h)).await.unwrap().live_version, 1);
    }

    #[tokio::test]
    async fn an_empty_tenant_source_is_saved() {
        let h = harness();
        let v = accepted(h.svc.put_set(&h.actor, h.org, "", 0).await);
        assert_eq!((v.version, v.source.as_str()), (1, ""));
    }

    #[tokio::test]
    async fn a_byte_identical_save_returns_the_live_version_and_writes_nothing() {
        let h = harness();
        let first = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let again = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 1).await);
        assert_eq!(again, first);
        assert_eq!((h.compiler.calls(), h.repo.writes(), h.audit.0.lock().unwrap().len(), h.uow.commits()), (1, 1, 1, 1));
    }

    #[tokio::test]
    async fn a_stale_expected_version_is_a_version_conflict() {
        let h = harness();
        assert_eq!(
            h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 5).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert_eq!(h.svc.put_floor(&h.actor, FLOOR_DENY, 2).await.unwrap_err(), TenancyError::ToolPolicyVersionConflict);
        assert_eq!(h.compiler.calls(), 0);
    }

    #[tokio::test]
    async fn two_saves_on_one_version_conflict() {
        let h = harness();
        h.repo.change_before_next_lock(ConcurrentChange::LiveVersion);
        assert_eq!(
            h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert_eq!((h.compiler.calls(), h.repo.writes(), h.audit.0.lock().unwrap().len(), h.uow.commits()), (1, 0, 0, 0));
    }

    #[tokio::test]
    async fn a_save_during_a_catalog_write_conflicts() {
        let h = harness();
        h.repo.change_before_next_lock(ConcurrentChange::CatalogRevision);
        assert_eq!(
            h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert_eq!((h.repo.writes(), h.audit.0.lock().unwrap().len(), h.uow.commits()), (0, 0, 0));
    }

    #[tokio::test]
    async fn a_rejection_is_a_successful_result_and_writes_nothing() {
        let h = harness();
        h.compiler.push(Ok(CompileOutcome::Rejected(vec![compiler_rejection(
            Some("bad-id"),
            RejectionReason::IdInvalid.code(),
            "the @id is malformed",
        )])));
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, "@id(\"Bad\")\npermit(principal, action, resource);", 0).await);
        assert_eq!(
            rejections,
            vec![ToolPolicyRejection {
                policy_id: Some("bad-id".into()),
                line: Some(2),
                column: Some(5),
                reason: RejectionReason::IdInvalid,
                message: "the @id is malformed".into(),
            }]
        );
        assert_eq!((h.repo.writes(), h.audit.0.lock().unwrap().len(), h.uow.commits()), (0, 0, 0));
    }

    #[tokio::test]
    async fn a_4_14_failure_becomes_a_policy_invalid_rejection() {
        let h = harness();
        // The identity compiler accepts; cedar 4.14 finds that no action `search` exists.
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, SEARCH_POLICY, 0).await);
        // Cedar reports the unknown action and the impossible policy: two findings, one policy.
        assert!(!rejections.is_empty());
        for r in &rejections {
            assert_eq!((r.reason, r.policy_id.as_deref()), (RejectionReason::PolicyInvalid, Some("search-q")), "{r:?}");
        }
        assert!(rejections.iter().any(|r| r.message.contains("unrecognized action")), "{rejections:?}");
        assert_eq!(h.repo.writes(), 0);
    }

    #[tokio::test]
    async fn busy_and_unavailable_map_to_their_errors() {
        let h = harness();
        h.compiler.push(Err(CompilerFailure::Busy));
        assert_eq!(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.unwrap_err(), TenancyError::ToolPolicyCompilerBusy);
        h.compiler.push(Err(CompilerFailure::Unavailable("crash".into())));
        assert_eq!(
            h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.unwrap_err(),
            TenancyError::ToolPolicyCompilerUnavailable
        );
        h.compiler.push(Err(CompilerFailure::Busy));
        assert_eq!(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await.unwrap_err(), TenancyError::ToolPolicyCompilerBusy);
        assert_eq!(h.repo.writes(), 0);
    }

    #[tokio::test]
    async fn an_unknown_rejection_reason_is_compiler_unavailable() {
        let h = harness();
        // A catalog reason is IAM's own: the compiler must never send it.
        for reason in ["tool-policy-made-up", RejectionReason::CatalogInvalid.code()] {
            h.compiler.push(Ok(CompileOutcome::Rejected(vec![compiler_rejection(None, reason, "m")])));
            assert_eq!(
                h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.unwrap_err(),
                TenancyError::ToolPolicyCompilerUnavailable,
                "{reason}"
            );
        }
    }

    #[tokio::test]
    async fn an_oversized_source_is_rejected_without_a_compile() {
        let h = harness();
        let source = "x".repeat(MAX_SOURCE + 1);
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, &source, 0).await);
        assert_eq!(rejections.len(), 1);
        assert_eq!(rejections[0].reason, RejectionReason::PolicyInvalid);
        let rejections = rejected(h.svc.put_floor(&h.actor, &source, 0).await);
        assert_eq!(rejections[0].reason, RejectionReason::PolicyInvalid);
        assert_eq!(h.compiler.calls(), 0);
        // The limit itself is allowed.
        assert!(matches!(h.svc.put_set(&h.actor, h.org, &" ".repeat(MAX_SOURCE), 0).await, Ok(WriteResult::Accepted(_))));
    }

    #[tokio::test]
    async fn the_floor_compiles_against_the_base_schema_with_kind_floor() {
        let h = harness();
        let v = accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        assert_eq!((v.key, v.version, v.catalog_revision, v.origin), (ToolPolicySetKey::Floor, 1, None, VersionOrigin::Save));
        let request = &h.compiler.requests()[0];
        assert_eq!(request.kind, SetKind::Floor);
        assert_eq!(request.schema_json, base_schema_json());
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries[0].action, "PutFloorPolicySet");
        assert_eq!(entries[0].resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(entries[0].detail, json!({"set": "floor", "version": 1, "origin": "save", "catalog_revision": null}));
        assert_eq!(h.svc.get_floor(&h.actor, None).await.unwrap(), Some(v.clone()));
        assert_eq!(h.svc.get_floor(&h.actor, Some(1)).await.unwrap(), Some(v));
    }

    #[tokio::test]
    async fn a_floor_permit_that_passed_the_compiler_is_rejected_by_the_second_check() {
        let h = harness();
        let source = format!("{FLOOR_DENY}@id(\"floor-allow\")\npermit(principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);\n");
        let rejections = rejected(h.svc.put_floor(&h.actor, &source, 0).await);
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].reason, RejectionReason::FloorPermit);
        assert_eq!(rejections[0].policy_id.as_deref(), Some("floor-allow"));
        assert_eq!(h.repo.writes(), 0);
    }

    #[tokio::test]
    async fn put_tool_with_no_live_set_does_not_compile() {
        let h = harness();
        let out = put_search(&h, 0).await;
        assert_eq!((out.catalog_revision, out.set_version), (1, 0));
        assert_eq!(out.tools.len(), 1);
        assert_eq!(out.tools[0].entry.name.name_key, "search");
        assert_eq!(h.compiler.calls(), 0);
        let (tools, revision) = h.svc.list_tools(&h.actor, h.org, Page { limit: 10, offset: 0 }).await.unwrap();
        assert_eq!((tools.len(), revision), (1, 1));
        assert_eq!(audit_actions(&h), vec!["PutTool".to_string()]);
        assert_eq!(h.audit.0.lock().unwrap()[0].detail, json!({"set": "tenant", "version": null, "origin": null, "catalog_revision": 1}));
    }

    #[tokio::test]
    async fn put_tool_with_a_case_variant_of_an_existing_entry_conflicts() {
        let h = harness();
        accepted(h.svc.put_tool(&h.actor, h.org, "Search", ToolKind::Function, None, 0).await);
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 1).await.unwrap_err(),
            TenancyError::ToolCatalogConflict
        );
        // The same display name is an update, not a conflict.
        accepted(h.svc.put_tool(&h.actor, h.org, "Search", ToolKind::Custom, None, 1).await);
    }

    #[tokio::test]
    async fn deleting_a_tool_that_a_live_policy_names_is_rejected() {
        let h = harness();
        put_search(&h, 0).await;
        accepted(h.svc.put_set(&h.actor, h.org, SEARCH_POLICY, 0).await);
        let writes = h.repo.writes();
        let rejections = rejected(h.svc.delete_tool(&h.actor, h.org, "search", 1).await);
        assert!(!rejections.is_empty());
        for r in &rejections {
            assert_eq!(r.reason, RejectionReason::CatalogBreaksPolicy, "{r:?}");
        }
        assert!(rejections.iter().any(|r| r.policy_id.as_deref() == Some("search-q")), "{rejections:?}");
        assert_eq!(h.repo.writes(), writes);
        assert_eq!(h.repo.catalog(h.org).await.unwrap().len(), 1);
        assert_eq!(h.repo.state(&tenant(&h)).await.unwrap().catalog_revision, 1);
    }

    #[tokio::test]
    async fn a_catalog_change_with_identical_output_keeps_the_set_version() {
        let h = harness();
        put_search(&h, 0).await;
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let out = put_search(&h, 1).await;
        assert_eq!((out.catalog_revision, out.set_version), (2, 0));
        assert_eq!(h.repo.versions(&tenant(&h)).len(), 1);
        assert_eq!(h.compiler.calls(), 2, "the coupling compile ran");
    }

    #[tokio::test]
    async fn a_catalog_change_with_different_output_writes_a_catalog_change_version() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let out = put_search(&h, 0).await;
        assert_eq!((out.catalog_revision, out.set_version), (1, 2));
        let versions = h.repo.versions(&tenant(&h));
        assert_eq!(versions.len(), 2);
        assert_eq!((versions[0].status, versions[1].status), (VersionStatus::Superseded, VersionStatus::Live));
        assert_eq!(
            (versions[1].origin, versions[1].catalog_revision, versions[1].source.as_str()),
            (VersionOrigin::CatalogChange, Some(1), STARTER_TOOL_POLICY_SOURCE)
        );
        let compiled = decode_compiled(&versions[1].compiled).unwrap();
        assert_eq!(compiled.cedar_schema_json, tenant_schema_json(&[out.tools[0].entry.clone()]));
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 2, "one entry for the save, one for the catalog write");
        assert_eq!(entries[1].action, "PutTool");
        assert_eq!(entries[1].detail, json!({"set": "tenant", "version": 2, "origin": "catalog_change", "catalog_revision": 1}));
        assert_eq!(h.uow.commits(), 2);
    }

    #[tokio::test]
    async fn a_live_set_from_an_older_generation_blocks_catalog_writes() {
        let h = harness();
        seed_live_tenant(&h, STARTER_TOOL_POLICY_SOURCE, 0, TOOL_SCHEMA_REVISION);
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 0).await.unwrap_err(),
            TenancyError::ToolPolicyRecompilePending
        );
        assert_eq!((h.compiler.calls(), h.repo.writes()), (0, 0));
    }

    #[tokio::test]
    async fn a_live_set_from_an_older_schema_revision_blocks_catalog_writes() {
        let h = harness();
        seed_live_tenant(&h, STARTER_TOOL_POLICY_SOURCE, 1, TOOL_SCHEMA_REVISION - 1);
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 0).await.unwrap_err(),
            TenancyError::ToolPolicyRecompilePending
        );
        assert_eq!((h.compiler.calls(), h.repo.writes()), (0, 0));
    }

    #[tokio::test]
    async fn a_save_clears_recompile_pending() {
        let h = harness();
        seed_live_tenant(&h, STARTER_TOOL_POLICY_SOURCE, 0, TOOL_SCHEMA_REVISION);
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 1).await);
        assert_eq!((v.version, v.compiler_generation, v.origin), (2, 1, VersionOrigin::Save));
        let out = put_search(&h, 0).await;
        assert_eq!(out.catalog_revision, 1);
    }

    #[tokio::test]
    async fn a_byte_identical_save_at_the_running_generation_writes_nothing() {
        let h = harness();
        seed_live_tenant(&h, STARTER_TOOL_POLICY_SOURCE, 1, TOOL_SCHEMA_REVISION);
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 1).await);
        assert_eq!(v.version, 1);
        assert_eq!((h.compiler.calls(), h.repo.writes(), h.audit.0.lock().unwrap().len()), (0, 0, 0));
    }

    #[tokio::test]
    async fn put_tool_of_a_builtin_with_no_schema_gets_an_empty_record() {
        let h = harness();
        let out = accepted(h.svc.put_tool(&h.actor, h.org, "LocalShell", ToolKind::Function, None, 0).await);
        assert_eq!(out.tools[0].entry.cedar_input_type, json!({"type": "Record", "attributes": {}}));
        assert_eq!(out.tools[0].entry.input_schema, None);
    }

    #[tokio::test]
    async fn a_saved_policy_that_reads_a_dropped_field_names_the_drop_reason() {
        let h = harness();
        put_search(&h, 0).await;
        h.compiler.push(Ok(CompileOutcome::Rejected(vec![compiler_rejection(
            Some("price-cap"),
            RejectionReason::PolicyInvalid.code(),
            "Cedar validation: attribute `price` on the context input is not found",
        )])));
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, "@id(\"price-cap\")\nforbid(principal, action, resource);", 0).await);
        assert_eq!(rejections[0].reason, RejectionReason::PolicyInvalid);
        assert!(
            rejections[0].message.contains("the field `price` of the tool `search` is not in the schema"),
            "{}",
            rejections[0].message
        );
    }

    #[tokio::test]
    async fn import_with_replace_removes_missing_tools() {
        let h = harness();
        accepted(h.svc.put_tool(&h.actor, h.org, "a", ToolKind::Function, None, 0).await);
        accepted(h.svc.put_tool(&h.actor, h.org, "b", ToolKind::Function, None, 1).await);
        let doc = json!([{"type": "function", "name": "b", "parameters": search_schema()}, {"type": "web_search"}]);
        let out = accepted(h.svc.import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &doc, None, true, 2).await);
        assert_eq!(out.catalog_revision, 3);
        assert_eq!(out.skipped, vec![(1, "web_search".to_string())]);
        let names: Vec<String> = h.repo.catalog(h.org).await.unwrap().into_iter().map(|t| t.entry.name.name_key).collect();
        assert_eq!(names, vec!["b".to_string()]);
        // Without replace, a tool not in the batch stays.
        let doc = json!([{"type": "function", "name": "c", "parameters": null}]);
        accepted(h.svc.import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &doc, None, false, 3).await);
        assert_eq!(h.repo.catalog(h.org).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_import_problem_is_a_rejection_and_a_duplicate_is_a_conflict() {
        let h = harness();
        let bad = json!([{"type": "function", "name": "bad name"}]);
        let rejections = rejected(h.svc.import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &bad, None, false, 0).await);
        assert_eq!(rejections[0].reason, RejectionReason::CatalogInvalid);
        let dup = json!([{"type": "function", "name": "x"}, {"type": "function", "name": "X"}]);
        assert_eq!(
            h.svc.import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &dup, None, false, 0).await.unwrap_err(),
            TenancyError::ToolCatalogConflict
        );
        assert_eq!(h.repo.writes(), 0);
    }

    #[tokio::test]
    async fn a_catalog_over_the_tool_limit_is_rejected() {
        let h = harness();
        let tools: Vec<serde_json::Value> = (0..CatalogLimits::default().max_tools).map(|i| json!({"type": "function", "name": format!("t{i}")})).collect();
        accepted(h.svc.import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &json!(tools), None, false, 0).await);
        let rejections = rejected(h.svc.put_tool(&h.actor, h.org, "one_more", ToolKind::Function, None, 1).await);
        assert_eq!(rejections[0].reason, RejectionReason::CatalogInvalid);
    }

    #[tokio::test]
    async fn delete_tool_of_an_absent_name_is_not_found() {
        let h = harness();
        assert_eq!(h.svc.delete_tool(&h.actor, h.org, "nope", 0).await.unwrap_err(), TenancyError::NotFound);
        put_search(&h, 0).await;
        let out = accepted(h.svc.delete_tool(&h.actor, h.org, "SEARCH", 1).await);
        assert_eq!((out.catalog_revision, out.set_version, out.tools.len()), (2, 0, 0));
    }

    #[tokio::test]
    async fn a_stale_catalog_revision_is_a_version_conflict() {
        let h = harness();
        put_search(&h, 0).await;
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "b", ToolKind::Function, None, 0).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        h.repo.change_before_next_lock(ConcurrentChange::CatalogRevision);
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "b", ToolKind::Function, None, 1).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert_eq!(h.repo.catalog(h.org).await.unwrap().len(), 1);
    }

    /// The catalog side of the race: a save stores a version between the coupling compile and
    /// the lock. The catalog write must not commit against the old live version.
    #[tokio::test]
    async fn a_save_during_a_catalog_write_with_a_live_set_conflicts() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let (writes, audits) = (h.repo.writes(), h.audit.0.lock().unwrap().len());
        h.repo.change_before_next_lock(ConcurrentChange::LiveVersion);
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, Some(search_schema()), 0).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert_eq!(h.compiler.calls(), 2, "the coupling compile ran before the lock");
        assert_eq!((h.repo.writes() - writes, h.audit.0.lock().unwrap().len() - audits, h.uow.commits()), (0, 0, 1));
        assert!(h.repo.catalog(h.org).await.unwrap().is_empty());
    }

    /// A compiler upgrade between a save and a catalog write: the running generation moves past
    /// the live version's, so the catalog write is recompile-pending until the set is saved again.
    #[tokio::test]
    async fn a_compiler_generation_change_makes_catalog_writes_recompile_pending() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        h.compiler.set_generation(2);
        assert_eq!(
            h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 0).await.unwrap_err(),
            TenancyError::ToolPolicyRecompilePending
        );
        // A byte-identical save is not a no-op at a new generation: it stores version 2.
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 1).await);
        assert_eq!((v.version, v.compiler_generation), (2, 2));
        accepted(h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 0).await);
    }

    #[tokio::test]
    async fn every_mutation_writes_exactly_one_audit_entry() {
        let doc = json!([{"type": "function", "name": "a"}]);
        let mutations: Vec<&str> = vec!["PutTool", "DeleteTool", "ImportTools", "PutToolPolicySet", "PutFloorPolicySet"];
        for action in mutations {
            let h = harness();
            // DeleteTool needs a tool: the PutTool before it is one entry of its own.
            let (before, expected) = if action == "DeleteTool" {
                accepted(h.svc.put_tool(&h.actor, h.org, "a", ToolKind::Function, None, 0).await);
                (1, 1)
            } else {
                (0, 0)
            };
            let committed = match action {
                "PutTool" => h
                    .svc
                    .put_tool(&h.actor, h.org, "a", ToolKind::Function, None, expected)
                    .await
                    .map(|r| matches!(r, WriteResult::Accepted(_))),
                "DeleteTool" => h.svc.delete_tool(&h.actor, h.org, "a", expected).await.map(|r| matches!(r, WriteResult::Accepted(_))),
                "ImportTools" => h
                    .svc
                    .import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &doc, None, false, expected)
                    .await
                    .map(|r| matches!(r, WriteResult::Accepted(_))),
                "PutToolPolicySet" => h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.map(|r| matches!(r, WriteResult::Accepted(_))),
                _ => h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await.map(|r| matches!(r, WriteResult::Accepted(_))),
            };
            assert_eq!(committed, Ok(true), "{action}");
            let actions = audit_actions(&h);
            assert_eq!(actions.len(), before + 1, "{action}: {actions:?}");
            assert_eq!(actions.last().map(String::as_str), Some(action));
            assert_eq!(h.uow.commits(), before + 1, "{action}");
        }
    }

    #[tokio::test]
    async fn each_rpc_checks_its_action() {
        let doc = json!([{"type": "function", "name": "a"}]);
        for rpc in 0..8 {
            let h = bare_harness();
            let (result, action, resource) = match rpc {
                0 => (
                    h.svc.put_tool(&h.actor, h.org, "a", ToolKind::Function, None, 0).await.map(|_| ()),
                    Action::ManageToolCatalog,
                    h.org_prn.clone(),
                ),
                1 => (h.svc.delete_tool(&h.actor, h.org, "a", 0).await.map(|_| ()), Action::ManageToolCatalog, h.org_prn.clone()),
                2 => (
                    h.svc.list_tools(&h.actor, h.org, Page { limit: 10, offset: 0 }).await.map(|_| ()),
                    Action::ReadToolCatalog,
                    h.org_prn.clone(),
                ),
                3 => (
                    h.svc.import_tools(&h.actor, h.org, ImportFormat::OpenAiResponses, &doc, None, false, 0).await.map(|_| ()),
                    Action::ManageToolCatalog,
                    h.org_prn.clone(),
                ),
                4 => (
                    h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.map(|_| ()),
                    Action::ManageToolPolicies,
                    h.org_prn.clone(),
                ),
                5 => (h.svc.get_set(&h.actor, h.org, None).await.map(|_| ()), Action::ReadToolPolicies, h.org_prn.clone()),
                6 => (h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await.map(|_| ()), Action::ManageToolPolicyFloor, root_prn()),
                _ => (h.svc.get_floor(&h.actor, None).await.map(|_| ()), Action::ReadToolPolicyFloor, root_prn()),
            };
            assert_eq!(result, Err(TenancyError::Forbidden), "rpc {rpc}");
            assert_eq!(h.authz.checks(), vec![(action, resource.canonical())], "rpc {rpc}");
            assert_eq!((h.compiler.calls(), h.repo.writes(), h.audit.0.lock().unwrap().len()), (0, 0, 0), "rpc {rpc}");
        }
    }

    #[tokio::test]
    async fn a_tenant_rpc_on_an_absent_org_is_not_found() {
        let h = harness();
        let absent = Uuid::from_u128(999);
        assert_eq!(h.svc.put_set(&h.actor, absent, "", 0).await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(h.svc.list_tools(&h.actor, absent, Page { limit: 1, offset: 0 }).await.unwrap_err(), TenancyError::NotFound);
        assert!(h.authz.checks().is_empty());
    }

    /// The floor actions' Cedar `appliesTo` lists Organization, Team and Project too, so the
    /// schema does not keep them at Root. The service must authorize the floor at `root_prn()`.
    /// An `org_admin` (here: every tool action allowed at the org, the floor actions included,
    /// as a platform_admin-style grant made at an org node would give) cannot manage the floor.
    #[tokio::test]
    async fn an_org_admin_is_denied_both_floor_rpcs() {
        let h = bare_harness();
        for action in [
            Action::ManageToolCatalog,
            Action::ReadToolCatalog,
            Action::ManageToolPolicies,
            Action::ReadToolPolicies,
            Action::ManageToolPolicyFloor,
            Action::ReadToolPolicyFloor,
        ] {
            h.authz.allow(action, &h.org_prn);
        }
        assert_eq!(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(h.svc.get_floor(&h.actor, None).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(
            h.authz.checks(),
            vec![(Action::ManageToolPolicyFloor, root_prn().canonical()), (Action::ReadToolPolicyFloor, root_prn().canonical())]
        );
        assert_eq!(h.compiler.calls(), 0);
        // The same grant does manage the tenant set at the org.
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
    }

    #[tokio::test]
    async fn a_platform_admin_is_allowed_both_floor_rpcs() {
        let h = bare_harness();
        h.authz.allow(Action::ManageToolPolicyFloor, &root_prn());
        h.authz.allow(Action::ReadToolPolicyFloor, &root_prn());
        accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        assert_eq!(h.svc.get_floor(&h.actor, None).await.unwrap().map(|v| v.version), Some(1));
    }

    #[tokio::test]
    async fn get_set_returns_not_found_for_an_absent_version() {
        let h = harness();
        assert_eq!(h.svc.get_set(&h.actor, h.org, None).await.unwrap(), None);
        assert_eq!(h.svc.get_set(&h.actor, h.org, Some(9)).await.unwrap_err(), TenancyError::ToolPolicyVersionNotFound);
        assert_eq!(h.svc.get_floor(&h.actor, Some(1)).await.unwrap_err(), TenancyError::ToolPolicyVersionNotFound);
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        assert_eq!(h.svc.get_set(&h.actor, h.org, Some(1)).await.unwrap(), Some(v.clone()));
        assert_eq!(h.svc.get_set(&h.actor, h.org, None).await.unwrap(), Some(v));
    }
}
