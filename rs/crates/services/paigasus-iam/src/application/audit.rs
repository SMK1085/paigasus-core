// SPDX-License-Identifier: Apache-2.0

//! `AuditQueryService`: the read-side use case over the append-only audit log (SMA-446 Slice
//! A). `list` is Root-only — the Cedar `SCHEMA_SRC` puts `Action::ListAuditLog` in the
//! shared action block (resource `[Root, Organization, Team, Project]`), so the schema alone
//! does not restrict it; this service enforces the Root-only-ness itself by always
//! authorizing at `root_prn()`, exactly like `PolicyService::list` restricts
//! `Action::ListPolicies`.

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, AuditEntry, AuditFilter, AuditLog};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// Audit-log read use case. `audit` is `Arc<dyn AuditLog>` — the same shared handle other
/// audit-writing call sites (denial buffer, cache-hit audit) hold, so a later task's wiring
/// clones one `Arc` rather than standing up a second store instance (mirrors
/// `PolicyService`'s posture toward `PolicyStore`).
#[derive(Clone)]
pub struct AuditQueryService {
    audit: Arc<dyn AuditLog>,
    authorize: Authorize,
}

impl AuditQueryService {
    #[must_use]
    pub fn new(audit: Arc<dyn AuditLog>, authorize: Authorize) -> Self {
        Self { audit, authorize }
    }

    /// Authorizes `actor` for `Action::ListAuditLog` at `root_prn()`, then queries the audit
    /// log with `filter`. The Root-only restriction lives here, not in the Cedar schema (see
    /// module doc) — mirrors `PolicyService::list`'s authorize-then-query shape exactly.
    pub async fn list(&self, actor: &Prn, filter: AuditFilter) -> Result<Vec<AuditEntry>, TenancyError> {
        self.authorize.check(actor, Action::ListAuditLog, &root_prn()).await?;
        Ok(self.audit.query(&filter).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeAuthorizer;
    use async_trait::async_trait;
    use chrono::Utc;
    use paigasus_iam_core::{AuditOutcome, RepositoryError, Transaction};
    use std::sync::Mutex;
    use uuid::Uuid;

    fn actor_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn entry(id: u128) -> AuditEntry {
        AuditEntry {
            id: Uuid::from_u128(id),
            occurred_at: Utc::now(),
            actor_prn: None,
            action: "ListOrganizations".to_string(),
            resource_prn: None,
            outcome: AuditOutcome::Denied,
            determining_policies: Vec::new(),
            detail: serde_json::Value::Null,
            correlation_id: None,
        }
    }

    fn filter() -> AuditFilter {
        AuditFilter {
            actor_prn: None,
            resource_prn: None,
            action: None,
            outcome: None,
            from: None,
            to: None,
            cursor: None,
            limit: 50,
        }
    }

    /// In-memory `AuditLog` fake for this unit test only: `record_out_of_band`/`record` both
    /// push (this fake has no notion of transactional atomicity — that's `PgAuditLog::record`'s
    /// job, exercised by its own Docker integration test, `tests/outbox_uow_pg.rs`), `query`
    /// returns every stored row (a simple return-all — filtering is `PgAuditLog`'s job too).
    #[derive(Default)]
    struct FakeAuditLog {
        rows: Mutex<Vec<AuditEntry>>,
    }

    #[async_trait]
    impl AuditLog for FakeAuditLog {
        async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError> {
            self.rows.lock().unwrap().push(e.clone());
            Ok(())
        }

        async fn record(&self, _tx: &dyn Transaction, e: &AuditEntry) -> Result<(), RepositoryError> {
            self.rows.lock().unwrap().push(e.clone());
            Ok(())
        }

        async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
            Ok(self.rows.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn list_denies_an_unauthorized_actor() {
        let svc = AuditQueryService::new(Arc::new(FakeAuditLog::default()), Authorize::new(Arc::new(FakeAuthorizer::default())));
        assert_eq!(svc.list(&actor_prn(1), filter()).await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn list_returns_rows_for_an_authorized_actor() {
        let fake_authz = FakeAuthorizer::default();
        fake_authz.allow(Action::ListAuditLog, &root_prn());
        let fake_audit = FakeAuditLog::default();
        fake_audit.rows.lock().unwrap().push(entry(1));
        fake_audit.rows.lock().unwrap().push(entry(2));

        let svc = AuditQueryService::new(Arc::new(fake_audit), Authorize::new(Arc::new(fake_authz)));
        let rows = svc.list(&actor_prn(1), filter()).await.unwrap();
        assert_eq!(rows.len(), 2);
    }
}
