# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

### Added

- Each `[[authn.issuers]]` entry has a new setting, `id_token_marker_claims`. It is a list of
  claim names that the IdP puts into its ID token and never into its access token. For Zitadel,
  use `["at_hash", "azp"]`. The default is an empty list, and then IAM adds no new check. IAM
  refuses an empty name, a name with leading or trailing whitespace, a repeated name, and the
  names `iss`, `sub`, `aud` and `exp`. IAM does not boot with such a list (SMA-703).
- IAM refuses a verified token that has a configured claim with any value except `null`. The
  refusal is `NotAnAccessToken`, the same as for the SMA-686 `typ` check. IAM logs it at `info`
  with the issuer and the marker `claim <name>`. The line does not show the claim value. IAM
  writes at most one line for each issuer and defect in 10 seconds (SMA-703).
- At boot, IAM writes one `info` line for each issuer that has configured marker claims. The line
  names the issuer and the claim names (SMA-703).
- The Helm chart has a new value, `oidc.idTokenMarkerClaims`. It renders the list into
  `IAM_AUTHN__ISSUERS`. The default is `[]`, and the render does not change. A change of the value
  restarts the IAM pod (SMA-703).

## [0.2.0] - 2026-10-01

### Added

- `ListRoleGrants` accepts new filters: `scope_prn`, `role_key` and `principal_kind`. A request
  needs a `principal_prn` or a `scope_prn`. IAM refuses an unknown `principal_kind` with the
  reason `invalid-principal-kind`. IAM pages a filtered request. `limit` is 1 to 200, with a
  default of 50. `offset` is 0 or more, with a default of 0. IAM orders the page by
  `principal_id`, then `id`. `scope_prn` matches its scope exactly. It does not match a
  descendant scope (SMA-676).
- `ListMemberships` accepts a `principal_kind` filter. IAM refuses an unknown value with the same
  reason, `invalid-principal-kind` (SMA-676).
- `GrantRole` is idempotent. A repeat grant for the same principal, role and scope returns the
  existing grant and writes nothing new. A race between two concurrent grants for the same
  principal, role and scope also returns one grant to both callers. Before, a repeat grant failed
  with `internal` (HTTP 500) (SMA-676).
- IAM adds a new database index. It speeds up a scope-filtered role grant query with no
  principal. The migration builds the index at startup. IAM refuses to start when the index
  already exists but its definition differs. IAM also refuses to start when the index exists but
  is INVALID. The build holds a SHARE lock on `role_grant`, with `lock_timeout = '5s'`: grants
  and revokes wait during the build. On a large table, an operator builds the index
  CONCURRENTLY before the upgrade (SMA-699).
- IAM logs a refused just-in-time provisioning at `warn`. The line starts with
  `just-in-time provisioning failed`. The line names the defect (`missing_email` or
  `email_conflict`). The line also names the issuer. For `missing_email`, the field `email_claim`
  tells if the claim is absent or invalid. The line does not show the email, the subject, the name
  or the token. IAM writes at most one line for each issuer and defect in 10 seconds. Before, IAM
  answered `403 provisioning-failed` and logged nothing (SMA-698).
- The counter `iam_jit_provisioning_failures_total` carries the label `defect`. It counts each
  refused request. The log rate limit does not apply to it. Both series start at zero when
  metrics are on (SMA-698).
- IAM logs a refused request at `info` when the identity is not provisioned and the issuer has
  `jit_provisioning = false`. The line starts with
  `request refused: the identity is not provisioned`. The line names the issuer and has the
  fields `reason="jit_disabled"` and `suppressed`. The line does not show the subject, the email,
  another claim or the token. Each IAM replica writes at most one line for each issuer in
  10 seconds. `Introspect` writes no line. Before, IAM answered `403 identity-not-provisioned`
  (gRPC `PermissionDenied`) and logged nothing (SMA-707).
- `UserService` has four new operator calls: `FindUserByEmail`, `LinkExternalIdentity`,
  `UnlinkExternalIdentity` and `ChangeUserEmail`. They repair a user who cannot sign in because
  of `email_conflict`. Over HTTP they are `POST /v1/users/find-by-email`,
  `POST /v1/users/{id}/external-identities`,
  `POST /v1/users/{id}/external-identities/{identity_id}/unlink` and
  `POST /v1/users/{id}/email`. `{id}` is the user's principal uuid (SMA-712).
- Each of the four calls checks a Cedar action at Root. The three writes have new actions:
  `LinkExternalIdentity`, `UnlinkExternalIdentity` and `ChangeUserEmail`. `FindUserByEmail` uses
  `GetUser`. The `platform_admin` role holds all four. The
  `enforce_tenancy` setting does not switch the check off. The three write actions join the
  `forbid-archived-writes` policy, so the starter policy revision is now 4 (SMA-712).
- `LinkExternalIdentity`, `UnlinkExternalIdentity` and `ChangeUserEmail` need a `reason` of 1 to
  500 characters. Each write and its audit entry commit in one transaction. The audit entry
  names the actor, the user and the reason. A link that the user already holds, and an email that
  does not change, write nothing and return the current state (SMA-712).
- The four calls add these error reasons: `invalid-reason`, `unknown-issuer` and
  `invalid-subject` (HTTP 400), `external-identity-exists` (HTTP 409) and
  `cannot-unlink-own-identity` (HTTP 409). `unknown-issuer` means
  that the issuer is not one of the configured `authn.issuers`. `external-identity-exists` means
  that another user holds the issuer and subject pair. `cannot-unlink-own-identity` means that
  the caller tried to remove the identity of its own OIDC login (SMA-712).

### Changed

- Over HTTP, IAM refuses a non-numeric `limit` or `offset` on the `ListRoleGrants`
  principal-only path with 400 `invalid-query-parameter`. Before, IAM ignored it (SMA-676).
- When the outbox relay is on, IAM sets the counter `iam_outbox_relay_publish_failures_total`
  to zero at startup, before the listener binds. Before, the series did not exist until the
  first publish failure. Prometheus `increase()` now sees that first failure (SMA-713).

### Fixed

- Two first logins of the same identity at the same time both succeed. Before, in Postgres the
  second login failed on the email and got `403 provisioning-failed` (SMA-698).
- IAM confirms the owner PRN of `CreateServiceAccount` and `ListServiceAccounts` against
  storage. IAM also confirms the scope PRN of `IssueApiKey`. Before, IAM did not check the
  organization slot or the region of these PRNs. IAM accepted a forged value. Now IAM refuses
  a PRN that differs from the stored PRN with `prn-mismatch` (HTTP 400), and an unknown node
  with `not-found`. IAM writes and returns the stored PRN. `IssueApiKey` now also authorizes
  `IssueApiKey` at the scope node. A refused PRN mismatch writes one warning line (SMA-646).
- IAM confirms the principal PRN of `ListMemberships` with a principal filter, of `GrantRole`
  and of `ListRoleGrants` with a principal filter against storage. Before, IAM used only the
  uuid. A forged region or organization slot listed the real principal's data. For `GrantRole`,
  IAM wrote the forged PRN into the outbox event and the response. Now IAM refuses a PRN that
  differs from the stored PRN with `prn-mismatch` (HTTP 400). IAM answers `not-found` for an
  unknown principal uuid (SMA-649).
- A bootstrap-admin seed that loses a concurrent race is now a success. IAM no longer
  increments `iam_bootstrap_admin_seed_failures_total{stage="txn"}` for it. IAM no longer logs
  the lockout warning for it either (SMA-676).

### Security

- IAM refuses a bearer token that is an ID token or a back-channel logout token. The markers are
  a `typ` claim of `ID` or `Logout` (Keycloak), a header `typ` of `logout+jwt`, or the
  back-channel logout `events` claim. Before, IAM accepted such a token when its `aud` held the
  accepted audience, which is the client id in the default chart configuration (SMA-686).
- IAM refuses a token with no `aud` as an audience mismatch. It logs a wrong or missing audience
  and a refused non-access token at `info`. The line names the issuer and a static or configured
  detail. IAM writes at most one line per issuer and kind in 10 seconds. Before, nothing logged
  these refusals, although the chart runbook said a wrong audience did (SMA-686).
- IAM refuses a sender-constrained access token: a token with a non-null `cnf` claim, or with a
  Keycloak `typ` of `DPoP`. Before, IAM accepted a DPoP-bound or mTLS-bound token as a plain
  bearer token, so the sender constraint did not protect it. IAM logs the refusal at `info` with
  the issuer and the marker (SMA-690).

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-iam` and
  `docker.io/smaschek/paigasus-iam` (SMA-658).
