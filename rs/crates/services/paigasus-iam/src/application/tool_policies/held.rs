// SPDX-License-Identifier: Apache-2.0

//! The held versions of the tool policy sets: list, accept and discard (SMA-781, spec section 8).
//!
//! The re-compile job stores a held row when a live set compiles to a different result at a new
//! compiler generation or schema revision. A held row is a proposal: the gateway never reads it.
//! An admin accepts it (it becomes a new live version) or discards it.
//!
//! **Authorization (H10).** The list of one tenant needs `ReadToolPolicies` at the org. The list
//! of every set needs `ManageToolPolicyRecompile` at `root_prn()`. Accept and discard of a
//! tenant row need `ManageToolPolicies` at the org. Accept and discard of a floor row need
//! `ManageToolPolicyFloor` at `root_prn()`.
//!
//! **A discard stays on record (Q1).** The row keeps its `base_version`, generation and schema
//! revision with the status `Discarded`. The re-compile job reads it through `proposal_exists`
//! and does not propose the same result again.

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::tool_policy::{HeldStatus, HeldVersion, TOOL_SCHEMA_REVISION, ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus};
use paigasus_iam_core::{Action, Clock, IdGenerator};
use paigasus_kernel::Prn;
use serde_json::json;
use uuid::Uuid;

use super::sets::restamp_version;
use super::{Author, SetTarget, ToolPolicyService, origin_str};
use crate::application::error::TenancyError;
use crate::application::pagination::Page;

/// The set that a held-row RPC names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldTarget {
    Tenant(Uuid),
    Floor,
}

impl<I: IdGenerator, C: Clock> ToolPolicyService<I, C> {
    /// One page of the held rows (status held only) of one tenant, or of every set when `org` is
    /// `None`, in the repository order (the floor first, then by org UUID). The repository
    /// applies the page.
    pub async fn list_held(&self, actor: &Prn, org: Option<Uuid>, page: Page) -> Result<Vec<HeldVersion>, TenancyError> {
        match org {
            Some(org) => {
                self.authorize_org(actor, org, Action::ReadToolPolicies).await?;
            }
            None => self.authorize.check(actor, Action::ManageToolPolicyRecompile, &root_prn()).await?,
        }
        Ok(self.repo.list_held(org, page.limit, page.offset).await?)
    }

    /// Makes the held row `held_version` the next live version of its set (spec section 8, H9).
    /// The compile output is the stored one, with its `version` field set to the new number. No
    /// compile runs.
    pub async fn accept_held(&self, actor: &Prn, target: HeldTarget, held_version: u64) -> Result<ToolPolicyVersion, TenancyError> {
        let target = self.held_target(actor, target).await?;

        // Step 1: the state and the held row, then the checks in the H9 order.
        let state = self.repo.state(&target.key).await?;
        let held = self.held_row(&target.key, held_version).await?;
        let Some(compiled) = &held.compiled else {
            return Err(TenancyError::ToolPolicyHeldNotAcceptable);
        };
        if held.compiler_generation != self.compiler.generation()
            || held.schema_revision != TOOL_SCHEMA_REVISION
            || held.base_version != state.live_version
            || (matches!(target.key, ToolPolicySetKey::Tenant(_)) && held.catalog_revision != Some(state.catalog_revision))
        {
            return Err(TenancyError::ToolPolicyVersionConflict);
        }

        // Steps 2 to 4: lock and recheck, then write the version and its event. No compile.
        let author = Author::of(Some(actor));
        let tx = self.uow.begin().await?;
        let locked = self.lock_and_recheck(&*tx, &target.key, &state).await?;
        // A discard moves neither counter. Take the row under the lock: a row that another admin
        // discarded since the read is not found, and a new held row cannot be discarded by H7
        // while a discarded one goes live.
        if !self.repo.discard_held_in(&*tx, &target.key, held_version).await? {
            return Err(TenancyError::ToolPolicyVersionNotFound);
        }
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let number = locked.next_version();
        let version = ToolPolicyVersion {
            key: target.key,
            version: number,
            source: held.source.clone(),
            compiled: restamp_version(compiled, number)?,
            compiler_generation: held.compiler_generation,
            schema_revision: held.schema_revision,
            catalog_revision: held.catalog_revision,
            status: VersionStatus::Live,
            origin: VersionOrigin::Accept,
            created_by: author.created_by.clone(),
            created_at: now,
        };
        self.store_live_version(&*tx, &author, &target, &version, corr, now).await?;
        let detail = json!({
            "set": target.audit_set(),
            "version": number,
            "origin": origin_str(VersionOrigin::Accept),
            "held_version": held.version,
            "base_version": held.base_version,
        });
        self.audit
            .record(&*tx, &self.audit_entry(&author, "AcceptHeldToolPolicyVersion", &target.resource, detail, now, corr))
            .await?;
        tx.commit().await?;
        Ok(version)
    }

    /// Discards the held row `held_version`. Returns the row with the status `Discarded`. The row
    /// stays in the store, so the re-compile job does not propose the same result again (Q1).
    /// Writes no event: a held row and a discard change nothing that the gateway reads.
    pub async fn discard_held(&self, actor: &Prn, target: HeldTarget, held_version: u64) -> Result<HeldVersion, TenancyError> {
        let target = self.held_target(actor, target).await?;
        let mut held = self.held_row(&target.key, held_version).await?;

        let tx = self.uow.begin().await?;
        self.repo.lock_state_in(&*tx, &target.key).await?;
        // A save or an accept that committed since the read discarded the row: not found.
        if !self.repo.discard_held_in(&*tx, &target.key, held_version).await? {
            return Err(TenancyError::ToolPolicyVersionNotFound);
        }
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let detail = json!({
            "set": target.audit_set(),
            "held_version": held.version,
            "base_version": held.base_version,
        });
        let entry = self.audit_entry(&Author::of(Some(actor)), "DiscardHeldToolPolicyVersion", &target.resource, detail, now, corr);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        held.status = HeldStatus::Discarded;
        Ok(held)
    }

    /// Authorizes a held-row write (H10) and names the set.
    async fn held_target(&self, actor: &Prn, target: HeldTarget) -> Result<SetTarget, TenancyError> {
        match target {
            HeldTarget::Tenant(org) => {
                let resource = self.authorize_org(actor, org, Action::ManageToolPolicies).await?;
                Ok(SetTarget {
                    key: ToolPolicySetKey::Tenant(org),
                    resource,
                })
            }
            HeldTarget::Floor => {
                let resource = root_prn();
                self.authorize.check(actor, Action::ManageToolPolicyFloor, &resource).await?;
                Ok(SetTarget {
                    key: ToolPolicySetKey::Floor,
                    resource,
                })
            }
        }
    }

    /// The held row of the set when it has this number. An absent row, or another number, is
    /// `ToolPolicyVersionNotFound`.
    async fn held_row(&self, key: &ToolPolicySetKey, held_version: u64) -> Result<HeldVersion, TenancyError> {
        match self.repo.held(key).await? {
            Some(held) if held.version == held_version => Ok(held),
            _ => Err(TenancyError::ToolPolicyVersionNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Harness, accepted, audit_actions, bare_harness, harness, seed_live_tenant, tenant};
    use super::*;
    use crate::application::fakes::ConcurrentChange;
    use crate::application::tool_policies::WriteResult;
    use crate::application::tool_policies::sets::{CompiledMeta, decode_compiled, encode_compiled};
    use paigasus_iam_core::tool_policy::{CompiledOutput, TOOL_SCHEMA_REVISION, ToolKind, ToolPolicyRepository, failed_report, tenant_schema_json};
    use paigasus_iam_core::{DomainEvent, EventType};
    use serde_json::json;

    const BASE_SOURCE: &str = "@id(\"base\")\npermit(principal, action, resource);\n";
    const HELD_SOURCE: &str = "@id(\"held\")\nforbid(principal, action, resource);\n";

    /// A live tenant version 1 at the running generation (1) and a held version 2 that was
    /// compiled against catalog revision 0.
    fn live_and_held(h: &Harness) {
        seed_live_tenant(h, BASE_SOURCE, 1, TOOL_SCHEMA_REVISION);
        h.repo.seed_held(held_row(h, 2, 1));
    }

    fn held_row(h: &Harness, version: u64, generation: u32) -> HeldVersion {
        let out = CompiledOutput {
            cedar_policies: HELD_SOURCE.to_owned(),
            cedar_schema_json: tenant_schema_json(&[]),
            conditions: Vec::new(),
        };
        let meta = CompiledMeta {
            key: tenant(h),
            version,
            compiler_generation: generation,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision: Some(0),
            max_window_ms: 3_600_000,
            catalog_names: Vec::new(),
        };
        HeldVersion {
            key: tenant(h),
            version,
            base_version: 1,
            source: HELD_SOURCE.to_owned(),
            compiled: Some(encode_compiled(&meta, &out)),
            compiler_generation: generation,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision: Some(0),
            status: HeldStatus::Held,
            report: failed_report(Vec::new()),
            created_by: "system:tool-policy-recompile".to_owned(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        }
    }

    fn events(h: &Harness) -> Vec<DomainEvent> {
        h.outbox.0.lock().unwrap().clone()
    }

    /// Nothing was written: no commit, no event, no audit entry, no insert, and the live
    /// version is the same.
    fn assert_nothing_written(h: &Harness, live: u64) {
        assert_eq!(h.uow.commits(), 0);
        assert!(events(h).is_empty());
        assert!(audit_actions(h).is_empty());
        assert_eq!(h.repo.insert_attempts(), 0);
        assert_eq!(h.repo.versions(&tenant(h)).last().map(|v| v.version), Some(live));
    }

    #[tokio::test]
    async fn accept_writes_a_new_live_version_with_origin_accept() {
        let h = harness();
        live_and_held(&h);
        let v = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap();
        // The held row took number 2, so the accepted version is 3.
        assert_eq!((v.version, v.origin, v.status), (3, VersionOrigin::Accept, VersionStatus::Live));
        assert_eq!(v.source, HELD_SOURCE);
        assert_eq!((v.compiler_generation, v.schema_revision, v.catalog_revision), (1, TOOL_SCHEMA_REVISION, Some(0)));
        assert_eq!(v.created_by, h.actor.canonical());
        let stored = h.repo.live(&tenant(&h)).await.unwrap().unwrap();
        assert_eq!(stored, v);
        assert_eq!(decode_compiled(&stored.compiled).unwrap().cedar_policies, HELD_SOURCE);
        let restamped = restamp_check(&stored.compiled);
        assert_eq!(restamped, 3);
        let state = h.repo.state(&tenant(&h)).await.unwrap();
        assert_eq!((state.live_version, state.last_version), (3, 3));
        // The held row is discarded, and the first live version is superseded.
        let rows = h.repo.held_rows(&tenant(&h));
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].version, rows[0].status), (2, HeldStatus::Discarded));
        assert_eq!(h.repo.held(&tenant(&h)).await.unwrap(), None);
    }

    fn restamp_check(bytes: &[u8]) -> u64 {
        use prost::Message;
        paigasus_proto::paigasus::iam::v1::CompiledToolPolicySet::decode(bytes).unwrap().version
    }

    #[tokio::test]
    async fn accept_writes_one_event_and_one_audit_entry_with_one_correlation_id() {
        let h = harness();
        live_and_held(&h);
        h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap();
        let events = events(&h);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ToolPolicySetPublished);
        assert_eq!(events[0].payload, json!({"org_id": h.org.to_string(), "version": 3, "origin": "accept"}));
        assert_eq!(events[0].aggregate_prn, h.org_prn.canonical());
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "AcceptHeldToolPolicyVersion");
        assert_eq!(entries[0].resource_prn.as_deref(), Some(h.org_prn.canonical().as_str()));
        assert_eq!(entries[0].actor_prn.as_deref(), Some(h.actor.canonical().as_str()));
        assert_eq!(entries[0].detail, json!({"set": "tenant", "version": 3, "origin": "accept", "held_version": 2, "base_version": 1}));
        assert!(entries[0].correlation_id.is_some());
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
        assert_eq!(h.uow.commits(), 1);
    }

    #[tokio::test]
    async fn accepting_a_stale_held_row_is_a_version_conflict() {
        // The catalog revision moved after the job compiled.
        let h = harness();
        live_and_held(&h);
        h.repo.seed_live(ToolPolicyVersion {
            catalog_revision: Some(1),
            version: 1,
            ..h.repo.live(&tenant(&h)).await.unwrap().unwrap()
        });
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionConflict.code());
        assert_eq!(h.repo.state(&tenant(&h)).await.unwrap().catalog_revision, 1);
        assert_nothing_written(&h, 1);

        // The held generation is not the running one.
        let h = harness();
        seed_live_tenant(&h, BASE_SOURCE, 1, TOOL_SCHEMA_REVISION);
        h.repo.seed_held(held_row(&h, 2, 0));
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionConflict.code());
        assert_nothing_written(&h, 1);

        // The base is not the live version.
        let h = harness();
        live_and_held(&h);
        h.repo.seed_held(HeldVersion {
            base_version: 7,
            ..held_row(&h, 2, 1)
        });
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionConflict.code());
        assert_nothing_written(&h, 1);
    }

    #[tokio::test]
    async fn accepting_a_failed_held_row_is_held_not_acceptable() {
        let h = harness();
        seed_live_tenant(&h, BASE_SOURCE, 1, TOOL_SCHEMA_REVISION);
        h.repo.seed_held(HeldVersion { compiled: None, ..held_row(&h, 2, 1) });
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyHeldNotAcceptable.code());
        assert_nothing_written(&h, 1);
        // The row stays held.
        assert_eq!(h.repo.held(&tenant(&h)).await.unwrap().map(|r| r.version), Some(2));
    }

    #[tokio::test]
    async fn accepting_another_number_is_version_not_found() {
        let h = harness();
        live_and_held(&h);
        for number in [1, 3, 99] {
            let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), number).await.unwrap_err();
            assert_eq!(err.code(), TenancyError::ToolPolicyVersionNotFound.code());
        }
        // A set with no held row.
        let h = harness();
        seed_live_tenant(&h, BASE_SOURCE, 1, TOOL_SCHEMA_REVISION);
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionNotFound.code());
        assert_nothing_written(&h, 1);
    }

    #[tokio::test]
    async fn a_save_between_the_accept_read_and_the_lock_is_a_version_conflict() {
        let h = harness();
        live_and_held(&h);
        h.repo.change_before_next_lock(ConcurrentChange::LiveVersion);
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionConflict.code());
        assert_eq!(h.repo.insert_attempts(), 0);
        assert_eq!(h.uow.commits(), 0);
        assert!(events(&h).is_empty());
        assert!(audit_actions(&h).is_empty());

        let h = harness();
        live_and_held(&h);
        h.repo.change_before_next_lock(ConcurrentChange::CatalogRevision);
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionConflict.code());
        assert_eq!(h.repo.insert_attempts(), 0);
        assert_eq!(h.uow.commits(), 0);
    }

    #[tokio::test]
    async fn a_discard_between_the_accept_read_and_the_lock_is_version_not_found() {
        let h = harness();
        live_and_held(&h);
        h.repo.change_before_next_lock(ConcurrentChange::HeldDiscarded);
        let err = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionNotFound.code());
        assert_nothing_written(&h, 1);
        assert_eq!(h.repo.held_rows(&tenant(&h))[0].status, HeldStatus::Discarded);
    }

    #[tokio::test]
    async fn a_tenant_accept_and_discard_need_manage_tool_policies() {
        let h = bare_harness();
        h.authz.allow(Action::ReadToolPolicies, &h.org_prn);
        live_and_held(&h);
        let accept = h.svc.accept_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(accept.code(), TenancyError::Forbidden.code());
        let discard = h.svc.discard_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(discard.code(), TenancyError::Forbidden.code());
        assert_nothing_written(&h, 1);
        assert_eq!(h.repo.held(&tenant(&h)).await.unwrap().map(|r| r.version), Some(2));
    }

    #[tokio::test]
    async fn discard_sets_the_row_discarded_and_writes_one_audit_entry_and_no_event() {
        let h = harness();
        live_and_held(&h);
        let row = h.svc.discard_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap();
        assert_eq!((row.version, row.status, row.base_version), (2, HeldStatus::Discarded, 1));
        assert_eq!(h.repo.held(&tenant(&h)).await.unwrap(), None);
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "DiscardHeldToolPolicyVersion");
        assert_eq!(entries[0].resource_prn.as_deref(), Some(h.org_prn.canonical().as_str()));
        assert_eq!(entries[0].detail, json!({"set": "tenant", "held_version": 2, "base_version": 1}));
        assert!(events(&h).is_empty());
        assert_eq!(h.uow.commits(), 1);
        // The live set is unchanged.
        assert_eq!(h.repo.state(&tenant(&h)).await.unwrap().live_version, 1);
        assert_eq!(h.repo.insert_attempts(), 0);
        // Q1: the row stays on record with its base and revisions, so the job does not propose
        // the same result again.
        let rows = h.repo.held_rows(&tenant(&h));
        assert_eq!(rows, vec![row]);
        assert!(h.repo.proposal_exists(&tenant(&h), 1, 1, TOOL_SCHEMA_REVISION).await.unwrap());
        // A second discard of the same number is not found.
        let err = h.svc.discard_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionNotFound.code());
    }

    #[tokio::test]
    async fn discarding_another_number_is_version_not_found() {
        let h = harness();
        live_and_held(&h);
        let err = h.svc.discard_held(&h.actor, HeldTarget::Tenant(h.org), 3).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::ToolPolicyVersionNotFound.code());
        assert_eq!(h.uow.commits(), 0);
        assert!(audit_actions(&h).is_empty());
        assert_eq!(h.repo.held(&tenant(&h)).await.unwrap().map(|r| r.version), Some(2));
    }

    /// A live set of an older compiler generation, with a held row, that is then discarded.
    async fn stale_live_with_a_discarded_proposal() -> Harness {
        let h = harness();
        seed_live_tenant(&h, BASE_SOURCE, 0, TOOL_SCHEMA_REVISION);
        h.repo.seed_held(held_row(&h, 2, 1));
        h.svc.discard_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap();
        h
    }

    #[tokio::test]
    async fn after_a_discard_catalog_writes_stay_recompile_pending() {
        let h = stale_live_with_a_discarded_proposal().await;
        let result = h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 0).await;
        assert_eq!(result.unwrap_err().code(), TenancyError::ToolPolicyRecompilePending.code());
    }

    #[tokio::test]
    async fn after_a_discard_a_save_clears_recompile_pending() {
        let h = stale_live_with_a_discarded_proposal().await;
        let saved = accepted(h.svc.put_set(&h.actor, h.org, BASE_SOURCE, 1).await);
        // The held row took number 2, so the save takes 3.
        assert_eq!((saved.version, saved.compiler_generation), (3, 1));
        let result = h.svc.put_tool(&h.actor, h.org, "search", ToolKind::Function, None, 0).await;
        assert!(matches!(result, Ok(WriteResult::Accepted(_))), "{result:?}");
    }

    #[tokio::test]
    async fn a_floor_accept_needs_manage_tool_policy_floor_at_root() {
        let h = bare_harness();
        // An org admin holds the tenant actions at the org, and no floor action.
        h.authz.allow(Action::ManageToolPolicies, &h.org_prn);
        h.authz.allow(Action::ReadToolPolicies, &h.org_prn);
        let floor = ToolPolicySetKey::Floor;
        h.repo.seed_held(HeldVersion {
            key: floor,
            catalog_revision: None,
            ..held_row(&h, 2, 1)
        });
        let accept = h.svc.accept_held(&h.actor, HeldTarget::Floor, 2).await.unwrap_err();
        assert_eq!(accept.code(), TenancyError::Forbidden.code());
        let discard = h.svc.discard_held(&h.actor, HeldTarget::Floor, 2).await.unwrap_err();
        assert_eq!(discard.code(), TenancyError::Forbidden.code());
        assert_eq!(h.repo.held(&floor).await.unwrap().map(|r| r.version), Some(2));
        assert_eq!(h.uow.commits(), 0);

        // With the floor action at Root, the accept writes a floor version and a floor event.
        h.authz.allow(Action::ManageToolPolicyFloor, &root_prn());
        h.repo.seed_live(ToolPolicyVersion {
            key: floor,
            version: 1,
            source: BASE_SOURCE.to_owned(),
            compiled: Vec::new(),
            compiler_generation: 1,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision: None,
            status: VersionStatus::Live,
            origin: VersionOrigin::Save,
            created_by: h.actor.canonical(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        });
        let v = h.svc.accept_held(&h.actor, HeldTarget::Floor, 2).await.unwrap();
        assert_eq!((v.key, v.version, v.origin, v.catalog_revision), (floor, 3, VersionOrigin::Accept, None));
        let events = events(&h);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ToolFloorPublished);
        assert_eq!(events[0].payload, json!({"version": 3, "origin": "accept"}));
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries[0].resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(entries[0].detail["set"], json!("floor"));
    }

    #[tokio::test]
    async fn list_of_every_set_needs_manage_tool_policy_recompile() {
        let h = bare_harness();
        h.authz.allow(Action::ReadToolPolicies, &h.org_prn);
        h.authz.allow(Action::ManageToolPolicyFloor, &root_prn());
        h.repo.seed_held(held_row(&h, 2, 1));
        h.repo.seed_held(HeldVersion {
            key: ToolPolicySetKey::Floor,
            catalog_revision: None,
            ..held_row(&h, 2, 1)
        });
        let err = h.svc.list_held(&h.actor, None, Page::new(None, None).unwrap()).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::Forbidden.code());

        h.authz.allow(Action::ManageToolPolicyRecompile, &root_prn());
        let rows = h.svc.list_held(&h.actor, None, Page::new(None, None).unwrap()).await.unwrap();
        // The repository order: the floor first.
        assert_eq!(rows.iter().map(|r| r.key).collect::<Vec<_>>(), vec![ToolPolicySetKey::Floor, tenant(&h)]);
    }

    #[tokio::test]
    async fn list_of_one_tenant_needs_read_tool_policies_and_pages() {
        let h = bare_harness();
        h.repo.seed_held(held_row(&h, 2, 1));
        h.repo.seed_held(HeldVersion {
            key: ToolPolicySetKey::Floor,
            catalog_revision: None,
            ..held_row(&h, 2, 1)
        });
        let err = h.svc.list_held(&h.actor, Some(h.org), Page::new(None, None).unwrap()).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::Forbidden.code());

        h.authz.allow(Action::ReadToolPolicies, &h.org_prn);
        let rows = h.svc.list_held(&h.actor, Some(h.org), Page::new(None, None).unwrap()).await.unwrap();
        // Only the held row of this tenant, not the floor row.
        assert_eq!(rows.iter().map(|r| (r.key, r.version)).collect::<Vec<_>>(), vec![(tenant(&h), 2)]);
        // A page past the end is empty.
        let past = h.svc.list_held(&h.actor, Some(h.org), Page::new(Some(10), Some(1)).unwrap()).await.unwrap();
        assert!(past.is_empty());
        // A discarded row is not listed.
        h.authz.allow(Action::ManageToolPolicies, &h.org_prn);
        seed_live_tenant(&h, BASE_SOURCE, 1, TOOL_SCHEMA_REVISION);
        h.svc.discard_held(&h.actor, HeldTarget::Tenant(h.org), 2).await.unwrap();
        let after = h.svc.list_held(&h.actor, Some(h.org), Page::new(None, None).unwrap()).await.unwrap();
        assert!(after.is_empty());
    }

    /// The repository applies the page itself (the floor first, then tenants by org UUID), so a
    /// page does not read every held row.
    #[tokio::test]
    async fn the_repository_pages_the_held_rows_in_the_floor_first_order() {
        let h = bare_harness();
        let (low, high) = (Uuid::from_u128(1), Uuid::from_u128(2));
        for org in [high, low] {
            h.repo.seed_held(HeldVersion {
                key: ToolPolicySetKey::Tenant(org),
                ..held_row(&h, 2, 1)
            });
        }
        h.repo.seed_held(HeldVersion {
            key: ToolPolicySetKey::Floor,
            catalog_revision: None,
            ..held_row(&h, 2, 1)
        });
        let keys = |rows: Vec<HeldVersion>| rows.into_iter().map(|r| r.key).collect::<Vec<_>>();
        let all = keys(h.repo.list_held(None, 10, 0).await.unwrap());
        assert_eq!(all, vec![ToolPolicySetKey::Floor, ToolPolicySetKey::Tenant(low), ToolPolicySetKey::Tenant(high)]);
        assert_eq!(keys(h.repo.list_held(None, 1, 0).await.unwrap()), vec![ToolPolicySetKey::Floor]);
        assert_eq!(keys(h.repo.list_held(None, 1, 1).await.unwrap()), vec![ToolPolicySetKey::Tenant(low)]);
        assert_eq!(keys(h.repo.list_held(None, 5, 2).await.unwrap()), vec![ToolPolicySetKey::Tenant(high)]);
        assert!(h.repo.list_held(None, 5, 3).await.unwrap().is_empty());
        // A tenant list pages the rows of that tenant only.
        assert_eq!(keys(h.repo.list_held(Some(low), 1, 0).await.unwrap()), vec![ToolPolicySetKey::Tenant(low)]);
        assert!(h.repo.list_held(Some(low), 1, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_held_methods_on_an_unknown_org_are_not_found() {
        let h = harness();
        let unknown = Uuid::from_u128(999);
        let page = Page::new(None, None).unwrap();
        assert_eq!(h.svc.list_held(&h.actor, Some(unknown), page).await.unwrap_err().code(), TenancyError::NotFound.code());
        assert_eq!(h.svc.accept_held(&h.actor, HeldTarget::Tenant(unknown), 2).await.unwrap_err().code(), TenancyError::NotFound.code());
        assert_eq!(h.svc.discard_held(&h.actor, HeldTarget::Tenant(unknown), 2).await.unwrap_err().code(), TenancyError::NotFound.code());
    }
}
