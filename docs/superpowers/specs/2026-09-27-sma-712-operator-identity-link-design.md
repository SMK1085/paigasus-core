# SMA-712: operator API to link an external identity or change a user's email

- Linear: [SMA-712](https://linear.app/smaschek/issue/SMA-712)
- Source: SMA-706 spec, section 11, decision 2
  (`docs/superpowers/specs/2026-09-27-sma-706-jit-failure-alert-design.md`)
- Related: SMA-698 (the `email_conflict` defect), SMA-706 (the alert and the runbook)
- Status: approved at Gate 1 (2026-09-27)

## 1. Problem

JIT provisioning refuses a new identity with `defect="email_conflict"` when another IAM user
already has the same email (`authenticate_token.rs:306-314`). IAM does not link identities by
email. This is the "no auto-link by email" rule, D5 (`authenticate_token.rs:266-270`). The alert
`IamJitProvisioningFailures` tells the operator about the refusal.

The operator has no API to fix the refusal. The only user-write call is `POST /v1/users` /
gRPC `CreateUser`. The runbook (`docs/ops/RUNBOOK-observability.md`, the
`IamJitProvisioningFailures` section) tells the operator to `INSERT` an `external_identity` row
or to `UPDATE` `"user".email` by hand.

The cases that need a link or an email change:

- C1. A user made with `CreateUser` has no external identity. The first login of this person
  fails every time.
- C2. The same person signs in through a second issuer.
- C3. The identity provider gave the same person a new `sub` (the user was deleted and made
  again, or a realm import).
- C4. An email moved from one person to another at the identity provider.

## 2. Goals and non-goals

Goals:

1. An operator can resolve C1 to C4 with API calls only. No Postgres read or edit is necessary.
2. Every committed link, unlink and email change writes an audit record in the same transaction
   as the change.
3. Each call needs its own explicit Cedar action, and a test proves which action each call
   checks.
4. The `IamJitProvisioningFailures` runbook section uses the new API.

Non-goals:

1. Console pages for these calls. The console has no user pages yet.
2. A bulk issuer rename (the issuer URL change case in the SMA-706 runbook). It keeps its
   guarded SQL statement.
3. Outbox events for a link, an unlink or an email change (see section 6.5).
4. Case-insensitive email matching. The match stays exact, the same as JIT and the unique
   constraint.
5. An audit record for `CreateUser`. That is a known, separate gap (`create_user.rs:14-23`).
6. A self-service flow in which the user proves control of both identities.
7. A call to disable or delete a user. No code path disables a user today (only service
   accounts, `application/service_accounts.rs:222`).

## 3. Decisions by Sven (2026-09-27)

1. **Proof model: operator attestation.** The operator confirms out of band that the identity is
   the same person. The API does not ask for technical proof. A mandatory `reason` goes into
   the audit record. The alternatives are in section 10.
2. **Permission: one Cedar action per call, granted by `platform_admin`.** `platform_admin` can
   already grant itself any action through `PutPolicy`. A separate role gives friction, not
   protection.
3. **Scope: link, unlink, change email, and find by email.** Unlink removes a dead key (C3) and
   reverses a wrong link. Find by email removes the Postgres read.
4. **API shape: option A.** User-scoped sub-resources on the existing `UserService`. Unlink is
   a `POST …/unlink`, not a `DELETE`, because it carries a `reason` body.

## 4. API

### 4.1 Routes and actions

| Operation | HTTP | gRPC (`UserService`) | Cedar action |
|---|---|---|---|
| Find a user by email | `POST /v1/users/find-by-email` | `FindUserByEmail` | `GetUser` (read) |
| Link an identity | `POST /v1/users/{id}/external-identities` | `LinkExternalIdentity` | `LinkExternalIdentity` |
| Unlink an identity | `POST /v1/users/{id}/external-identities/{identity_id}/unlink` | `UnlinkExternalIdentity` | `UnlinkExternalIdentity` |
| Change the email | `POST /v1/users/{id}/email` | `ChangeUserEmail` | `ChangeUserEmail` |

- Find by email is a `POST` with the email in the body. A `GET ?email=` puts the email in the
  request line. `TraceLayer::new_for_http()` (`adapters/http/mod.rs:994`) and proxy access logs
  can record the request line. gRPC already has the email in the body.
- HTTP names the user by its principal UUID `{id}`. This is the convention of
  `/v1/service-accounts/{sa}` (`http/service_accounts.rs:11-15`). The handlers use new path
  markers `UserId` and `ExternalIdentityId` in `http/path.rs:60-78`, with `UuidPath<UserId>` and,
  for unlink, `UuidPathPair<UserId, ExternalIdentityId>` (`path.rs:141-163`). The stable-names
  test (`path.rs:353-364`) gets both markers.
- gRPC names the user by its full PRN, the convention of `GetServiceAccountRequest { string prn }`
  (`iam.proto:466-468`). A bad value gives `invalid-prn` in gRPC and `invalid-uuid` in HTTP.
  This is the existing, recorded transport divergence for principal ids
  (`grpc/convert.rs:1379-1405`).
- Every HTTP body uses the `EnvelopeJson` extractor (`repo:http-extractor-envelope`).

### 4.2 Proto contract

In `contracts/proto/paigasus/iam/v1/iam.proto`:

```proto
message User {
  string prn = 1;
  string email = 2;
  string display_name = 3;
  string status = 4; // principal status
  repeated ExternalIdentity external_identities = 5;
  paigasus.common.v1.AuditMetadata audit = 6;
}

message ExternalIdentity {
  string id = 1; // uuid
  string issuer = 2;
  string subject = 3;
  paigasus.common.v1.AuditMetadata audit = 4; // modified_at == created_at (immutable)
}

message FindUserByEmailRequest { string email = 1; }
message FindUserByEmailResponse { User user = 1; }

message LinkExternalIdentityRequest {
  string user_prn = 1;
  string issuer = 2;
  string subject = 3;
  string reason = 4;
}
message LinkExternalIdentityResponse { ExternalIdentity external_identity = 1; }

message UnlinkExternalIdentityRequest {
  string user_prn = 1;
  string external_identity_id = 2;
  string reason = 3;
}
message UnlinkExternalIdentityResponse {}

message ChangeUserEmailRequest {
  string user_prn = 1;
  string email = 2;
  string reason = 3;
}
message ChangeUserEmailResponse { User user = 1; }
```

- `status` is a `string` with the value `active` or `disabled`. This is the house convention
  for principal status (`iam.proto:433`). It is not a new enum.
- `User` and `ExternalIdentity` carry `AuditMetadata`, the same as every other resource message
  (`iam.proto:50-82`). `modified_at` of a user is `"user".updated_at`.
- The `external_identities` list is in `(created_at, id)` order.

### 4.3 HTTP bodies and responses

The HTTP DTOs mirror the proto messages, with the user named by `{id}` in the path.

- **Find by email.** Body `{email}`. Response 200 with the `User` object. No user gives 404
  `not-found`.
- **Link.** Body `{issuer, subject, reason}`. Response 201 with the `ExternalIdentity` object.
  If the same user already holds `(issuer, subject)`, the response is 200 with the existing
  object (section 5.2).
- **Unlink.** Body `{reason}`. Response 204.
- **Change email.** Body `{email, reason}`. Response 200 with the `User` object.

Every DTO field is `Option<String>` with `#[serde(default)]`. So a missing field reaches the
service as an empty value, and both transports return the same code (for example
`invalid-reason`). A body with bad JSON syntax still gets the extractor's 400 or 422 before the
handler runs (`http/json.rs:126-139`).

## 5. Rules

### 5.1 Authorization

- **The service authorizes, not the adapter.** `UserIdentityService` holds `authorize` in its
  deps. The first statement of each of its four methods is
  `self.authorize.check(actor, Action::X, &root_prn()).await?`. There is **no**
  `enforce_tenancy` check. This is the pattern of `DeadLetterService`
  (`application/dead_letters.rs:108`) and `PolicyService`.
- Reason: with `enforce_tenancy = false`, the adapter checks of `CreateUser` do not run
  (`http/users.rs:61-63`, `grpc/users.rs:83-85`). If the new calls used that pattern, any
  authenticated principal could link its own identity to a `platform_admin` user. The boot
  warning (`http/mod.rs:788-791`) says that application-layer authorization "still applies", and
  it names each such service. The new calls are in that group, so the warning adds
  "user identity links" to its list.
- The check runs before any input validation and before any row read. A denied caller cannot
  use the calls to test if an email or an identity exists. The exception is a body that the
  extractor refuses (section 4.3).
- A deny writes an `audit_log` row with `outcome = Denied` through the existing
  `BufferedDenialAuditSink` (`adapters/authz/denial_audit.rs`). That path is best effort
  (fail-open). This spec adds no new denial mechanism.
- "Root only" comes from the resource that the service passes (`root_prn()`). It does not come
  from the Cedar schema. The schema has one shared
  `appliesTo { resource: [Root, Organization, Team, Project] }` for every action
  (`authz/schema.rs:20-28`). The new actions go into that same list. The schema is not split.

### 5.2 Link

- `{id}` must name an existing **user** principal. An unknown id, or the id of a service
  account, gives 404 `not-found`.
- The link does not check the user's status. No user can be `Disabled` today (non-goal 7).
- `issuer` must be exactly equal to one of the configured `authn.issuers[].issuer` values.
  Otherwise the call gives 400 `unknown-issuer`. This stops a typo, for example a missing or an
  extra `/`, from making a link that no token ever matches. The comparison is on the trimmed
  value, the same as the config validation (`config.rs:1009`). The stored value is the
  configured value.
- `subject` is stored exactly as given, with no trim. JIT also stores the `sub` claim with no
  change, and a trimmed value would never match the token. It must have 1 to 255 characters
  (Unicode scalar values, `chars().count()`), and it must not start or end with whitespace.
  Otherwise the call gives 400 `invalid-subject`.
- If the **same** user already holds `(issuer, subject)`, the call returns the existing identity
  with HTTP 200 and writes no audit record. So a retry after a lost response is safe. This is
  the pattern of `RoleService::grant` (`application/roles.rs:249-253`).
- If **another** user holds `(issuer, subject)`, the call gives 409 `external-identity-exists`.
  There is no implicit move. The operator unlinks first, which gives two audit records.
- The link does not change D5. JIT still never links by email. Only an attested and audited
  operator call makes a link.

### 5.3 Unlink

- `{id}` must name an existing user principal, and `identity_id` must belong to it. Otherwise
  the call gives 404 `not-found`. A retry after a lost response therefore gives 404. The
  runbook states this.
- The caller cannot unlink the identity that authenticated the current request. That gives 409
  `cannot-unlink-own-identity`. This stops a self-lockout. The adapter reads the identity from
  `AuthContext.credential` (`Credential::Oidc { issuer, subject }`,
  `paigasus-iam-core/src/authn.rs:74`) and passes it to the service. A request that an API key
  authenticated (`Credential::ApiKey`) has no such identity, so the guard does not apply to it.
- The call can remove the last identity of a user. The user then cannot log in until a new link
  exists. The runbook states this.

### 5.4 Change email

- `{id}` must name an existing user principal. Otherwise the call gives 404 `not-found`.
- `Email::parse` validates the new email. A failure gives 400 `invalid-email`.
- If another user has the email, the call gives 409 `email-conflict` (the existing
  `user_email_key` mapping, `persistence/mod.rs:69-84`).
- If the new email equals the current email (exact match after `Email::parse`), the call is a
  no-op. It returns 200 and writes no audit record. The "equal" decision comes from the row that
  the transaction locked, not from an earlier read (section 6.3).
- The change sets `"user".updated_at`. A no-op does not change it (SMA-606 D1).

### 5.5 Reason

- `reason` is mandatory. After trim it has 1 to 500 characters (Unicode scalar values).
  Otherwise the call gives 400 `invalid-reason`. The stored value is the trimmed value.
- A new value type `AuditReason` in `paigasus-iam-core/src/value.rs`, in the style of
  `Email::parse`, holds this rule. A value type `ExternalSubject` holds the rule of section 5.2.

### 5.6 Concurrency

- Link against a concurrent JIT login of the same `(issuer, subject)`: the unique constraint
  `uq_external_identity_issuer_subject` lets only one insert commit. If JIT wins, the link gives
  409 `external-identity-exists`, because JIT made a new user. If the link wins, JIT re-reads the
  identity (`authenticate_token.rs:306-314`) and resolves to the linked user. In both orders one
  principal holds the key. The first order leaves a duplicate user (section 7, "orphan user").
- Two email changes to the same email: `user_email_key` lets only one commit. The other gets
  409 `email-conflict`.
- Two email changes to the same user: the row lock (section 6.3) serializes them. Each audit
  record has the correct `old_email`, so the history A→B→C is complete.

### 5.7 Error codes

New `TenancyError` variants. Each is a unit variant, so no caller input goes into an error body
(`application/error.rs:43-56`). Each gets a `field()` value.

| Code | Class | HTTP | gRPC | `field()` |
|---|---|---|---|---|
| `invalid-reason` | Validation | 400 | `INVALID_ARGUMENT` | `reason` |
| `unknown-issuer` | Validation | 400 | `INVALID_ARGUMENT` | `issuer` |
| `invalid-subject` | Validation | 400 | `INVALID_ARGUMENT` | `subject` |
| `cannot-unlink-own-identity` | Precondition | 409 | `FAILED_PRECONDITION` | none |
| `external-identity-exists` | Conflict | 409 | `ALREADY_EXISTS` | none |

The calls also reuse `not-found`, `invalid-email`, `email-conflict`, `invalid-uuid`,
`invalid-prn` and `forbidden`.

- **The registry.** The canonical code registry is `contracts/proto/paigasus/common/v1/error.proto`
  (`ci/error-registry/check.py:26`). Five new `ERROR_REASON_*` values go there. Two tests
  enforce it: `every_tenancy_code_is_declared_in_the_canonical_registry`
  (`grpc/convert.rs:963-974`) and the hand-kept mirror `EXPECTED_REASONS` in
  `rs/crates/libs/paigasus-proto/src/error.rs`. Its count anchor changes from 60 to 65
  (`error.rs:233`). The `common/v1` bindings are regenerated in Rust, Python and TypeScript.
- **The conflict mapping.** `From<RepositoryError> for TenancyError` maps
  `ConflictKind::ExternalIdentityExists` to `TenancyError::Internal` today
  (`application/error.rs:313`), because no tenancy call produced it. The link call does, so the
  mapping changes to the new variant `ExternalIdentityConflict`. The plan confirms that no other
  caller depends on the `Internal` mapping. JIT does not: it handles its conflict in
  `authenticate_token.rs` before any `TenancyError` conversion.
- If a unit test in `src/application/user_identities.rs` spells a code as a literal, the file goes
  into the `MANIFEST` of `ci/error-registry/check.py`.

## 6. Design

### 6.1 Application service

New file `rs/crates/services/paigasus-iam/src/application/user_identities.rs` with
`UserIdentityService`:

- Deps: `{authorize, users, links, uow, audit, issuers, id_gen, clock}`. `issuers` is the set of
  configured issuer strings, built once from `authn.issuers`.
- Methods: `find_by_email(actor, email)`, `link(actor, user, issuer, subject, reason)`,
  `unlink(actor, caller_identity, user, identity_id, reason)` and
  `change_email(actor, user, email, reason)`.
- Each method authorizes first (section 5.1). Each write then runs in one unit of work:
  `uow.begin()`, the write, `audit.record(&*tx, &entry)` only when the write changed something,
  `tx.commit()`. If the audit write fails, the change rolls back.

### 6.2 Ports

A new, narrow port `IdentityLinkStore` in `paigasus-iam-core/src/ports.rs`. The existing
`ExternalIdentityRepository` does not change. `AuthenticateToken` uses that port on the hot
authentication path, and it must stay read-and-provision only. The narrow port also keeps the
eleven existing fakes of the old ports unchanged.

```rust
#[async_trait]
pub trait IdentityLinkStore: Send + Sync {
    /// The user, with its identities in (created_at, id) order. None when no USER has the email.
    async fn find_user_by_email(&self, email: &Email) -> Result<Option<UserWithIdentities>, RepositoryError>;
    /// Locks the user row FOR SHARE. None when the principal is not a user.
    async fn lock_user_in(&self, tx: &dyn Transaction, id: PrincipalId) -> Result<Option<User>, RepositoryError>;
    /// Inserts the link. Conflict(ExternalIdentityExists) on the unique constraint.
    async fn link_in(&self, tx: &dyn Transaction, identity: &ExternalIdentity) -> Result<(), RepositoryError>;
    /// The identity with (issuer, subject), or None.
    async fn find_identity_in(&self, tx: &dyn Transaction, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError>;
    /// DELETE … WHERE id = $1 AND principal_id = $2 RETURNING *. None when no row matched.
    async fn unlink_in(&self, tx: &dyn Transaction, user: PrincipalId, identity_id: ExternalIdentityId) -> Result<Option<ExternalIdentity>, RepositoryError>;
    /// SELECT … FOR UPDATE, then UPDATE when the email differs. None when the principal is not a user.
    async fn change_email_in(&self, tx: &dyn Transaction, user: PrincipalId, email: &Email, now: DateTime<Utc>) -> Result<Option<Mutated<EmailChange>>, RepositoryError>;
}

pub struct EmailChange { pub old: Email, pub new: Email }
```

`Mutated<T>` is the existing no-op contract (`ports.rs:63-81`, SMA-606 D1). The exact names and
types are for the plan to settle against the code. The contract is fixed: every read that
decides an audit value or a no-op happens inside the transaction.

### 6.3 Link flow inside the transaction

1. `lock_user_in(tx, user)`. `None` gives 404.
2. `find_identity_in(tx, issuer, subject)`. The same user gives 200 with the existing identity
   and no audit record. Another user gives 409.
3. `link_in(tx, …)`. A unique violation here (a race with JIT) gives 409.
4. `audit.record(&*tx, …)`, then `commit`.

The persistence adapter is SeaORM, in a new file `adapters/persistence/pg_identity_links.rs`.
The existing `map_err` maps the unique violations. **No migration is necessary.**
`user_email_key`, `uq_external_identity_issuer_subject` and `ix_external_identity_principal`
cover every query.

### 6.4 Audit records

One row per committed write that changed something, `outcome = Committed`:

| Field | Value |
|---|---|
| `action` | `LinkExternalIdentity`, `UnlinkExternalIdentity` or `ChangeUserEmail` |
| `actor_prn` | the caller |
| `resource_prn` | the user's principal PRN |
| `detail` (link, unlink) | `{reason, identity_id, issuer, subject}` |
| `detail` (change email) | `{reason, old_email, new_email}` |

For an unlink, `issuer` and `subject` come from the deleted row (`RETURNING`), not from the
request. For an email change, `old_email` comes from the locked row.

The `detail` holds an email and a subject. These are personal data, and this is the first time
that an email goes into `audit_log`. The `audit_log` is restricted by the `ListAuditLog` action,
and an auditor needs these values to reconstruct a takeover. The retention of these rows is the
existing `[audit.retention]` config (SMA-467, `config.rs:380-393`). The IAM log lines of these
calls contain neither value, and the find call carries the email in the body, not in the URL.

### 6.5 No outbox events

No consumer reads the user email or the identities. A new `EventType` also changes the event
contracts and the `repo:nats-permissions` gate. The audit record is the durable trail. If a
consumer later needs an event, a separate issue adds it.

### 6.6 Cedar and the starter policies

- Four new `Action` variants in `paigasus-iam-core/src/authz/action.rs`: `GetUser`,
  `LinkExternalIdentity`, `UnlinkExternalIdentity` and `ChangeUserEmail`. The three writes
  return `true` from `is_write()`. `Action::ALL` grows from 41 to 45, and its length test and
  message change (`action.rs:308-312`).
- The embedded schema (`authz/schema.rs:20-28`) adds the four names to the shared action list.
- `platform_admin`'s template has no action list (`authz/roles.rs:325-326`), so it permits the
  new actions with no change. No other starter role gets them.
- The three writes join the derived `forbid-archived-writes` source (`roles.rs:311-315`). The
  starter content therefore changes. `STARTER_POLICY_REVISION` goes from 3 to 4 (`roles.rs:62`),
  and `EXPECTED_STARTER_CONTENT_HASH` (`roles.rs:90`) is recomputed. SMA-584 made the same bump.
- Twin tests, as in SMA-584: the schema validates the new actions (`schema.rs:74-76`), and the
  forbid source contains the three writes and not `GetUser` (`roles.rs:828-833`).

### 6.7 `bootstrapAdmins` and escalation

`BootstrapAdminSeeder` grants `platform_admin` to the principal that holds a configured
`(issuer, subject)` at its first login (`application/bootstrap_admin.rs:3-17`).

- **`LinkExternalIdentity` is equal to `platform_admin` in power.** A principal that holds only
  this action can link an identity that it controls to any `platform_admin` user, or link a
  `bootstrapAdmins` key to itself. Either way it becomes `platform_admin`. So do not grant the
  action to anyone who must not hold `platform_admin`.
- **`UnlinkExternalIdentity` alone can lock out any user.**
- An unlink of a bootstrap key does not revoke the `platform_admin` grant that it seeded. A new
  link of that key gives a second user the grant.
- An API-key caller is exempt from the own-identity guard. It can unlink the identity of the
  last human admin. Recovery then needs SQL.

The ADR "Consequences" and the runbook state these facts.

## 7. Runbook

Change the `IamJitProvisioningFailures` remediation in `docs/ops/RUNBOOK-observability.md`
(about lines 1367-1390):

- Remove "IAM has no API to update a user, change an email or link an identity (SMA-712 tracks
  one)" and the `INSERT` statement.
- **Step 1, get the email.** Get it from the user or from the identity provider's login events.
  The IAM log does not contain it (unchanged).
- **Step 2, find the IAM user.** `POST /v1/users/find-by-email`. It gives the principal and its
  linked identities.
- **Step 3, get the new `(issuer, subject)`.** The `issuer` is the configured
  `authn.issuers[].issuer` of the identity provider that the person used. For Keycloak, the
  `subject` is the user ID on the user's page in the admin console. For another identity
  provider, read its documentation for where the `sub` claim value is shown. If the identity
  provider uses a pairwise `sub` and does not show it, the person can read the `sub` claim from
  a token that the identity provider issued to them. Never guess a value.
- **Step 4, decide the case.** Compare the person at the identity provider with the IAM user from
  step 2:
  - The IAM user has no identities, and the person is the one for whom `CreateUser` made the
    user: C1.
  - The IAM user has an identity at another issuer, and both identity-provider accounts belong to
    the same person: C2.
  - The IAM user has an identity at the same issuer with a different `sub`, and the identity
    provider confirms that the old account was deleted or imported again for the same person:
    C3.
  - The identity provider confirms that the email now belongs to a different person than the
    IAM user: C4.
  - If you cannot confirm one of these, stop. Do not make a link.
- **Step 5, act.**
  - C1, C2: `POST /v1/users/{id}/external-identities`.
  - C3: link the new `sub`, then unlink the old `sub`. The old `sub` would give `email_conflict`
    again if the identity provider ever issues it.
  - C4: `POST /v1/users/{id}/email` on the old user. If the old person has no new email, use a
    unique address that can never be delivered: `<principal-uuid>@example.invalid`. JIT then
    makes a new user for the new person at the next login. The old user keeps its identities.
- **Reverse a wrong link.** Unlink it. Then read `GET /v1/audit?resource=<user prn>` for the time
  of the link to see what the linked identity did.
- Give each call as a `curl` example with a `reason`.
- Keep the D5 warning. Add:
  - the calls need `platform_admin`, or an explicit grant of the action;
  - the facts of section 6.7;
  - the reason is in the audit log, and so are the email and the subject;
  - an unlink of the last identity locks the user out, and a repeated unlink gives 404;
  - **orphan user:** when JIT wins a race with a link (section 5.6), or when a second issuer sent
    a different email, JIT makes a duplicate user. No API removes or disables it. It holds its
    email, so that email cannot go to another user until SQL changes it.
  - `CreateUser` plus a link is the way to add a user on an issuer that has JIT disabled.
- The issuer URL change case keeps its guarded SQL statement. State that SMA-712 does not cover
  it.

## 8. Rollout and rollback

- **Rollout.** The new binary writes starter revision 4 at boot. The fleet converges by the
  SMA-477 D11 revision mechanism. The new calls are available on each replica when it runs the
  new binary. No migration and no config change is necessary.
- **Disable the calls without a binary revert.** A static Cedar `forbid` policy through
  `PutPolicy` that names the three write actions overrides the `platform_admin` permit. The
  runbook gives this policy. So no capability key is necessary.
- **Binary rollback.** The plan reads the rollback analysis of SMA-584 (section 4.4) and records
  what the old binary does with starter revision 4 in the database. Other state that stays after
  a rollback: audit rows with the new action names (the audit query reads `action` as text, so
  they stay readable), and the new `ERROR_REASON_*` values (old clients see an unknown enum
  value). No row that the new calls wrote needs an old-binary change: links and email values are
  ordinary rows.

## 9. ADR

The issue asks for an ADR before code. Plan task 1 creates it in Notion, in the ADR database
that `CONTRIBUTING.md` links. The highest number in the repo is ADR-0023, so this is probably
ADR-0024. The plan confirms the number in Notion. Section 10 is the draft text.

## 10. ADR draft

> **Title.** Operator-attested external identity linking in IAM.
>
> **Context.** JIT provisioning does not link identities by email (D5), because an email match
> is not proof of the same person. Four real cases (a `CreateUser` user, a second issuer, a new
> `sub`, an email moved to another person) then need a manual Postgres edit. A manual edit has
> no permission check and no audit record.
>
> **Decision.** IAM gets four operator calls: find a user by email, link an identity, unlink an
> identity, and change an email. The only exception to D5 is an explicit operator call. The
> operator attests out of band that the identity is the same person. Each call needs its own
> root-scoped Cedar action, which the application service checks with no bypass. The
> `platform_admin` starter role grants the actions. Each write requires a `reason` and writes an
> audit record in the same transaction.
>
> **Alternatives rejected.**
> (a) A pending link that binds at the next login when the email claim matches. It adds state
> and a table. It does not help C3 when the email also changed, and it is a partial auto-link by
> email, which is what D5 forbids.
> (b) The user proves control of both identities. It cannot work for C1, where the user has no
> identity, and it is a self-service flow, not an operator tool.
> (c) A separate role for the actions. `platform_admin` can grant itself any action through
> `PutPolicy`, so a separate role adds friction but no protection.
>
> **Consequences.** A holder of `LinkExternalIdentity` can attach any identity to any user, so
> the action is equal to `platform_admin` in power. A holder of `UnlinkExternalIdentity` can lock
> out any user. This is a takeover path by design. The audit record, the mandatory `reason` and
> the Cedar action are its controls. A static `forbid` policy disables the calls. JIT behavior
> does not change. No outbox events are added.

## 11. Tests

- **Unit, `user_identities.rs`, with in-memory fakes:**
  - every rule and every error code in section 5;
  - authorization first: a `FakeAuthorizer` that denies gives `forbidden`, and the test proves
    that no store method ran, including for an invalid email or reason;
  - **action identity:** a `FakeAuthorizer` that records the `Action` and the resource proves
    that each method checks its own action at `root_prn()`;
  - a no-op (same email, same-user link) writes no audit record;
  - the audit entry fields of section 6.4;
  - the rules of `AuditReason` and `ExternalSubject` (unit tests in `value.rs`).
- **Postgres, Docker-gated, new `tests/user_identities_pg.rs`:**
  - each write commits with its audit row, and a forced audit failure rolls back the change;
  - the concurrency cases of section 5.6, including two email changes to one user that give a
    complete A→B→C history;
  - `unlink_in` with an identity of another user matches no row.
- **HTTP and gRPC twins, `tests/http_users.rs` and `tests/grpc_users.rs`:**
  - for each call: 401 without a token, 403 without the action, and 2xx with `platform_admin`;
  - a denied caller with a well-formed body that has bad values gets 403, not 400;
  - **action identity end to end:** for each call, a static policy that permits exactly that one
    action lets the call through, and a policy that permits only one of the other three actions
    does not (the pattern of `tests/http_users.rs:79-150`);
  - **the toggle:** with `enforce_tenancy = false`, a principal without the action still gets
    403 on every write (`tests/authz_enforce_toggle.rs`).
- **End-to-end cases through the mock IdP:**
  - C1: `CreateUser` makes a user; its first login fails with `email_conflict`; the operator
    links `(issuer, subject)`; the next login resolves to the same principal.
  - C2: two mock issuers (`test_config_with` takes a slice); link the second issuer's identity;
    both logins resolve to one principal.
  - C3: link the new `sub`, then unlink the old one; the new `sub` resolves to the same
    principal; the old `sub` gets `email_conflict`.
  - C4: change the old user's email; the new person's login makes a different principal; the old
    `sub` still resolves to the old principal.
- **Cedar:** the twin tests of section 6.6, and the new content hash.
- **Mutation checks.** Each mutation must compile, and a test must fail:
  - delete the `audit.record` call;
  - delete each `authorize.check` call;
  - swap the action of one method for another of the four;
  - remove the own-identity guard;
  - remove the same-user idempotency branch.

## 12. Files

- `contracts/proto/paigasus/iam/v1/iam.proto`, `contracts/proto/paigasus/common/v1/error.proto`,
  and the regenerated Rust, Python and TypeScript bindings for both.
- `rs/crates/libs/paigasus-proto/src/error.rs` (`EXPECTED_REASONS` and the count anchor).
- `rs/crates/libs/paigasus-iam-core/src/authz/{action.rs,schema.rs,roles.rs}`,
  `src/ports.rs`, `src/value.rs`.
- `rs/crates/services/paigasus-iam/src/application/{user_identities.rs,error.rs,mod.rs,fakes.rs}`.
- `rs/crates/services/paigasus-iam/src/adapters/persistence/{pg_identity_links.rs,mod.rs}`.
- `rs/crates/services/paigasus-iam/src/adapters/http/{users.rs,dto.rs,path.rs,mod.rs}` (the
  `AppState` field and the wiring), `adapters/grpc/{users.rs,convert.rs}`.
- `rs/crates/services/paigasus-iam/tests/{http_users.rs,grpc_users.rs,user_identities_pg.rs,authz_enforce_toggle.rs}`
  and `tests/support/mod.rs`.
- `ci/error-registry/check.py` (`MANIFEST`), only if a test file spells a code as a literal.
- `docs/ops/RUNBOOK-observability.md`.

## 13. Questions from the spec challenge

Sven approved the spec at Gate 1 (2026-09-27) with no change to these recommendations. So each
recommendation below is the decision.

1. **Audit a successful find by email?** It reads personal data. Recommendation: no. No other
   read call in IAM writes an audit row, and the calls that change data are audited.
2. **Audit a refused write (404 or 409 on link or unlink)?** Recommendation: no. The audit log
   holds committed changes and Cedar denials only. A refused write changed nothing.
3. **Put the target user into the denial audit row?** The row names `root_prn()` only.
   Recommendation: no. It needs a change to the shared denial sink. Open a separate issue if
   you want it.
4. **Is `CreateUser` plus a link the intended way to add a user on an issuer that has JIT
   disabled?** Recommendation: yes. Section 7 and the ADR say so.
5. **Add a counter, and possibly an alert, on the number of links, unlinks and email changes?**
   Recommendation: no for this issue. The audit log answers the forensic question. A counter
   touches `repo:observability-drift`. Open a separate issue if you want it.

## 14. Spec challenge changelog (2026-09-27)

Verdict: APPROVE WITH CHANGES. One BLOCKER.

Folded in:

- BLOCKER, the authorization check was under the `enforce_tenancy` toggle. With the toggle off,
  any principal could take over a `platform_admin`. The service now authorizes, with no toggle
  (5.1). A toggle test is added (11).
- MAJOR, the wrong error registry. Corrected to `error.proto` and the mirror in
  `paigasus-proto` (5.7, 12).
- MAJOR, the Cedar and starter-policy work was wrong or missing. "Root only in the schema" was
  false; `platform_admin` needs no change; the revision bump, the hash and `Action::ALL` are now
  listed (5.1, 6.6). A rollout and rollback section is added (8).
- MAJOR, goal 3 had no test. Action-identity tests and a "swap the action" mutation are added
  (11).
- MAJOR, "not an escalation" was false for an explicit grant. Section 6.7, the ADR and the
  runbook now state that `LinkExternalIdentity` is equal to `platform_admin` in power.
- MAJOR, no source for the `subject`. The runbook now has step 3 (7). The challenger's option to
  record refused logins in the audit log is not taken: it adds a write on the authentication hot
  path, and the identity provider is the correct source.
- MAJOR, no procedure to tell C3 from C4. The runbook now has step 4 and a reversal step (7).
- MAJOR, audit values and the no-op came from a read outside the transaction. The port now locks
  and returns the changed row, with `Mutated<T>` and `RETURNING` (6.2, 6.3, 6.4).
- MAJOR, the proto contract was not specified. All messages are now in 4.2, with the PRN, the
  `string status` and the `AuditMetadata` conventions.
- MAJOR, the email in a `GET` query string. Find by email is now a `POST` (4.1).
- MAJOR, only C1 had an end-to-end test. C2, C3 and C4 are added (11).
- MINOR, the extractor order and a missing field: DTO fields are `Option` with
  `#[serde(default)]`, and the test text is corrected (4.3, 11).
- MINOR, `principal-disabled` described a state that cannot occur. The rule and its code are
  removed (5.2, non-goal 7).
- MINOR, link was not idempotent. A same-user link now returns the existing identity (5.2).
- MINOR, path markers, the missing files, the narrow port, unit error variants, the bootstrap
  warning, the C4 replacement email, the orphan user, the length units, the list order and the
  value types. All folded in (4.1, 5.2, 5.5, 5.7, 6.2, 6.7, 7, 12).

Changed from the challenger's fix:

- The subject no longer gets a trim (5.2). JIT stores the claim as-is, so a trimmed value could
  not match.
- The challenger suggested a capability key to unmount the calls. Not taken: a static Cedar
  `forbid` policy disables them with no new config (8).

Rejected: none. The five challenger questions are section 13.
