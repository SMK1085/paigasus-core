// SPDX-License-Identifier: Apache-2.0

//! Pure IAM domain (M0 walking skeleton): entities, value objects, and port traits.
//! No I/O, no SeaORM, no axum/tonic — the service crate provides adapters (ADR-0005,
//! hexagonal). IDs/PRNs come from `paigasus-kernel`; time/entropy are injected via ports.

pub mod api_key;
pub mod audit;
pub mod authn;
pub mod authz;
pub mod dead_letter;
pub mod domain_event;
pub mod ports;
pub mod principal;
pub mod service_account;
pub mod tenancy;
pub mod user;
pub mod value;

pub use api_key::{ApiKey, ApiKeyDefect, ApiKeyId, ApiKeyStatus, NewApiKey, ParsedToken, display_prefix, format_token, parse_token};
pub use audit::{AuditEntry, AuditFilter, AuditOutcome};
pub use authn::{AuthnError, AuthnPrincipal, Credential, ExternalIdentity, Issuer, PrincipalContext, ProvisioningDefect, TokenDefect, ValidatedClaims};
pub use authz::{
    AccessRequest, Action, AuditSink, Authorizer, AuthzError, Decision, DecisionCache, Effect, EntitySliceLoader, GrantScope, PolicyDocument, PolicyStore, PutOutcome, RequestContext, Role, RoleGrant,
    RoleGrantRef, RoleGrantStore, SystemPolicyReconciler, SystemRoleReconciler,
};
pub use dead_letter::{BulkReplayRequest, DeadLetterEntry, DeadLetterFilter, DeadLetters};
pub use domain_event::{DomainEvent, EventType};
pub use ports::{
    ApiKeyRepository, AuditLog, Authenticator, Clock, ConflictKind, EventPublisher, ExternalIdentityRepository, IdGenerator, KeyEntropy, MembershipRecord, MembershipRepository, NodeView,
    OrganizationRepository, Outbox, PolicyGenBumper, PreconditionKind, PrincipalRepository, ProjectRepository, PublishError, RepositoryError, Savepoint, SecretHasher, ServiceAccountRepository,
    TeamRepository, Transaction, UnitOfWork,
};
pub use principal::{Principal, PrincipalKind, PrincipalStatus};
pub use service_account::{ServiceAccount, ServiceAccountRecord};
pub use tenancy::{Membership, NAME_MAX_CHARS, NodeStatus, Organization, OrganizationId, Project, ProjectId, Slug, Team, TeamId, TenancyNodeRef, validate_name};
pub use user::User;
pub use value::{DomainError, Email, PrincipalId};
