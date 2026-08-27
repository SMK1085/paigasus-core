// SPDX-License-Identifier: Apache-2.0

//! `/v1/memberships` handlers: attach/detach/list principal-to-tenancy-node memberships.
//! Thin extract -> service call -> map, mirroring `organizations.rs`/`teams.rs`, except
//! `list_memberships` also validates the query itself: exactly one of `principal`/`node`
//! must be set, else `TenancyError::MissingRequiredField("principal|node")` (neither set) or
//! `TenancyError::MutuallyExclusiveFields("principal|node")` (both set) — both 400, mirroring
//! the proto oneof rule (ADR-0014). See [`membership_filter`] for the split (SMA-586 D6).
//!
//! **SMA-444 Task 20 enforcement (spec §9.4: "Attach/Detach/List Membership -> the target
//! tenancy node"):**
//! - `AttachMembership` authorizes against the caller-supplied `node_prn`'s node, resolved
//!   through [`resolve_node`] (looked up by uuid, NEVER the caller's raw string) so a forged
//!   org slot can't route the entity-slice loader at a nonexistent org (which would surface
//!   an opaque 500 instead of the existing `PrnMismatch` defense) — the subsequent
//!   `MembershipService::attach` call still re-parses the ORIGINAL (possibly forged)
//!   `node_prn` and still returns `PrnMismatch` for it, unchanged.
//! - `DetachMembership` loads the membership FIRST (`MembershipService::get`) and authorizes
//!   against its STORED `node_prn` (already trustworthy — not caller-suppliable) — there is
//!   no node prn on a `DELETE /v1/memberships/{id}` request at all.
//! - `ListMemberships` authorizes against the queried node for a node-filtered query;
//!   `Root` for a principal-filtered query (there is no single "target node" for "every node
//!   a principal belongs to" — mirrofs `ListOrganizations`' platform-only posture, D4).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, TenancyNodeRef};
use paigasus_kernel::Prn;

use super::AppState;
use super::dto::{CreateMembershipBody, MembershipDto, MembershipQuery};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{MembershipId, UuidPath};
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::memberships::MembershipFilter;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/memberships", post(create_membership).get(list_memberships))
        .route("/v1/memberships/{id}", delete(delete_membership))
}

fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(TenancyNodeRef::from_prn(prn)?)
}

/// Resolves `node`'s REAL, stored PRN by looking it up (by uuid alone) through the owning
/// tenancy service — see the module docs for why this, not the caller-supplied prn directly,
/// is what `AttachMembership`/`ListMemberships` authorize against.
async fn resolve_node(s: &AppState, node: &TenancyNodeRef) -> Result<Prn, TenancyError> {
    Ok(match node {
        TenancyNodeRef::Organization(id) => s.orgs.get(id.uuid()).await?.node.id.prn().clone(),
        TenancyNodeRef::Team(id) => s.teams.get(id.uuid()).await?.node.id.prn().clone(),
        TenancyNodeRef::Project(id) => s.projects.get(id.uuid()).await?.node.id.prn().clone(),
    })
}

async fn create_membership(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    EnvelopeJson(b): EnvelopeJson<CreateMembershipBody>,
) -> Result<(StatusCode, Json<MembershipDto>), ApiError> {
    if s.enforce_tenancy {
        let node = parse_node_prn(&b.node_prn)?;
        let resource = resolve_node(&s, &node).await?;
        s.authorize.check(&actor_prn(&ctx), Action::AttachMembership, &resource).await?;
    }
    let record = s.memberships.attach(&b.principal_prn, &b.node_prn).await?;
    Ok((StatusCode::CREATED, Json(record.into())))
}

/// `DELETE /v1/memberships/{id}`. Detaching an ORG membership cascades: the
/// principal's team/project memberships within that same org are removed in
/// the same transaction (spec §5.1 rule 5). Detaching a team/project
/// membership removes only itself. Detaching a nonexistent id is a 404, not
/// an idempotent no-op.
async fn delete_membership(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<MembershipId>) -> Result<StatusCode, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let record = s.memberships.get(id).await?;
        let node_prn = Prn::parse(&record.node_prn).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
        s.authorize.check(&actor_prn(&ctx), Action::DetachMembership, &node_prn).await?;
    }
    s.memberships.detach(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Maps the two mutually-exclusive query params to a `MembershipFilter`.
///
/// The old single `_ =>` arm folded "neither set" and "both set" into one reason, which is the
/// catch-all this ticket removes, in miniature (SMA-586 D6). An empty string counts as absent
/// (D7), matching the gRPC surface where proto3's empty string IS the unset sentinel.
///
/// Unlike gRPC, this surface CAN receive both — its two query params are independent, where
/// the wire models the same choice as a `oneof`. So `MutuallyExclusiveFields` is emitted here
/// and nowhere else in the service.
pub(crate) fn membership_filter(principal: Option<String>, node: Option<String>) -> Result<MembershipFilter, TenancyError> {
    let principal = principal.filter(|s| !s.trim().is_empty());
    let node = node.filter(|s| !s.trim().is_empty());
    match (principal, node) {
        (Some(principal), None) => Ok(MembershipFilter::Principal(principal)),
        (None, Some(node)) => Ok(MembershipFilter::Node(node)),
        (None, None) => Err(TenancyError::MissingRequiredField("principal|node")),
        (Some(_), Some(_)) => Err(TenancyError::MutuallyExclusiveFields("principal|node")),
    }
}

async fn list_memberships(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<MembershipQuery>) -> Result<Json<Vec<MembershipDto>>, ApiError> {
    let filter = membership_filter(q.principal, q.node)?;
    if s.enforce_tenancy {
        let resource = match &filter {
            MembershipFilter::Principal(_) => root_prn(),
            MembershipFilter::Node(raw) => resolve_node(&s, &parse_node_prn(raw)?).await?,
        };
        s.authorize.check(&actor_prn(&ctx), Action::ListMemberships, &resource).await?;
    }
    let page = Page::new(q.limit, q.offset)?;
    let records = s.memberships.list(filter, page).await?;
    Ok(Json(records.into_iter().map(MembershipDto::from).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-586 D6: the two halves of the old `_ =>` catch-all are different client mistakes and
    /// now get different reasons. Both were `invalid-prn` before, which is the same catch-all this
    /// ticket removes, in miniature.
    #[test]
    fn the_membership_filter_distinguishes_neither_set_from_both_set() {
        // Reasons are pinned as ErrorReason values compared via `as_wire_reason()`, NEVER as bare
        // kebab literals. Two reasons: it routes the assertion through the registry, so an
        // unregistered rename fails here too; and a literal in a `src/` file would put this
        // production module on `ci/error-registry/check.py`'s MANIFEST, which would blind that gate
        // to a future *production* code literal anywhere in this file.
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        let wire = |r: ErrorReason| r.as_wire_reason().expect("not the Unspecified sentinel");

        let neither = membership_filter(None, None).unwrap_err();
        assert_eq!(neither, TenancyError::MissingRequiredField("principal|node"));
        assert_eq!(neither.code(), wire(ErrorReason::MissingRequiredField));

        let both = membership_filter(Some("a".into()), Some("b".into())).unwrap_err();
        assert_eq!(both, TenancyError::MutuallyExclusiveFields("principal|node"));
        assert_eq!(both.code(), wire(ErrorReason::MutuallyExclusiveFields));

        assert!(matches!(membership_filter(Some("a".into()), None).unwrap(), MembershipFilter::Principal(_)));
        assert!(matches!(membership_filter(None, Some("b".into())).unwrap(), MembershipFilter::Node(_)));
    }

    /// SMA-586 D7: `?principal=` (present but empty) means the same thing as an absent param.
    /// Without this, D5.2's gRPC repair — where proto3's empty string IS the unset sentinel —
    /// would make the two transports disagree again.
    #[test]
    fn an_empty_membership_param_is_treated_as_absent() {
        assert_eq!(membership_filter(Some(String::new()), None).unwrap_err(), TenancyError::MissingRequiredField("principal|node"));
    }
}
