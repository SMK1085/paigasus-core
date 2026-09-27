# SMA-712 Operator Identity Link API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give an operator four IAM calls (find a user by email, link an identity, unlink an
identity, change an email). Each call has its own Cedar action. Each write has a mandatory reason
and an audit record in the same transaction. The `IamJitProvisioningFailures` runbook then uses
the API, not SQL.

**Architecture:** A new application service `UserIdentityService` holds the rules. It authorizes
first, at `root_prn()`, with no `enforce_tenancy` toggle. It drives a new narrow port
`IdentityLinkStore` through one unit of work per write. A new SeaORM adapter
`PgIdentityLinkStore` implements the port. Thin gRPC handlers on `UserService` and thin HTTP
handlers under `/v1/users` call the service. Five new error reasons go into the canonical
registry. Four new Cedar actions go into the catalog, and the starter policy revision moves 3 to 4.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum, tonic, SeaORM 2.0.3, Cedar,
protobuf with buf (Rust prost/tonic, Python betterproto2, TypeScript protobuf-es), Postgres 16
through testcontainers, nextest, Moon 2.5.3.

**Spec:** docs/superpowers/specs/2026-09-27-sma-712-operator-identity-link-design.md

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Rust crates use edition 2024 and rust-version 1.95. Do not change a crate manifest.
- Branch: `feature/sma-712-operator-link-identity`. Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity`.
- Before each task, run `git -C <worktree> branch --show-current` and confirm the branch.
- Conventional commits with a workspace scope: `feat(rs): … (SMA-712)`, `feat(contracts): … (SMA-712)`, `feat(repo): … (SMA-712)`, `docs(repo): … (SMA-712)`. Allowed scopes: `rs py ts contracts ci docs deps release repo claude workspace`.
- A commit body line must not start with `#NNN` and must not look like `token: value`. commitlint reads such a line as a footer (`footer-leading-blank`).
- End each commit message with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. Never use `git commit --amend`. Never use `git reset`. Never use a bare `git stash`.
- Never run `git checkout -- <file>` to undo an edit. Undo an edit with the Edit tool.
- Before each shell command that needs a proto tool, run `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Rust checks run from `rs/`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --no-tests=pass -p <crate>`.
- The workspace sets `[workspace.lints.rust] warnings = "deny"` (`rs/Cargo.toml:274-275`). Dead code is a compile error. Do not add an item that only a later task uses.
- After you edit a `.proto`, run `buf format -w` in `contracts/`, then `moon run contracts:generate --force`. Do not run a bare `buf generate`.
- After `contracts:generate`, check that `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` still exists. A BSR rate limit deletes it. If it is gone, run `moon run contracts:generate --force` again.
- Docker-gated suites skip silently on a filtered run. Always set `PAIGASUS_REQUIRE_DOCKER=1` when you run one test binary with `--test` or `-E`.
- Do not write an error code as a string literal in a new file under `rs/crates/**/src/`. The gate `repo:error-code-single-site` (`ci/error-registry/check.py:77-120`) scans those files. Assert on `TenancyError` variants or on `ErrorReason::X.as_wire_reason()` there. Literals in `tests/` and in `application/error.rs` are allowed.
- After you touch a `.ts` file, run `moon run ts:fmt`.
- Do not install host software (no `brew`). Do not start background jobs in a subagent.
- Write all prose (comments, docs, commit bodies) in ASD-STE100 Simplified Technical English. Code identifiers stay as they are.

## Review Focus

These five input classes follow from the spec, but no existing test covers them. The most likely
failure is first. Each has a named test in the task that owns it.

1. **Length and whitespace units.** The spec counts Unicode scalar values, not bytes, and a
   non-ASCII whitespace (`U+00A0`, `U+2003`) is whitespace. A `len()` in place of
   `chars().count()` passes every ASCII fixture. Tests:
   `audit_reason_is_trimmed_and_counts_characters_not_bytes`,
   `audit_reason_rejects_empty_and_whitespace_only_values`,
   `external_subject_is_stored_exactly_and_counts_characters_not_bytes`,
   `external_subject_rejects_empty_and_edge_whitespace` (Task 3).
2. **Issuer spelling.** The match is exact after trim. A trailing `/`, a letter-case change or a
   different realm path must give `unknown-issuer`. Padding must be accepted, and the stored
   value must be the configured value. Test: `link_matches_the_issuer_exactly_after_trim`
   (Task 5).
3. **Email letter case and padding.** The unique key and `Email::parse` keep letter case. So a
   change to a case variant of the current email is a real change, a case variant of another
   user's email is not a conflict, and a padded equal email is a no-op. Tests:
   `an_email_that_differs_only_in_letter_case_is_a_change_not_a_conflict`,
   `changing_to_the_current_email_is_a_no_op_with_no_audit_record` (Task 5),
   `a_case_variant_of_another_users_email_is_not_a_conflict_in_postgres` (Task 6).
4. **A principal id that names a service account.** Spec 5.2 says 404. A service account has a
   `principal` row but no `"user"` row. Test:
   `a_service_account_id_is_not_found_on_every_write` (Task 6).
5. **Caller credential variants for the own-identity guard.** The guard must compare issuer AND
   subject together, and it must not apply to an API-key caller. Tests:
   `the_own_identity_guard_matches_issuer_and_subject_together` (Task 5),
   `an_own_identity_refusal_rolls_the_delete_back` (Task 6).

---

### Task 1: ADR in Notion (controller only)

The controller does this task. Do not give it to a subagent. No code changes.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-27-sma-712-operator-identity-link-design.md:1-7` (the header list)

**Interfaces:**
- Consumes: spec section 10 (the ADR draft text).
- Produces: the ADR number `ADR-00NN` and its Notion URL, recorded in the spec header.

- [ ] **Step 1: Open the ADR database.** `CONTRIBUTING.md:298` links it:
  `https://www.notion.so/368830e8fbaa816cb411c7ee1682c175`. Fetch it with the Notion tool.

- [ ] **Step 2: Find the next free number.** Read the highest `ADR-00NN` number in the database.
  The repository mentions ADR-0023 as the highest number that exists (`git grep -oh "ADR-00[0-9][0-9]"`).
  The only mention of ADR-0024 is the spec's own guess (spec line 472). Use the next free number
  in Notion, not the guess.

- [ ] **Step 3: Create the page.** Title: `ADR-00NN: Operator-attested external identity linking in IAM`.
  Status: `Accepted` (Sven approved the spec at Gate 1 on 2026-09-27). Body: the five paragraphs
  of spec section 10 (Title, Context, Decision, Alternatives rejected, Consequences), with the
  quote markers removed. Add a line "Linear: SMA-712" and a link to the spec file on the branch.

- [ ] **Step 4: Record the link in the spec.** Add this line after spec line 6 (`- Related: …`):

```markdown
- ADR: [ADR-00NN](<the Notion URL of the new page>) — Operator-attested external identity linking in IAM
```

- [ ] **Step 5: Commit.**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity add docs/superpowers/specs/2026-09-27-sma-712-operator-identity-link-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity commit -m "docs(repo): link the SMA-712 ADR from the spec (SMA-712)

The ADR records operator attestation as the proof model for an
identity link.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Five error reasons in the registry and in `TenancyError`

This task comes before the value types. The value types in Task 3 map their errors onto these
variants, and every task must compile.

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto:189-190` (insert after `ERROR_REASON_SERVICE_MIGRATING = 39;`)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/`, `py/packages/paigasus-proto/src/paigasus_proto/generated/`, `ts/packages/paigasus-proto/src/generated/`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:198-199` (`EXPECTED_REASONS`), `:233` (count anchor), and the end of its test module
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs:168-170` (variants), `:174-212` (`code`), `:215-244` (`class`), `:263-301` (`field`), `:311-313` (conflict mapping), test module end (`:569`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` test module (after `every_tenancy_code_is_declared_in_the_canonical_registry`, `:962-973`)
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts:11-12` (comment) and after `:61` (`SERVICE_MIGRATING`)
- Modify: `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`
- Modify: `ts/packages/paigasus-proto/src/error.test.ts:57-60`

**Interfaces:**
- Produces (proto): `ERROR_REASON_INVALID_REASON = 41`, `ERROR_REASON_UNKNOWN_ISSUER = 42`,
  `ERROR_REASON_INVALID_SUBJECT = 43`, `ERROR_REASON_CANNOT_UNLINK_OWN_IDENTITY = 44`,
  `ERROR_REASON_EXTERNAL_IDENTITY_EXISTS = 45`.
- Produces (Rust registry): `ErrorReason::{InvalidReason, UnknownIssuer, InvalidSubject, CannotUnlinkOwnIdentity, ExternalIdentityExists}`.
- Produces (service): unit variants `TenancyError::{InvalidReason, UnknownIssuer, InvalidSubject, CannotUnlinkOwnIdentity, ExternalIdentityConflict}`.
- Produces: `TenancyError::from(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)) == TenancyError::ExternalIdentityConflict`.

- [ ] **Step 1: Prove that no caller depends on the old `Internal` mapping.** Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && git grep -n "ExternalIdentityExists" -- rs
```

Expected output (measured when this plan was written):

```text
rs/crates/libs/paigasus-iam-core/src/ports.rs:26:    ExternalIdentityExists,
rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs:81:        ConflictKind::ExternalIdentityExists
rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs:106:        assert_eq!(conflict_kind("uq_external_identity_issuer_subject"), ConflictKind::ExternalIdentityExists);
rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:270: ...
rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:299:            Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)) => self
rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:425: ...
rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:443: ...
rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:939: ...
rs/crates/services/paigasus-iam/src/application/error.rs:313:                ConflictKind::ExternalIdentityExists => Self::Internal,
rs/crates/services/paigasus-iam/tests/authn_identities.rs:65: ...
rs/crates/services/paigasus-iam/tests/authn_identities.rs:84: ...
rs/crates/services/paigasus-iam/tests/authn_identities.rs:85: ...
```

`authenticate_token.rs:299` matches the conflict itself and returns an `AuthnError`. It never
converts the conflict to a `TenancyError`. The only `TenancyError` site is `error.rs:313`. If
the output shows another site that converts this conflict into a `TenancyError`, stop and report it.

- [ ] **Step 2: Write the failing Rust tests.** Append to the test module of
  `rs/crates/services/paigasus-iam/src/application/error.rs` (before its final `}` at line 570):

```rust
    /// SMA-712: the five identity-link codes. Each is a unit variant, so no caller input can
    /// reach an error body. The three validation codes name their field for `ErrorInfo`.
    #[test]
    fn the_identity_link_codes_classify_and_name_their_field() {
        for (err, code, class, field) in [
            (TenancyError::InvalidReason, "invalid-reason", ErrorClass::Validation, Some("reason")),
            (TenancyError::UnknownIssuer, "unknown-issuer", ErrorClass::Validation, Some("issuer")),
            (TenancyError::InvalidSubject, "invalid-subject", ErrorClass::Validation, Some("subject")),
            (TenancyError::CannotUnlinkOwnIdentity, "cannot-unlink-own-identity", ErrorClass::Precondition, None),
            (TenancyError::ExternalIdentityConflict, "external-identity-exists", ErrorClass::Conflict, None),
        ] {
            assert_eq!(err.code(), code);
            assert_eq!(err.class(), class, "{code}");
            assert_eq!(err.field(), field, "{code}");
        }
    }

    /// SMA-712 spec 5.7: the link call is the first tenancy call that produces this conflict, so
    /// it is a 409, not the `Internal` placeholder it was while only JIT produced it.
    #[test]
    fn an_external_identity_conflict_from_the_store_is_a_409() {
        let err = TenancyError::from(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists));
        assert_eq!(err, TenancyError::ExternalIdentityConflict);
        assert_eq!(err.class(), ErrorClass::Conflict);
    }
```

Append to the test module of `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs`,
directly after `every_tenancy_code_is_declared_in_the_canonical_registry` (ends at line 973):

```rust
    /// SMA-712: the five identity-link reasons reach gRPC with the code of their class, and the
    /// three validation reasons carry their field in `ErrorInfo.metadata["field"]`.
    #[test]
    fn the_identity_link_reasons_map_to_their_grpc_codes() {
        for (err, code, field) in [
            (TenancyError::InvalidReason, Code::InvalidArgument, Some("reason")),
            (TenancyError::UnknownIssuer, Code::InvalidArgument, Some("issuer")),
            (TenancyError::InvalidSubject, Code::InvalidArgument, Some("subject")),
            (TenancyError::CannotUnlinkOwnIdentity, Code::FailedPrecondition, None),
            (TenancyError::ExternalIdentityConflict, Code::AlreadyExists, None),
        ] {
            let wire = err.code();
            let status = status_to_grpc(err);
            assert_eq!(status.code(), code, "{wire}");
            let info = status.get_error_details().error_info().cloned().expect("every IAM status carries ErrorInfo");
            assert_eq!(info.reason, wire);
            assert_eq!(info.metadata.get("field").map(String::as_str), field, "{wire}");
        }
    }
```

Append to the test module of `rs/crates/libs/paigasus-proto/src/error.rs`, after
`the_service_migrating_reason_resolves_both_ways` (the last test):

```rust
    /// SMA-712: the five identity-link reasons, asserted by wire string so that a rename that
    /// changes the kebab spelling fails here.
    #[test]
    fn the_identity_link_reasons_resolve_both_ways() {
        for (variant, wire) in [
            (ErrorReason::InvalidReason, "invalid-reason"),
            (ErrorReason::UnknownIssuer, "unknown-issuer"),
            (ErrorReason::InvalidSubject, "invalid-subject"),
            (ErrorReason::CannotUnlinkOwnIdentity, "cannot-unlink-own-identity"),
            (ErrorReason::ExternalIdentityExists, "external-identity-exists"),
        ] {
            assert_eq!(variant.as_wire_reason().as_deref(), Some(wire));
            assert_eq!(ErrorReason::from_wire_reason(wire), Some(variant));
        }
    }
```

- [ ] **Step 3: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-proto -p paigasus-iam -E 'test(/identity_link|external_identity_conflict/)'
```

Expected: compile errors `E0599: no variant or associated item named InvalidReason` (and the
other four names) in `ErrorReason` and `TenancyError`.

- [ ] **Step 4: Add the proto values.** In `contracts/proto/paigasus/common/v1/error.proto`,
  insert after line 189 (`ERROR_REASON_SERVICE_MIGRATING = 39;`) and before the blank line that
  precedes `// ---- Gateway (300-599)`:

```proto

  // ---- IAM: identity links (1-299) -----------------------------------------
  // SMA-712. Emitted by the operator identity-link calls on UserService and
  // on /v1/users/*. Both transports carry every one of them.

  // "invalid-reason" — the audit reason is empty after trim, or it is longer
  // than 500 characters.
  ERROR_REASON_INVALID_REASON = 41;
  // "unknown-issuer" — the issuer is not one of the configured authn issuers.
  ERROR_REASON_UNKNOWN_ISSUER = 42;
  // "invalid-subject" — the subject is empty, it is longer than 255
  // characters, or it starts or ends with whitespace.
  ERROR_REASON_INVALID_SUBJECT = 43;
  // "cannot-unlink-own-identity" — the call would unlink the identity that
  // authenticated the request.
  ERROR_REASON_CANNOT_UNLINK_OWN_IDENTITY = 44;
  // "external-identity-exists" — another user already holds this
  // (issuer, subject) pair.
  ERROR_REASON_EXTERNAL_IDENTITY_EXISTS = 45;
```

- [ ] **Step 5: Format and regenerate the bindings.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/contracts && buf format -w
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && moon run contracts:fmt contracts:lint contracts:breaking && moon run contracts:generate --force
test -f ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts && echo "error_details_pb.ts present"
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity status --short
```

Expected: `contracts:fmt`, `contracts:lint` and `contracts:breaking` pass (new enum values are
not breaking). `status` shows `error.proto` and the three generated `common/v1` files changed,
and no deleted file.

- [ ] **Step 6: Update the Rust registry mirror.** In `rs/crates/libs/paigasus-proto/src/error.rs`,
  after line 199 (`"service-migrating",`) insert:

```rust
        // IAM: identity links (SMA-712)
        "invalid-reason",
        "unknown-issuer",
        "invalid-subject",
        "cannot-unlink-own-identity",
        "external-identity-exists",
```

Then change the count anchor (line 233, which is now line 239):

```rust
        assert_eq!(actual.len(), 65, "the registry should hold 65 reasons");
```

- [ ] **Step 7: Add the `TenancyError` variants.** In
  `rs/crates/services/paigasus-iam/src/application/error.rs`, insert before
  `#[error("internal server error")]` (line 168):

```rust
    /// SMA-712. The audit `reason` of an identity-link write is empty after trim, or it is
    /// longer than 500 characters. A unit variant, so the caller's text never reaches a body.
    #[error("reason must have 1 to 500 characters after trim")]
    InvalidReason,
    /// SMA-712. The `issuer` of a link call is not one of the configured `authn.issuers`.
    #[error("issuer is not a configured issuer")]
    UnknownIssuer,
    /// SMA-712. The `subject` of a link call is empty, is longer than 255 characters, or starts
    /// or ends with whitespace.
    #[error("subject must have 1 to 255 characters and no leading or trailing whitespace")]
    InvalidSubject,
    /// SMA-712. The unlink would remove the identity that authenticated this request.
    #[error("cannot unlink the identity that authenticated this request")]
    CannotUnlinkOwnIdentity,
    /// SMA-712. Another user already holds this `(issuer, subject)` pair
    /// (`ConflictKind::ExternalIdentityExists`, `uq_external_identity_issuer_subject`).
    #[error("this external identity is linked to another user")]
    ExternalIdentityConflict,
```

In `code()`, insert before `Self::Internal => "internal",` (line 210):

```rust
            Self::InvalidReason => "invalid-reason",
            Self::UnknownIssuer => "unknown-issuer",
            Self::InvalidSubject => "invalid-subject",
            Self::CannotUnlinkOwnIdentity => "cannot-unlink-own-identity",
            Self::ExternalIdentityConflict => "external-identity-exists",
```

In `class()`, replace lines 236-240 with:

```rust
            | Self::InvalidAction(_)
            | Self::InvalidBulkReplay
            | Self::InvalidReason
            | Self::UnknownIssuer
            | Self::InvalidSubject => ErrorClass::Validation,
            Self::NotFound => ErrorClass::NotFound,
            Self::SlugConflict | Self::DuplicateMembership | Self::EmailConflict | Self::ServiceAccountNameConflict | Self::PolicyConflict(_) | Self::ExternalIdentityConflict => ErrorClass::Conflict,
            Self::ParentArchived
            | Self::NodeArchived
            | Self::MissingOrgMembership
            | Self::SystemImmutable(_)
            | Self::NotSystemOwned(_)
            | Self::FleetNotConverged
            | Self::CannotUnlinkOwnIdentity => ErrorClass::Precondition,
```

In `field()`, insert after the first arm (after line 272, `| Self::InvalidPathSegment(f) => Some(f),`):

```rust
            Self::InvalidReason => Some("reason"),
            Self::UnknownIssuer => Some("issuer"),
            Self::InvalidSubject => Some("subject"),
```

and in the `None` arm, replace `| Self::FleetNotConverged` (line 298) with:

```rust
            | Self::FleetNotConverged
            | Self::CannotUnlinkOwnIdentity
            | Self::ExternalIdentityConflict
```

In `From<RepositoryError>`, replace lines 311-313 with:

```rust
                // SMA-712: `UserIdentityService::link` produces it (another user holds the
                // `(issuer, subject)` pair, or a JIT login inserted it first). JIT itself
                // handles its own conflict in `authenticate_token.rs` before any conversion.
                ConflictKind::ExternalIdentityExists => Self::ExternalIdentityConflict,
```

- [ ] **Step 8: Update the TypeScript mirrors.** In
  `ts/packages/paigasus-sdk/src/errors/presentation.ts`, change line 12 from
  `// demanding an entry for it would make the table 61 keys rather than 60.` to:

```ts
// demanding an entry for it would make the table 66 keys rather than 65.
```

and insert after line 61 (`[ErrorReason.SERVICE_MIGRATING]: 'from-transport',`):

```ts
  // SMA-712. The operator identity-link calls. The transport status table already presents
  // 400 as invalid input and 409 as a conflict.
  [ErrorReason.INVALID_REASON]: 'from-transport',
  [ErrorReason.UNKNOWN_ISSUER]: 'from-transport',
  [ErrorReason.INVALID_SUBJECT]: 'from-transport',
  [ErrorReason.CANNOT_UNLINK_OWN_IDENTITY]: 'from-transport',
  [ErrorReason.EXTERNAL_IDENTITY_EXISTS]: 'from-transport',
```

In `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`, change `toHaveLength(60)` to
`toHaveLength(65)`. In `ts/packages/paigasus-proto/src/error.test.ts`, replace lines 57-60 with:

```ts
    // Cardinality guard. The Rust mirror asserts 65 at
    // rs/crates/libs/paigasus-proto/src/error.rs:239; the two must agree,
    // because both derive from the same proto.
    expect(values).toHaveLength(65);
```

(Confirm with `grep -n "the registry should hold 65" rs/crates/libs/paigasus-proto/src/error.rs`
that the Rust assert is on line 239. Put the real line number in the comment.)

- [ ] **Step 9: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-proto -p paigasus-iam -E 'test(/identity_link|external_identity_conflict|registry|every_tenancy_code/)'
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && moon run paigasus-proto-ts:test paigasus-proto-ts:typecheck paigasus-sdk-ts:test paigasus-sdk-ts:typecheck ts:fmt
python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every command passes. `the_registry_contains_exactly_the_expected_reasons` passes with 65.

- [ ] **Step 10: Commit.**

```bash
W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity
git -C $W add contracts/proto/paigasus/common/v1/error.proto rs/crates/libs/paigasus-proto rs/crates/services/paigasus-iam/src/application/error.rs rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs py/packages/paigasus-proto/src/paigasus_proto/generated ts/packages/paigasus-proto ts/packages/paigasus-sdk/src/errors/presentation.ts ts/packages/paigasus-sdk/tests/presentation.test.ts
git -C $W commit -m "feat(contracts): add the five identity-link error reasons (SMA-712)

The registry, its Rust and TypeScript mirrors and TenancyError get
invalid-reason, unknown-issuer, invalid-subject,
cannot-unlink-own-identity and external-identity-exists. A store
conflict on an external identity is now a 409, not an internal error.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

If the sandbox refuses the `W=` variable form, write the two commands into
`/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/71683ef6-8e71-4103-8682-5898ec0d0dbe/scratchpad/commit-task2.sh`
and run `/bin/bash` on that file.

---

### Task 3: Value types `AuditReason` and `ExternalSubject`

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/value.rs:3` (module doc), `:11-24` (`DomainError`), after `:49` (new types), test module (`:101-135`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs:39`
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs:335-350` (`From<DomainError>`) and its test module

**Interfaces:**
- Consumes: `TenancyError::{InvalidReason, InvalidSubject}` (Task 2).
- Produces:
  - `pub const AUDIT_REASON_MAX_CHARS: usize = 500;`
  - `pub const EXTERNAL_SUBJECT_MAX_CHARS: usize = 255;`
  - `pub struct AuditReason(String)` with `pub fn parse(raw: &str) -> Result<AuditReason, DomainError>` and `pub fn as_str(&self) -> &str`.
  - `pub struct ExternalSubject(String)` with `pub fn parse(raw: &str) -> Result<ExternalSubject, DomainError>`, `pub fn as_str(&self) -> &str`, `pub fn into_string(self) -> String`.
  - `DomainError::InvalidReason`, `DomainError::InvalidSubject` (unit variants).
  - Re-exports `paigasus_iam_core::{AuditReason, ExternalSubject}`.

- [ ] **Step 1: Write the failing tests.** Append to the test module in
  `rs/crates/libs/paigasus-iam-core/src/value.rs` (before its final `}`):

```rust
    /// SMA-712 spec 5.5: the reason is trimmed, and the limit counts Unicode scalar values, not
    /// bytes. `é` is two bytes, so a byte count would refuse the 500-character value.
    #[test]
    fn audit_reason_is_trimmed_and_counts_characters_not_bytes() {
        assert_eq!(AuditReason::parse("  INC-42: same person  ").unwrap().as_str(), "INC-42: same person");
        let max = "é".repeat(AUDIT_REASON_MAX_CHARS);
        assert_eq!(AuditReason::parse(&format!("  {max}\n")).unwrap().as_str(), max);
        assert_eq!(AuditReason::parse(&"é".repeat(AUDIT_REASON_MAX_CHARS + 1)), Err(DomainError::InvalidReason));
    }

    /// A reason of whitespace only is empty after trim. `U+00A0` and `U+2003` are whitespace.
    #[test]
    fn audit_reason_rejects_empty_and_whitespace_only_values() {
        for bad in ["", "   ", "\n\t", "\u{00A0}\u{2003}"] {
            assert_eq!(AuditReason::parse(bad), Err(DomainError::InvalidReason), "{bad:?}");
        }
    }

    /// SMA-712 spec 5.2: the subject is stored exactly as given. JIT stores the `sub` claim with
    /// no change, so a trimmed value would never match a token. Interior spaces are legal.
    #[test]
    fn external_subject_is_stored_exactly_and_counts_characters_not_bytes() {
        assert_eq!(ExternalSubject::parse("f:1b2c:user 7").unwrap().as_str(), "f:1b2c:user 7");
        let max = "é".repeat(EXTERNAL_SUBJECT_MAX_CHARS);
        assert_eq!(ExternalSubject::parse(&max).unwrap().into_string(), max);
        assert_eq!(ExternalSubject::parse(&"é".repeat(EXTERNAL_SUBJECT_MAX_CHARS + 1)), Err(DomainError::InvalidSubject));
    }

    /// A subject that starts or ends with whitespace is refused, not trimmed.
    #[test]
    fn external_subject_rejects_empty_and_edge_whitespace() {
        for bad in ["", " ", " abc", "abc ", "\u{00A0}abc", "abc\n", "\tabc"] {
            assert_eq!(ExternalSubject::parse(bad), Err(DomainError::InvalidSubject), "{bad:?}");
        }
    }
```

Append to the test module of `rs/crates/services/paigasus-iam/src/application/error.rs`:

```rust
    /// SMA-712: the two identity-link value types keep their own codes through the funnel.
    #[test]
    fn the_identity_link_domain_errors_map_to_their_own_codes() {
        assert_eq!(TenancyError::from(DomainError::InvalidReason), TenancyError::InvalidReason);
        assert_eq!(TenancyError::from(DomainError::InvalidSubject), TenancyError::InvalidSubject);
    }
```

- [ ] **Step 2: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam-core -p paigasus-iam -E 'test(/audit_reason|external_subject|identity_link_domain/)'
```

Expected: compile errors `cannot find type AuditReason`, `cannot find type ExternalSubject`,
`no variant named InvalidReason` in `DomainError`.

- [ ] **Step 3: Implement.** In `rs/crates/libs/paigasus-iam-core/src/value.rs`, change line 3 to:

```rust
//! Domain value objects: `Email`, `PrincipalId`, `Stamp`, `AuditReason` and `ExternalSubject`.
```

Add to `DomainError`, after `InvalidApiKeyToken(String),` (line 23):

```rust
    /// SMA-712. A unit variant: the caller's reason text is never kept in the error.
    #[error("invalid audit reason")]
    InvalidReason,
    /// SMA-712. A unit variant: the caller's subject is never kept in the error.
    #[error("invalid external subject")]
    InvalidSubject,
```

Insert after the `impl Email` block (after line 49):

```rust
/// The longest audit reason, in Unicode scalar values after trim (SMA-712 spec 5.5).
pub const AUDIT_REASON_MAX_CHARS: usize = 500;

/// The longest external subject, in Unicode scalar values (SMA-712 spec 5.2).
pub const EXTERNAL_SUBJECT_MAX_CHARS: usize = 255;

/// The operator's reason for an identity-link write (SMA-712). The value is trimmed. After the
/// trim it has 1 to [`AUDIT_REASON_MAX_CHARS`] characters. It goes into the audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReason(String);

impl AuditReason {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        let chars = trimmed.chars().count();
        if chars == 0 || chars > AUDIT_REASON_MAX_CHARS {
            return Err(DomainError::InvalidReason);
        }
        Ok(AuditReason(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An OIDC `sub` value that an operator links to a user (SMA-712). It is NOT trimmed: JIT stores
/// the `sub` claim with no change, and a changed value would never match a token. It has 1 to
/// [`EXTERNAL_SUBJECT_MAX_CHARS`] characters, and it does not start or end with whitespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSubject(String);

impl ExternalSubject {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let chars = raw.chars().count();
        if chars == 0 || chars > EXTERNAL_SUBJECT_MAX_CHARS || raw.starts_with(char::is_whitespace) || raw.ends_with(char::is_whitespace) {
            return Err(DomainError::InvalidSubject);
        }
        Ok(ExternalSubject(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
```

In `rs/crates/libs/paigasus-iam-core/src/lib.rs`, replace line 39 with:

```rust
pub use value::{AUDIT_REASON_MAX_CHARS, AuditReason, DomainError, EXTERNAL_SUBJECT_MAX_CHARS, Email, ExternalSubject, PrincipalId, Stamp};
```

In `rs/crates/services/paigasus-iam/src/application/error.rs`, in `From<DomainError>`, insert
after `DomainError::InvalidApiKeyToken(_) => Self::Internal,` (line 347):

```rust
            // SMA-712: the identity-link value types.
            DomainError::InvalidReason => Self::InvalidReason,
            DomainError::InvalidSubject => Self::InvalidSubject,
```

- [ ] **Step 4: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam-core -p paigasus-iam -E 'test(/audit_reason|external_subject|identity_link_domain|email_/)'
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS, and no fmt or clippy output.

- [ ] **Step 5: Commit.**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity add rs/crates/libs/paigasus-iam-core/src/value.rs rs/crates/libs/paigasus-iam-core/src/lib.rs rs/crates/services/paigasus-iam/src/application/error.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity commit -m "feat(rs): add the AuditReason and ExternalSubject value types (SMA-712)

A reason is trimmed and has 1 to 500 characters. A subject is kept as
given, has 1 to 255 characters and has no whitespace at either end.
Both count Unicode scalar values, not bytes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Four Cedar actions and starter policy revision 4

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs:57-63` (enum), `:67-109` (`ALL`), `:114-157` (`as_wire`), `:176-219` (`is_write`), `:258-313` (`all_covers_every_variant`), test module end
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs:26-27` (action list), test module end (`:76`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:55-62` (revision), `:90` (hash), starter table after `:675`, test module end (`:833`)
- Modify: `docs/superpowers/specs/2026-09-27-sma-712-operator-identity-link-design.md:461-466` (section 8, binary rollback)

**Interfaces:**
- Produces: `Action::GetUser` (read), `Action::LinkExternalIdentity`, `Action::UnlinkExternalIdentity`, `Action::ChangeUserEmail` (writes). Wire names equal the variant names.
- Produces: `STARTER_POLICY_REVISION == 4`. `Action::ALL.len() == 45`.

- [ ] **Step 1: Read the SMA-584 rollback analysis.** Read
  `docs/superpowers/specs/2026-08-23-sma-584-create-user-authorization-design.md:275-281`
  (section 4.4). Then confirm two facts in the code:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity
sed -n 43,62p rs/crates/libs/paigasus-iam-core/src/authz/roles.rs
grep -n "schema()" rs/crates/libs/paigasus-iam-core/src/authz/engine.rs
```

Expected: the doc at `roles.rs:46-51` says a replica with a LOWER revision leaves a stored row
alone (SMA-477 D11). `engine.rs` uses `schema()` only at lines 119 (entities) and 129 (the
request). It does not validate the stored policy set against the schema at compile time.

- [ ] **Step 2: Write the failing tests.** Append to the test module of `action.rs`:

```rust
    /// SMA-712: `GetUser` is a read; the three identity writes are writes, not restores, so
    /// they reach the generated `forbid-archived-writes` list.
    #[test]
    fn the_user_identity_actions_are_classified_and_round_trip() {
        assert!(!Action::GetUser.is_write(), "finding a user is a read");
        for a in [Action::LinkExternalIdentity, Action::UnlinkExternalIdentity, Action::ChangeUserEmail] {
            assert!(a.is_write(), "{} changes a user", a.as_wire());
            assert!(!a.is_restore(), "{} is not a restore", a.as_wire());
        }
        for a in [Action::GetUser, Action::LinkExternalIdentity, Action::UnlinkExternalIdentity, Action::ChangeUserEmail] {
            assert_eq!(Action::parse(a.as_wire()), Some(a), "{} must round-trip", a.as_wire());
            assert!(Action::ALL.contains(&a), "{} must be in ALL", a.as_wire());
        }
    }
```

Append to the test module of `schema.rs`:

```rust
    /// SMA-712: the twin of the two tests above for the four identity actions. A name present
    /// in `Action::ALL` but missing from `SCHEMA_SRC` makes the generated forbid source fail
    /// validation at boot.
    #[test]
    fn the_user_identity_actions_validate_against_the_embedded_schema() {
        for name in ["GetUser", "LinkExternalIdentity", "UnlinkExternalIdentity", "ChangeUserEmail"] {
            let src = format!(r#"permit(principal, action == Pgs::Iam::Action::"{name}", resource);"#);
            assert!(validate_policy(&src).is_ok(), "{name} must be declared in SCHEMA_SRC");
        }
    }
```

Append to the test module of `roles.rs`:

```rust
    /// SMA-712: the three identity writes reach the generated forbid list, which is the reason
    /// `STARTER_POLICY_REVISION` moves to 4. `GetUser` is a read and must not be there.
    #[test]
    fn the_user_identity_writes_are_in_the_generated_forbid_source_and_get_user_is_not() {
        let src = forbid_archived_writes_source();
        for name in ["LinkExternalIdentity", "UnlinkExternalIdentity", "ChangeUserEmail"] {
            assert!(src.contains(&format!(r#"Pgs::Iam::Action::"{name}""#)), "{name} is a write, so it must appear in forbid-archived-writes");
        }
        assert!(!src.contains(r#"Pgs::Iam::Action::"GetUser""#), "GetUser is a read and must not appear in forbid-archived-writes");
    }
```

In `roles.rs` `starter_policy_table`, insert these cases directly after the case named
`"org_admin denies CreateUser at Root (D4 allow-list regression guard)"` (its `},` is line 674):

```rust
            // -- SMA-712: platform_admin's template has no action list, so it permits the four
            // identity actions with no change. No other starter role carries them.
            Case {
                name: "platform_admin at Root allows GetUser at Root (SMA-712)",
                grants: vec![grant(110, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::GetUser,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "platform_admin at Root allows LinkExternalIdentity at Root (SMA-712)",
                grants: vec![grant(111, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::LinkExternalIdentity,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "platform_admin at Root allows UnlinkExternalIdentity at Root (SMA-712)",
                grants: vec![grant(112, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::UnlinkExternalIdentity,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "platform_admin at Root allows ChangeUserEmail at Root (SMA-712)",
                grants: vec![grant(113, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::ChangeUserEmail,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_admin denies LinkExternalIdentity at Root (SMA-712)",
                grants: vec![grant(114, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::LinkExternalIdentity,
                resource: root_prn(),
                expect: Effect::Deny,
            },
```

(Grant ids 110-114 are unused: the highest id in the table is 97.)

- [ ] **Step 3: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam-core -E 'test(/user_identity|starter_policy/)'
```

Expected: compile errors `no variant named GetUser` (and the other three) in `Action`.

- [ ] **Step 4: Add the variants.** In `action.rs`, after `CreateUser,` (line 62) insert:

```rust
    /// Read one user with its external identities (SMA-712, `FindUserByEmail`). Authorized at
    /// `Root` only, in `UserIdentityService`, not in the Cedar schema.
    GetUser,
    /// Link an external `(issuer, subject)` to a user (SMA-712). Equal to `platform_admin` in
    /// power: a holder can attach an identity that it controls to any user.
    LinkExternalIdentity,
    /// Unlink an external identity from a user (SMA-712). A holder can lock out any user.
    UnlinkExternalIdentity,
    /// Change a user's email (SMA-712).
    ChangeUserEmail,
```

In `ALL`, after `Action::CreateUser,` (line 108) insert:

```rust
        Action::GetUser,
        Action::LinkExternalIdentity,
        Action::UnlinkExternalIdentity,
        Action::ChangeUserEmail,
```

In `as_wire`, after `Action::CreateUser => "CreateUser",` (line 156) insert:

```rust
            Action::GetUser => "GetUser",
            Action::LinkExternalIdentity => "LinkExternalIdentity",
            Action::UnlinkExternalIdentity => "UnlinkExternalIdentity",
            Action::ChangeUserEmail => "ChangeUserEmail",
```

In `is_write`, replace `| Action::ListOutboxDeadLetters => false,` (line 191) with:

```rust
            | Action::ListOutboxDeadLetters
            | Action::GetUser => false,
```

and replace `| Action::CreateUser => true,` (line 218) with:

```rust
            | Action::CreateUser
            | Action::LinkExternalIdentity
            | Action::UnlinkExternalIdentity
            | Action::ChangeUserEmail => true,
```

In `all_covers_every_variant`, replace `| Action::CreateUser => {}` (line 302) with:

```rust
                | Action::CreateUser
                | Action::GetUser
                | Action::LinkExternalIdentity
                | Action::UnlinkExternalIdentity
                | Action::ChangeUserEmail => {}
```

and replace the count assertion (lines 308-312) with:

```rust
        assert_eq!(
            Action::ALL.len(),
            45,
            "27 pre-existing + 7 M4 + 1 audit + 1 invoke-model + 3 outbox dead-letter + 1 SMA-481 RetireSystemPolicy + 1 SMA-584 CreateUser + 4 SMA-712 user identity"
        );
```

In `schema.rs`, replace lines 26-27 with:

```rust
         ReplayOutboxDeadLetter, DiscardOutboxDeadLetter, RetireSystemPolicy, InvokeModel,
         CreateUser, GetUser, LinkExternalIdentity, UnlinkExternalIdentity, ChangeUserEmail
```

- [ ] **Step 5: Run the tests. The content pin now fails and prints the new hash.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam-core -E 'test(starter_policy_content_is_pinned_to_the_declared_revision)'
```

Expected: FAIL. The message says `The starter policy set's content changed` and prints
`2. Replace EXPECTED_STARTER_CONTENT_HASH with:` followed by a 64-character hex string. The test
(`roles.rs:792-813`) computes this value: a `blake3` hash over each starter policy's `policy_id`
and `content_fingerprint(kind, source, description)`. Copy the printed string exactly.

- [ ] **Step 6: Bump the revision and pin the new hash.** In `roles.rs`, replace lines 58-62 with:

```rust
/// `3`: SMA-584 added `CreateUser` for the same reason. The forbid can never actually bite on
/// it (`entity Root;` declares no attributes, so `resource has effective_status` is
/// unsatisfiable at `Root`), but the action list is *derived*, not hand-written, so the
/// content moves and every deployed database now holds an older set.
///
/// `4`: SMA-712 added `LinkExternalIdentity`, `UnlinkExternalIdentity` and `ChangeUserEmail`.
/// They are non-restore writes, so they join the generated forbid list. `GetUser` is a read and
/// does not change the content.
pub const STARTER_POLICY_REVISION: u32 = 4;
```

Replace the value of `EXPECTED_STARTER_CONTENT_HASH` (line 90) with the string that step 5 printed.

- [ ] **Step 7: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam-core
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every `paigasus-iam-core` test passes, including `all_covers_every_variant`,
`starter_policy_table`, `every_starter_policy_passes_schema_validation` and the content pin.

- [ ] **Step 8: Record the rollback behaviour in spec section 8.** In the spec, replace the
  "**Binary rollback.**" bullet (lines 461-466) with:

```markdown
- **Binary rollback.** Plan Task 4 read the SMA-584 analysis (SMA-584 spec, section 4.4) and the
  code. An old binary (revision 3) that boots against starter revision 4 leaves the stored rows
  alone: a replica whose `STARTER_POLICY_REVISION` is lower than a stored row's revision does not
  rewrite that row (SMA-477 D11, `roles.rs:46-51`). So a mixed or rolled-back fleet keeps the
  revision-4 set and does not flap. The old binary compiles the stored `forbid-archived-writes`
  source without a schema check (`engine.rs` uses the schema only for entities and the request,
  lines 119 and 129). The three new action names in its `action in [...]` list therefore never
  match a request of the old binary, and the old binary has no route that asks for them. The
  forbid also cannot match at `Root`, because `Root` has no `effective_status` attribute. Other
  state that stays after a rollback: audit rows with the new action names (the audit query reads
  `action` as text, so they stay readable), and the new `ERROR_REASON_*` values (old clients see
  an unknown enum value). No row that the new calls wrote needs an old-binary change: links and
  email values are ordinary rows.
```

- [ ] **Step 9: Commit.**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity add rs/crates/libs/paigasus-iam-core/src/authz/action.rs rs/crates/libs/paigasus-iam-core/src/authz/schema.rs rs/crates/libs/paigasus-iam-core/src/authz/roles.rs docs/superpowers/specs/2026-09-27-sma-712-operator-identity-link-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity commit -m "feat(rs): add the four user identity Cedar actions (SMA-712)

GetUser, LinkExternalIdentity, UnlinkExternalIdentity and
ChangeUserEmail join the catalog and the schema. The three writes join
the generated forbid list, so the starter policy revision moves to 4.
The spec now records what an old binary does with revision 4.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Port `IdentityLinkStore`, its fake, and `UserIdentityService`

The port has no caller until the service exists. So the port, the fake and the service are one
task (the `-D warnings` staging rule).

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:15` (imports), after `:115` (new port), test module (`:486-535`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs:30-34` (re-exports)
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs:10-24` (imports), `:803-824` (`FakeAuthorizer`), before the first `#[cfg(test)]` at `:1499` (new fake)
- Create: `rs/crates/services/paigasus-iam/src/application/user_identities.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs:27-28`

**Interfaces:**
- Consumes: `AuditReason`, `ExternalSubject` (Task 3), the five `TenancyError` variants (Task 2), the four `Action` variants (Task 4).
- Produces (core, `paigasus_iam_core`):

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct UserWithIdentities { pub user: User, pub status: PrincipalStatus, pub identities: Vec<ExternalIdentity> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailChange { pub old: Email, pub new: Email }

#[async_trait]
pub trait IdentityLinkStore: Send + Sync {
    async fn find_user_by_email(&self, email: &Email) -> Result<Option<UserWithIdentities>, RepositoryError>;
    async fn lock_user_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<User>, RepositoryError>;
    async fn find_identity_in(&self, tx: &dyn Transaction, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError>;
    async fn link_in(&self, tx: &dyn Transaction, identity: &ExternalIdentity) -> Result<(), RepositoryError>;
    async fn unlink_in(&self, tx: &dyn Transaction, user: &PrincipalId, identity_id: Uuid) -> Result<Option<ExternalIdentity>, RepositoryError>;
    async fn change_email_in(&self, tx: &dyn Transaction, user: &PrincipalId, email: &Email, now: DateTime<Utc>) -> Result<Option<Mutated<EmailChange>>, RepositoryError>;
    async fn user_view_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<UserWithIdentities>, RepositoryError>;
}
```

- Produces (service, `crate::application::user_identities`):

```rust
pub struct UserIdentityDeps {
    pub authorize: Authorize,
    pub links: Arc<dyn IdentityLinkStore>,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub issuers: Vec<Issuer>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}
#[derive(Clone)] pub struct UserIdentityService { /* private */ }
impl UserIdentityService {
    pub fn new(deps: UserIdentityDeps) -> Self;
    pub async fn find_by_email(&self, actor: &Prn, email: &str) -> Result<UserWithIdentities, TenancyError>;
    pub async fn link(&self, actor: &Prn, user: &PrincipalId, issuer: &str, subject: &str, reason: &str) -> Result<Mutated<ExternalIdentity>, TenancyError>;
    pub async fn unlink(&self, actor: &Prn, caller: &Credential, user: &PrincipalId, identity_id: Uuid, reason: &str) -> Result<(), TenancyError>;
    pub async fn change_email(&self, actor: &Prn, user: &PrincipalId, email: &str, reason: &str) -> Result<UserWithIdentities, TenancyError>;
}
```

- Produces (fakes, `#[cfg(test)]`): `FakeAuthorizer::checks(&self) -> Vec<(Action, String)>`;
  `InMemoryIdentityLinks` with `seed_user`, `seed_identity`, `calls`, `fail_next_link_with_conflict`, `identities`, `user`.

- [ ] **Step 1: Add the port to the core.** In `rs/crates/libs/paigasus-iam-core/src/ports.rs`,
  change line 15 to:

```rust
use crate::value::{Email, PrincipalId, Stamp};
```

Insert after the `ExternalIdentityRepository` trait (after line 115):

```rust
/// A user with its principal status and its external identities, in `(created_at, id)` order
/// (SMA-712). The read model of the operator identity-link calls.
#[derive(Debug, Clone, PartialEq)]
pub struct UserWithIdentities {
    pub user: User,
    pub status: PrincipalStatus,
    pub identities: Vec<ExternalIdentity>,
}

/// The two emails of one email change (SMA-712). `old` comes from the row that the transaction
/// locked, never from an earlier read, so each audit record names the correct old email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailChange {
    pub old: Email,
    pub new: Email,
}

/// The narrow port of the operator identity-link calls (SMA-712 spec 6.2). It is separate from
/// [`ExternalIdentityRepository`], which stays read-and-provision only on the authentication
/// path. Every read that decides an audit value or a no-op happens inside the caller's
/// transaction.
#[async_trait]
pub trait IdentityLinkStore: Send + Sync {
    /// The user with this exact email, or `None` when no USER has it.
    async fn find_user_by_email(&self, email: &Email) -> Result<Option<UserWithIdentities>, RepositoryError>;
    /// Locks the `"user"` row FOR SHARE. `None` when the principal is not a user (unknown id, or
    /// a service account).
    async fn lock_user_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<User>, RepositoryError>;
    /// The identity with this `(issuer, subject)`, or `None`.
    async fn find_identity_in(&self, tx: &dyn Transaction, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError>;
    /// Inserts the link. `Conflict(ExternalIdentityExists)` on the unique constraint.
    async fn link_in(&self, tx: &dyn Transaction, identity: &ExternalIdentity) -> Result<(), RepositoryError>;
    /// Deletes the identity only when it belongs to `user`, and returns the deleted row. `None`
    /// when no row matched.
    async fn unlink_in(&self, tx: &dyn Transaction, user: &PrincipalId, identity_id: Uuid) -> Result<Option<ExternalIdentity>, RepositoryError>;
    /// Locks the `"user"` row FOR UPDATE, then updates the email and `updated_at` when the email
    /// differs. `changed == false` is the no-op. `None` when the principal is not a user.
    /// `Conflict(EmailTaken)` when another user has the email.
    async fn change_email_in(&self, tx: &dyn Transaction, user: &PrincipalId, email: &Email, now: DateTime<Utc>) -> Result<Option<Mutated<EmailChange>>, RepositoryError>;
    /// The user as the transaction sees it now, for the `ChangeUserEmail` response.
    async fn user_view_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<UserWithIdentities>, RepositoryError>;
}
```

In the `ports.rs` test module, after `fn audit_log_is_object_safe(_: &dyn AuditLog) {}` (line 517) insert:

```rust

    // Compile-time proof the SMA-712 identity-link port is object-safe (injected as a trait
    // object into `UserIdentityService`).
    #[allow(dead_code)]
    fn identity_link_store_is_object_safe(_: &dyn IdentityLinkStore) {}
```

In `rs/crates/libs/paigasus-iam-core/src/lib.rs`, replace lines 30-34 with:

```rust
pub use ports::{
    ApiKeyRepository, AuditLog, Authenticator, Clock, ConflictKind, EmailChange, EntityGenBumper, EventPublisher, ExternalIdentityRepository, IdGenerator, IdentityLinkStore, KeyEntropy,
    MembershipAxis, MembershipKindQuery, MembershipRecord, MembershipRepository, Mutated, NodeView, OrganizationRepository, Outbox, PolicyGenBumper, PreconditionKind, PrincipalRepository,
    ProjectRepository, PublishError, RepositoryError, Savepoint, SecretHasher, ServiceAccountRepository, TeamRepository, Transaction, UnitOfWork, UserWithIdentities,
};
```

- [ ] **Step 2: Make `FakeAuthorizer` record its checks.** In
  `rs/crates/services/paigasus-iam/src/application/fakes.rs`, replace lines 803-824 (from the
  `#[derive(Clone, Default)]` above `pub struct FakeAuthorizer` to the end of
  `impl Authorizer for FakeAuthorizer`) with:

```rust
#[derive(Clone, Default)]
pub struct FakeAuthorizer {
    allowed: Arc<Mutex<HashSet<(Action, String)>>>,
    checks: Arc<Mutex<Vec<(Action, String)>>>,
}

impl FakeAuthorizer {
    pub fn allow(&self, action: Action, resource: &Prn) {
        self.allowed.lock().unwrap().insert((action, resource.canonical()));
    }

    /// Every `(action, resource canonical prn)` pair this fake was asked about, in call order
    /// (SMA-712). A test reads it to prove WHICH action a method checks, and at WHICH resource.
    pub fn checks(&self) -> Vec<(Action, String)> {
        self.checks.lock().unwrap().clone()
    }
}

#[async_trait]
impl Authorizer for FakeAuthorizer {
    async fn is_authorized(&self, req: &AccessRequest) -> Result<Decision, AuthzError> {
        self.checks.lock().unwrap().push((req.action, req.resource.canonical()));
        let allow = self.allowed.lock().unwrap().contains(&(req.action, req.resource.canonical()));
        Ok(Decision {
            effect: if allow { Effect::Allow } else { Effect::Deny },
            determining_policies: Vec::new(),
        })
    }
}
```

- [ ] **Step 3: Add the in-memory fake.** In `fakes.rs`, add after line 24 (`use uuid::Uuid;`):

```rust
use paigasus_iam_core::{Email, EmailChange, ExternalIdentity, IdentityLinkStore, Issuer, User, UserWithIdentities};
use std::sync::atomic::AtomicBool;
```

Insert this block directly before the first `#[cfg(test)]` in `fakes.rs` (line 1499, the
`mod tests` whose first helper is `fn org(uuid: Uuid, slug: &str, stamp: &Stamp)`):

```rust
/// In-memory [`IdentityLinkStore`] for `user_identities.rs` unit tests (SMA-712). Like every fake
/// here it ignores the `&dyn Transaction` and mutates at once (see [`FakeUnitOfWork`]), so a
/// write that the service later abandons is NOT rolled back here. `tests/user_identities_pg.rs`
/// proves the rollback against Postgres. `calls()` counts every port call, so a test can prove
/// that a denied caller never reached the store.
#[derive(Clone, Default)]
pub struct InMemoryIdentityLinks {
    users: Arc<Mutex<Vec<User>>>,
    identities: Arc<Mutex<Vec<ExternalIdentity>>>,
    calls: Arc<AtomicUsize>,
    fail_next_link: Arc<AtomicBool>,
}

impl InMemoryIdentityLinks {
    pub fn seed_user(&self, user: User) {
        self.users.lock().unwrap().push(user);
    }

    pub fn seed_identity(&self, identity: ExternalIdentity) {
        self.identities.lock().unwrap().push(identity);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The next `link_in` fails with `Conflict(ExternalIdentityExists)`, as when a JIT login
    /// inserts the same `(issuer, subject)` between the service's read and its insert.
    pub fn fail_next_link_with_conflict(&self) {
        self.fail_next_link.store(true, Ordering::SeqCst);
    }

    pub fn identities(&self) -> Vec<ExternalIdentity> {
        self.identities.lock().unwrap().clone()
    }

    pub fn user(&self, id: &PrincipalId) -> Option<User> {
        self.users.lock().unwrap().iter().find(|u| u.principal_id.uuid() == id.uuid()).cloned()
    }

    fn touch(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    fn view(&self, id: &PrincipalId) -> Option<UserWithIdentities> {
        let user = self.user(id)?;
        let mut identities: Vec<ExternalIdentity> = self.identities.lock().unwrap().iter().filter(|i| i.principal_id.uuid() == id.uuid()).cloned().collect();
        identities.sort_by(|a, b| (a.created_at, a.id).cmp(&(b.created_at, b.id)));
        Some(UserWithIdentities {
            user,
            status: PrincipalStatus::Active,
            identities,
        })
    }
}

#[async_trait]
impl IdentityLinkStore for InMemoryIdentityLinks {
    async fn find_user_by_email(&self, email: &Email) -> Result<Option<UserWithIdentities>, RepositoryError> {
        self.touch();
        let id = self.users.lock().unwrap().iter().find(|u| u.email == *email).map(|u| u.principal_id.clone());
        Ok(id.and_then(|id| self.view(&id)))
    }

    async fn lock_user_in(&self, _tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<User>, RepositoryError> {
        self.touch();
        Ok(self.user(id))
    }

    async fn find_identity_in(&self, _tx: &dyn Transaction, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
        self.touch();
        Ok(self.identities.lock().unwrap().iter().find(|i| i.issuer == *issuer && i.subject == subject).cloned())
    }

    async fn link_in(&self, _tx: &dyn Transaction, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
        self.touch();
        let mut identities = self.identities.lock().unwrap();
        let taken = identities.iter().any(|i| i.issuer == identity.issuer && i.subject == identity.subject);
        if self.fail_next_link.swap(false, Ordering::SeqCst) || taken {
            return Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists));
        }
        identities.push(identity.clone());
        Ok(())
    }

    async fn unlink_in(&self, _tx: &dyn Transaction, user: &PrincipalId, identity_id: Uuid) -> Result<Option<ExternalIdentity>, RepositoryError> {
        self.touch();
        let mut identities = self.identities.lock().unwrap();
        let position = identities.iter().position(|i| i.id == identity_id && i.principal_id.uuid() == user.uuid());
        Ok(position.map(|p| identities.remove(p)))
    }

    async fn change_email_in(&self, _tx: &dyn Transaction, user: &PrincipalId, email: &Email, now: DateTime<Utc>) -> Result<Option<Mutated<EmailChange>>, RepositoryError> {
        self.touch();
        let mut users = self.users.lock().unwrap();
        // Postgres locks the row first, so an unknown user is `None` before any conflict.
        let Some(index) = users.iter().position(|u| u.principal_id.uuid() == user.uuid()) else {
            return Ok(None);
        };
        let old = users[index].email.clone();
        if old == *email {
            return Ok(Some(Mutated {
                value: EmailChange { old: old.clone(), new: old },
                changed: false,
            }));
        }
        if users.iter().any(|u| u.email == *email) {
            return Err(RepositoryError::Conflict(ConflictKind::EmailTaken));
        }
        users[index].email = email.clone();
        users[index].updated_at = now;
        Ok(Some(Mutated {
            value: EmailChange { old, new: email.clone() },
            changed: true,
        }))
    }

    async fn user_view_in(&self, _tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<UserWithIdentities>, RepositoryError> {
        self.touch();
        Ok(self.view(id))
    }
}
```

- [ ] **Step 4: Write the service shell and the failing tests.** Create
  `rs/crates/services/paigasus-iam/src/application/user_identities.rs` with only the module doc,
  the imports, the `UserIdentityDeps` struct, the `UserIdentityService` struct, and this test
  module. Register it in `application/mod.rs` after line 27 (`pub mod system_retirement;`):

```rust
pub mod user_identities;
```

The test module (keep it at the end of the file in step 6 too):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuditLog, FakeAuthorizer, FakeUnitOfWork, FixedClock, InMemoryIdentityLinks, SeqIds};
    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_iam_core::{ApiKeyId, AuditFilter, RepositoryError, Transaction, User};
    use serde_json::json;

    const ISSUER: &str = "https://idp.example.com/realms/main";
    const SECOND_ISSUER: &str = "https://second.example.com";
    const ALL_FOUR: [Action; 4] = [Action::GetUser, Action::LinkExternalIdentity, Action::UnlinkExternalIdentity, Action::ChangeUserEmail];

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn now() -> DateTime<Utc> {
        at(1_750_000_000)
    }

    fn pid(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    fn actor() -> Prn {
        pid(1).prn().clone()
    }

    fn iss(raw: &str) -> Issuer {
        Issuer::parse(raw).unwrap()
    }

    fn user(n: u128, email: &str) -> User {
        User::new(pid(n), Email::parse(email).unwrap(), format!("User {n}"), None, None, at(1_700_000_000), at(1_700_000_000))
    }

    fn identity(id: u128, owner: u128, issuer: &str, subject: &str, created: i64) -> ExternalIdentity {
        ExternalIdentity {
            id: Uuid::from_u128(id),
            principal_id: pid(owner),
            issuer: iss(issuer),
            subject: subject.to_string(),
            created_at: at(created),
            updated_at: at(created),
        }
    }

    fn oidc(issuer: &str, subject: &str) -> Credential {
        Credential::Oidc {
            issuer: iss(issuer),
            subject: subject.to_string(),
            expires_at: at(1_800_000_000),
        }
    }

    fn api_key() -> Credential {
        Credential::ApiKey {
            key_id: ApiKeyId::from_uuid(Uuid::from_u128(77)),
            expires_at: None,
            scope_prn: root_prn().canonical(),
        }
    }

    struct Fixture {
        svc: UserIdentityService,
        links: InMemoryIdentityLinks,
        audit: FakeAuditLog,
        uow: FakeUnitOfWork,
        authz: FakeAuthorizer,
    }

    fn fixture_with(allow: &[Action], audit_port: Option<Arc<dyn AuditLog>>) -> Fixture {
        let authz = FakeAuthorizer::default();
        for a in allow {
            authz.allow(*a, &root_prn());
        }
        let links = InMemoryIdentityLinks::default();
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let clock = FixedClock::default();
        clock.set(now());
        let svc = UserIdentityService::new(UserIdentityDeps {
            authorize: Authorize::new(Arc::new(authz.clone())),
            links: Arc::new(links.clone()),
            uow: Arc::new(uow.clone()),
            audit: audit_port.unwrap_or_else(|| Arc::new(audit.clone()) as Arc<dyn AuditLog>),
            issuers: vec![iss(ISSUER), iss(SECOND_ISSUER)],
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(clock),
        });
        Fixture { svc, links, audit, uow, authz }
    }

    fn fixture(allow: &[Action]) -> Fixture {
        fixture_with(allow, None)
    }

    /// An `AuditLog` whose in-transaction write always fails, to prove that a failed audit write
    /// fails the call and never commits.
    struct FailingRecord;

    #[async_trait]
    impl AuditLog for FailingRecord {
        async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), RepositoryError> {
            unimplemented!("the identity calls audit inside the transaction")
        }

        async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), RepositoryError> {
            Err(RepositoryError::Backend(Box::new(std::io::Error::other("audit sink down"))))
        }

        async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
            unimplemented!("the identity calls never query")
        }
    }

    /// Spec 5.1 and 11: each method asks for ITS OWN action, at Root, and asks first. The fake
    /// denies everything, so each call must stop at the check with no store call.
    #[tokio::test]
    async fn every_method_checks_its_own_action_at_root_before_any_store_call() {
        let root = root_prn().canonical();

        let f = fixture(&[]);
        assert_eq!(f.svc.find_by_email(&actor(), "a@example.com").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::GetUser, root.clone())]);
        assert_eq!(f.links.calls(), 0);

        let f = fixture(&[]);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "reason").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::LinkExternalIdentity, root.clone())]);
        assert_eq!(f.links.calls(), 0);

        let f = fixture(&[]);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(5), "reason").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::UnlinkExternalIdentity, root.clone())]);
        assert_eq!(f.links.calls(), 0);

        let f = fixture(&[]);
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "b@example.com", "reason").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::ChangeUserEmail, root)]);
        assert_eq!(f.links.calls(), 0);
        assert_eq!(f.uow.commits(), 0);
    }

    /// A caller that holds the other three actions, but not this method's own, is denied.
    #[tokio::test]
    async fn holding_the_other_three_actions_does_not_open_a_method() {
        for (index, action) in ALL_FOUR.iter().enumerate() {
            let others: Vec<Action> = ALL_FOUR.iter().copied().filter(|a| a != action).collect();
            let f = fixture(&others);
            f.links.seed_user(user(10, "u@example.com"));
            let err = match index {
                0 => f.svc.find_by_email(&actor(), "u@example.com").await.map(|_| ()).unwrap_err(),
                1 => f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "reason").await.map(|_| ()).unwrap_err(),
                2 => f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(5), "reason").await.unwrap_err(),
                _ => f.svc.change_email(&actor(), &pid(10), "v@example.com", "reason").await.map(|_| ()).unwrap_err(),
            };
            assert_eq!(err, TenancyError::Forbidden, "{} must need its own action", action.as_wire());
        }
    }

    /// Spec 5.1: the check runs before any validation, so a denied caller cannot use a 400 to
    /// learn anything.
    #[tokio::test]
    async fn a_denied_caller_gets_forbidden_even_when_every_value_is_invalid() {
        let f = fixture(&[]);
        assert_eq!(f.svc.find_by_email(&actor(), "not-an-email").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.svc.link(&actor(), &pid(10), "https://unknown.example.com/", " padded", "").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(5), "   ").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "@", "").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.links.calls(), 0, "a denied caller must never reach the store");
    }

    #[tokio::test]
    async fn find_by_email_returns_the_user_and_its_identities_in_creation_order() {
        let f = fixture(&[Action::GetUser]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(21, 10, SECOND_ISSUER, "later", 1_700_000_200));
        f.links.seed_identity(identity(20, 10, ISSUER, "earlier", 1_700_000_100));
        let view = f.svc.find_by_email(&actor(), "  u@example.com ").await.unwrap();
        assert_eq!(view.user.principal_id, pid(10));
        let subjects: Vec<&str> = view.identities.iter().map(|i| i.subject.as_str()).collect();
        assert_eq!(subjects, vec!["earlier", "later"]);
        assert!(f.audit.0.lock().unwrap().is_empty(), "a find writes no audit record (spec 13, question 1)");
    }

    #[tokio::test]
    async fn find_by_email_rejects_an_invalid_email_and_reports_an_unknown_one_as_not_found() {
        let f = fixture(&[Action::GetUser]);
        assert!(matches!(f.svc.find_by_email(&actor(), "not-an-email").await, Err(TenancyError::InvalidEmail(_))));
        assert_eq!(f.svc.find_by_email(&actor(), "nobody@example.com").await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn link_creates_the_identity_and_writes_one_audit_entry() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        let out = f.svc.link(&actor(), &pid(10), &format!("  {ISSUER} "), "sub-1", "  INC-7: second issuer  ").await.unwrap();
        assert!(out.changed);
        assert_eq!(out.value.issuer.as_str(), ISSUER, "the stored issuer is the configured value, not the padded input");
        assert_eq!(out.value.subject, "sub-1");
        assert_eq!(out.value.principal_id, pid(10));
        assert_eq!(out.value.created_at, now());
        assert_eq!(f.links.identities(), vec![out.value.clone()]);
        assert_eq!(f.uow.commits(), 1);
        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.action, "LinkExternalIdentity");
        assert_eq!(entry.outcome, AuditOutcome::Committed);
        assert_eq!(entry.actor_prn, Some(actor().canonical()));
        assert_eq!(entry.resource_prn, Some(pid(10).canonical()));
        assert_eq!(
            entry.detail,
            json!({"reason": "INC-7: second issuer", "identity_id": out.value.id.to_string(), "issuer": ISSUER, "subject": "sub-1"})
        );
    }

    /// Spec 5.2: a retry after a lost response is safe. The same user already holds the pair,
    /// so the call returns the stored identity, writes no audit record and commits nothing.
    #[tokio::test]
    async fn linking_an_identity_the_user_already_holds_returns_it_unchanged() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        let held = identity(20, 10, ISSUER, "sub-1", 1_700_000_100);
        f.links.seed_identity(held.clone());
        let out = f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "retry").await.unwrap();
        assert!(!out.changed);
        assert_eq!(out.value, held);
        assert!(f.audit.0.lock().unwrap().is_empty(), "a same-user link writes no audit record");
        assert_eq!(f.uow.commits(), 0);
    }

    #[tokio::test]
    async fn linking_an_identity_that_another_user_holds_is_a_conflict() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_user(user(11, "v@example.com"));
        f.links.seed_identity(identity(20, 11, ISSUER, "sub-1", 1_700_000_100));
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::ExternalIdentityConflict);
        assert_eq!(f.links.identities().len(), 1, "there is no implicit move");
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    /// Spec 6.3 step 3: a JIT login can insert the pair between the read and the insert. The
    /// unique violation then gives 409, not 500.
    #[tokio::test]
    async fn a_unique_violation_at_insert_time_is_a_conflict() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.fail_next_link_with_conflict();
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::ExternalIdentityConflict);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    #[tokio::test]
    async fn link_to_an_unknown_principal_is_not_found() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::NotFound);
        assert!(f.audit.0.lock().unwrap().is_empty());
    }

    /// Review focus 2: the issuer match is exact after trim.
    #[tokio::test]
    async fn link_matches_the_issuer_exactly_after_trim() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        for bad in [format!("{ISSUER}/"), ISSUER.to_uppercase(), "https://idp.example.com/realms/other".to_string(), String::new()] {
            assert_eq!(f.svc.link(&actor(), &pid(10), &bad, "sub-1", "r").await.unwrap_err(), TenancyError::UnknownIssuer, "{bad:?}");
        }
        assert_eq!(f.links.calls(), 0, "an unknown issuer is refused before any store call");
        let out = f.svc.link(&actor(), &pid(10), &format!("\t{SECOND_ISSUER}\n"), "sub-2", "r").await.unwrap();
        assert_eq!(out.value.issuer.as_str(), SECOND_ISSUER);
    }

    #[tokio::test]
    async fn link_validates_issuer_then_subject_then_reason() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        assert_eq!(f.svc.link(&actor(), &pid(10), "https://nope.example.com", " bad", "").await.unwrap_err(), TenancyError::UnknownIssuer);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, " bad", "").await.unwrap_err(), TenancyError::InvalidSubject);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "good", "  ").await.unwrap_err(), TenancyError::InvalidReason);
        assert_eq!(f.links.calls(), 0);
        assert_eq!(f.uow.commits(), 0);
    }

    /// Spec 6.4: the audit `issuer` and `subject` of an unlink come from the deleted row.
    #[tokio::test]
    async fn unlink_removes_the_identity_and_audits_the_deleted_row() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(20, 10, ISSUER, "old-sub", 1_700_000_100));
        f.svc.unlink(&actor(), &oidc(ISSUER, "operator-sub"), &pid(10), Uuid::from_u128(20), " INC-9: dead sub ").await.unwrap();
        assert!(f.links.identities().is_empty());
        assert_eq!(f.uow.commits(), 1);
        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "UnlinkExternalIdentity");
        assert_eq!(entries[0].resource_prn, Some(pid(10).canonical()));
        assert_eq!(
            entries[0].detail,
            json!({"reason": "INC-9: dead sub", "identity_id": Uuid::from_u128(20).to_string(), "issuer": ISSUER, "subject": "old-sub"})
        );
    }

    #[tokio::test]
    async fn unlink_of_an_unknown_identity_or_another_users_identity_is_not_found() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_user(user(11, "v@example.com"));
        f.links.seed_identity(identity(20, 11, ISSUER, "sub-1", 1_700_000_100));
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(99), "r").await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(12), Uuid::from_u128(20), "r").await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(f.links.identities().len(), 1);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    /// Spec 5.3: the caller cannot unlink the identity that authenticated the request.
    #[tokio::test]
    async fn unlinking_the_identity_that_authenticated_the_request_is_refused() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(20, 10, ISSUER, "me", 1_700_000_100));
        let err = f.svc.unlink(&actor(), &oidc(ISSUER, "me"), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err();
        assert_eq!(err, TenancyError::CannotUnlinkOwnIdentity);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0, "the refused delete must not commit");
    }

    /// Review focus 5: the guard compares issuer AND subject, and an API key has no identity.
    #[tokio::test]
    async fn the_own_identity_guard_matches_issuer_and_subject_together() {
        for caller in [api_key(), oidc(ISSUER, "someone-else"), oidc(SECOND_ISSUER, "me")] {
            let f = fixture(&[Action::UnlinkExternalIdentity]);
            f.links.seed_user(user(10, "u@example.com"));
            f.links.seed_identity(identity(20, 10, ISSUER, "me", 1_700_000_100));
            f.svc.unlink(&actor(), &caller, &pid(10), Uuid::from_u128(20), "r").await.unwrap_or_else(|e| panic!("{caller:?}: {e:?}"));
            assert_eq!(f.uow.commits(), 1, "{caller:?}");
        }
    }

    #[tokio::test]
    async fn unlink_rejects_an_invalid_reason_before_any_store_call() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(20), "\n").await.unwrap_err(), TenancyError::InvalidReason);
        assert_eq!(f.links.calls(), 0);
    }

    #[tokio::test]
    async fn change_email_updates_the_row_and_audits_old_and_new() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "old@example.com"));
        let view = f.svc.change_email(&actor(), &pid(10), " new@example.com ", "INC-3: moved").await.unwrap();
        assert_eq!(view.user.email.as_str(), "new@example.com");
        assert_eq!(view.user.updated_at, now());
        assert_eq!(f.uow.commits(), 1);
        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "ChangeUserEmail");
        assert_eq!(entries[0].resource_prn, Some(pid(10).canonical()));
        assert_eq!(entries[0].detail, json!({"reason": "INC-3: moved", "old_email": "old@example.com", "new_email": "new@example.com"}));
    }

    /// Spec 5.4: a change to the current email is a no-op. SMA-606 D1: it does not restamp.
    #[tokio::test]
    async fn changing_to_the_current_email_is_a_no_op_with_no_audit_record() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "same@example.com"));
        let view = f.svc.change_email(&actor(), &pid(10), " same@example.com ", "r").await.unwrap();
        assert_eq!(view.user.updated_at, at(1_700_000_000), "a no-op must not restamp");
        assert!(f.audit.0.lock().unwrap().is_empty());
    }

    /// Review focus 3: the match is exact and case-sensitive, as in JIT and the unique key.
    #[tokio::test]
    async fn an_email_that_differs_only_in_letter_case_is_a_change_not_a_conflict() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "person@example.com"));
        f.links.seed_user(user(11, "other@example.com"));
        let view = f.svc.change_email(&actor(), &pid(10), "Person@Example.com", "case fix").await.unwrap();
        assert_eq!(view.user.email.as_str(), "Person@Example.com");
        assert_eq!(f.audit.0.lock().unwrap().len(), 1, "a case change is a real change");
        let other = f.svc.change_email(&actor(), &pid(11), "PERSON@EXAMPLE.COM", "r").await.unwrap();
        assert_eq!(other.user.email.as_str(), "PERSON@EXAMPLE.COM", "a case variant of another user's email is not a conflict");
    }

    #[tokio::test]
    async fn change_email_reports_a_conflict_an_unknown_user_and_invalid_input() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "a@example.com"));
        f.links.seed_user(user(11, "b@example.com"));
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "b@example.com", "r").await.unwrap_err(), TenancyError::EmailConflict);
        assert_eq!(f.svc.change_email(&actor(), &pid(12), "c@example.com", "r").await.unwrap_err(), TenancyError::NotFound);
        assert!(matches!(f.svc.change_email(&actor(), &pid(10), "no-at-sign", "r").await, Err(TenancyError::InvalidEmail(_))));
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "c@example.com", " ").await.unwrap_err(), TenancyError::InvalidReason);
        assert!(f.audit.0.lock().unwrap().is_empty());
    }

    /// Spec 6.1: if the audit write fails, the change must not commit.
    #[tokio::test]
    async fn a_failed_audit_write_fails_the_call_and_never_commits() {
        let f = fixture_with(&ALL_FOUR, Some(Arc::new(FailingRecord)));
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(20, 10, ISSUER, "old", 1_700_000_100));
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "new", "r").await.unwrap_err(), TenancyError::Internal);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err(), TenancyError::Internal);
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "v@example.com", "r").await.unwrap_err(), TenancyError::Internal);
        assert_eq!(f.uow.commits(), 0);
    }
}
```

- [ ] **Step 5: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam -E 'test(/user_identities/)'
```

Expected: compile error `no method named find_by_email found for struct UserIdentityService`
(and the same for `link`, `unlink`, `change_email`, `new`).

- [ ] **Step 6: Implement the service.** The full file
  `rs/crates/services/paigasus-iam/src/application/user_identities.rs` above the test module:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `UserIdentityService` (SMA-712): the operator calls that find a user by email, link and
//! unlink an external identity, and change a user's email.
//!
//! **This service authorizes, not the adapter.** The first statement of each method is
//! `self.authorize.check(actor, Action::X, &root_prn())`. There is no `enforce_tenancy` gate:
//! with that toggle off, an adapter-level check would not run, and any authenticated principal
//! could link its own identity to a `platform_admin` user (spec 5.1). The check runs before any
//! validation and before any row read, so a denied caller learns nothing.
//!
//! Root-only-ness comes from the resource (`root_prn()`), not from the Cedar schema, exactly
//! like `DeadLetterService`.
//!
//! Each write runs in ONE unit of work: the change, then its audit entry only when the change
//! did something, then the commit. A failed audit write rolls the change back. There is no
//! outbox event (spec 6.5).
//!
//! The audit `detail` holds an email and a subject. These are personal data. `ListAuditLog`
//! restricts the audit log. No log line of this service contains either value.

use std::sync::Arc;

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, AuditReason, Clock, Credential, Email, ExternalIdentity, ExternalSubject, IdGenerator, IdentityLinkStore, Issuer, Mutated, PrincipalId,
    UnitOfWork, UserWithIdentities,
};
use paigasus_kernel::Prn;
use uuid::Uuid;

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;

/// Constructor bag, mirroring `DeadLetterDeps`. `issuers` is the configured
/// `authn.issuers[].issuer` set, parsed once at boot.
pub struct UserIdentityDeps {
    pub authorize: Authorize,
    pub links: Arc<dyn IdentityLinkStore>,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub issuers: Vec<Issuer>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct UserIdentityService {
    authorize: Authorize,
    links: Arc<dyn IdentityLinkStore>,
    uow: Arc<dyn UnitOfWork>,
    audit: Arc<dyn AuditLog>,
    issuers: Arc<[Issuer]>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn Clock>,
}

/// Parses an email and drops the raw input from the error. `DomainError::InvalidEmail` keeps
/// the raw value, and this service keeps no caller value in an error.
fn parse_email(raw: &str) -> Result<Email, TenancyError> {
    Email::parse(raw).map_err(|_| TenancyError::InvalidEmail(String::new()))
}

impl UserIdentityService {
    #[must_use]
    pub fn new(deps: UserIdentityDeps) -> Self {
        UserIdentityService {
            authorize: deps.authorize,
            links: deps.links,
            uow: deps.uow,
            audit: deps.audit,
            issuers: deps.issuers.into(),
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// The committed-outcome audit entry of one write. The resource is the user, not Root.
    fn audit_entry(&self, actor: &Prn, action: Action, user: &PrincipalId, detail: serde_json::Value) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: self.clock.now(),
            actor_prn: Some(actor.canonical()),
            action: action.as_wire().to_string(),
            resource_prn: Some(user.canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: Vec::new(),
            detail,
            correlation_id: Some(self.ids.new_correlation_id()),
        }
    }

    /// Spec 5.2: the issuer must equal a configured issuer exactly, after trim. The stored value
    /// is the configured value.
    fn configured_issuer(&self, raw: &str) -> Result<Issuer, TenancyError> {
        let wanted = raw.trim();
        self.issuers.iter().find(|issuer| issuer.as_str() == wanted).cloned().ok_or(TenancyError::UnknownIssuer)
    }

    /// `FindUserByEmail`, `POST /v1/users/find-by-email`. A read: no audit record (spec 13).
    pub async fn find_by_email(&self, actor: &Prn, email: &str) -> Result<UserWithIdentities, TenancyError> {
        self.authorize.check(actor, Action::GetUser, &root_prn()).await?;
        let email = parse_email(email)?;
        self.links.find_user_by_email(&email).await?.ok_or(TenancyError::NotFound)
    }

    /// `LinkExternalIdentity`. `changed == false` means the same user already held the pair:
    /// the call returns that identity and writes nothing (a safe retry, spec 5.2).
    pub async fn link(&self, actor: &Prn, user: &PrincipalId, issuer: &str, subject: &str, reason: &str) -> Result<Mutated<ExternalIdentity>, TenancyError> {
        self.authorize.check(actor, Action::LinkExternalIdentity, &root_prn()).await?;
        let issuer = self.configured_issuer(issuer)?;
        let subject = ExternalSubject::parse(subject)?;
        let reason = AuditReason::parse(reason)?;

        let tx = self.uow.begin().await?;
        if self.links.lock_user_in(&*tx, user).await?.is_none() {
            return Err(TenancyError::NotFound);
        }
        if let Some(existing) = self.links.find_identity_in(&*tx, &issuer, subject.as_str()).await? {
            if existing.principal_id.uuid() == user.uuid() {
                // Dropping `tx` releases the share lock. Nothing was written.
                return Ok(Mutated { value: existing, changed: false });
            }
            return Err(TenancyError::ExternalIdentityConflict);
        }
        let now = self.clock.now();
        let identity = ExternalIdentity {
            id: self.ids.new_external_identity_id(),
            principal_id: user.clone(),
            issuer,
            subject: subject.into_string(),
            created_at: now,
            updated_at: now,
        };
        // A JIT login can insert the same pair after the read above. The unique constraint then
        // raises `Conflict(ExternalIdentityExists)`, which `?` maps to 409.
        self.links.link_in(&*tx, &identity).await?;
        let detail = serde_json::json!({
            "reason": reason.as_str(),
            "identity_id": identity.id.to_string(),
            "issuer": identity.issuer.as_str(),
            "subject": identity.subject,
        });
        let entry = self.audit_entry(actor, Action::LinkExternalIdentity, user, detail);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(Mutated { value: identity, changed: true })
    }

    /// `UnlinkExternalIdentity`. `caller` is the credential that authenticated this request. The
    /// call refuses to remove that credential's own identity (spec 5.3). An API key has no such
    /// identity, so the guard does not apply to it.
    pub async fn unlink(&self, actor: &Prn, caller: &Credential, user: &PrincipalId, identity_id: Uuid, reason: &str) -> Result<(), TenancyError> {
        self.authorize.check(actor, Action::UnlinkExternalIdentity, &root_prn()).await?;
        let reason = AuditReason::parse(reason)?;

        let tx = self.uow.begin().await?;
        if self.links.lock_user_in(&*tx, user).await?.is_none() {
            return Err(TenancyError::NotFound);
        }
        let Some(removed) = self.links.unlink_in(&*tx, user, identity_id).await? else {
            return Err(TenancyError::NotFound);
        };
        if let Credential::Oidc { issuer, subject, .. } = caller
            && removed.issuer == *issuer
            && removed.subject == *subject
        {
            // Dropping `tx` without a commit rolls the delete back.
            return Err(TenancyError::CannotUnlinkOwnIdentity);
        }
        // `issuer` and `subject` come from the deleted row, not from the request (spec 6.4).
        let detail = serde_json::json!({
            "reason": reason.as_str(),
            "identity_id": removed.id.to_string(),
            "issuer": removed.issuer.as_str(),
            "subject": removed.subject,
        });
        let entry = self.audit_entry(actor, Action::UnlinkExternalIdentity, user, detail);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// `ChangeUserEmail`. The no-op decision and `old_email` come from the row that the
    /// transaction locked (spec 5.4, 6.3).
    pub async fn change_email(&self, actor: &Prn, user: &PrincipalId, email: &str, reason: &str) -> Result<UserWithIdentities, TenancyError> {
        self.authorize.check(actor, Action::ChangeUserEmail, &root_prn()).await?;
        let email = parse_email(email)?;
        let reason = AuditReason::parse(reason)?;

        let tx = self.uow.begin().await?;
        let Some(change) = self.links.change_email_in(&*tx, user, &email, self.clock.now()).await? else {
            return Err(TenancyError::NotFound);
        };
        if change.changed {
            let detail = serde_json::json!({
                "reason": reason.as_str(),
                "old_email": change.value.old.as_str(),
                "new_email": change.value.new.as_str(),
            });
            let entry = self.audit_entry(actor, Action::ChangeUserEmail, user, detail);
            self.audit.record(&*tx, &entry).await?;
        }
        // The row is locked, so it cannot vanish between the change and this read.
        let view = self.links.user_view_in(&*tx, user).await?.ok_or(TenancyError::Internal)?;
        tx.commit().await?;
        Ok(view)
    }
}
```

- [ ] **Step 7: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam -p paigasus-iam-core -E 'test(/user_identities|object_safe|authorize|dead_letters/)'
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every `user_identities` test passes. The `authorize.rs` and `dead_letters.rs` tests
still pass with the changed `FakeAuthorizer`. No fmt or clippy output. If clippy reports
`collapsible_if` or another lint on the `let` chain in `unlink`, keep the logic and fix the form.

- [ ] **Step 8: Commit.**

```bash
W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity
git -C $W add rs/crates/libs/paigasus-iam-core/src/ports.rs rs/crates/libs/paigasus-iam-core/src/lib.rs rs/crates/services/paigasus-iam/src/application/fakes.rs rs/crates/services/paigasus-iam/src/application/mod.rs rs/crates/services/paigasus-iam/src/application/user_identities.rs
git -C $W commit -m "feat(rs): add UserIdentityService and the IdentityLinkStore port (SMA-712)

The service authorizes each call first at Root with its own action,
with no enforce_tenancy gate. Each write and its audit entry share one
unit of work. A same-user link and an unchanged email write nothing.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Postgres adapter `PgIdentityLinkStore`

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_identity_links.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs:12` (module list) and `:35` (re-exports)
- Create: `rs/crates/services/paigasus-iam/tests/user_identities_pg.rs`

**Interfaces:**
- Consumes: `IdentityLinkStore`, `UserWithIdentities`, `EmailChange` (Task 5); `UserIdentityService`, `UserIdentityDeps` (Task 5).
- Produces: `pub struct PgIdentityLinkStore` with `pub fn new(db: DatabaseConnection) -> PgIdentityLinkStore`, re-exported as `paigasus_iam::adapters::persistence::PgIdentityLinkStore`.

- [ ] **Step 1: Write the failing Docker-gated tests.** Create
  `rs/crates/services/paigasus-iam/tests/user_identities_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres coverage for SMA-712: `UserIdentityService` over the REAL adapters
//! (`PgIdentityLinkStore`, `SeaOrmUnitOfWork`, `PgAuditLog`). Each write commits with its audit
//! row. A failed audit write rolls the change back. The concurrency cases of spec 5.6 are
//! deterministic: the peer holds an open transaction until the racer is provably blocked on it.
//!
//! Runs against an ephemeral Postgres in Docker (see `tests/support/mod.rs`).

mod support;

use std::sync::Arc;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{external_identity, principal};
use paigasus_iam::adapters::persistence::uow::recover_txn;
use paigasus_iam::adapters::persistence::{PgAuditLog, PgExternalIdentityRepository, PgIdentityLinkStore, PgPrincipalRepository, SeaOrmUnitOfWork};
use paigasus_iam::application::authorize::Authorize;
use paigasus_iam::application::error::TenancyError;
use paigasus_iam::application::user_identities::{UserIdentityDeps, UserIdentityService};
use paigasus_iam_core::{
    AccessRequest, AuditEntry, AuditFilter, AuditLog, Authorizer, AuthzError, Credential, Decision, Effect, Email, ExternalIdentityRepository, IdentityLinkStore, Issuer, Principal,
    PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, Transaction, UnitOfWork, User,
};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, Set, Statement, TransactionTrait};
use uuid::Uuid;

const ISSUER: &str = "https://idp.example.com/realms/main";

/// Allows every request. `user_identities.rs`'s unit tests cover authorization; these tests
/// need a caller that passes the check so they reach the rows.
struct AllowAllAuthorizer;

#[async_trait::async_trait]
impl Authorizer for AllowAllAuthorizer {
    async fn is_authorized(&self, _req: &AccessRequest) -> Result<Decision, AuthzError> {
        Ok(Decision {
            effect: Effect::Allow,
            determining_policies: Vec::new(),
        })
    }
}

/// An audit sink whose in-transaction write always fails.
struct FailingAudit;

#[async_trait::async_trait]
impl AuditLog for FailingAudit {
    async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), RepositoryError> {
        unimplemented!("the identity calls audit inside the transaction")
    }

    async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(Box::new(std::io::Error::other("audit sink down"))))
    }

    async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
        unimplemented!("the identity calls never query")
    }
}

fn service(db: &DatabaseConnection, audit: Arc<dyn AuditLog>) -> UserIdentityService {
    UserIdentityService::new(UserIdentityDeps {
        authorize: Authorize::new(Arc::new(AllowAllAuthorizer)),
        links: Arc::new(PgIdentityLinkStore::new(db.clone())),
        uow: Arc::new(SeaOrmUnitOfWork::new(db.clone())),
        audit,
        issuers: vec![Issuer::parse(ISSUER).unwrap()],
        ids: Arc::new(KernelIdGenerator),
        clock: Arc::new(SystemClock),
    })
}

fn pid(n: u128) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
}

fn actor() -> Prn {
    pid(1).prn().clone()
}

fn operator() -> Credential {
    Credential::Oidc {
        issuer: Issuer::parse(ISSUER).unwrap(),
        subject: "operator-sub".to_string(),
        expires_at: Utc::now() + chrono::Duration::hours(1),
    }
}

async fn seed_user(db: &DatabaseConnection, n: u128, email: &str) -> PrincipalId {
    let id = pid(n);
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(id.clone(), Email::parse(email).unwrap(), "Seeded".to_string(), None, None, now, now);
    PgPrincipalRepository::new(db.clone()).create_user(&principal, &user).await.unwrap();
    id
}

async fn audit_rows(db: &DatabaseConnection, action: &str, resource: &PrincipalId) -> Vec<AuditEntry> {
    PgAuditLog::new(db.clone())
        .query(&AuditFilter {
            actor_prn: None,
            resource_prn: Some(resource.canonical()),
            action: Some(action.to_string()),
            outcome: None,
            from: None,
            to: None,
            cursor: None,
            limit: 50,
        })
        .await
        .unwrap()
}

async fn email_of(db: &DatabaseConnection, id: &PrincipalId) -> String {
    let (_, user) = PgPrincipalRepository::new(db.clone()).find_user(id).await.unwrap().expect("user row");
    user.email.as_str().to_string()
}

#[tokio::test]
async fn each_write_commits_with_its_audit_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "pg-a@example.com").await;

    let first = svc.link(&actor(), &user, ISSUER, "pg-sub-1", "INC-1: first").await.unwrap();
    let second = svc.link(&actor(), &user, ISSUER, "pg-sub-2", "INC-2: second").await.unwrap();
    assert!(first.changed && second.changed);
    assert_eq!(audit_rows(&db, "LinkExternalIdentity", &user).await.len(), 2);

    let view = svc.find_by_email(&actor(), "pg-a@example.com").await.unwrap();
    let subjects: Vec<&str> = view.identities.iter().map(|i| i.subject.as_str()).collect();
    assert_eq!(subjects, vec!["pg-sub-1", "pg-sub-2"], "identities come back in (created_at, id) order");
    assert_eq!(view.status, PrincipalStatus::Active);

    let again = svc.link(&actor(), &user, ISSUER, "pg-sub-1", "retry").await.unwrap();
    assert!(!again.changed);
    assert_eq!(again.value.id, first.value.id);
    assert_eq!(audit_rows(&db, "LinkExternalIdentity", &user).await.len(), 2, "a same-user link writes no audit row");

    let changed = svc.change_email(&actor(), &user, "pg-b@example.com", "INC-3: moved").await.unwrap();
    assert_eq!(changed.user.email.as_str(), "pg-b@example.com");
    assert_eq!(changed.identities.len(), 2);
    let rows = audit_rows(&db, "ChangeUserEmail", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["old_email"], "pg-a@example.com");
    assert_eq!(rows[0].detail["new_email"], "pg-b@example.com");

    svc.change_email(&actor(), &user, "pg-b@example.com", "no-op").await.unwrap();
    assert_eq!(audit_rows(&db, "ChangeUserEmail", &user).await.len(), 1, "an unchanged email writes no audit row");

    svc.unlink(&actor(), &operator(), &user, first.value.id, "INC-4: dead sub").await.unwrap();
    let rows = audit_rows(&db, "UnlinkExternalIdentity", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["subject"], "pg-sub-1", "the unlink audit names the deleted row");
    assert_eq!(svc.unlink(&actor(), &operator(), &user, first.value.id, "retry").await.unwrap_err(), TenancyError::NotFound);
}

#[tokio::test]
async fn a_failed_audit_write_rolls_every_change_back() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let good = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let failing = service(&db, Arc::new(FailingAudit));
    let user = seed_user(&db, 10, "rb@example.com").await;
    let kept = good.link(&actor(), &user, ISSUER, "kept-sub", "setup").await.unwrap().value;
    let identities = PgExternalIdentityRepository::new(db.clone());
    let issuer = Issuer::parse(ISSUER).unwrap();

    assert_eq!(failing.link(&actor(), &user, ISSUER, "lost-sub", "r").await.unwrap_err(), TenancyError::Internal);
    assert!(identities.find_by_issuer_subject(&issuer, "lost-sub").await.unwrap().is_none(), "the link must roll back");

    assert_eq!(failing.unlink(&actor(), &operator(), &user, kept.id, "r").await.unwrap_err(), TenancyError::Internal);
    assert!(identities.find_by_issuer_subject(&issuer, "kept-sub").await.unwrap().is_some(), "the unlink must roll back");

    assert_eq!(failing.change_email(&actor(), &user, "rb-new@example.com", "r").await.unwrap_err(), TenancyError::Internal);
    assert_eq!(email_of(&db, &user).await, "rb@example.com", "the email change must roll back");
}

/// Review focus 5: the refused delete is rolled back, so the identity is still there.
#[tokio::test]
async fn an_own_identity_refusal_rolls_the_delete_back() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "self@example.com").await;
    let mine = svc.link(&actor(), &user, ISSUER, "operator-sub", "setup").await.unwrap().value;
    assert_eq!(svc.unlink(&actor(), &operator(), &user, mine.id, "r").await.unwrap_err(), TenancyError::CannotUnlinkOwnIdentity);
    let still = PgExternalIdentityRepository::new(db.clone()).find_by_issuer_subject(&Issuer::parse(ISSUER).unwrap(), "operator-sub").await.unwrap();
    assert_eq!(still.map(|i| i.id), Some(mine.id));
    assert!(audit_rows(&db, "UnlinkExternalIdentity", &user).await.is_empty());
}

#[tokio::test]
async fn unlink_in_with_another_users_identity_matches_no_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let owner = seed_user(&db, 10, "owner@example.com").await;
    let other = seed_user(&db, 11, "other@example.com").await;
    let owned = svc.link(&actor(), &owner, ISSUER, "owned-sub", "setup").await.unwrap().value;

    let store = PgIdentityLinkStore::new(db.clone());
    let tx = SeaOrmUnitOfWork::new(db.clone()).begin().await.unwrap();
    assert!(store.unlink_in(&*tx, &other, owned.id).await.unwrap().is_none());
    drop(tx);

    assert_eq!(svc.unlink(&actor(), &operator(), &other, owned.id, "r").await.unwrap_err(), TenancyError::NotFound);
    assert!(PgExternalIdentityRepository::new(db.clone()).find_by_issuer_subject(&owned.issuer, "owned-sub").await.unwrap().is_some());
}

/// Review focus 4: a service account has a `principal` row but no `"user"` row.
#[tokio::test]
async fn a_service_account_id_is_not_found_on_every_write() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let sa = pid(20);
    let now = Utc::now().trunc_subsecs(6);
    principal::ActiveModel {
        id: Set(sa.uuid()),
        prn: Set(sa.canonical()),
        kind: Set(PrincipalKind::ServiceAccount.as_str().to_string()),
        status: Set(PrincipalStatus::Active.as_str().to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    assert_eq!(svc.link(&actor(), &sa, ISSUER, "sa-sub", "r").await.unwrap_err(), TenancyError::NotFound);
    assert_eq!(svc.unlink(&actor(), &operator(), &sa, Uuid::from_u128(5), "r").await.unwrap_err(), TenancyError::NotFound);
    assert_eq!(svc.change_email(&actor(), &sa, "sa@example.com", "r").await.unwrap_err(), TenancyError::NotFound);
}

/// Review focus 3: `user_email_key` is exact, so a case variant is a different email.
#[tokio::test]
async fn a_case_variant_of_another_users_email_is_not_a_conflict_in_postgres() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    seed_user(&db, 10, "case@example.com").await;
    let other = seed_user(&db, 11, "other-case@example.com").await;
    let view = svc.change_email(&actor(), &other, "Case@Example.com", "r").await.unwrap();
    assert_eq!(view.user.email.as_str(), "Case@Example.com");
    assert_eq!(svc.change_email(&actor(), &other, "case@example.com", "r").await.unwrap_err(), TenancyError::EmailConflict);
}

/// Spec 5.6, first case, in the order where JIT wins: the peer inserts the pair and holds its
/// transaction open. The link reads nothing, blocks on the unique index, and gets 409 when the
/// peer commits.
#[tokio::test]
async fn a_link_that_loses_the_race_to_a_jit_insert_is_a_conflict() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let target = seed_user(&db, 10, "race-target@example.com").await;
    let jit_user = seed_user(&db, 11, "race-jit@example.com").await;

    let peer = db.begin().await.unwrap();
    let now = Utc::now().trunc_subsecs(6);
    external_identity::ActiveModel {
        id: Set(Uuid::from_u128(0x7171)),
        principal_id: Set(jit_user.uuid()),
        issuer: Set(ISSUER.to_string()),
        subject: Set("race-sub".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&peer)
    .await
    .unwrap();
    let peer_pid = support::race::backend_pid(&peer).await;

    let racer_svc = svc.clone();
    let racer_target = target.clone();
    let mut racer = tokio::spawn(async move { racer_svc.link(&actor(), &racer_target, ISSUER, "race-sub", "race").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "insert into \"external_identity\"%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    assert_eq!(racer.await.unwrap().unwrap_err(), TenancyError::ExternalIdentityConflict);
    assert!(audit_rows(&db, "LinkExternalIdentity", &target).await.is_empty());
}

/// Spec 5.6, second case: two changes to the same email. Only one commits.
#[tokio::test]
async fn two_email_changes_to_the_same_email_let_only_one_commit() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let first = seed_user(&db, 10, "first@example.com").await;
    let second = seed_user(&db, 11, "second@example.com").await;

    let peer = db.begin().await.unwrap();
    peer.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE "user" SET email = $1 WHERE principal_id = $2"#,
        ["taken@example.com".into(), first.uuid().into()],
    ))
    .await
    .unwrap();
    let peer_pid = support::race::backend_pid(&peer).await;

    let racer_svc = svc.clone();
    let racer_user = second.clone();
    let mut racer = tokio::spawn(async move { racer_svc.change_email(&actor(), &racer_user, "taken@example.com", "race").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "update \"user\"%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    assert_eq!(racer.await.unwrap().unwrap_err(), TenancyError::EmailConflict);
    assert_eq!(email_of(&db, &second).await, "second@example.com");
    assert!(audit_rows(&db, "ChangeUserEmail", &second).await.is_empty());
}

/// Spec 5.6, third case: two changes to ONE user. The peer changes A to B and holds the row
/// lock. The racer's change to C blocks on the lock, then reads B from the locked row. So its
/// audit row says old B, new C, and the history A to B to C has no gap.
#[tokio::test]
async fn two_email_changes_to_one_user_serialize_and_the_second_reads_the_first() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "a@example.com").await;

    let store = PgIdentityLinkStore::new(db.clone());
    let peer = SeaOrmUnitOfWork::new(db.clone()).begin().await.unwrap();
    let first = store.change_email_in(&*peer, &user, &Email::parse("b@example.com").unwrap(), Utc::now()).await.unwrap().expect("user row");
    assert!(first.changed);
    assert_eq!(first.value.old.as_str(), "a@example.com");
    let peer_pid = support::race::backend_pid(recover_txn(&*peer).unwrap()).await;

    let racer_svc = svc.clone();
    let racer_user = user.clone();
    let mut racer = tokio::spawn(async move { racer_svc.change_email(&actor(), &racer_user, "c@example.com", "second change").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "select%for update%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    let view = racer.await.unwrap().unwrap();
    assert_eq!(view.user.email.as_str(), "c@example.com");
    let rows = audit_rows(&db, "ChangeUserEmail", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["old_email"], "b@example.com", "the second change must read the first change's value from the locked row");
    assert_eq!(rows[0].detail["new_email"], "c@example.com");
}
```

- [ ] **Step 2: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass -p paigasus-iam --test user_identities_pg
```

Expected: compile error `unresolved import paigasus_iam::adapters::persistence::PgIdentityLinkStore`.

- [ ] **Step 3: Implement the adapter.** Create
  `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_identity_links.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `IdentityLinkStore` (SMA-712, SeaORM). Every `_in` method runs on the
//! caller's own transaction, recovered through `uow::recover_txn`. Every read that decides an
//! audit value or a no-op runs inside that transaction, under a row lock:
//!
//! - `lock_user_in` locks the `"user"` row FOR SHARE, so a concurrent email change cannot remove
//!   the user under a link or an unlink.
//! - `unlink_in` locks the identity row FOR UPDATE, then deletes it. The returned row is the
//!   deleted row, so the audit names what was removed, not what the request said.
//! - `change_email_in` locks the `"user"` row FOR UPDATE before it compares, so two changes to
//!   one user serialize and each reads the value the other committed.
//!
//! The unique constraints `user_email_key` and `uq_external_identity_issuer_subject` report
//! conflicts through the shared `map_err`. No migration is necessary.

use super::entities::{external_identity, principal, user};
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{Email, EmailChange, ExternalIdentity, IdentityLinkStore, Issuer, Mutated, PrincipalId, PrincipalStatus, RepositoryError, Transaction, User, UserWithIdentities};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set};
use uuid::Uuid;

// `Clone` mirrors the other Pg adapters: `DatabaseConnection` is an `Arc`-backed pool handle.
#[derive(Clone)]
pub struct PgIdentityLinkStore {
    db: DatabaseConnection,
}

impl PgIdentityLinkStore {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgIdentityLinkStore { db }
    }
}

/// A backend error with a static message. It never carries a stored email or subject.
fn backend(message: &'static str) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(message)))
}

/// A user principal's id from its bare uuid. A principal PRN is `prn:pgs:iam:::principal/<uuid>`.
fn principal_id_of(uuid: Uuid) -> Result<PrincipalId, RepositoryError> {
    Prn::build("iam", "", None, "principal", uuid).map(PrincipalId::from_prn).map_err(|_| backend("a stored principal id does not build a prn"))
}

fn to_user(id: &PrincipalId, m: user::Model) -> Result<User, RepositoryError> {
    let email = Email::parse(&m.email).map_err(|_| backend("a stored user email does not parse"))?;
    Ok(User::new(id.clone(), email, m.display_name, m.locale, m.timezone, m.created_at, m.updated_at))
}

fn to_identity(id: &PrincipalId, m: external_identity::Model) -> Result<ExternalIdentity, RepositoryError> {
    let issuer = Issuer::parse(&m.issuer).map_err(|_| backend("a stored issuer does not parse"))?;
    Ok(ExternalIdentity {
        id: m.id,
        principal_id: id.clone(),
        issuer,
        subject: m.subject,
        created_at: m.created_at,
        updated_at: m.updated_at,
    })
}

/// The user, its principal status, and its identities in `(created_at, id)` order.
async fn load_view<C: ConnectionTrait>(conn: &C, um: user::Model) -> Result<UserWithIdentities, RepositoryError> {
    let id = principal_id_of(um.principal_id)?;
    let pm = principal::Entity::find_by_id(um.principal_id).one(conn).await.map_err(map_err)?.ok_or_else(|| backend("a user row references a missing principal"))?;
    let status = PrincipalStatus::parse(&pm.status).ok_or_else(|| backend("a stored principal status is unknown"))?;
    let rows = external_identity::Entity::find()
        .filter(external_identity::Column::PrincipalId.eq(um.principal_id))
        .order_by_asc(external_identity::Column::CreatedAt)
        .order_by_asc(external_identity::Column::Id)
        .all(conn)
        .await
        .map_err(map_err)?;
    let identities = rows.into_iter().map(|m| to_identity(&id, m)).collect::<Result<Vec<_>, _>>()?;
    Ok(UserWithIdentities {
        user: to_user(&id, um)?,
        status,
        identities,
    })
}

#[async_trait]
impl IdentityLinkStore for PgIdentityLinkStore {
    async fn find_user_by_email(&self, email: &Email) -> Result<Option<UserWithIdentities>, RepositoryError> {
        let Some(um) = user::Entity::find().filter(user::Column::Email.eq(email.as_str())).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        load_view(&self.db, um).await.map(Some)
    }

    async fn lock_user_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<User>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(um) = user::Entity::find_by_id(id.uuid()).lock_shared().one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        to_user(id, um).map(Some)
    }

    async fn find_identity_in(&self, tx: &dyn Transaction, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(m) = external_identity::Entity::find()
            .filter(external_identity::Column::Issuer.eq(issuer.as_str()))
            .filter(external_identity::Column::Subject.eq(subject))
            .one(txn)
            .await
            .map_err(map_err)?
        else {
            return Ok(None);
        };
        let owner = principal_id_of(m.principal_id)?;
        to_identity(&owner, m).map(Some)
    }

    async fn link_in(&self, tx: &dyn Transaction, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        external_identity::ActiveModel {
            id: Set(identity.id),
            principal_id: Set(identity.principal_id.uuid()),
            issuer: Set(identity.issuer.as_str().to_string()),
            subject: Set(identity.subject.clone()),
            created_at: Set(identity.created_at),
            updated_at: Set(identity.updated_at),
        }
        .insert(txn)
        .await
        .map_err(map_err)?;
        Ok(())
    }

    async fn unlink_in(&self, tx: &dyn Transaction, user: &PrincipalId, identity_id: Uuid) -> Result<Option<ExternalIdentity>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(m) = external_identity::Entity::find_by_id(identity_id)
            .filter(external_identity::Column::PrincipalId.eq(user.uuid()))
            .lock_exclusive()
            .one(txn)
            .await
            .map_err(map_err)?
        else {
            return Ok(None);
        };
        external_identity::Entity::delete_by_id(identity_id).exec(txn).await.map_err(map_err)?;
        to_identity(user, m).map(Some)
    }

    async fn change_email_in(&self, tx: &dyn Transaction, user: &PrincipalId, email: &Email, now: DateTime<Utc>) -> Result<Option<Mutated<EmailChange>>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(um) = user::Entity::find_by_id(user.uuid()).lock_exclusive().one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        let old = Email::parse(&um.email).map_err(|_| backend("a stored user email does not parse"))?;
        if old == *email {
            // SMA-606 D1: a write that changes nothing stamps nothing.
            return Ok(Some(Mutated {
                value: EmailChange { old: old.clone(), new: old },
                changed: false,
            }));
        }
        let mut active = um.into_active_model();
        active.email = Set(email.as_str().to_string());
        active.updated_at = Set(now);
        active.update(txn).await.map_err(map_err)?;
        Ok(Some(Mutated {
            value: EmailChange { old, new: email.clone() },
            changed: true,
        }))
    }

    async fn user_view_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<UserWithIdentities>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(um) = user::Entity::find_by_id(id.uuid()).one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        load_view(txn, um).await.map(Some)
    }
}
```

In `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`, insert after line 12
(`pub mod pg_external_identities;`):

```rust
pub mod pg_identity_links;
```

and after line 35 (`pub use pg_external_identities::PgExternalIdentityRepository;`):

```rust
pub use pg_identity_links::PgIdentityLinkStore;
```

- [ ] **Step 4: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass -p paigasus-iam --test user_identities_pg
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 9 tests pass. If a race test panics with "the racer did not block", read the
`non-idle backends` dump in the panic text. It shows the real statement text. Change only the
`query_prefix` argument to match it, and do not add a sleep.

- [ ] **Step 5: Commit.**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_identity_links.rs rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs rs/crates/services/paigasus-iam/tests/user_identities_pg.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity commit -m "feat(rs): add the Postgres identity link store (SMA-712)

Each read that decides an audit value runs under a row lock in the
caller's transaction. Docker tests prove the commit with its audit row,
the rollback on a failed audit write, and the three races of spec 5.6.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Proto contract, gRPC handlers and the `AppState` wiring

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:585-591` (messages before `service UserService`, and the service)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/iam/v1/`, `ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:17-30` (imports), after `:496` (conversions), test module
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs` (whole file)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:7-8` and `:92-93` (comments)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:66-69` (imports), `:79` area (application imports), `:252-255` (field), after `:785` (construction), `:817` (struct literal)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_users.rs`

**Interfaces:**
- Consumes: `UserIdentityService` (Task 5), `PgIdentityLinkStore` (Task 6).
- Produces: `AppState.user_identities: UserIdentityService` (pub). Task 8 uses it.
- Produces (proto → Rust): `User`, `ExternalIdentity`, `FindUserByEmailRequest{email}`, `FindUserByEmailResponse{user}`, `LinkExternalIdentityRequest{user_prn, issuer, subject, reason}`, `LinkExternalIdentityResponse{external_identity}`, `UnlinkExternalIdentityRequest{user_prn, external_identity_id, reason}`, `UnlinkExternalIdentityResponse{}`, `ChangeUserEmailRequest{user_prn, email, reason}`, `ChangeUserEmailResponse{user}`.
- Produces: `convert::to_proto_user(v: &UserWithIdentities) -> ProtoUser`, `convert::to_proto_external_identity(i: &ExternalIdentity) -> ProtoExternalIdentity`.

- [ ] **Step 1: Add the proto contract.** In `contracts/proto/paigasus/iam/v1/iam.proto`, insert
  after `message CreateUserResponse { … }` (ends at line 587) and before `service UserService`:

```proto

// Operator identity links (SMA-712). Each RPC authorizes its own Cedar action
// at the hierarchy root inside the application service, with no
// enforce_tenancy gate: FindUserByEmail needs GetUser, and the three writes
// need LinkExternalIdentity, UnlinkExternalIdentity and ChangeUserEmail. Each
// write needs a reason, which goes into the audit record. A user is named by
// its full principal PRN.
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

message FindUserByEmailRequest {
  string email = 1;
}
message FindUserByEmailResponse {
  User user = 1;
}

message LinkExternalIdentityRequest {
  string user_prn = 1;
  string issuer = 2;
  string subject = 3;
  string reason = 4;
}
message LinkExternalIdentityResponse {
  ExternalIdentity external_identity = 1;
}

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
message ChangeUserEmailResponse {
  User user = 1;
}
```

and replace the service block (lines 589-591) with:

```proto
service UserService {
  rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
  rpc FindUserByEmail(FindUserByEmailRequest) returns (FindUserByEmailResponse);
  rpc LinkExternalIdentity(LinkExternalIdentityRequest) returns (LinkExternalIdentityResponse);
  rpc UnlinkExternalIdentity(UnlinkExternalIdentityRequest) returns (UnlinkExternalIdentityResponse);
  rpc ChangeUserEmail(ChangeUserEmailRequest) returns (ChangeUserEmailResponse);
}
```

- [ ] **Step 2: Format and regenerate.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/contracts && buf format -w
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && moon run contracts:fmt contracts:lint contracts:breaking && moon run contracts:generate --force
test -f ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts && echo "error_details_pb.ts present"
```

Expected: all pass. The workspace does not compile yet: `UserGrpc` now misses four trait
methods. That is the failing state of this task.

- [ ] **Step 3: Write the failing tests.** In `rs/crates/services/paigasus-iam/tests/grpc_users.rs`,
  replace line 21 (`use paigasus_proto::paigasus::iam::v1::CreateUserRequest;`) with:

```rust
use paigasus_proto::paigasus::common::v1::ErrorReason;
use paigasus_proto::paigasus::iam::v1::{ChangeUserEmailRequest, CreateUserRequest, FindUserByEmailRequest, LinkExternalIdentityRequest, UnlinkExternalIdentityRequest};
use uuid::Uuid;
```

Append to the file:

```rust
// --- SMA-712: the operator identity RPCs ----------------------------------------------------

/// The four identity RPCs, by index, with the Cedar action each one checks.
const OPS: [&str; 4] = ["GetUser", "LinkExternalIdentity", "UnlinkExternalIdentity", "ChangeUserEmail"];

fn maybe_authed<T>(msg: T, token: Option<&str>) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    if let Some(token) = token {
        support::grpc_bearer(&mut req, token);
    }
    req
}

fn reason_of(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

fn missing_user_prn() -> String {
    format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0xdead))
}

/// Calls identity op `op` (an index into `OPS`) with well-formed values against `user_prn`.
async fn call_op(client: &mut UserServiceClient<Channel>, op: usize, token: Option<&str>, user_prn: &str, issuer: &str) -> Result<(), tonic::Status> {
    let user_prn = user_prn.to_string();
    match op {
        0 => client.find_user_by_email(maybe_authed(FindUserByEmailRequest { email: "nobody-grpc@example.com".to_string() }, token)).await.map(|_| ()),
        1 => client
            .link_external_identity(maybe_authed(
                LinkExternalIdentityRequest {
                    user_prn,
                    issuer: issuer.to_string(),
                    subject: "matrix-sub".to_string(),
                    reason: "matrix".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        2 => client
            .unlink_external_identity(maybe_authed(
                UnlinkExternalIdentityRequest {
                    user_prn,
                    external_identity_id: Uuid::from_u128(0xbeef).to_string(),
                    reason: "matrix".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        3 => client
            .change_user_email(maybe_authed(
                ChangeUserEmailRequest {
                    user_prn,
                    email: "matrix-grpc@example.com".to_string(),
                    reason: "matrix".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        other => panic!("no identity op {other}"),
    }
}

/// SMA-712 spec 11: for each RPC, Unauthenticated without a bearer, PermissionDenied without
/// the action, and success for platform_admin, through a full link, change and unlink cycle.
#[tokio::test]
async fn identity_rpcs_need_a_bearer_and_their_action_and_work_for_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let plain_token = idp.bearer("grpc-id-plain", Some("grpc-id-plain@example.com"), "paigasus", 3600);
    support::provision(&state, &plain_token).await;
    let admin_token = idp.bearer("grpc-id-admin", Some("grpc-id-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let target = client.create_user(authed(create_user_request("grpc-id-target@example.com"), &admin_token)).await.unwrap().into_inner().principal_prn;

    for op in 0..OPS.len() {
        let err = call_op(&mut client, op, None, &target, &idp.issuer).await.unwrap_err();
        assert_eq!(err.code(), Code::Unauthenticated, "{} without a bearer", OPS[op]);
        let err = call_op(&mut client, op, Some(plain_token.as_str()), &target, &idp.issuer).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "{} without the action", OPS[op]);
    }

    let found = client
        .find_user_by_email(authed(FindUserByEmailRequest { email: "grpc-id-target@example.com".to_string() }, &admin_token))
        .await
        .unwrap()
        .into_inner()
        .user
        .expect("user");
    assert_eq!(found.prn, target);
    assert_eq!(found.status, "active");
    assert!(found.external_identities.is_empty());

    let link = |subject: &str| LinkExternalIdentityRequest {
        user_prn: target.clone(),
        issuer: idp.issuer.clone(),
        subject: subject.to_string(),
        reason: "INC-1: same person".to_string(),
    };
    let linked = client.link_external_identity(authed(link("grpc-linked-sub"), &admin_token)).await.unwrap().into_inner().external_identity.expect("identity");
    assert_eq!(linked.subject, "grpc-linked-sub");
    assert_eq!(linked.issuer, idp.issuer);
    let again = client.link_external_identity(authed(link("grpc-linked-sub"), &admin_token)).await.unwrap().into_inner().external_identity.expect("identity");
    assert_eq!(again.id, linked.id, "a same-user link returns the stored identity");

    let err = client
        .link_external_identity(authed(
            LinkExternalIdentityRequest {
                user_prn: target.clone(),
                issuer: String::new(),
                subject: "x".to_string(),
                reason: "r".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    assert_eq!(reason_of(&err), ErrorReason::UnknownIssuer.as_wire_reason().unwrap(), "an empty issuer answers like a missing HTTP field");

    let changed = client
        .change_user_email(authed(
            ChangeUserEmailRequest {
                user_prn: target.clone(),
                email: "grpc-id-moved@example.com".to_string(),
                reason: "INC-2: moved".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .user
        .expect("user");
    assert_eq!(changed.email, "grpc-id-moved@example.com");
    assert_eq!(changed.external_identities.len(), 1);

    let unlink = || UnlinkExternalIdentityRequest {
        user_prn: target.clone(),
        external_identity_id: linked.id.clone(),
        reason: "INC-3: undo".to_string(),
    };
    client.unlink_external_identity(authed(unlink(), &admin_token)).await.unwrap();
    let err = client.unlink_external_identity(authed(unlink(), &admin_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "a repeated unlink is not found");

    server.abort();
}

/// SMA-712 spec 11, action identity end to end: each subject holds a static policy that permits
/// exactly ONE of the four actions. It passes the check on that RPC (NotFound on a missing user)
/// and fails it on the other three (PermissionDenied). A role grant cannot show this, because
/// `platform_admin` permits all four.
#[tokio::test]
async fn each_identity_rpc_checks_its_own_action() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.policy_cache_ttl_secs = 1;
    let state = AppState::new(db, &cfg).await.unwrap();
    let admin_token = idp.bearer("grpc-matrix-admin", Some("grpc-matrix-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin_token).await;
    let admin = Prn::parse(&admin_prn).unwrap();

    let mut tokens = Vec::new();
    for (index, action) in OPS.iter().enumerate() {
        let token = idp.bearer(&format!("grpc-matrix-{index}"), Some(format!("grpc-matrix-{index}@example.com").as_str()), "paigasus", 3600);
        let prn = support::provision(&state, &token).await;
        let uuid = Prn::parse(&prn).unwrap().resource_id();
        let doc = paigasus_iam_core::PolicyDocument {
            policy_id: format!("sma-712-grpc-matrix-{index}"),
            kind: paigasus_iam_core::authz::model::PolicyKind::Static,
            source: format!(r#"permit(principal == Pgs::Iam::Principal::"{uuid}", action == Pgs::Iam::Action::"{action}", resource);"#),
            description: format!("SMA-712 action-identity pin: {action} only"),
            system: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        state.policies.put(&admin, doc).await.expect("platform_admin may PutPolicy at Root");
        tokens.push(token);
    }

    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);
    for (holder, token) in tokens.iter().enumerate() {
        for op in 0..OPS.len() {
            let err = call_op(&mut client, op, Some(token.as_str()), &missing_user_prn(), &idp.issuer).await.unwrap_err();
            let want = if holder == op { Code::NotFound } else { Code::PermissionDenied };
            assert_eq!(err.code(), want, "a subject that holds only {} calls {}", OPS[holder], OPS[op]);
        }
    }
    server.abort();
}

/// SMA-712 spec 5.1 and 11: the check runs before validation, and `enforce_tenancy = false`
/// does not open the calls. A denied caller with bad values gets PermissionDenied.
#[tokio::test]
async fn identity_rpcs_ignore_enforce_tenancy_and_authorize_before_validation() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let state = AppState::new(db, &cfg).await.unwrap();
    let token = idp.bearer("grpc-toggle-id", Some("grpc-toggle-id@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    for op in 0..OPS.len() {
        let err = call_op(&mut client, op, Some(token.as_str()), &missing_user_prn(), &idp.issuer).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "{} must stay closed with enforce_tenancy = false", OPS[op]);
    }

    let err = client.find_user_by_email(authed(FindUserByEmailRequest { email: "not-an-email".to_string() }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let err = client
        .link_external_identity(authed(
            LinkExternalIdentityRequest {
                user_prn: missing_user_prn(),
                issuer: "https://unknown.example.com/".to_string(),
                subject: " padded".to_string(),
                reason: String::new(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let err = client
        .unlink_external_identity(authed(
            UnlinkExternalIdentityRequest {
                user_prn: missing_user_prn(),
                external_identity_id: Uuid::from_u128(1).to_string(),
                reason: "   ".to_string(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let err = client
        .change_user_email(authed(
            ChangeUserEmailRequest {
                user_prn: missing_user_prn(),
                email: "@".to_string(),
                reason: String::new(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);

    server.abort();
}
```

In the test module of `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs`, append:

```rust
    /// SMA-712 spec 4.2: identities keep their `(created_at, id)` order, an identity is
    /// immutable (`modified_at == created_at`), and a user's `modified_at` is `updated_at`.
    #[test]
    fn a_user_projects_its_identities_in_order_and_each_identity_audit_is_immutable() {
        use paigasus_iam_core::{Email, Issuer, PrincipalStatus, User};
        let at = |secs: i64| DateTime::from_timestamp(secs, 0).unwrap();
        let pid = principal(5);
        let identity = |n: u128, subject: &str, secs: i64| ExternalIdentity {
            id: Uuid::from_u128(n),
            principal_id: pid.clone(),
            issuer: Issuer::parse("https://idp.example.com").unwrap(),
            subject: subject.to_string(),
            created_at: at(secs),
            updated_at: at(secs + 60),
        };
        let view = UserWithIdentities {
            user: User::new(pid.clone(), Email::parse("u@example.com").unwrap(), "U".to_string(), None, None, at(100), at(200)),
            status: PrincipalStatus::Active,
            identities: vec![identity(1, "first", 300), identity(2, "second", 400)],
        };
        let proto = to_proto_user(&view);
        assert_eq!(proto.prn, pid.canonical());
        assert_eq!(proto.email, "u@example.com");
        assert_eq!(proto.display_name, "U");
        assert_eq!(proto.status, "active");
        let subjects: Vec<&str> = proto.external_identities.iter().map(|i| i.subject.as_str()).collect();
        assert_eq!(subjects, vec!["first", "second"]);
        assert_eq!(proto.external_identities[0].id, Uuid::from_u128(1).to_string());
        let identity_audit = proto.external_identities[0].audit.as_ref().unwrap();
        assert_eq!(identity_audit.created_at, identity_audit.modified_at, "an identity is immutable");
        let user_audit = proto.audit.as_ref().unwrap();
        assert_eq!(user_audit.modified_at, Some(ts(at(200))));
    }
```

In the test module of `rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs`, append:

```rust
    /// SMA-712 spec 4.1: gRPC names a user by its full principal PRN. Anything else is
    /// `invalid-prn` (the recorded transport divergence: HTTP answers `invalid-uuid`).
    #[test]
    fn a_user_prn_must_name_an_iam_principal() {
        let uuid = uuid::Uuid::from_u128(7);
        assert_eq!(user_id(&format!("prn:pgs:iam:::principal/{uuid}")).unwrap().uuid(), uuid);
        assert!(matches!(user_id(""), Err(TenancyError::InvalidPrn(_))));
        assert!(matches!(user_id("not a prn"), Err(TenancyError::InvalidPrn(_))));
        let org = paigasus_iam_core::OrganizationId::from_uuid(uuid);
        assert!(matches!(user_id(&org.canonical()), Err(TenancyError::InvalidPrn(_))));
    }

    #[test]
    fn an_external_identity_id_must_be_a_uuid() {
        let id = uuid::Uuid::from_u128(3);
        assert_eq!(external_identity_id(&id.to_string()).unwrap(), id);
        assert_eq!(external_identity_id("x").unwrap_err(), TenancyError::InvalidUuid("external_identity_id"));
    }
```

- [ ] **Step 4: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam -E 'test(/user_prn|external_identity_id|projects_its_identities/)'
```

Expected: compile error `not all trait items implemented, missing: find_user_by_email,
link_external_identity, unlink_external_identity, change_user_email` for `UserGrpc`.

- [ ] **Step 5: Add the conversions.** In `convert.rs`, add to the `paigasus_iam_core` import
  (lines 18-21) the names `ExternalIdentity` and `UserWithIdentities`, and add to the
  `paigasus_proto::paigasus::iam::v1` import (lines 26-30) the names
  `ExternalIdentity as ProtoExternalIdentity` and `User as ProtoUser`. Insert after
  `to_proto_service_account` (ends at line 496):

```rust

/// Projects one external identity into its wire message (SMA-712). An identity is immutable, so
/// `audit.modified_at == created_at` (proto doc). The identity row has no actor columns.
pub fn to_proto_external_identity(identity: &ExternalIdentity) -> ProtoExternalIdentity {
    ProtoExternalIdentity {
        id: identity.id.to_string(),
        issuer: identity.issuer.as_str().to_string(),
        subject: identity.subject.clone(),
        audit: Some(audit(AuditFields {
            created: identity.created_at,
            modified: identity.created_at,
            creator: None,
            modifier: None,
        })),
    }
}

/// Projects a user with its identities into its wire message (SMA-712). `status` is the
/// principal status string (`active`/`disabled`), the house convention. `modified_at` is
/// `"user".updated_at`. The `"user"` row has no actor columns.
pub fn to_proto_user(view: &UserWithIdentities) -> ProtoUser {
    ProtoUser {
        prn: view.user.principal_id.canonical(),
        email: view.user.email.as_str().to_string(),
        display_name: view.user.display_name.clone(),
        status: view.status.as_str().to_string(),
        external_identities: view.identities.iter().map(to_proto_external_identity).collect(),
        audit: Some(audit(AuditFields {
            created: view.user.created_at,
            modified: view.user.updated_at,
            creator: None,
            modifier: None,
        })),
    }
}
```

- [ ] **Step 6: Implement the gRPC handlers.** In `grpc/users.rs`, add a paragraph to the module
  doc after line 22:

```rust
//!
//! **The four identity RPCs (SMA-712)** — `FindUserByEmail`, `LinkExternalIdentity`,
//! `UnlinkExternalIdentity`, `ChangeUserEmail` — authorize INSIDE `UserIdentityService`, with no
//! `enforce_tenancy` gate (spec 5.1). This adapter only parses the wire values and forwards the
//! bearer-resolved actor and credential. A malformed `user_prn` or `external_identity_id` is
//! refused here, before the service runs, the same way the HTTP path extractor refuses a
//! malformed uuid before its handler runs.
```

Replace the imports at lines 27-28 with:

```rust
use paigasus_proto::paigasus::iam::v1::user_service_server::UserService;
use paigasus_proto::paigasus::iam::v1::{
    ChangeUserEmailRequest, ChangeUserEmailResponse, CreateUserRequest, CreateUserResponse, FindUserByEmailRequest, FindUserByEmailResponse, LinkExternalIdentityRequest,
    LinkExternalIdentityResponse, UnlinkExternalIdentityRequest, UnlinkExternalIdentityResponse,
};
```

and add after line 32 (`use paigasus_iam_core::authz::model::root_prn;`):

```rust
use paigasus_iam_core::PrincipalId;
use paigasus_kernel::Prn;
use uuid::Uuid;
```

Insert after `opt_string` (ends at line 70):

```rust

/// Parses a wire `user_prn` into the [`PrincipalId`] it names (SMA-712). It must be an `iam`
/// `principal` PRN. The service then answers 404 when that principal is not a user.
fn user_id(raw: &str) -> Result<PrincipalId, TenancyError> {
    let parsed = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    if parsed.service() != "iam" || parsed.resource_type() != "principal" {
        return Err(TenancyError::InvalidPrn(parsed.canonical()));
    }
    Ok(PrincipalId::from_prn(parsed))
}

/// Parses a wire `external_identity_id` (SMA-712).
fn external_identity_id(raw: &str) -> Result<Uuid, TenancyError> {
    Uuid::parse_str(raw).map_err(|_| TenancyError::InvalidUuid("external_identity_id"))
}
```

Insert inside `impl UserService for UserGrpc`, after `create_user` (after line 101):

```rust

    /// `FindUserByEmail` (SMA-712): `GetUser` at Root, checked in `UserIdentityService`.
    async fn find_user_by_email(&self, request: Request<FindUserByEmailRequest>) -> Result<Response<FindUserByEmailResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<FindUserByEmailResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let view = self.state.user_identities.find_by_email(&actor, &req.email).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(FindUserByEmailResponse { user: Some(convert::to_proto_user(&view)) }))
        }
        .await;
        record_grpc("User", "FindUserByEmail", started, &result);
        result
    }

    /// `LinkExternalIdentity` (SMA-712). A same-user retry returns the stored identity.
    async fn link_external_identity(&self, request: Request<LinkExternalIdentityRequest>) -> Result<Response<LinkExternalIdentityResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<LinkExternalIdentityResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let user = user_id(&req.user_prn).map_err(convert::status_to_grpc)?;
            let out = self.state.user_identities.link(&actor, &user, &req.issuer, &req.subject, &req.reason).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(LinkExternalIdentityResponse {
                external_identity: Some(convert::to_proto_external_identity(&out.value)),
            }))
        }
        .await;
        record_grpc("User", "LinkExternalIdentity", started, &result);
        result
    }

    /// `UnlinkExternalIdentity` (SMA-712). The caller's own credential goes to the service, which
    /// refuses to unlink the identity that authenticated this request.
    async fn unlink_external_identity(&self, request: Request<UnlinkExternalIdentityRequest>) -> Result<Response<UnlinkExternalIdentityResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<UnlinkExternalIdentityResponse>, Status> = async {
            let ctx = actor_context(&request)?;
            let actor = ctx.principal_id.prn().clone();
            let req = request.into_inner();
            let user = user_id(&req.user_prn).map_err(convert::status_to_grpc)?;
            let identity = external_identity_id(&req.external_identity_id).map_err(convert::status_to_grpc)?;
            self.state
                .user_identities
                .unlink(&actor, &ctx.credential, &user, identity, &req.reason)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(UnlinkExternalIdentityResponse {}))
        }
        .await;
        record_grpc("User", "UnlinkExternalIdentity", started, &result);
        result
    }

    /// `ChangeUserEmail` (SMA-712).
    async fn change_user_email(&self, request: Request<ChangeUserEmailRequest>) -> Result<Response<ChangeUserEmailResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ChangeUserEmailResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let user = user_id(&req.user_prn).map_err(convert::status_to_grpc)?;
            let view = self.state.user_identities.change_email(&actor, &user, &req.email, &req.reason).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ChangeUserEmailResponse { user: Some(convert::to_proto_user(&view)) }))
        }
        .await;
        record_grpc("User", "ChangeUserEmail", started, &result);
        result
    }
```

In `grpc/mod.rs`, replace lines 7-8:

```rust
//! mounted — every RPC authorizes at `Root`: `CreateUser` since SMA-584, the four identity
//! RPCs since SMA-712, see `users` module doc), the `OutboxService`
```

and replace lines 92-93:

```rust
        // SMA-501: always served, mirroring HTTP's unconditional `/v1/users` mount — every RPC
        // authorizes at `Root` (SMA-584, SMA-712, see `users` module doc).
```

- [ ] **Step 7: Wire `AppState`.** In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`:

  - In the `crate::adapters::persistence` import (lines 66-69), add `PgIdentityLinkStore`.
  - After line 79 (`use crate::application::dead_letters::{DeadLetterDeps, DeadLetterService};`) add:

```rust
use crate::application::user_identities::{UserIdentityDeps, UserIdentityService};
```

  - After the `dead_letters` field (line 255) add:

```rust
    /// The operator identity-link use case (SMA-712) — `/v1/users/find-by-email`,
    /// `/v1/users/{id}/external-identities*`, `/v1/users/{id}/email` and the four matching
    /// `UserService` RPCs call through this. It authorizes each call itself, at Root, with no
    /// `enforce_tenancy` gate.
    pub user_identities: UserIdentityService,
```

  - After the `let authn = AuthenticateToken::new(…);` statement (ends at line 785) add:

```rust

        // SMA-712: the operator identity-link calls. The issuer set is the one `jit_flags`
        // parsed from `authn.issuers` above, so a link can only name an issuer that IAM accepts
        // a token from. Its own `SeaOrmUnitOfWork`, like `dead_letter_uow`: each write and its
        // audit entry commit on one transaction, on the shared `audit_log` handle.
        let user_identities = UserIdentityService::new(UserIdentityDeps {
            authorize: authorize.clone(),
            links: Arc::new(PgIdentityLinkStore::new(db.clone())),
            uow: Arc::new(SeaOrmUnitOfWork::new(db.clone())),
            audit: audit_log.clone(),
            issuers: jit_flags.iter().map(|(issuer, _)| issuer.clone()).collect(),
            ids: Arc::new(KernelIdGenerator),
            clock: Arc::new(SystemClock),
        });
```

  - In the `Ok(AppState { … })` literal, after `dead_letters,` (line 817) add `user_identities,`.
  - In the `enforce_tenancy` boot warning (line 790), add the new service to the list of
    application-layer groups that still authorize. Change
    `audit, outbox dead letters, system retirement) still applies` to
    `audit, outbox dead letters, system retirement, user identity links) still applies`.
    The list must name every service that authorizes with no toggle. An operator reads the list
    to learn what the toggle bypasses. Grep the tests for the old warning text first, and update
    any test that asserts it.

- [ ] **Step 8: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam -E 'test(/user_prn|external_identity_id|projects_its_identities|service_registration/)'
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass -p paigasus-iam --test grpc_users
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && moon run paigasus-proto-ts:test paigasus-proto-ts:typecheck paigasus-sdk-ts:test paigasus-sdk-ts:typecheck
```

Expected: every test passes, including the seven existing `grpc_users.rs` tests.

- [ ] **Step 9: Commit.**

```bash
W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity
git -C $W add contracts/proto/paigasus/iam/v1/iam.proto rs/crates/libs/paigasus-proto/src/generated py/packages/paigasus-proto/src/paigasus_proto/generated ts/packages/paigasus-proto/src/generated rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs rs/crates/services/paigasus-iam/tests/grpc_users.rs
git -C $W commit -m "feat(repo): add the four identity RPCs to UserService (SMA-712)

FindUserByEmail, LinkExternalIdentity, UnlinkExternalIdentity and
ChangeUserEmail are thin handlers over UserIdentityService, which
AppState now builds from the configured issuers.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: HTTP routes, DTOs and path markers

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/path.rs:78` (markers) and `:353-364` (stable-names test)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs:217` (after `CreateUserResponse`) and its imports (`:10-13`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/users.rs` (whole file)
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (new helper after `provision_platform_admin`, `:686-690`)
- Modify: `rs/crates/services/paigasus-iam/tests/http_users.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_enforce_toggle.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs:427` (protected route list)

**Interfaces:**
- Consumes: `AppState.user_identities` (Task 7).
- Produces: path markers `UserId => "user_id"`, `ExternalIdentityId => "external_identity_id"`.
- Produces: DTOs `FindUserByEmailBody`, `LinkExternalIdentityBody`, `UnlinkExternalIdentityBody`, `ChangeUserEmailBody` (all fields `Option<String>` with `#[serde(default)]`), `ExternalIdentityDto {id, issuer, subject, created_at}`, `UserDto {prn, email, display_name, status, external_identities, created_at, updated_at}`.
- Produces: `support::principal_uuid(prn: &str) -> uuid::Uuid`.

- [ ] **Step 1: Add the test helper.** In `tests/support/mod.rs`, insert after
  `provision_platform_admin` (ends at line 690):

```rust

/// The bare uuid inside a canonical principal PRN — the `{id}` segment of the SMA-712
/// `/v1/users/{id}/*` routes.
#[allow(dead_code)]
pub fn principal_uuid(prn: &str) -> uuid::Uuid {
    Prn::parse(prn).expect("valid principal prn").resource_id()
}
```

- [ ] **Step 2: Write the failing tests.** In `tests/http_users.rs`, replace lines 13-18 (the
  `use` block) with:

```rust
use axum::http::StatusCode;
use paigasus_iam::adapters::persistence::entities::principal;
use paigasus_kernel::Prn;
use sea_orm::{EntityTrait, PaginatorTrait};
use serde_json::json;
use support::{app_with_config, app_with_state, principal_uuid, provision, provision_platform_admin, send, test_config};
use uuid::Uuid;
```

Append to `tests/http_users.rs`:

```rust
// --- SMA-712: the operator identity routes --------------------------------------------------

/// The four identity routes as `(action, uri, body)`, aimed at a principal that does not exist.
/// A caller that passes the check gets 404. A caller that fails it gets 403.
fn identity_calls(issuer: &str) -> Vec<(&'static str, String, serde_json::Value)> {
    let missing = Uuid::from_u128(0xdead);
    let identity = Uuid::from_u128(0xbeef);
    vec![
        ("GetUser", "/v1/users/find-by-email".to_string(), json!({"email": "nobody-http@example.com"})),
        (
            "LinkExternalIdentity",
            format!("/v1/users/{missing}/external-identities"),
            json!({"issuer": issuer, "subject": "matrix-sub", "reason": "matrix"}),
        ),
        ("UnlinkExternalIdentity", format!("/v1/users/{missing}/external-identities/{identity}/unlink"), json!({"reason": "matrix"})),
        ("ChangeUserEmail", format!("/v1/users/{missing}/email"), json!({"email": "matrix-http@example.com", "reason": "matrix"})),
    ]
}

/// SMA-712 spec 11: 401 without a token, 403 without the action, and the full platform_admin
/// cycle: find, link (201, then 200 on a retry), change the email, unlink (204, then 404), and
/// the own-identity refusal (409).
#[tokio::test]
async fn identity_routes_need_a_bearer_and_their_action_and_work_for_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let plain = idp.bearer("http-id-plain", Some("http-id-plain@example.com"), "paigasus", 3600);
    provision(&state, &plain).await;
    let admin = idp.bearer("http-id-admin", Some("http-id-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;

    for (action, uri, body) in identity_calls(&idp.issuer) {
        let (status, err) = send(&app, "POST", &uri, Some(body.clone()), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{action} without a bearer: {err}");
        let (status, err) = send(&app, "POST", &uri, Some(body), Some(plain.as_str())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{action} without the action: {err}");
        assert_eq!(err["error"]["code"], "forbidden");
    }

    let (status, created) = send(&app, "POST", "/v1/users", Some(json!({"email": "http-id-target@example.com", "display_name": "Target"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let target_prn = created["principal_prn"].as_str().unwrap().to_string();
    let target = principal_uuid(&target_prn);

    let (status, user) = send(&app, "POST", "/v1/users/find-by-email", Some(json!({"email": "http-id-target@example.com"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["prn"], target_prn);
    assert_eq!(user["status"], "active");
    assert_eq!(user["external_identities"], json!([]));

    let link_body = json!({"issuer": idp.issuer, "subject": "http-linked-sub", "reason": "INC-1: same person"});
    let (status, linked) = send(&app, "POST", &format!("/v1/users/{target}/external-identities"), Some(link_body.clone()), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{linked}");
    assert_eq!(linked["subject"], "http-linked-sub");
    let (status, again) = send(&app, "POST", &format!("/v1/users/{target}/external-identities"), Some(link_body), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::OK, "a same-user link answers 200: {again}");
    assert_eq!(again["id"], linked["id"]);

    let (status, moved) = send(
        &app,
        "POST",
        &format!("/v1/users/{target}/email"),
        Some(json!({"email": "http-id-moved@example.com", "reason": "INC-2: moved"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["email"], "http-id-moved@example.com");
    assert_eq!(moved["external_identities"].as_array().unwrap().len(), 1);

    let identity_id = linked["id"].as_str().unwrap();
    let unlink_uri = format!("/v1/users/{target}/external-identities/{identity_id}/unlink");
    let (status, body) = send(&app, "POST", &unlink_uri, Some(json!({"reason": "INC-3: undo"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, err) = send(&app, "POST", &unlink_uri, Some(json!({"reason": "INC-3: undo"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a repeated unlink is not found: {err}");
    assert_eq!(err["error"]["code"], "not-found");

    let (status, me) = send(&app, "POST", "/v1/users/find-by-email", Some(json!({"email": "http-id-admin@example.com"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let my_user = principal_uuid(me["prn"].as_str().unwrap());
    let my_identity = me["external_identities"][0]["id"].as_str().unwrap();
    let (status, err) = send(
        &app,
        "POST",
        &format!("/v1/users/{my_user}/external-identities/{my_identity}/unlink"),
        Some(json!({"reason": "lock myself out"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "cannot-unlink-own-identity");
}

/// SMA-712 spec 11, action identity end to end, the HTTP half of
/// `tests/grpc_users.rs::each_identity_rpc_checks_its_own_action`.
#[tokio::test]
async fn each_identity_route_checks_its_own_action() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.policy_cache_ttl_secs = 1;
    let (app, state) = app_with_config(db, &cfg).await;
    let admin = idp.bearer("http-matrix-admin", Some("http-matrix-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;

    let calls = identity_calls(&idp.issuer);
    let mut tokens = Vec::new();
    for (index, (action, _, _)) in calls.iter().enumerate() {
        let token = idp.bearer(&format!("http-matrix-{index}"), Some(format!("http-matrix-{index}@example.com").as_str()), "paigasus", 3600);
        let uuid = principal_uuid(&provision(&state, &token).await);
        let policy = json!({
            "policy_id": format!("sma-712-http-matrix-{index}"),
            "kind": "static",
            "source": format!(r#"permit(principal == Pgs::Iam::Principal::"{uuid}", action == Pgs::Iam::Action::"{action}", resource);"#),
            "description": format!("SMA-712 action-identity pin: {action} only"),
        });
        let (status, put) = send(&app, "POST", "/v1/authz/policies", Some(policy), Some(admin.as_str())).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        tokens.push(token);
    }

    for (holder, token) in tokens.iter().enumerate() {
        for (called, (action, uri, body)) in calls.iter().enumerate() {
            let (status, err) = send(&app, "POST", uri, Some(body.clone()), Some(token.as_str())).await;
            let want = if holder == called { StatusCode::NOT_FOUND } else { StatusCode::FORBIDDEN };
            assert_eq!(status, want, "a subject that holds only {} calls {action}: {err}", calls[holder].0);
        }
    }
}

/// SMA-712 spec 5.1 and 11: a denied caller with a well-formed body of bad values gets 403, not
/// 400, so the routes are not a validation oracle.
#[tokio::test]
async fn a_denied_caller_with_bad_values_gets_403_not_400() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let plain = idp.bearer("http-bad-plain", Some("http-bad-plain@example.com"), "paigasus", 3600);
    provision(&state, &plain).await;
    let target = Uuid::from_u128(0xdead);
    for (uri, body) in [
        ("/v1/users/find-by-email".to_string(), json!({"email": "not-an-email"})),
        (format!("/v1/users/{target}/external-identities"), json!({"issuer": "https://unknown.example.com/", "subject": " padded", "reason": ""})),
        (format!("/v1/users/{target}/external-identities/{}/unlink", Uuid::from_u128(1)), json!({"reason": "   "})),
        (format!("/v1/users/{target}/email"), json!({"email": "@", "reason": ""})),
    ] {
        let (status, err) = send(&app, "POST", &uri, Some(body), Some(plain.as_str())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {err}");
        assert_eq!(err["error"]["code"], "forbidden");
    }
}

/// SMA-712 spec 4.3: every DTO field is optional with a default, so a missing field reaches the
/// service as an empty value and gets the same code as an empty gRPC field. A body of the wrong
/// type still gets the extractor's 422, and a bad path uuid gets `invalid-uuid`.
#[tokio::test]
async fn a_missing_body_field_gets_the_same_code_as_an_empty_grpc_field() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin = idp.bearer("http-empty-admin", Some("http-empty-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;
    let (status, created) = send(&app, "POST", "/v1/users", Some(json!({"email": "http-empty-target@example.com", "display_name": "T"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let target = principal_uuid(created["principal_prn"].as_str().unwrap());

    for (uri, code) in [
        ("/v1/users/find-by-email".to_string(), "invalid-email"),
        (format!("/v1/users/{target}/external-identities"), "unknown-issuer"),
        (format!("/v1/users/{target}/external-identities/{}/unlink", Uuid::from_u128(1)), "invalid-reason"),
        (format!("/v1/users/{target}/email"), "invalid-email"),
    ] {
        let (status, err) = send(&app, "POST", &uri, Some(json!({})), Some(admin.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {err}");
        assert_eq!(err["error"]["code"], code, "{uri}");
    }

    let (status, err) = support::send_bytes(
        &app,
        "POST",
        &format!("/v1/users/{target}/external-identities"),
        Some("application/json"),
        br#"{"reason": 5}"#,
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["error"]["code"], "invalid-request-schema");

    let (status, err) = send(&app, "POST", "/v1/users/not-a-uuid/email", Some(json!({"email": "a@example.com", "reason": "r"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-uuid");
    assert_eq!(err["error"]["message"], "user_id must be a uuid");
}
```

Append to `tests/authz_enforce_toggle.rs`:

```rust

/// SMA-712 spec 5.1 (the spec challenge BLOCKER): the identity routes authorize in the
/// application service, NOT behind `enforce_tenancy`. With the toggle off, a principal without
/// the actions still gets 403 on every route. If the check moved into an
/// `if state.enforce_tenancy` block, any principal could link its own identity to an admin.
#[tokio::test]
async fn enforce_tenancy_false_does_not_open_the_identity_routes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let (app, state) = app_with_config(db, &cfg).await;
    let token = idp.bearer("no-grants-identity", Some("no-grants-identity@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let missing = uuid::Uuid::from_u128(0xdead);
    for (uri, body) in [
        ("/v1/users/find-by-email".to_string(), json!({"email": "someone@example.com"})),
        (format!("/v1/users/{missing}/external-identities"), json!({"issuer": idp.issuer, "subject": "attacker-sub", "reason": "takeover"})),
        (format!("/v1/users/{missing}/external-identities/{}/unlink", uuid::Uuid::from_u128(1)), json!({"reason": "lockout"})),
        (format!("/v1/users/{missing}/email"), json!({"email": "attacker@example.com", "reason": "takeover"})),
    ] {
        let (status, body) = send(&app, "POST", &uri, Some(body), Some(token.as_str())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} must stay closed with enforce_tenancy = false: {body}");
    }
}
```

In `tests/http_authn.rs`, after line 427 (`("POST", "/v1/users".to_string()),`) insert:

```rust
        // users.rs (SMA-712)
        ("POST", "/v1/users/find-by-email".to_string()),
        ("POST", format!("/v1/users/{id}/external-identities")),
        ("POST", format!("/v1/users/{id}/external-identities/{id}/unlink")),
        ("POST", format!("/v1/users/{id}/email")),
```

In `src/adapters/http/path.rs`, append to `the_path_field_names_are_stable` (before its
closing `}` at line 364):

```rust
        assert_eq!(UserId::NAME, "user_id");
        assert_eq!(ExternalIdentityId::NAME, "external_identity_id");
```

- [ ] **Step 3: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam -E 'test(the_path_field_names_are_stable)'
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass --no-fail-fast -p paigasus-iam --test http_users --test authz_enforce_toggle --test http_authn
```

Expected: the first command fails to compile (`cannot find type UserId`). After you add only the
two markers, the Docker suites fail: the new routes answer 404 or 405 (no route), not 401/403/2xx.

- [ ] **Step 4: Add the path markers.** In `path.rs`, insert after line 78 (the `PolicyId` marker):

```rust
path_field!(/// `{id}` on a user route (SMA-712) — the user's principal uuid.
    UserId => "user_id");
path_field!(/// `{identity_id}` on the unlink route (SMA-712) — an external identity's uuid.
    ExternalIdentityId => "external_identity_id");
```

- [ ] **Step 5: Add the DTOs.** In `dto.rs`, add after the `paigasus_iam_core` import (line 13):

```rust
use paigasus_iam_core::{ExternalIdentity, UserWithIdentities};
```

Insert after `CreateUserResponse` (line 217):

```rust

// --- SMA-712: the operator identity routes -----------------------------------------------------
//
// Every body field is `Option<String>` with `#[serde(default)]`. A missing field reaches the
// service as an empty value, so both transports answer with the same code (for example
// `invalid-reason`). A body of the wrong type still gets the extractor's 422.

/// Body of `POST /v1/users/find-by-email`. The email is in the body, not the URL, so no request
/// line or access log records it.
#[derive(Debug, Clone, Deserialize)]
pub struct FindUserByEmailBody {
    #[serde(default)]
    pub email: Option<String>,
}

/// Body of `POST /v1/users/{id}/external-identities`.
#[derive(Debug, Clone, Deserialize)]
pub struct LinkExternalIdentityBody {
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Body of `POST /v1/users/{id}/external-identities/{identity_id}/unlink`.
#[derive(Debug, Clone, Deserialize)]
pub struct UnlinkExternalIdentityBody {
    #[serde(default)]
    pub reason: Option<String>,
}

/// Body of `POST /v1/users/{id}/email`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChangeUserEmailBody {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// An external identity over HTTP, the twin of proto `ExternalIdentity`. It flattens the audit
/// times like every other HTTP DTO (SMA-440 D7). An identity is immutable, so it has no
/// `updated_at`.
#[derive(Debug, Clone, Serialize)]
pub struct ExternalIdentityDto {
    pub id: Uuid,
    pub issuer: String,
    pub subject: String,
    pub created_at: DateTime<Utc>,
}

impl From<ExternalIdentity> for ExternalIdentityDto {
    fn from(identity: ExternalIdentity) -> Self {
        ExternalIdentityDto {
            id: identity.id,
            issuer: identity.issuer.as_str().to_string(),
            subject: identity.subject,
            created_at: identity.created_at,
        }
    }
}

/// A user over HTTP, the twin of proto `User`. `status` is the principal status string.
#[derive(Debug, Clone, Serialize)]
pub struct UserDto {
    pub prn: String,
    pub email: String,
    pub display_name: String,
    pub status: String,
    pub external_identities: Vec<ExternalIdentityDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<UserWithIdentities> for UserDto {
    fn from(view: UserWithIdentities) -> Self {
        UserDto {
            prn: view.user.principal_id.canonical(),
            email: view.user.email.as_str().to_string(),
            display_name: view.user.display_name,
            status: view.status.as_str().to_string(),
            external_identities: view.identities.into_iter().map(ExternalIdentityDto::from).collect(),
            created_at: view.user.created_at,
            updated_at: view.user.updated_at,
        }
    }
}
```

- [ ] **Step 6: Add the handlers.** In `http/users.rs`, add a paragraph to the module doc after
  line 19:

```rust
//!
//! **The identity routes (SMA-712)** — `POST /v1/users/find-by-email`,
//! `/v1/users/{id}/external-identities`, `/v1/users/{id}/external-identities/{identity_id}/unlink`
//! and `/v1/users/{id}/email` — authorize INSIDE `UserIdentityService`, with no
//! `enforce_tenancy` gate (spec 5.1). `{id}` is the user's principal uuid, the convention of
//! `/v1/service-accounts/{sa}`. Unlink is a `POST`, not a `DELETE`, because it carries a
//! `reason` body.
```

Replace the imports (lines 21-33) with:

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, PrincipalId};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{
    ChangeUserEmailBody, CreateUserBody, CreateUserResponse, ExternalIdentityDto, FindUserByEmailBody, LinkExternalIdentityBody, UnlinkExternalIdentityBody, UserDto,
};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{ExternalIdentityId, UserId, UuidPath, UuidPathPair};
use crate::adapters::auth::AuthContext;
use crate::application::create_user::NewUser;
```

Replace `router()` (lines 35-37) with:

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/users", post(create_user))
        .route("/v1/users/find-by-email", post(find_by_email))
        .route("/v1/users/{id}/external-identities", post(link_identity))
        .route("/v1/users/{id}/external-identities/{identity_id}/unlink", post(unlink_identity))
        .route("/v1/users/{id}/email", post(change_email))
}
```

Insert after `create_user` (ends at line 67):

```rust

/// The `PrincipalId` a `{id}` segment names. `Prn::build` with these fixed parts cannot fail.
fn user_id(uuid: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).expect("static principal prn parts are valid"))
}

/// `POST /v1/users/find-by-email` (SMA-712): 200 with the user, or 404.
async fn find_by_email(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<FindUserByEmailBody>) -> Result<Json<UserDto>, ApiError> {
    let view = s.user_identities.find_by_email(&actor_prn(&ctx), b.email.as_deref().unwrap_or_default()).await?;
    Ok(Json(view.into()))
}

/// `POST /v1/users/{id}/external-identities` (SMA-712): 201 with the new identity, or 200 with
/// the stored identity when the same user already holds the pair (a safe retry).
async fn link_identity(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<UserId>,
    EnvelopeJson(b): EnvelopeJson<LinkExternalIdentityBody>,
) -> Result<(StatusCode, Json<ExternalIdentityDto>), ApiError> {
    let out = s
        .user_identities
        .link(
            &actor_prn(&ctx),
            &user_id(path.id),
            b.issuer.as_deref().unwrap_or_default(),
            b.subject.as_deref().unwrap_or_default(),
            b.reason.as_deref().unwrap_or_default(),
        )
        .await?;
    let status = if out.changed { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(out.value.into())))
}

/// `POST /v1/users/{id}/external-identities/{identity_id}/unlink` (SMA-712): 204. A repeated
/// unlink gives 404. The caller's own credential goes to the service for the self-lockout guard.
async fn unlink_identity(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPathPair<UserId, ExternalIdentityId>,
    EnvelopeJson(b): EnvelopeJson<UnlinkExternalIdentityBody>,
) -> Result<StatusCode, ApiError> {
    s.user_identities
        .unlink(&actor_prn(&ctx), &ctx.credential, &user_id(path.first), path.second, b.reason.as_deref().unwrap_or_default())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/users/{id}/email` (SMA-712): 200 with the user.
async fn change_email(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<UserId>,
    EnvelopeJson(b): EnvelopeJson<ChangeUserEmailBody>,
) -> Result<Json<UserDto>, ApiError> {
    let view = s
        .user_identities
        .change_email(&actor_prn(&ctx), &user_id(path.id), b.email.as_deref().unwrap_or_default(), b.reason.as_deref().unwrap_or_default())
        .await?;
    Ok(Json(view.into()))
}
```

The existing `create_user` handler keeps `Action` and `root_prn` in use.

- [ ] **Step 7: Run the tests and see them pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam -E 'test(/path_field|router_merge|create_user_projects/)'
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass --no-fail-fast -p paigasus-iam --test http_users --test authz_enforce_toggle --test http_authn
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && python3 ci/http-extractor/check.py --self-test && python3 ci/http-extractor/check.py --check
python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
```

Expected: every test passes, `protected_router_merge_has_no_path_conflicts_in_any_capability_combination`
still passes, and both Python gates pass.

- [ ] **Step 8: Commit.**

```bash
W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity
git -C $W add rs/crates/services/paigasus-iam/src/adapters/http/path.rs rs/crates/services/paigasus-iam/src/adapters/http/dto.rs rs/crates/services/paigasus-iam/src/adapters/http/users.rs rs/crates/services/paigasus-iam/tests/support/mod.rs rs/crates/services/paigasus-iam/tests/http_users.rs rs/crates/services/paigasus-iam/tests/authz_enforce_toggle.rs rs/crates/services/paigasus-iam/tests/http_authn.rs
git -C $W commit -m "feat(rs): add the four identity routes under /v1/users (SMA-712)

The routes are thin handlers over UserIdentityService. Tests pin 401,
403 before 400, the action of each route, the enforce_tenancy toggle,
and the same code for a missing HTTP field and an empty gRPC field.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: End-to-end cases C1 to C4 through the mock IdP

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/http_users.rs` (append)

**Interfaces:**
- Consumes: the HTTP routes (Task 8), `support::{start_mock_idp, test_config_with, app_with_config, app_with_state, principal_uuid}`, `MockIdp::bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64) -> String` (`tests/support/mod.rs:181-194`), `test_config_with(idps: &[(&MockIdp, bool)], jwks_refresh_cooldown_secs: u64) -> IamConfig` (`tests/support/mod.rs:462-463`).
- Produces: no new API. These are the first tests in the tree that drive a JIT login to `email_conflict` through the mock IdP.

- [ ] **Step 1: Write the tests.** Append to `tests/http_users.rs`:

```rust
// --- SMA-712: the four cases of spec 1, end to end through the mock IdP ----------------------

type Login = Result<String, (StatusCode, String)>;

/// Logs in with `token` through `POST /v1/authn/whoami`, which JIT-provisions an unknown
/// identity. `Ok(principal_prn)` on 200; the status and error code otherwise.
async fn login(app: &axum::Router, token: &str) -> Login {
    let (status, body) = send(app, "POST", "/v1/authn/whoami", None, Some(token)).await;
    if status == StatusCode::OK {
        Ok(body["principal_prn"].as_str().expect("principal_prn").to_string())
    } else {
        Err((status, body["error"]["code"].as_str().unwrap_or_default().to_string()))
    }
}

/// A JIT login that IAM refused. With an email claim present, the defect is `email_conflict`.
fn refused() -> Login {
    Err((StatusCode::FORBIDDEN, "provisioning-failed".to_string()))
}

/// C1: `CreateUser` makes a user with no identity. Its first login fails with `email_conflict`.
/// The operator links `(issuer, subject)`. The next login resolves to the same principal.
#[tokio::test]
async fn c1_a_create_user_user_logs_in_after_the_operator_links_its_identity() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin = idp.bearer("c1-admin", Some("c1-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;
    let (status, created) = send(&app, "POST", "/v1/users", Some(json!({"email": "c1-person@example.com", "display_name": "C1"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let prn = created["principal_prn"].as_str().unwrap().to_string();

    let person = idp.bearer("c1-person-sub", Some("c1-person@example.com"), "paigasus", 3600);
    assert_eq!(login(&app, &person).await, refused(), "the first login of a CreateUser user fails");

    let (status, body) = send(
        &app,
        "POST",
        &format!("/v1/users/{}/external-identities", principal_uuid(&prn)),
        Some(json!({"issuer": idp.issuer, "subject": "c1-person-sub", "reason": "C1: confirmed by phone"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(login(&app, &person).await, Ok(prn));
}

/// C2: the same person signs in through a second issuer. The operator links the second
/// issuer's identity. Both logins resolve to one principal.
#[tokio::test]
async fn c2_a_second_issuer_resolves_to_the_same_principal_after_a_link() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp_a = support::start_mock_idp().await;
    let idp_b = support::start_mock_idp().await;
    let cfg = support::test_config_with(&[(&idp_a, true), (&idp_b, true)], 30);
    let (app, state) = app_with_config(db, &cfg).await;
    let admin = idp_a.bearer("c2-admin", Some("c2-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;

    let via_a = idp_a.bearer("c2-sub-a", Some("c2@example.com"), "paigasus", 3600);
    let prn = login(&app, &via_a).await.expect("JIT makes the user at the first issuer");
    let via_b = idp_b.bearer("c2-sub-b", Some("c2@example.com"), "paigasus", 3600);
    assert_eq!(login(&app, &via_b).await, refused(), "the second issuer's first login fails");

    let (status, body) = send(
        &app,
        "POST",
        &format!("/v1/users/{}/external-identities", principal_uuid(&prn)),
        Some(json!({"issuer": idp_b.issuer, "subject": "c2-sub-b", "reason": "C2: same person, second issuer"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(login(&app, &via_b).await, Ok(prn.clone()));
    assert_eq!(login(&app, &via_a).await, Ok(prn));
}

/// C3: the IdP gave the person a new `sub`. The operator links the new `sub`, then unlinks the
/// old one. The new `sub` resolves to the same principal. The old `sub` now fails with
/// `email_conflict`, because the email still belongs to the user.
#[tokio::test]
async fn c3_a_new_sub_is_linked_and_the_old_sub_is_unlinked() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin = idp.bearer("c3-admin", Some("c3-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;

    let old = idp.bearer("c3-old-sub", Some("c3@example.com"), "paigasus", 3600);
    let prn = login(&app, &old).await.expect("JIT makes the user with the old sub");
    let new = idp.bearer("c3-new-sub", Some("c3@example.com"), "paigasus", 3600);
    assert_eq!(login(&app, &new).await, refused());

    let user = principal_uuid(&prn);
    let (status, body) = send(
        &app,
        "POST",
        &format!("/v1/users/{user}/external-identities"),
        Some(json!({"issuer": idp.issuer, "subject": "c3-new-sub", "reason": "C3: realm import gave a new sub"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(login(&app, &new).await, Ok(prn.clone()));

    let (status, found) = send(&app, "POST", "/v1/users/find-by-email", Some(json!({"email": "c3@example.com"})), Some(admin.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let old_id = found["external_identities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["subject"] == "c3-old-sub")
        .expect("the old identity is listed")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = send(
        &app,
        "POST",
        &format!("/v1/users/{user}/external-identities/{old_id}/unlink"),
        Some(json!({"reason": "C3: the old sub is dead"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    assert_eq!(login(&app, &new).await, Ok(prn));
    assert_eq!(login(&app, &old).await, refused(), "the old sub now gets email_conflict");
}

/// C4: the email moved to another person at the IdP. The operator changes the old user's email
/// to an address that can never be delivered. The new person's login makes a different
/// principal. The old `sub` still resolves to the old principal.
#[tokio::test]
async fn c4_an_email_that_moved_to_another_person_makes_a_new_user() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin = idp.bearer("c4-admin", Some("c4-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;

    let old_person = idp.bearer("c4-old-sub", Some("c4@example.com"), "paigasus", 3600);
    let old_prn = login(&app, &old_person).await.expect("JIT makes the old person's user");
    let old_user = principal_uuid(&old_prn);

    let (status, body) = send(
        &app,
        "POST",
        &format!("/v1/users/{old_user}/email"),
        Some(json!({"email": format!("{old_user}@example.invalid"), "reason": "C4: the IdP moved the email to another person"})),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let new_person = idp.bearer("c4-new-sub", Some("c4@example.com"), "paigasus", 3600);
    let new_prn = login(&app, &new_person).await.expect("JIT makes a new user for the new person");
    assert_ne!(new_prn, old_prn);
    assert_eq!(login(&app, &old_person).await, Ok(old_prn));
}
```

- [ ] **Step 2: Run the tests.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass -p paigasus-iam --test http_users -E 'test(/^c[1-4]_/)'
```

Expected: 4 tests pass. These tests exercise code that Tasks 5-8 already made. To prove that
each test can fail, do step 3.

- [ ] **Step 3: Prove that C1 and C3 can fail.** In `http/users.rs` `link_identity`, change the
  subject argument `b.subject.as_deref().unwrap_or_default()` to
  `b.subject.as_deref().unwrap_or_default().trim_end_matches("-sub")`. This compiles. Run the
  step 2 command. Expected: C1, C2 and C3 fail, because the stored subject no longer matches the
  token. Undo the change with the Edit tool. Run `git -C <worktree> diff --exit-code -- rs/crates/services/paigasus-iam/src/adapters/http/users.rs`
  and expect exit 0. Run step 2 again and expect 4 passes.

- [ ] **Step 4: Commit.**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity add rs/crates/services/paigasus-iam/tests/http_users.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity commit -m "test(rs): drive the four identity-link cases through the mock IdP (SMA-712)

Each case starts from a real JIT refusal with email_conflict and ends
with the login that the operator call repaired.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Runbook `IamJitProvisioningFailures`

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md:1373-1387` (remediation step 4 and the Warning)

**Interfaces:**
- Consumes: the four HTTP routes (Task 8) and the Cedar action names (Task 4).
- Produces: no code.

- [ ] **Step 1: Write the failing check.**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && python3 - <<'EOF'
o = open("docs/ops/RUNBOOK-observability.md").read()
h = "### `IamJitProvisioningFailures` — IAM refused just-in-time provisioning (warning)"
assert o.count(h) == 1, "section heading missing"
s = o.index(h); e = o.index("### `IamRedisBreakerOpen`")
sec = o[s:e]
for gone in ["IAM has no API to update a user", "INSERT INTO external_identity"]:
    assert gone not in sec, f"section still has {gone!r}"
for needle in ["/v1/users/find-by-email", "/external-identities", "/unlink\"", "/email\"",
               "LinkExternalIdentity", "UnlinkExternalIdentity", "ChangeUserEmail", "platform_admin",
               "example.invalid", "orphan user", "forbid(principal", "/v1/audit", "repeated unlink",
               "bootstrapAdmins", "UPDATE external_identity SET issuer", "SMA-712", "external_identity",
               "case-sensitive", "CreateUser", "**Warning.**", "**When the alert is silent:**",
               "**Meaning.**", "**Remediation (`email_conflict`):**"]:
    assert needle in sec, f"section lacks {needle!r}"
print("ok: SMA-712 runbook text in place")
EOF
```

Expected: `AssertionError: section still has 'IAM has no API to update a user'`.

- [ ] **Step 2: Replace step 4 and the Warning.** In `docs/ops/RUNBOOK-observability.md`,
  replace lines 1373-1387 (from `4. Otherwise a manual Postgres change is necessary.` to the end
  of the `**Warning.**` paragraph, `both cases, also confirm that the identity provider verifies emails.`)
  with:

````markdown
4. Otherwise use the IAM identity-link API (SMA-712). Each call needs `platform_admin`, or an
   explicit grant of its Cedar action: `GetUser` for the find, `LinkExternalIdentity`,
   `UnlinkExternalIdentity` and `ChangeUserEmail` for the writes. Each write needs a `reason`.
   Write a ticket number and what you confirmed. The examples use these variables:

   ```bash
   IAM=https://iam.example.com        # the IAM HTTP base URL
   TOKEN=<an access token of a platform_admin>
   ```

   1. **Find the IAM user.** The call gives the principal and its linked identities. The email
      goes in the body, not the URL.

      ```bash
      curl -sS -X POST "$IAM/v1/users/find-by-email" \
        -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
        --data '{"email": "person@example.com"}'
      ```

      `prn` is `prn:pgs:iam:::principal/<principal-uuid>`. The routes below use the
      `<principal-uuid>` part. A `404 not-found` means that no user has the email.
   2. **Get the new `(issuer, subject)`.** The `issuer` is the configured
      `authn.issuers[].issuer` of the identity provider that the person used. It must be exactly
      equal to it, or the call gives `400 unknown-issuer`. For Keycloak, the `subject` is the
      user ID on the user's page in the admin console. For another identity provider, read its
      documentation for where the `sub` claim value is shown. If the identity provider uses a
      pairwise `sub` and does not show it, the person can read the `sub` claim from a token
      that the identity provider issued to them. Never guess a value.
   3. **Decide the case.** Compare the person at the identity provider with the IAM user from
      step 4.1:
      - C1. The IAM user has no identities, and the person is the one for whom `CreateUser` made
        the user.
      - C2. The IAM user has an identity at another issuer, and both identity-provider accounts
        belong to the same person.
      - C3. The IAM user has an identity at the same issuer with a different `sub`, and the
        identity provider confirms that the old account was deleted or imported again for the
        same person.
      - C4. The identity provider confirms that the email now belongs to a different person than
        the IAM user.
      - If you cannot confirm one of these, stop. Do not make a link.
   4. **Act.**
      - C1, C2: link the identity. A `201` is a new link. A `200` means that the user already
        had this identity (a safe retry).

        ```bash
        curl -sS -X POST "$IAM/v1/users/<principal-uuid>/external-identities" \
          -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
          --data '{"issuer": "https://idp.example.com/realms/main", "subject": "<sub>", "reason": "INC-1234: second issuer, confirmed by phone"}'
        ```

        A `409 external-identity-exists` means that another user holds the identity. There is
        no implicit move: unlink it from that user first, which gives two audit records.
      - C3: link the new `sub` with the call above. Then unlink the old `sub`. The old `sub`
        would give `email_conflict` again if the identity provider ever issued it.

        ```bash
        curl -sS -X POST "$IAM/v1/users/<principal-uuid>/external-identities/<identity-id>/unlink" \
          -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
          --data '{"reason": "INC-1234: realm import gave a new sub, old sub is dead"}'
        ```

        `<identity-id>` is the `id` of the old identity in the step 4.1 result. The call gives
        `204`. A repeated unlink gives `404 not-found`, also when the first call succeeded and
        only its response was lost. You cannot unlink the identity that you used to sign in:
        that gives `409 cannot-unlink-own-identity`.
      - C4: change the email of the old user. If the old person has no new email, use a unique
        address that can never be delivered: `<principal-uuid>@example.invalid`. JIT then makes
        a new user for the new person at the next login. The old user keeps its identities.

        ```bash
        curl -sS -X POST "$IAM/v1/users/<principal-uuid>/email" \
          -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
          --data '{"email": "<principal-uuid>@example.invalid", "reason": "INC-1234: the IdP moved the email to another person"}'
        ```

        A `409 email-conflict` means that another user has the email. The match is exact and
        case-sensitive, the same as JIT.
   5. **Reverse a wrong link.** Unlink it with the C3 call. Then read the audit log for the user
      to see the time of the link, and what the linked identity did after it:

      ```bash
      curl -sS -G "$IAM/v1/audit" -H "Authorization: Bearer $TOKEN" \
        --data-urlencode "resource=prn:pgs:iam:::principal/<principal-uuid>"
      ```

   `CreateUser` plus a link is also the way to add a user on an issuer that has JIT disabled.

   The issuer URL change case (step 2) keeps its guarded SQL statement. SMA-712 does not cover
   it.

**Warning.** A link can bring back the account-takeover risk that rule D5 prevents. JIT never
links by email. Only an operator call makes a link. For C1 to C3, confirm that the identity is
the same person before you link it. For C4, confirm at the identity provider that the email now
belongs to the new person. For every case, also confirm that the identity provider verifies
emails. Also know these facts:

- `LinkExternalIdentity` is equal to `platform_admin` in power. A holder can link an identity
  that it controls to any `platform_admin` user, or link a `zones.iam.backend.bootstrapAdmins`
  key to itself. Either way it becomes `platform_admin`. Do not grant the action to anyone who
  must not hold `platform_admin`.
- `UnlinkExternalIdentity` alone can lock out any user. An unlink of the last identity of a user
  locks that user out until a new link exists. A repeated unlink gives `404`.
- An unlink of a `bootstrapAdmins` key does not revoke the `platform_admin` grant that the key
  seeded. A new link of that key gives a second user the grant.
- A caller that signed in with an API key has no identity, so the own-identity guard does not
  apply to it. It can unlink the identity of the last human admin. Recovery then needs SQL.
- The audit log holds the `reason`, the email and the subject of each change. These are
  personal data. `ListAuditLog` restricts who can read them.
- **Orphan user.** When JIT wins a race with a link, or when a second issuer sent a different
  email, JIT makes a duplicate user. No API removes or disables it. It holds its email, so that
  email cannot go to another user until SQL changes it.
- To disable the three write calls without a binary revert, add a static Cedar `forbid`
  policy. It overrides the `platform_admin` permit:

  ```bash
  curl -sS -X POST "$IAM/v1/authz/policies" \
    -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    --data '{"policy_id": "disable-identity-link-writes", "kind": "static", "source": "forbid(principal, action in [Pgs::Iam::Action::\"LinkExternalIdentity\", Pgs::Iam::Action::\"UnlinkExternalIdentity\", Pgs::Iam::Action::\"ChangeUserEmail\"], resource);", "description": "SMA-712: identity-link writes are disabled"}'
  ```

  To enable them again, delete it: `curl -sS -X DELETE "$IAM/v1/authz/policies/disable-identity-link-writes" -H "Authorization: Bearer $TOKEN"`.
````

- [ ] **Step 3: Run the check and see it pass.** Run the step 1 command again.

Expected: `ok: SMA-712 runbook text in place`.

- [ ] **Step 4: Check the forbid policy text against the schema.** The JSON above escapes the
  inner quotes. The unescaped source must validate. Add nothing to the code. Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass -p paigasus-iam-core -E 'test(the_user_identity_actions_validate_against_the_embedded_schema)'
```

Expected: PASS. (That test validates each of the three action names against `SCHEMA_SRC`; the
forbid uses only those names.)

- [ ] **Step 5: Commit.**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity add docs/ops/RUNBOOK-observability.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity commit -m "docs(repo): use the identity-link API in the JIT failure runbook (SMA-712)

The email_conflict remediation now finds the user, decides the case and
acts with API calls. The SQL insert is gone. The warning states the
escalation facts and the forbid policy that disables the writes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Mutation checks and the full gate run

**Files:**
- Modify then restore: `rs/crates/services/paigasus-iam/src/application/user_identities.rs` (temporary edits only)

**Interfaces:**
- Consumes: everything above. Produces: evidence for the PR description. No commit unless a
  mutation survives and a test must be added.

Each mutation must COMPILE. The workspace denies warnings, so a mutation that leaves a variable
unused dies at `rustc` and proves nothing. Before each mutation, confirm a clean tree with
`git -C <worktree> status --short` (expect no output). Undo each mutation with the Edit tool,
then run `git -C <worktree> diff --exit-code` and expect exit 0. Never use `git checkout --`.

The unit-test command for each mutation:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity/rs && cargo nextest run --no-tests=pass --no-fail-fast -p paigasus-iam -E 'test(/user_identities/)'
```

- [ ] **Step 1: M1, delete the audit write (link).** In `link`, replace
  `self.audit.record(&*tx, &entry).await?;` with `let _ = (&self.audit, &entry);`.
  Expected failure: `link_creates_the_identity_and_writes_one_audit_entry` (0 entries).
  Undo it.

- [ ] **Step 2: M2a-M2d, delete each authorization check.** One at a time, replace the first
  line of each method, for example
  `self.authorize.check(actor, Action::GetUser, &root_prn()).await?;`, with
  `let _ = (&self.authorize, actor);`. Do this for `find_by_email`, `link`, `unlink` and
  `change_email`. Expected failures each time:
  `every_method_checks_its_own_action_at_root_before_any_store_call`,
  `holding_the_other_three_actions_does_not_open_a_method`,
  `a_denied_caller_gets_forbidden_even_when_every_value_is_invalid`. Undo each one.

- [ ] **Step 3: M3, swap one action.** In `link`, change `Action::LinkExternalIdentity` in the
  `check` call to `Action::UnlinkExternalIdentity`. Expected unit failures:
  `every_method_checks_its_own_action_at_root_before_any_store_call` and
  `holding_the_other_three_actions_does_not_open_a_method`. Also run
  `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass --no-fail-fast -p paigasus-iam --test http_users --test grpc_users -E 'test(/checks_its_own_action/)'`
  and expect `each_identity_route_checks_its_own_action` and
  `each_identity_rpc_checks_its_own_action` to fail. Undo it.

- [ ] **Step 4: M4, remove the own-identity guard.** In `unlink`, change the `if let` chain to
  `if let Credential::Oidc { issuer, subject, .. } = caller && false && removed.issuer == *issuer && removed.subject == *subject`.
  The bindings stay used, so it compiles. Expected failure:
  `unlinking_the_identity_that_authenticated_the_request_is_refused`. Also run
  `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --no-tests=pass --no-fail-fast -p paigasus-iam --test user_identities_pg -E 'test(an_own_identity_refusal_rolls_the_delete_back)'`
  and expect a failure. Undo it.

- [ ] **Step 5: M5, remove the same-user idempotency branch.** In `link`, change
  `if existing.principal_id.uuid() == user.uuid() {` to
  `if false && existing.principal_id.uuid() == user.uuid() {`. Expected failure:
  `linking_an_identity_the_user_already_holds_returns_it_unchanged` (it gets
  `ExternalIdentityConflict`). Undo it.

- [ ] **Step 6: Record the results.** Write one line per mutation (M1, M2a-d, M3, M4, M5): the
  edit, the tests that failed, and "restored, diff clean". Keep it for the PR description. If a
  mutation did not make a test fail, stop. Add a test that fails under it, commit that test as
  `test(rs): … (SMA-712)`, and run the mutation again.

- [ ] **Step 7: Run the full gate graph like CI.** Push nothing before this passes. The command
  is the one between the `ci-targets` markers in the root `CLAUDE.md`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-712-operator-link-identity && git fetch origin main && moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main \
  --include-relations
```

Expected: every selected target passes. Bash caveat (root `CLAUDE.md`, "This development Mac
only"): no single local bash runs every gate. `repo:affected-smoke` needs `/bin/bash` 3.2;
`repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep` and
`repo:nats-permissions` need bash 4+; `repo:actionlint` needs bash 5 and a pipe of at least
8192 bytes. Re-run a gate that failed only for its bash with `<bash-binary> ci/<gate>/run.sh`
and read that result instead. For an unattributed failure, follow the root `CLAUDE.md`
diagnosis procedure; capture `.moon/cache/ciReport.json` first.

- [ ] **Step 8: Check the codegen drift like CI.** CI runs an unconditional drift step
  (`contracts/CLAUDE.md`). Run `moon run contracts:generate --force` and then
  `git -C <worktree> status --short`. Expected: no output. If a generated file changed, commit
  it as `feat(contracts): regenerate the bindings (SMA-712)`.

---

## Spec deviations found

1. **The TypeScript registry mirrors are missing from spec section 12.** Adding five
   `ErrorReason` values breaks three TS files that the spec does not list:
   `ts/packages/paigasus-sdk/src/errors/presentation.ts:20` is a total
   `Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, …>` (a missing key is a `tsc` error),
   and `ts/packages/paigasus-sdk/tests/presentation.test.ts:13` and
   `ts/packages/paigasus-proto/src/error.test.ts:60` both assert `toHaveLength(60)`. Smallest
   fix: Task 2 adds the five entries as `'from-transport'` and moves both counts to 65.
2. **`ExternalIdentityId` is not a type.** The spec port uses `identity_id: ExternalIdentityId`
   and `id: PrincipalId` by value. `ExternalIdentity.id` is a plain `Uuid`
   (`paigasus-iam-core/src/authn.rs:150`), and `PrincipalId` is not `Copy`. The plan uses
   `Uuid` and `&PrincipalId`. The name `ExternalIdentityId` exists only as the HTTP path marker.
   The spec allows the plan to settle names and types (spec 6.2).
3. **The port gains `user_view_in`, and the service has no separate `users` dependency.**
   `ChangeUserEmailResponse` returns a `User` with its identities, and the spec port has no read
   that builds it. `user_view_in` reads it in the same transaction. `lock_user_in` covers the
   "is a user" check that a `users` port would do. The contract (every deciding read inside the
   transaction) is unchanged.
4. **`unlink_in` uses `SELECT … FOR UPDATE` then `DELETE`, not `DELETE … RETURNING`.** Both run
   in the caller's transaction, and the returned row is the deleted row. The plan does not rely
   on a SeaORM 2.0.3 `exec_with_returning` for deletes, which it could not verify.
5. **gRPC parses `user_prn` before the service authorizes.** A denied gRPC caller with a
   malformed `user_prn` (or `external_identity_id`) gets `invalid-prn` (or `invalid-uuid`), not
   `PermissionDenied`. Spec 5.1 names only "a body that the extractor refuses" as the exception.
   The plan treats the PRN parse as the gRPC twin of the HTTP path extractor, which already
   refuses a bad `{id}` before the handler runs. A well-formed PRN with bad body values still
   gets `PermissionDenied`, which is what spec 11 tests. The `grpc/users.rs` module doc states
   this.
6. **The HTTP DTOs flatten `AuditMetadata`.** Spec 4.3 says the DTOs mirror the proto messages.
   Every existing HTTP DTO flattens audit metadata into `created_at`/`updated_at`
   (`http/dto.rs`, SMA-440 D7). The plan follows the house convention.
7. **Line references in the spec are approximate.** The forbid generator is at `roles.rs:311-315`
   (function at 311, doc from 303); the `platform_admin` template is at `roles.rs:322-324`, not
   325-326; the runbook remediation is at lines 1355-1387, not about 1367-1390. The plan uses
   the measured lines.
8. **The boot warning lists application-layer groups by name** (`http/mod.rs:790`), and identity
   links are not in that list. Spec 5.1 says the text needs no change. The controller corrected
   this at plan review: Task 7 Step 7 adds "user identity links" to the list, so the warning
   names every service that authorizes with no toggle. Spec 5.1 is corrected to match.
