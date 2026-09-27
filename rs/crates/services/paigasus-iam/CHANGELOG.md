# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

## [0.2.0] - 2026-09-27

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

### Changed

- Over HTTP, IAM refuses a non-numeric `limit` or `offset` on the `ListRoleGrants`
  principal-only path with 400 `invalid-query-parameter`. Before, IAM ignored it (SMA-676).

### Fixed

- Two first logins of the same identity at the same time both succeed. Before, in Postgres the
  second login failed on the email and got `403 provisioning-failed` (SMA-698).
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
