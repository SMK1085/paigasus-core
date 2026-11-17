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
//! conflict. (4) Write, enqueue one outbox event for a new live version, record one audit entry, commit.
//! The event and the audit entry share one correlation id. A write that stores no new live version
//! enqueues no event.
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
mod events;
mod fetch;
pub mod rejection;
pub mod sets;
mod settings;
pub mod wire;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::tool_policy::{
    CatalogLimits, CompileOutcome, CompileRequest, CompiledOutput, CompilerFailure, FailMode, ImportFormat, PROTOCOL_VERSION, PolicyCompiler, STARTER_TOOL_POLICY_SOURCE, SetKind, StoredTool,
    TOOL_SCHEMA_REVISION, ToolEntry, ToolKind, ToolPolicyMode, ToolPolicyRepository, ToolPolicySetKey, ToolPolicySettings, ToolPolicyState, ToolPolicyVersion, VersionOrigin, VersionStatus,
    base_schema_json, build_manual, catalog_names, check_compiled, outputs_identical, parse_import, tenant_schema_json,
};
use paigasus_iam_core::{Action, AuditEntry, AuditLog, AuditOutcome, Clock, IdGenerator, OrganizationRepository, Outbox, Transaction, UnitOfWork};
use paigasus_kernel::Prn;
use serde_json::json;
use uuid::Uuid;

use self::catalog::{CatalogPlan, PlanStop, apply_plan, check_catalog_size, check_tool_limit, plan_delete, plan_import, plan_put, problems_to_stop, size_check_time, stored_next, stored_upserts};
pub use self::fetch::CompiledTenant;
pub use self::rejection::{RejectionReason, ToolPolicyRejection};
use self::sets::{CompiledMeta, add_dropped_field_reasons, decode_compiled, encode_compiled, reject_history_until_gateway_supports_it};
use self::settings::check_transition;
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
    /// The largest catalog, measured as the encoded size of all its tools in one
    /// `ListToolsResponse`. A write over it is a `tool-catalog-invalid` rejection.
    pub max_catalog_response_bytes: usize,
}

/// The ports and values that [`ToolPolicyService::new`] takes.
pub struct ToolPolicyServiceDeps<I, C> {
    pub repo: Arc<dyn ToolPolicyRepository>,
    pub orgs: Arc<dyn OrganizationRepository>,
    pub compiler: Arc<dyn PolicyCompiler>,
    pub authorize: Authorize,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub outbox: Arc<dyn Outbox>,
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

/// An accepted settings write.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsAccepted {
    pub settings: ToolPolicySettings,
    /// The version of the starter set that this write saved. 0 when it saved none.
    pub starter_version: u64,
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
    outbox: Arc<dyn Outbox>,
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
        VersionOrigin::Starter => "starter",
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
            outbox: deps.outbox,
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
        // One consistent read: two reads could pair the tools of a newer write with an older
        // revision, and a caller that writes with that revision then gets a false conflict or
        // trusts a stale page.
        let (tools, catalog_revision) = self.repo.catalog_snapshot(org).await?;
        let offset = usize::try_from(page.offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(page.limit).unwrap_or(usize::MAX);
        Ok((tools.into_iter().skip(offset).take(limit).collect(), catalog_revision))
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

    /// The tool policy settings of `org`. An organization with no row is `off` and `closed`.
    pub async fn get_settings(&self, actor: &Prn, org: Uuid) -> Result<ToolPolicySettings, TenancyError> {
        self.authorize_org(actor, org, Action::ReadToolPolicies).await?;
        Ok(self.repo.settings(org).await?.unwrap_or_else(ToolPolicySettings::default_for_absent_row))
    }

    /// Writes the tool policy settings of `org` (spec section 9.2). The first change away from
    /// `off` must be to `log_only`; with no set, it also saves the starter set as version 1.
    /// `enforce` before a first change away from `off`, or with no set, is
    /// `tool-policy-mode-transition-invalid` (final review I-2). The first change away from
    /// `off` stores `first_enabled_at` in the same transaction.
    pub async fn put_settings(&self, actor: &Prn, org: Uuid, mode: ToolPolicyMode, fail_mode: FailMode) -> Result<SettingsAccepted, TenancyError> {
        let org_prn = self.authorize_org(actor, org, Action::ManageToolPolicySettings).await?;
        let key = ToolPolicySetKey::Tenant(org);

        // Step 1: the state, and the decision.
        let state = self.repo.state(&key).await?;
        let first_enabled_at = self.repo.settings(org).await?.and_then(|s| s.first_enabled_at);
        let save_starter = check_transition(mode, state.live_version > 0, first_enabled_at.is_some())?;

        // Step 2: compile the starter, with no transaction open.
        let starter = if save_starter {
            let catalog: Vec<ToolEntry> = self.repo.catalog(org).await?.into_iter().map(|s| s.entry).collect();
            match self.compile(SetKind::Tenant, STARTER_TOOL_POLICY_SOURCE, &catalog).await? {
                Compiled::Accepted(out) => Some((out, catalog)),
                Compiled::Rejected(rejections) => {
                    tracing::error!(?rejections, "the compiler rejected the starter tool policy; this must not happen");
                    return Err(TenancyError::Internal);
                }
            }
        } else {
            None
        };

        // Steps 3 and 4: a settings write with no starter does not depend on the set, so it takes
        // no state lock. The starter write locks and rechecks inside the helper.
        let tx = self.uow.begin().await?;
        let now = self.clock.now();
        // One correlation id for every event and the audit entry.
        let corr = self.ids.new_correlation_id();
        let mut starter_version = 0;
        if let Some((out, catalog)) = &starter {
            let target = SetTarget { key, resource: org_prn.clone() };
            let version = self
                .write_new_live_version(&*tx, actor, &target, &state, STARTER_TOOL_POLICY_SOURCE, out, catalog, VersionOrigin::Starter, corr, now)
                .await?;
            starter_version = version.version;
        }
        let settings = ToolPolicySettings {
            mode,
            fail_mode,
            updated_at: Some(now),
            updated_by: Some(actor.canonical()),
            // Written once: the repository keeps a stored value (also under a concurrent write).
            first_enabled_at: first_enabled_at.or((mode != ToolPolicyMode::Off).then_some(now)),
        };
        self.repo.upsert_settings_in(&*tx, org, &settings).await?;
        self.outbox.enqueue(&*tx, &events::settings_changed(&*self.ids, corr, actor, &org_prn, org, &settings, now)).await?;
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "PutToolPolicySettings".to_owned(),
            resource_prn: Some(org_prn.canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: json!({
                "mode": mode.as_str(),
                "fail_mode": fail_mode.as_str(),
                "starter_version": (starter_version > 0).then_some(starter_version),
            }),
            correlation_id: Some(corr),
        };
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(SettingsAccepted { settings, starter_version })
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
                Ok(()) => {
                    // SMA-753: remove this check with `reject_history_until_gateway_supports_it`
                    // when the gateway evaluates history conditions.
                    let history = reject_history_until_gateway_supports_it(&out);
                    return Ok(if history.is_empty() { Compiled::Accepted(out) } else { Compiled::Rejected(history) });
                }
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
        let live = self.repo.live(&target.key).await?;
        // `state` and `live` are two reads. A save that commits between them makes a live row
        // newer than `expected_version`, so this save is a conflict.
        if live.as_ref().is_some_and(|l| l.version != state.live_version) {
            return Err(TenancyError::ToolPolicyVersionConflict);
        }
        if let Some(live) = live
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

        // Steps 3 and 4: lock and recheck, then write the version and its event.
        let tx = self.uow.begin().await?;
        let catalog_revision = match target.key {
            ToolPolicySetKey::Tenant(_) => Some(state.catalog_revision),
            ToolPolicySetKey::Floor => None,
        };
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let version = self.write_new_live_version(&*tx, actor, target, &state, source, &out, &catalog, VersionOrigin::Save, corr, now).await?;
        let entry = self.audit_entry(actor, target.audit_action(), &target.resource, target.audit_set(), Some(&version), catalog_revision, now, corr);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(WriteResult::Accepted(version))
    }

    /// Locks the state of the set and compares it with `state` (the read before the compile). A
    /// difference is `ToolPolicyVersionConflict` and nothing is written. Then stores `source` as
    /// the next live version and enqueues its published event. The audit entry and the commit
    /// belong to the caller, with the same `corr`.
    #[allow(clippy::too_many_arguments)]
    async fn write_new_live_version(
        &self,
        tx: &dyn Transaction,
        actor: &Prn,
        target: &SetTarget,
        state: &ToolPolicyState,
        source: &str,
        out: &CompiledOutput,
        catalog: &[ToolEntry],
        origin: VersionOrigin,
        corr: Uuid,
        now: DateTime<Utc>,
    ) -> Result<ToolPolicyVersion, TenancyError> {
        let locked = self.repo.lock_state_in(tx, &target.key).await?;
        if locked != *state {
            return Err(TenancyError::ToolPolicyVersionConflict);
        }
        let catalog_revision = match target.key {
            ToolPolicySetKey::Tenant(_) => Some(state.catalog_revision),
            ToolPolicySetKey::Floor => None,
        };
        let version = self.new_version(actor, &target.key, state.live_version + 1, source, out, catalog_revision, catalog, origin, now);
        self.repo.insert_live_version_in(tx, &version).await?;
        let event = match target.key {
            ToolPolicySetKey::Tenant(org) => events::set_published(&*self.ids, corr, actor, &target.resource, org, &version, now),
            ToolPolicySetKey::Floor => events::floor_published(&*self.ids, corr, actor, &version, now),
        };
        self.outbox.enqueue(tx, &event).await?;
        Ok(version)
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
        // The size cap: the whole new catalog as one `ListToolsResponse`, with the revision that
        // this write gives. It also bounds the compile request and the compiled schema.
        let encoded = wire::catalog_response_len(&stored_next(&current, &plan, size_check_time(self.clock.now())), state.catalog_revision + 1);
        if let Err(s) = check_catalog_size(encoded, self.limits.max_catalog_response_bytes) {
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
        let corr = self.ids.new_correlation_id();
        if let Some(v) = &version {
            let event = events::set_published(&*self.ids, corr, actor, org_prn, org, v, now);
            self.outbox.enqueue(&*tx, &event).await?;
        }
        let entry = self.audit_entry(actor, action, org_prn, "tenant", version.as_ref(), Some(catalog_revision), now, corr);
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
    fn audit_entry(&self, actor: &Prn, action: &str, resource: &Prn, set: &str, version: Option<&ToolPolicyVersion>, catalog_revision: Option<u64>, now: DateTime<Utc>, corr: Uuid) -> AuditEntry {
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
            correlation_id: Some(corr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{ConcurrentChange, FakeAuditLog, FakeAuthorizer, FakeOutbox, FakePolicyCompiler, FakeUnitOfWork, FixedClock, InMemoryOrgs, InMemoryToolPolicies, SeqIds, seed_org};
    use crate::application::tool_policies::sets::{CompiledMeta, decode_compiled, encode_compiled};
    use paigasus_iam_core::tool_policy::{CompileOutcome, CompiledOutput, CompilerFailure, Rejection, STARTER_TOOL_POLICY_SOURCE, ToolPolicyRepository, VersionStatus};
    use paigasus_iam_core::tool_policy::{SetKind, base_schema_json, tenant_schema_json};
    use serde_json::json;

    const ORG: u128 = 100;
    const MAX_SOURCE: usize = 4096;
    const MAX_WINDOW_MS: u64 = 3_600_000;
    const DEFAULT_CATALOG_CAP: usize = 2_097_152;
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
        clock: FixedClock,
        repo: InMemoryToolPolicies,
        compiler: FakePolicyCompiler,
        authz: FakeAuthorizer,
        audit: FakeAuditLog,
        outbox: FakeOutbox,
        uow: FakeUnitOfWork,
        org: Uuid,
        org_prn: Prn,
        actor: Prn,
    }

    /// A harness whose actor holds every tenant action at the org and every floor action at Root.
    fn harness() -> Harness {
        let h = bare_harness();
        for action in [
            Action::ManageToolCatalog,
            Action::ReadToolCatalog,
            Action::ManageToolPolicies,
            Action::ReadToolPolicies,
            Action::ManageToolPolicySettings,
        ] {
            h.authz.allow(action, &h.org_prn);
        }
        for action in [Action::ManageToolPolicyFloor, Action::ReadToolPolicyFloor] {
            h.authz.allow(action, &root_prn());
        }
        h
    }

    /// A harness whose actor holds nothing.
    fn bare_harness() -> Harness {
        bare_harness_with_cap(DEFAULT_CATALOG_CAP)
    }

    /// [`bare_harness`] with another catalog size cap.
    fn bare_harness_with_cap(max_catalog_response_bytes: usize) -> Harness {
        let orgs = InMemoryOrgs::default();
        let org_id = seed_org(&orgs.0, ORG);
        let repo = InMemoryToolPolicies::default();
        let compiler = FakePolicyCompiler::default();
        let authz = FakeAuthorizer::default();
        let audit = FakeAuditLog::default();
        let outbox = FakeOutbox::default();
        let uow = FakeUnitOfWork::default();
        let clock = FixedClock::default();
        let svc = ToolPolicyService::new(ToolPolicyServiceDeps {
            repo: Arc::new(repo.clone()),
            orgs: Arc::new(orgs),
            compiler: Arc::new(compiler.clone()),
            authorize: Authorize::new(Arc::new(authz.clone())),
            uow: Arc::new(uow.clone()),
            audit: Arc::new(audit.clone()),
            outbox: Arc::new(outbox.clone()),
            limits: ToolPolicyLimits {
                catalog: CatalogLimits::default(),
                max_source_bytes: MAX_SOURCE,
                max_window_ms: MAX_WINDOW_MS,
                max_catalog_response_bytes,
            },
            ids: SeqIds::default(),
            clock: clock.clone(),
        });
        Harness {
            svc,
            clock,
            repo,
            compiler,
            authz,
            audit,
            outbox,
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
    async fn an_identical_save_after_a_concurrent_commit_is_a_version_conflict() {
        let h = harness();
        let first = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        // Another writer stores version 2, with the same source, between `state` and `live`.
        h.repo.store_live_before_next_live(ToolPolicyVersion { version: 2, ..first.clone() });
        assert_eq!(
            h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 1).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert_eq!((h.compiler.calls(), h.repo.insert_attempts()), (1, 1));
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
        // The step-3 recheck stops the save before the insert. The insert's compare-and-set
        // would give the same conflict, so only this count shows that the recheck ran.
        assert_eq!(h.repo.insert_attempts(), 0);
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
    async fn a_predicate_over_the_depth_limit_is_rejected_before_the_store() {
        use paigasus_iam_core::tool_policy::MAX_PREDICATE_DEPTH;
        use paigasus_iam_core::tool_policy::protocol::{CmpOp, EventKind, HistoryCondition, Predicate, Scope, Shape};
        let h = harness();
        let mut predicate = Predicate::EventKind { kind: EventKind::Request };
        for _ in 0..MAX_PREDICATE_DEPTH {
            predicate = Predicate::AllOf {
                left: Box::new(predicate),
                right: Box::new(Predicate::AnyCatalogTool {}),
            };
        }
        h.compiler.push(Ok(CompileOutcome::Accepted(CompiledOutput {
            cedar_policies: STARTER_TOOL_POLICY_SOURCE.to_owned(),
            cedar_schema_json: tenant_schema_json(&[]),
            conditions: vec![HistoryCondition {
                slot: "s0".into(),
                policy_id: "deep".into(),
                scope: Scope::Session,
                window_ms: 1,
                shape: Shape::CountWithin {
                    predicate,
                    op: CmpOp::Ge,
                    threshold: 1,
                },
            }],
        })));
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!((rejections[0].reason, rejections[0].policy_id.as_deref()), (RejectionReason::PolicyInvalid, Some("deep")));
        assert!(rejections[0].message.contains("the limit is 32"), "{}", rejections[0].message);
        assert_eq!((h.repo.insert_attempts(), h.uow.commits()), (0, 0));
    }

    /// A compile result that holds one count_within condition for each id in `ids`.
    fn output_with_history(cedar: &str, schema: serde_json::Value, ids: &[&str]) -> CompiledOutput {
        use paigasus_iam_core::tool_policy::protocol::{CmpOp, HistoryCondition, Predicate, Scope, Shape};
        CompiledOutput {
            cedar_policies: cedar.to_owned(),
            cedar_schema_json: schema,
            conditions: ids
                .iter()
                .enumerate()
                .map(|(i, id)| HistoryCondition {
                    slot: format!("s{i}"),
                    policy_id: (*id).to_owned(),
                    scope: Scope::Session,
                    window_ms: 1,
                    shape: Shape::CountWithin {
                        predicate: Predicate::AnyCatalogTool {},
                        op: CmpOp::Ge,
                        threshold: 1,
                    },
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn a_tenant_save_with_a_history_condition_is_rejected_and_stores_nothing() {
        let h = harness();
        h.compiler
            .push(Ok(CompileOutcome::Accepted(output_with_history(STARTER_TOOL_POLICY_SOURCE, tenant_schema_json(&[]), &["rate"]))));
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!((rejections[0].reason, rejections[0].policy_id.as_deref()), (RejectionReason::HistoryUnsupported, Some("rate")));
        assert_eq!((rejections[0].line, rejections[0].column), (None, None));
        assert_eq!(
            rejections[0].message,
            "policy rate uses a history rule (formerly, count_within or sum_within). The gateway cannot enforce history rules yet, so this set cannot be saved. Remove the temporal block, or wait for history support."
        );
        assert_eq!((h.repo.writes(), h.repo.insert_attempts(), h.uow.commits()), (0, 0, 0));
        assert!(audit_actions(&h).is_empty());
    }

    #[tokio::test]
    async fn a_floor_save_with_a_history_condition_is_rejected_and_stores_nothing() {
        let h = harness();
        h.compiler.push(Ok(CompileOutcome::Accepted(output_with_history(FLOOR_DENY, base_schema_json(), &["floor-rate"]))));
        let rejections = rejected(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!((rejections[0].reason, rejections[0].policy_id.as_deref()), (RejectionReason::HistoryUnsupported, Some("floor-rate")));
        assert_eq!((h.repo.writes(), h.repo.insert_attempts(), h.uow.commits()), (0, 0, 0));
        assert!(audit_actions(&h).is_empty());
    }

    #[tokio::test]
    async fn a_set_with_no_history_condition_still_saves() {
        let h = harness();
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        assert_eq!(v.version, 1);
        assert!(decode_compiled(&v.compiled).unwrap().conditions.is_empty());
    }

    #[tokio::test]
    async fn each_policy_with_a_history_condition_gets_one_rejection() {
        let h = harness();
        h.compiler
            .push(Ok(CompileOutcome::Accepted(output_with_history(STARTER_TOOL_POLICY_SOURCE, tenant_schema_json(&[]), &["b", "a", "a"]))));
        let rejections = rejected(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let ids: Vec<Option<&str>> = rejections.iter().map(|r| r.policy_id.as_deref()).collect();
        assert_eq!(ids, vec![Some("a"), Some("b")]);
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

    /// The size that the cap check computes for a put of `search` into an empty catalog: one
    /// `ListToolsResponse` with revision 1 and the largest encoding of the write time.
    fn search_put_size() -> usize {
        let entry = build_manual("search", ToolKind::Function, Some(search_schema()), &CatalogLimits::default()).unwrap();
        let at = catalog::size_check_time(FixedClock::default().now());
        let stored = StoredTool {
            entry,
            created_at: at,
            updated_at: at,
        };
        wire::catalog_response_len(&[stored], 1)
    }

    /// Fix 2 (local review pass 2): a write that makes the catalog exactly the cap succeeds.
    #[tokio::test]
    async fn a_catalog_write_at_the_size_cap_is_accepted() {
        let size = search_put_size();
        let h = bare_harness_with_cap(size);
        for action in [Action::ManageToolCatalog, Action::ReadToolCatalog] {
            h.authz.allow(action, &h.org_prn);
        }
        let out = put_search(&h, 0).await;
        assert_eq!(out.catalog_revision, 1);
        // The stored catalog is not larger than the check measured.
        let (tools, revision) = h.svc.list_tools(&h.actor, h.org, Page { limit: 10, offset: 0 }).await.unwrap();
        assert!(wire::catalog_response_len(&tools, revision) <= size);
    }

    /// Fix 2: a write one byte over the cap is a `tool-catalog-invalid` rejection that names the
    /// size and the cap, and it stores nothing.
    #[tokio::test]
    async fn a_catalog_write_over_the_size_cap_is_rejected() {
        let size = search_put_size();
        let h = bare_harness_with_cap(size - 1);
        h.authz.allow(Action::ManageToolCatalog, &h.org_prn);
        let rejections = rejected(h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, Some(search_schema()), 0).await);
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].reason, RejectionReason::CatalogInvalid);
        let message = &rejections[0].message;
        assert!(message.contains(&format!("{size}-byte")) && message.contains(&format!("limit is {} bytes", size - 1)), "{message}");
        assert_eq!(h.repo.writes(), 0);
        assert_eq!(h.compiler.calls(), 0);
    }

    /// Fix 2: a catalog filled to the default cap encodes below 4 MiB as one `ListToolsResponse`,
    /// so every `ListTools` page decodes in a default tonic client.
    #[tokio::test]
    async fn a_catalog_at_the_default_cap_encodes_below_4_mib() {
        let h = harness();
        // Each tool has a schema just below the 64 KiB limit.
        let props: serde_json::Map<String, serde_json::Value> = (0..2400).map(|i| (format!("p{i:05}"), json!({"type": "string"}))).collect();
        let schema = json!({"type": "object", "properties": props});
        assert!(schema.to_string().len() < CatalogLimits::default().max_input_schema_bytes);
        let mut revision = 0;
        let mut rejected_at = None;
        for i in 0..200 {
            match h.svc.put_tool(&h.actor, h.org, &format!("t{i:03}"), ToolKind::Function, Some(schema.clone()), revision).await.unwrap() {
                WriteResult::Accepted(a) => revision = a.catalog_revision,
                WriteResult::Rejected(r) => {
                    assert_eq!(r[0].reason, RejectionReason::CatalogInvalid, "{r:?}");
                    rejected_at = Some(i);
                    break;
                }
            }
        }
        assert!(rejected_at.is_some(), "the cap never stopped a write");
        let (tools, rev) = h.svc.list_tools(&h.actor, h.org, Page { limit: 1000, offset: 0 }).await.unwrap();
        let full = wire::catalog_response_len(&tools, rev);
        let one = full / tools.len();
        assert!(full <= DEFAULT_CATALOG_CAP && full + one > DEFAULT_CATALOG_CAP, "the catalog is not full: {full} bytes, {one} per tool");
        assert!(full < crate::config::TONIC_DEFAULT_DECODE_LIMIT_BYTES, "{full} bytes");
    }

    /// Fix 3 (local review pass 2): a catalog write that commits during `list_tools` is in both
    /// the tools and the revision, or in neither. Two reads gave the new tool with the old
    /// revision.
    #[tokio::test]
    async fn list_tools_returns_the_tools_and_the_revision_of_one_observation() {
        let h = harness();
        put_search(&h, 0).await;
        let mut extra = h.svc.list_tools(&h.actor, h.org, Page { limit: 10, offset: 0 }).await.unwrap().0.remove(0);
        extra.entry.name = paigasus_iam_core::tool_policy::ToolName {
            display: "fetch".to_string(),
            name_key: "fetch".to_string(),
        };
        h.repo.write_before_next_catalog_read(h.org, extra);
        let (tools, revision) = h.svc.list_tools(&h.actor, h.org, Page { limit: 10, offset: 0 }).await.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.entry.name.name_key.as_str()).collect();
        assert_eq!((names, revision), (vec!["fetch", "search"], 2), "the tools and the revision come from two observations");
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
    async fn a_catalog_write_over_a_live_history_set_is_rejected_with_the_history_code() {
        // F17: a live set that PR 1 stored with a history rule. The coupling compile of a catalog
        // write gives the history rule again, so R19 refuses it as `tool-catalog-breaks-policy`.
        let h = harness();
        seed_live_tenant(&h, STARTER_TOOL_POLICY_SOURCE, 1, TOOL_SCHEMA_REVISION);
        h.compiler
            .push(Ok(CompileOutcome::Accepted(output_with_history(STARTER_TOOL_POLICY_SOURCE, tenant_schema_json(&[]), &["rate"]))));
        let rejections = rejected(h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, Some(search_schema()), 0).await);
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!((rejections[0].reason, rejections[0].policy_id.as_deref()), (RejectionReason::CatalogBreaksPolicy, Some("rate")));
        assert!(rejections[0].message.ends_with("(tool-policy-history-unsupported)"), "{}", rejections[0].message);
        assert_eq!(h.compiler.calls(), 1, "the coupling compile ran");
        assert_eq!((h.repo.writes(), h.uow.commits()), (0, 0));
        assert!(h.repo.catalog(h.org).await.unwrap().is_empty(), "the catalog write stored nothing");
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

    fn events(h: &Harness) -> Vec<paigasus_iam_core::DomainEvent> {
        h.outbox.0.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_tenant_save_writes_one_set_published_event() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let ev = events(&h);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].event_type.as_wire(), "iam.policy.tool_set.published");
        assert_eq!(ev[0].aggregate_prn, h.org_prn.canonical());
        assert_eq!(ev[0].schema_version, 1);
        assert_eq!(ev[0].actor_prn, Some(h.actor.canonical()));
        assert_eq!(ev[0].payload, json!({"org_id": h.org.to_string(), "version": 1, "origin": "save"}));
    }

    #[tokio::test]
    async fn a_floor_save_writes_one_floor_published_event() {
        let h = harness();
        accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        let ev = events(&h);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].event_type.as_wire(), "iam.policy.tool_floor.published");
        assert_eq!(ev[0].aggregate_prn, root_prn().canonical());
        assert_eq!(ev[0].payload, json!({"version": 1, "origin": "save"}));
    }

    #[tokio::test]
    async fn a_catalog_change_version_writes_a_set_published_event() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        put_search(&h, 0).await;
        let ev = events(&h);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[1].event_type.as_wire(), "iam.policy.tool_set.published");
        assert_eq!(ev[1].aggregate_prn, h.org_prn.canonical());
        assert_eq!(ev[1].payload, json!({"org_id": h.org.to_string(), "version": 2, "origin": "catalog_change"}));
    }

    #[tokio::test]
    async fn a_catalog_write_with_identical_output_writes_no_event() {
        let h = harness();
        put_search(&h, 0).await;
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let before = events(&h).len();
        let out = put_search(&h, 1).await;
        assert_eq!(out.set_version, 0);
        assert_eq!(events(&h).len(), before);
    }

    #[tokio::test]
    async fn a_byte_identical_save_writes_no_event() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 1).await);
        assert_eq!(events(&h).len(), 1);
    }

    #[tokio::test]
    async fn a_rejected_or_conflicting_write_writes_no_event() {
        let h = harness();
        h.compiler.push(Ok(CompileOutcome::Accepted(output_with_history(FLOOR_DENY, base_schema_json(), &["floor-rate"]))));
        rejected(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        assert!(events(&h).is_empty(), "a rejection");
        assert!(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 5).await.is_err());
        assert!(events(&h).is_empty(), "a stale expected version");
        h.repo.change_before_next_lock(ConcurrentChange::LiveVersion);
        assert_eq!(
            h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await.unwrap_err(),
            TenancyError::ToolPolicyVersionConflict
        );
        assert!(events(&h).is_empty(), "a conflict between compile and lock");
    }

    #[tokio::test]
    async fn the_event_and_the_audit_entry_share_one_correlation_id() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        put_search(&h, 0).await;
        let (ev, entries) = (events(&h), h.audit.0.lock().unwrap().clone());
        assert_eq!(ev.len(), 3);
        assert_eq!(entries.len(), 3);
        for (e, a) in ev.iter().zip(entries.iter()) {
            assert!(e.correlation_id.is_some());
            assert_eq!(e.correlation_id, a.correlation_id);
        }
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

    // ---- SMA-775: the settings ----

    use paigasus_iam_core::tool_policy::{FailMode, ToolPolicyMode};

    async fn put_mode(h: &Harness, mode: ToolPolicyMode, fail_mode: FailMode) -> Result<SettingsAccepted, TenancyError> {
        h.svc.put_settings(&h.actor, h.org, mode, fail_mode).await
    }

    #[tokio::test]
    async fn get_settings_of_an_org_with_no_row_is_off_and_closed() {
        let h = harness();
        let s = h.svc.get_settings(&h.actor, h.org).await.unwrap();
        assert_eq!(s, ToolPolicySettings::default_for_absent_row());
    }

    #[tokio::test]
    async fn the_first_enable_to_log_only_saves_the_starter_as_version_1() {
        let h = harness();
        let r = put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Open).await.unwrap();
        assert_eq!(r.starter_version, 1);
        assert_eq!((r.settings.mode, r.settings.fail_mode), (ToolPolicyMode::LogOnly, FailMode::Open));
        let live = h.repo.live(&tenant(&h)).await.unwrap().unwrap();
        assert_eq!((live.version, live.origin, live.status), (1, VersionOrigin::Starter, VersionStatus::Live));
        assert_eq!(live.source, STARTER_TOOL_POLICY_SOURCE);
        assert_eq!(live.created_by, h.actor.canonical());
        assert_eq!(h.svc.get_settings(&h.actor, h.org).await.unwrap(), r.settings);
    }

    #[tokio::test]
    async fn the_first_enable_writes_two_events_and_one_audit_entry() {
        let h = harness();
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        let ev = events(&h);
        let wire: Vec<&str> = ev.iter().map(|e| e.event_type.as_wire()).collect();
        assert_eq!(wire, vec!["iam.policy.tool_set.published", "iam.policy.tool_settings.changed"]);
        assert_eq!(ev[0].payload, json!({"org_id": h.org.to_string(), "version": 1, "origin": "starter"}));
        assert_eq!(ev[1].payload, json!({"org_id": h.org.to_string(), "mode": "log_only", "fail_mode": "closed"}));
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "PutToolPolicySettings");
        assert_eq!(entries[0].resource_prn, Some(h.org_prn.canonical()));
        assert_eq!(entries[0].detail, json!({"mode": "log_only", "fail_mode": "closed", "starter_version": 1}));
        for e in &ev {
            assert!(e.correlation_id.is_some());
            assert_eq!(e.correlation_id, entries[0].correlation_id);
        }
        assert_eq!(h.uow.commits(), 1);
    }

    #[tokio::test]
    async fn off_to_enforce_with_no_set_is_a_mode_transition_error_and_writes_nothing() {
        let h = harness();
        assert_eq!(
            put_mode(&h, ToolPolicyMode::Enforce, FailMode::Closed).await.unwrap_err(),
            TenancyError::ToolPolicyModeTransitionInvalid
        );
        assert_eq!((h.repo.writes(), events(&h).len(), h.audit.0.lock().unwrap().len(), h.compiler.calls()), (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn off_to_enforce_with_a_set_saved_in_off_is_a_mode_transition_error() {
        // I-2 (strict spec 9.2): the first change away from `off` must be `log_only`, also when
        // the org saved a set while in `off`.
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let (writes, ev, audit) = (h.repo.writes(), events(&h).len(), h.audit.0.lock().unwrap().len());
        let err = put_mode(&h, ToolPolicyMode::Enforce, FailMode::Closed).await.unwrap_err();
        assert_eq!(err, TenancyError::ToolPolicyModeTransitionInvalid);
        assert_eq!(err.code(), "tool-policy-mode-transition-invalid", "reason 61");
        assert_eq!((h.repo.writes(), events(&h).len(), h.audit.0.lock().unwrap().len()), (writes, ev, audit));
        assert_eq!(h.repo.settings(h.org).await.unwrap(), None);
    }

    #[tokio::test]
    async fn off_to_enforce_after_an_earlier_enable_with_a_set_is_allowed_and_saves_no_starter() {
        let h = harness();
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        put_mode(&h, ToolPolicyMode::Off, FailMode::Closed).await.unwrap();
        let r = put_mode(&h, ToolPolicyMode::Enforce, FailMode::Closed).await.unwrap();
        assert_eq!((r.settings.mode, r.starter_version), (ToolPolicyMode::Enforce, 0));
        assert_eq!(h.repo.versions(&tenant(&h)).len(), 1, "only the starter of the first enable");
    }

    #[tokio::test]
    async fn the_first_enable_sets_first_enabled_at_and_a_later_write_keeps_it() {
        let h = harness();
        put_mode(&h, ToolPolicyMode::Off, FailMode::Closed).await.unwrap();
        assert_eq!(h.repo.settings(h.org).await.unwrap().unwrap().first_enabled_at, None, "off is not an enable");
        let first = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + chrono::Duration::seconds(100);
        h.clock.set(first);
        let r = put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        assert_eq!(r.settings.first_enabled_at, Some(first));
        assert_eq!(h.repo.settings(h.org).await.unwrap().unwrap().first_enabled_at, Some(first));
        for (secs, mode) in [(200, ToolPolicyMode::Enforce), (300, ToolPolicyMode::Off), (400, ToolPolicyMode::LogOnly)] {
            h.clock.set(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs));
            let r = put_mode(&h, mode, FailMode::Open).await.unwrap();
            assert_eq!(r.settings.first_enabled_at, Some(first), "{mode:?}");
            assert_eq!(h.repo.settings(h.org).await.unwrap().unwrap().first_enabled_at, Some(first), "{mode:?}");
        }
    }

    #[tokio::test]
    async fn log_only_with_an_existing_set_saves_no_starter() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let calls = h.compiler.calls();
        let r = put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        assert_eq!(r.starter_version, 0);
        assert_eq!(h.compiler.calls(), calls);
        assert_eq!(h.repo.versions(&tenant(&h)).len(), 1);
        // The save wrote one event; the settings write wrote one more, and no set event.
        let wire: Vec<&str> = events(&h).iter().map(|e| e.event_type.as_wire()).collect();
        assert_eq!(wire, vec!["iam.policy.tool_set.published", "iam.policy.tool_settings.changed"]);
        assert_eq!(h.audit.0.lock().unwrap().last().unwrap().detail["starter_version"], json!(null));
    }

    #[tokio::test]
    async fn a_fail_mode_change_alone_is_allowed_in_every_mode() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        for mode in [ToolPolicyMode::Off, ToolPolicyMode::LogOnly, ToolPolicyMode::Enforce] {
            for fail in [FailMode::Open, FailMode::Closed] {
                let r = put_mode(&h, mode, fail).await.unwrap();
                assert_eq!((r.settings.mode, r.settings.fail_mode, r.starter_version), (mode, fail, 0));
            }
        }
    }

    #[tokio::test]
    async fn any_mode_can_go_to_off() {
        let h = harness();
        // From the absent row.
        put_mode(&h, ToolPolicyMode::Off, FailMode::Open).await.unwrap();
        assert_eq!(h.repo.versions(&tenant(&h)).len(), 0);
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Open).await.unwrap();
        put_mode(&h, ToolPolicyMode::Enforce, FailMode::Open).await.unwrap();
        put_mode(&h, ToolPolicyMode::Off, FailMode::Open).await.unwrap();
        assert_eq!(h.svc.get_settings(&h.actor, h.org).await.unwrap().mode, ToolPolicyMode::Off);
    }

    #[tokio::test]
    async fn a_set_saved_during_the_first_enable_is_a_version_conflict() {
        let h = harness();
        h.repo.change_before_next_lock(ConcurrentChange::LiveVersion);
        assert_eq!(put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap_err(), TenancyError::ToolPolicyVersionConflict);
        assert_eq!(
            (h.repo.writes(), h.repo.insert_attempts(), events(&h).len(), h.audit.0.lock().unwrap().len(), h.uow.commits()),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(h.repo.settings(h.org).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_busy_compiler_during_the_first_enable_writes_nothing() {
        let h = harness();
        h.compiler.push(Err(CompilerFailure::Busy));
        assert_eq!(put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap_err(), TenancyError::ToolPolicyCompilerBusy);
        assert_eq!((h.repo.writes(), events(&h).len(), h.audit.0.lock().unwrap().len(), h.uow.commits()), (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn a_rejected_starter_is_an_internal_error_and_writes_nothing() {
        let h = harness();
        h.compiler
            .push(Ok(CompileOutcome::Rejected(vec![compiler_rejection(None, RejectionReason::PolicyInvalid.code(), "x")])));
        assert_eq!(put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap_err(), TenancyError::Internal);
        assert_eq!((h.repo.writes(), events(&h).len(), h.audit.0.lock().unwrap().len()), (0, 0, 0));
    }

    #[tokio::test]
    async fn a_settings_write_with_no_starter_takes_no_state_lock() {
        let h = harness();
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        // I-2: `enforce` needs an earlier change away from `off`.
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        // A concurrent change would conflict if the write locked and compared the state.
        h.repo.change_before_next_lock(ConcurrentChange::CatalogRevision);
        put_mode(&h, ToolPolicyMode::Enforce, FailMode::Closed).await.unwrap();
    }

    #[tokio::test]
    async fn a_write_with_the_stored_values_still_writes() {
        let h = harness();
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        let (ev, audit) = (events(&h).len(), h.audit.0.lock().unwrap().len());
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.unwrap();
        assert_eq!((events(&h).len(), h.audit.0.lock().unwrap().len()), (ev + 1, audit + 1));
    }

    #[tokio::test]
    async fn the_settings_methods_check_their_actions() {
        for write in [false, true] {
            let h = bare_harness();
            let (result, action) = if write {
                (put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Closed).await.map(|_| ()), Action::ManageToolPolicySettings)
            } else {
                (h.svc.get_settings(&h.actor, h.org).await.map(|_| ()), Action::ReadToolPolicies)
            };
            assert_eq!(result, Err(TenancyError::Forbidden));
            assert_eq!(h.authz.checks(), vec![(action, h.org_prn.canonical())]);
            assert_eq!((h.compiler.calls(), h.repo.writes(), h.audit.0.lock().unwrap().len()), (0, 0, 0));
        }
    }

    #[tokio::test]
    async fn an_unknown_org_is_not_found() {
        let h = harness();
        let absent = Uuid::from_u128(999);
        assert_eq!(h.svc.get_settings(&h.actor, absent).await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(h.svc.put_settings(&h.actor, absent, ToolPolicyMode::Off, FailMode::Closed).await.unwrap_err(), TenancyError::NotFound);
    }

    // ---- SMA-775: the compiled-policy fetch ----

    fn allow_compiled(h: &Harness) {
        h.authz.allow(Action::ReadCompiledToolPolicies, &h.org_prn);
        h.authz.allow(Action::ReadCompiledToolPolicies, &root_prn());
    }

    #[tokio::test]
    async fn get_compiled_of_an_org_with_no_set_is_none_with_default_settings() {
        let h = bare_harness();
        allow_compiled(&h);
        let r = h.svc.get_compiled(&h.actor, h.org, None).await.unwrap();
        assert_eq!(r.set, None);
        assert_eq!(r.settings, ToolPolicySettings::default_for_absent_row());
        assert_eq!(r.floor_version, 0);
    }

    #[tokio::test]
    async fn get_compiled_returns_the_live_set_the_settings_and_the_floor_version() {
        let h = harness();
        allow_compiled(&h);
        let v = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        // I-2: `enforce` needs an earlier change away from `off`.
        put_mode(&h, ToolPolicyMode::LogOnly, FailMode::Open).await.unwrap();
        put_mode(&h, ToolPolicyMode::Enforce, FailMode::Open).await.unwrap();
        let r = h.svc.get_compiled(&h.actor, h.org, None).await.unwrap();
        assert_eq!(r.set, Some(v));
        assert_eq!((r.settings.mode, r.settings.fail_mode), (ToolPolicyMode::Enforce, FailMode::Open));
        assert_eq!(r.floor_version, 1);
    }

    #[tokio::test]
    async fn get_compiled_by_version_returns_a_superseded_version() {
        let h = harness();
        allow_compiled(&h);
        let v1 = accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        let other = format!("{STARTER_TOOL_POLICY_SOURCE}\n");
        let v2 = accepted(h.svc.put_set(&h.actor, h.org, &other, 1).await);
        assert_eq!(v2.version, 2);
        let r = h.svc.get_compiled(&h.actor, h.org, Some(1)).await.unwrap();
        assert_eq!(r.set.as_ref().map(|s| (s.version, s.status)), Some((1, VersionStatus::Superseded)));
        assert_eq!(r.set.unwrap().compiled, v1.compiled);
        assert_eq!(h.svc.get_compiled(&h.actor, h.org, Some(9)).await.unwrap_err(), TenancyError::ToolPolicyVersionNotFound);
        assert_eq!(h.svc.get_compiled_floor(&h.actor, Some(9)).await.unwrap_err(), TenancyError::ToolPolicyVersionNotFound);
    }

    #[tokio::test]
    async fn the_fetch_of_an_org_in_mode_off_returns_the_floor_version() {
        let h = harness();
        allow_compiled(&h);
        accepted(h.svc.put_set(&h.actor, h.org, STARTER_TOOL_POLICY_SOURCE, 0).await);
        accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        let other = format!("{FLOOR_DENY}\n");
        accepted(h.svc.put_floor(&h.actor, &other, 1).await);
        let r = h.svc.get_compiled(&h.actor, h.org, None).await.unwrap();
        assert_eq!(r.settings.mode, ToolPolicyMode::Off);
        assert_eq!(r.floor_version, 2);
        assert_eq!(r.set.map(|s| s.version), Some(1));
    }

    #[tokio::test]
    async fn get_compiled_floor_with_no_floor_is_none() {
        let h = bare_harness();
        allow_compiled(&h);
        assert_eq!(h.svc.get_compiled_floor(&h.actor, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn get_compiled_floor_returns_the_live_floor() {
        let h = harness();
        allow_compiled(&h);
        let v = accepted(h.svc.put_floor(&h.actor, FLOOR_DENY, 0).await);
        assert_eq!(h.svc.get_compiled_floor(&h.actor, None).await.unwrap(), Some(v.clone()));
        assert_eq!(h.svc.get_compiled_floor(&h.actor, Some(1)).await.unwrap(), Some(v));
    }

    #[tokio::test]
    async fn the_fetch_methods_check_read_compiled_tool_policies() {
        let h = bare_harness();
        assert_eq!(h.svc.get_compiled(&h.actor, h.org, None).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(h.svc.get_compiled_floor(&h.actor, None).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(
            h.authz.checks(),
            vec![(Action::ReadCompiledToolPolicies, h.org_prn.canonical()), (Action::ReadCompiledToolPolicies, root_prn().canonical())]
        );
        // A grant of the other read actions does not allow the fetch.
        h.authz.allow(Action::ReadToolPolicies, &h.org_prn);
        h.authz.allow(Action::ReadToolPolicyFloor, &root_prn());
        assert_eq!(h.svc.get_compiled(&h.actor, h.org, None).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(h.svc.get_compiled_floor(&h.actor, None).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!((h.repo.writes(), h.audit.0.lock().unwrap().len(), h.uow.commits()), (0, 0, 0));
    }

    #[tokio::test]
    async fn get_compiled_of_an_absent_org_is_not_found() {
        let h = harness();
        allow_compiled(&h);
        assert_eq!(h.svc.get_compiled(&h.actor, Uuid::from_u128(999), None).await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn get_compiled_reads_the_settings_before_the_set_and_the_floor_version() {
        let h = bare_harness();
        allow_compiled(&h);
        h.svc.get_compiled(&h.actor, h.org, None).await.unwrap();
        assert_eq!(h.repo.read_log(), vec!["settings", "live", "state"]);
    }
}
