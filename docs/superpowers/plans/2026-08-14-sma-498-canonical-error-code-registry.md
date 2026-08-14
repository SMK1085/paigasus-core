# SMA-498 Canonical Error Code Registry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `contracts/proto/paigasus/common/v1/error.proto` — the canonical `(domain, reason)` error vocabulary — plus derived Rust wire-string helpers and the tests that prove the registry covers every code IAM's tenancy layer emits.

**Architecture:** Two proto enums (`ErrorDomain`, `ErrorReason`) act as **registries only, never wire types** — no `.proto` declares a field of either type, and the wire keeps carrying `google.rpc.ErrorInfo.reason`/`.domain` as strings. A hand-written `paigasus-proto` module derives the kebab wire string from prost's `as_str_name()`/`from_str_name()` so the spellings live in exactly one place: the `.proto`. This issue changes no error path; SMA-504 emits `ErrorInfo`, SMA-507 gates drift.

**Tech Stack:** protobuf3 + buf 2.x (`buf lint` STANDARD, `buf breaking` FILE), prost 0.14 (Rust), betterproto2 (Python), protobuf-es 2.13 (TypeScript), Moon 2.3.2, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-13-sma-498-canonical-error-code-registry-design.md`
**ADR:** ADR-0019 + Amendment A1 (2026-08-14).

> **This plan was executed and then amended to match what shipped.** Two things changed during
> implementation: Task 4's original design (a hand-listed variant array guarded by a wildcard-free
> `match`) was built and experimentally disproved twice, and `request-too-large` was renumbered from
> the IAM range into the shared range. Both are reflected below. Where any embedded code block still
> differs from the committed tree, **the committed code is authoritative** — do not copy from here
> without checking. The spec carries the reasoning for both amendments.

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (first line, before `syntax`).
- Shell setup for **every** command in this plan — the Bash tool's PATH lacks the proto-managed CLIs:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- All work happens in the worktree `/Users/smaschek/dev/paigasus/paigasus-core-sma498` on branch `feature/sma-498-canonical-error-code-registry`. **If you are a subagent, your first action must be `EnterWorktree {path: "/Users/smaschek/dev/paigasus/paigasus-core-sma498"}`** — subagents launched from a worktree session default to the MAIN checkout, and committing there is wrong.
- After editing any `.proto`, run `buf format -w` **before committing**, or `contracts:fmt` reds `moon ci` silently.
- Never bypass the commit hook with `--no-verify`.
- Commit subjects: conventional, **lowercase after the colon**, ≤100 chars, scope from `{rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace}`. Body lines ≤100 chars. **Never** put a bare `#NNN` in the body (it breaks `footer-leading-blank`); write "SMA-498" instead.
- `ErrorReason`/`ErrorDomain` must never become the type of a field in any `.proto`. They are registries.
- Rust: edition 2024, workspace lints are `warnings = deny` — dead code is a hard compile error on the lib target.

---

### Task 1: Make enum values retractable in `buf breaking`

`contracts/buf.yaml` already swaps `FIELD_NO_DELETE` for the reserve-tolerant variants (SMA-444) but never did the same for enum values, so `ENUM_VALUE_NO_DELETE` rejects a removed enum value **even when properly reserved**. Task 2 declares 15 spellings no emitter has validated; without this task a wrong one is permanent from first commit.

**Files:**
- Modify: `contracts/buf.yaml:15-26` (the `breaking:` block)

**Interfaces:**
- Consumes: nothing.
- Produces: a `buf.yaml` under which a `reserved` enum value passes `buf breaking`. Task 2 relies on this.

- [ ] **Step 1: Reproduce the defect**

Temporarily retire an existing enum value, properly reserved by both name and number. Edit `contracts/proto/paigasus/iam/v1/iam.proto`, replacing:

```proto
enum NodeStatus {
  NODE_STATUS_UNSPECIFIED = 0;
  NODE_STATUS_ACTIVE = 1;
  NODE_STATUS_ARCHIVED = 2;
}
```

with:

```proto
enum NodeStatus {
  NODE_STATUS_UNSPECIFIED = 0;
  NODE_STATUS_ACTIVE = 1;
  reserved 2;
  reserved "NODE_STATUS_ARCHIVED";
}
```

- [ ] **Step 2: Run the gate to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf breaking --against '../.git#branch=main,subdir=contracts'
```

Expected: exit 100 with
`proto/paigasus/iam/v1/iam.proto:24:1:Previously present enum value "2" on enum "NodeStatus" was deleted.`

This is the defect. Leave the `iam.proto` edit in place for Step 4.

- [ ] **Step 3: Add the enum-value reserve rules**

In `contracts/buf.yaml`, replace the `breaking:` block with:

```yaml
breaking:
  use:
    - FILE
    # FILE's default FIELD_NO_DELETE forbids ANY field removal, even a
    # `reserved`-and-retired one (SMA-444 IntrospectResponse.role_group_prns ->
    # role_grants). Swap it for the reserve-tolerant siblings so the
    # reserve+add pattern (spec §9.1) stays legal without loosening any other
    # FILE-category check.
    - FIELD_NO_DELETE_UNLESS_NAME_RESERVED
    - FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED
    # Same asymmetry, same fix, for enum values (SMA-498). The canonical error
    # registry in common/v1/error.proto declares codes ahead of their emitters,
    # so a mis-spelled value must be retractable via reserve+add rather than
    # permanent from first commit. ADR-0019 amendment A1.2.
    - ENUM_VALUE_NO_DELETE_UNLESS_NAME_RESERVED
    - ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED
  except:
    - FIELD_NO_DELETE
    - ENUM_VALUE_NO_DELETE
```

- [ ] **Step 4: Run the gate to verify it now passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf breaking --against '../.git#branch=main,subdir=contracts'
```

Expected: exit 0, no output. The same reserved-value edit that failed in Step 2 is now tolerated.

- [ ] **Step 5: Revert the probe edit**

Restore `contracts/proto/paigasus/iam/v1/iam.proto` to `NODE_STATUS_ARCHIVED = 2;` exactly as in Step 1's "before" block. Then confirm only `buf.yaml` is modified:

```bash
git status --short
```

Expected: exactly one line, ` M contracts/buf.yaml`.

- [ ] **Step 6: Confirm the gate is still green on an unmodified tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf breaking --against '../.git#branch=main,subdir=contracts' && buf lint
```

Expected: both exit 0.

- [ ] **Step 7: Commit**

```bash
git add contracts/buf.yaml
git commit -F - <<'EOF'
build(contracts): let a reserved enum value pass buf breaking (SMA-498)

buf's FILE category applies ENUM_VALUE_NO_DELETE, which rejects a removed enum
value even when it is reserved by both name and number. SMA-444 already swapped
the field-level rule for its reserve-tolerant siblings; enum values never got
the same treatment.

The canonical error registry landing next declares codes ahead of the services
that will emit them, so a mis-spelled value has to be retractable via
reserve-and-add instead of permanent from first commit.
EOF
```

---

### Task 2: The registry — `error.proto` and its generated bindings

**Files:**
- Create: `contracts/proto/paigasus/common/v1/error.proto`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`
- Regenerate (new file): `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`
- Regenerate: `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py`

**Interfaces:**
- Consumes: Task 1's `buf.yaml`.
- Produces: `paigasus_proto::paigasus::common::v1::{ErrorReason, ErrorDomain}` in Rust (prost emits one file per *package*, so these land beside `AuditMetadata` and **`lib.rs` needs no `include!` change**); `ErrorReason`/`ErrorDomain` in TS and Python. Every value carries `as_str_name()`/`from_str_name()` and `TryFrom<i32>` (from the `::prost::Enumeration` derive). Task 3 builds on all of these.

- [ ] **Step 1: Write `error.proto`**

Create `contracts/proto/paigasus/common/v1/error.proto` with exactly this content. Per-value comments are **one line** on purpose: prost embeds `SourceCodeInfo` in `FILE_DESCRIPTOR_SET`, so every comment byte becomes hex in the generated Rust.

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.common.v1;

// Canonical error registry (ADR-0019). Error identity is (domain, reason),
// carried on the wire as google.rpc.ErrorInfo — which this file deliberately
// does NOT import: referencing it emits Rust and TypeScript pointing at modules
// buf never generates. See the SMA-498 design doc §2.1.
//
// These enums are REGISTRIES, never wire types. No field anywhere is of type
// ErrorReason or ErrorDomain; the wire carries strings.
//
// Mapping rules (normative):
//   reason: strip ERROR_REASON_, lowercase, '_' -> '-'
//   domain: strip ERROR_DOMAIN_, lowercase, '_' -> '-', append '.paigasus.io'
// Each value's comment repeats the resulting literal verbatim. Kebab casing is
// a deliberate, documented deviation from Google's UPPER_SNAKE reason
// convention (ADR-0019 decision 3).
//
// THE REGISTRY IS APPEND-ONLY. Removing a value is a breaking change. Retract
// only by reserving both name and number. buf breaking is NOT what protects
// this vocabulary — it cannot see the kebab strings, cannot tell whether a code
// is still emitted or consumed, and does not run against Rust or TypeScript.
// The two-way drift gate (SMA-507) is the real guard.
//
// Numbering encodes which domain may emit a code, so the drift gate can derive
// it arithmetically:
//   1-299    IAM only
//   300-599  gateway only
//   600-899  reserved for a future domain (e.g. ERROR_DOMAIN_MODEL_ROUTER)
//   900-999  shared — any domain may emit
// Sub-group banners below are documentation only and carry no numbering
// meaning; a new IAM code takes the next free number in 1-299.
//
// ErrorInfo.metadata additionally carries two standard keys, populated by
// SMA-504: `retryable` and `correlation_id`. They are prose here, not
// enumerated: metadata is an open map, not a closed vocabulary consumers branch
// on exhaustively. The append-only guarantee above covers reasons and domains
// ONLY, not metadata keys.

// The service that produced an error. Not emitted on the gateway's
// OpenAI-compatible envelope, which has no domain field and carries only
// `code`; it applies wherever ErrorInfo itself travels.
enum ErrorDomain {
  ERROR_DOMAIN_UNSPECIFIED = 0;

  // "iam.paigasus.io"
  ERROR_DOMAIN_IAM = 1;

  // "gateway.paigasus.io"
  ERROR_DOMAIN_GATEWAY = 2;
}

// The canonical error vocabulary. See the file comment for the mapping rule,
// the append-only guarantee and the numbering ranges.
enum ErrorReason {
  ERROR_REASON_UNSPECIFIED = 0;

  // ---- IAM: tenancy (1-299) ------------------------------------------------
  // Emitted verbatim today by TenancyError::code().

  // "slug-conflict" — slug already taken in this scope.
  ERROR_REASON_SLUG_CONFLICT = 1;
  // "duplicate-membership" — principal already a member of this scope.
  ERROR_REASON_DUPLICATE_MEMBERSHIP = 2;
  // "email-conflict" — email address already taken.
  ERROR_REASON_EMAIL_CONFLICT = 3;
  // "service-account-name-conflict" — name taken under this owner node.
  ERROR_REASON_SERVICE_ACCOUNT_NAME_CONFLICT = 4;
  // "invalid-email" — email address failed validation.
  ERROR_REASON_INVALID_EMAIL = 5;
  // "invalid-slug" — slug failed validation.
  ERROR_REASON_INVALID_SLUG = 6;
  // "invalid-name" — name failed validation.
  ERROR_REASON_INVALID_NAME = 7;
  // "invalid-prn" — resource PRN failed to parse.
  ERROR_REASON_INVALID_PRN = 8;
  // "prn-mismatch" — PRN does not match the stored resource.
  ERROR_REASON_PRN_MISMATCH = 9;
  // "invalid-pagination" — limit/offset outside the accepted range.
  ERROR_REASON_INVALID_PAGINATION = 10;
  // "nothing-to-rename" — rename carried no new slug or name.
  ERROR_REASON_NOTHING_TO_RENAME = 11;
  // "not-found" — resource does not exist.
  ERROR_REASON_NOT_FOUND = 12;
  // "parent-archived" — parent resource is archived.
  ERROR_REASON_PARENT_ARCHIVED = 13;
  // "node-archived" — resource is archived.
  ERROR_REASON_NODE_ARCHIVED = 14;
  // "missing-org-membership" — principal is not a member of the organization.
  ERROR_REASON_MISSING_ORG_MEMBERSHIP = 15;
  // "forbidden" — authorization denied the request.
  ERROR_REASON_FORBIDDEN = 16;
  // "unknown-role" — role key is not in the role catalog.
  ERROR_REASON_UNKNOWN_ROLE = 17;
  // "invalid-scope" — scope PRN's node kind is outside the role's allow-list.
  ERROR_REASON_INVALID_SCOPE = 18;
  // "system-immutable" — system-owned row is immutable via the CRUD API.
  ERROR_REASON_SYSTEM_IMMUTABLE = 19;
  // "policy-invalid" — policy failed Cedar parse, schema or template-link checks.
  ERROR_REASON_POLICY_INVALID = 20;
  // "policy-conflict" — lost a concurrent-create race for this policy id.
  ERROR_REASON_POLICY_CONFLICT = 21;
  // "invalid-action" — action does not name a known Action variant.
  ERROR_REASON_INVALID_ACTION = 22;
  // "invalid-bulk-replay" — bulk replay needs an explicit non-zero max_rows.
  ERROR_REASON_INVALID_BULK_REPLAY = 23;
  // "not-system-owned" — retirement refused: the row is not system-owned.
  ERROR_REASON_NOT_SYSTEM_OWNED = 24;
  // "fleet-not-converged" — fleet is behind this binary's starter-policy revision.
  ERROR_REASON_FLEET_NOT_CONVERGED = 25;

  // ---- IAM: authn (1-299) --------------------------------------------------
  // Emitted snake_case today; SMA-504 recases the JSON body. The RFC 6750
  // WWW-Authenticate challenge value `invalid_token` is standardised and stays
  // snake_case — only the body's `code` becomes "invalid-token".

  // "invalid-token" — bearer token was rejected.
  ERROR_REASON_INVALID_TOKEN = 26;
  // "identity-not-provisioned" — no principal is provisioned for this identity.
  ERROR_REASON_IDENTITY_NOT_PROVISIONED = 27;
  // "provisioning-failed" — just-in-time provisioning could not complete.
  ERROR_REASON_PROVISIONING_FAILED = 28;
  // "principal-inactive" — principal exists but is not active.
  ERROR_REASON_PRINCIPAL_INACTIVE = 29;
  // "authn-unavailable" — IAM's own authentication backend is unreachable.
  ERROR_REASON_AUTHN_UNAVAILABLE = 30;

  // ---- IAM: system-row retirement (1-299) ----------------------------------
  // Emitted verbatim today by system_retirement.rs.

  // "grants-survive" — surviving grants must be revoked before retirement.
  ERROR_REASON_GRANTS_SURVIVE = 31;
  // "decision-change-unacknowledged" — retiring a static policy needs acknowledgement.
  ERROR_REASON_DECISION_CHANGE_UNACKNOWLEDGED = 32;

  // ---- Gateway (300-599) ---------------------------------------------------
  // Emitted snake_case today; SMA-504 recases them. `type` is untouched and
  // keeps its OpenAI semantics.

  // "missing-authorization" — no usable Authorization bearer credential.
  ERROR_REASON_MISSING_AUTHORIZATION = 300;
  // "invalid-api-key" — the credential was rejected by IAM.
  ERROR_REASON_INVALID_API_KEY = 301;
  // "insufficient-permissions" — identity is authenticated but not permitted.
  ERROR_REASON_INSUFFICIENT_PERMISSIONS = 302;
  // "missing-scope" — introspection returned no scope PRN.
  ERROR_REASON_MISSING_SCOPE = 303;
  // "iam-unavailable" — the gateway cannot reach IAM.
  ERROR_REASON_IAM_UNAVAILABLE = 304;
  // "upstream-unavailable" — the gateway cannot reach the model provider.
  ERROR_REASON_UPSTREAM_UNAVAILABLE = 305;
  // "upstream-timeout" — the model provider did not respond in time.
  ERROR_REASON_UPSTREAM_TIMEOUT = 306;
  // "upstream-error" — a streamed response failed mid-flight.
  ERROR_REASON_UPSTREAM_ERROR = 307;

  // ---- Shared (900-999) ----------------------------------------------------
  // Any domain may emit these; `domain` is what distinguishes them.

  // "internal" — an unexpected fault; detail stays in logs.
  ERROR_REASON_INTERNAL = 900;
  // "invalid-request-body" — the request body could not be deserialized; covers IAM's
  // invalid_request extractor rejection and the gateway's invalid_request_body, merged.
  ERROR_REASON_INVALID_REQUEST_BODY = 901;
  // "request-too-large" — request body exceeded the configured byte limit.
  ERROR_REASON_REQUEST_TOO_LARGE = 902;
}
```

- [ ] **Step 2: Format, lint and check the breaking gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf lint && buf breaking --against '../.git#branch=main,subdir=contracts'
```

Expected: all exit 0. `buf lint` is the check that the STANDARD ruleset is satisfied — `ENUM_VALUE_PREFIX` (every value prefixed `ERROR_REASON_`/`ERROR_DOMAIN_`), `ENUM_ZERO_VALUE_SUFFIX` (`_UNSPECIFIED = 0`) and `ENUM_PASCAL_CASE`. If lint fails, fix the proto — do **not** add a lint exception.

- [ ] **Step 3: Generate the bindings**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run contracts:generate
```

Expected: exit 0. A warning line `duplicate generated file name "paigasus/common/v1/__init__.py"` is **pre-existing betterproto2 noise** — ignore it.

- [ ] **Step 4: Verify all three languages generated the registry**

```bash
grep -c "ERROR_REASON_UPSTREAM_ERROR" rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs
grep -c "UPSTREAM_ERROR" ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts
grep -c "UPSTREAM_ERROR" py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py
```

Expected: a non-zero count from each. This is AC1.

- [ ] **Step 5: Verify the Rust and TypeScript trees compile**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-proto
```

Expected: exit 0.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run :typecheck
```

Expected: exit 0. (If either fails with a missing `google/rpc` module, the proto has acquired an import it must not have — remove it; see spec §2.1.)

- [ ] **Step 6: Commit**

```bash
git add contracts/proto/paigasus/common/v1/error.proto \
        rs/crates/libs/paigasus-proto/src/generated \
        ts/packages/paigasus-proto/src/generated \
        py/packages/paigasus-proto/src/paigasus_proto/generated
git commit -F - <<'EOF'
feat(contracts): add the canonical error code registry (SMA-498)

Declares the (domain, reason) vocabulary ADR-0019 decision 8 calls for: 43
reasons across two domains, as two proto enums that are registries only and
never wire types. The wire keeps carrying google.rpc.ErrorInfo.reason as a
string, so a new code cannot break an old client.

The file imports nothing. Referencing google.rpc.ErrorInfo makes buf generate
exit 0 while emitting a Rust path and a TypeScript import to modules the run
never produces, because it is not a well-known type.

Codes are seeded from all six emission sites, not just TenancyError::code():
the authn funnel, the extractor rejections, system-row retirement and the
gateway's terminal SSE frame each contribute. 28 spellings exist verbatim on
the wire today; the other 15 are canonical forms SMA-504 renames onto.
EOF
```

---

### Task 3: Derived wire-string helpers in `paigasus-proto`

**Files:**
- Create: `rs/crates/libs/paigasus-proto/src/error.rs`
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs` (append one `pub mod` line)

**Interfaces:**
- Consumes: `paigasus_proto::paigasus::common::v1::{ErrorReason, ErrorDomain}` from Task 2.
- Produces:
  - `ErrorReason::as_wire_reason(&self) -> Option<String>`
  - `ErrorReason::from_wire_reason(reason: &str) -> Option<ErrorReason>`
  - `ErrorDomain::as_wire_domain(&self) -> Option<String>`
  - `ErrorDomain::from_wire_domain(domain: &str) -> Option<ErrorDomain>`

  All four are inherent impls, so callers need no extra `use` beyond the enum itself. Task 4 and SMA-504 use `from_wire_reason` / `as_wire_reason`.

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/libs/paigasus-proto/src/error.rs` containing **only** the SPDX line, the module doc and this test module for now:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Wire-string helpers for the canonical error registry (ADR-0019, SMA-498).

#[cfg(test)]
mod tests {
    use crate::paigasus::common::v1::{ErrorDomain, ErrorReason};

    /// Every `ErrorReason` the registry declares. `::prost::Enumeration` provides
    /// `TryFrom<i32>`, so scanning enumerates the enum without a hand-maintained list.
    ///
    /// The bound must stay comfortably above the highest declared number (999, the top of
    /// the shared range) or a value added above it goes invisible to every test derived from
    /// `all_reasons`/`all_domains` — including the `assert_eq!(actual.len(), 43)` anchor and
    /// the range-enforcement test, both of which would then pass while covering nothing.
    fn all_reasons() -> Vec<ErrorReason> {
        (0..=9999).filter_map(|i| ErrorReason::try_from(i).ok()).collect()
    }

    fn all_domains() -> Vec<ErrorDomain> {
        (0..=9999).filter_map(|i| ErrorDomain::try_from(i).ok()).collect()
    }

    /// The registry, spelled out. This DELIBERATELY duplicates error.proto — in a test,
    /// which is the right place for a redundant assertion. Without it every other test
    /// here is self-consistent by construction and a typo such as
    /// ERROR_REASON_UPSTREAM_TIMOUT would ship green.
    const EXPECTED_REASONS: &[&str] = &[
        // IAM: tenancy
        "slug-conflict",
        "duplicate-membership",
        "email-conflict",
        "service-account-name-conflict",
        "invalid-email",
        "invalid-slug",
        "invalid-name",
        "invalid-prn",
        "prn-mismatch",
        "invalid-pagination",
        "nothing-to-rename",
        "not-found",
        "parent-archived",
        "node-archived",
        "missing-org-membership",
        "forbidden",
        "unknown-role",
        "invalid-scope",
        "system-immutable",
        "policy-invalid",
        "policy-conflict",
        "invalid-action",
        "invalid-bulk-replay",
        "not-system-owned",
        "fleet-not-converged",
        // IAM: authn
        "invalid-token",
        "identity-not-provisioned",
        "provisioning-failed",
        "principal-inactive",
        "authn-unavailable",
        // IAM: system-row retirement
        "grants-survive",
        "decision-change-unacknowledged",
        // Gateway
        "missing-authorization",
        "invalid-api-key",
        "insufficient-permissions",
        "missing-scope",
        "iam-unavailable",
        "upstream-unavailable",
        "upstream-timeout",
        "upstream-error",
        // Shared
        "internal",
        "invalid-request-body",
        "request-too-large",
    ];

    #[test]
    fn the_registry_contains_exactly_the_expected_reasons() {
        let actual: std::collections::BTreeSet<String> =
            all_reasons().iter().filter_map(|r| r.as_wire_reason()).collect();
        let expected: std::collections::BTreeSet<String> =
            EXPECTED_REASONS.iter().map(|s| (*s).to_string()).collect();

        let missing: Vec<_> = expected.difference(&actual).collect();
        let unexpected: Vec<_> = actual.difference(&expected).collect();
        assert!(missing.is_empty(), "declared in the test but not in the registry: {missing:?}");
        assert!(unexpected.is_empty(), "in the registry but not declared in the test: {unexpected:?}");
        assert_eq!(actual.len(), 43, "the registry should hold 43 reasons");
    }

    #[test]
    fn the_registry_contains_exactly_the_expected_domains() {
        let actual: Vec<String> = all_domains().iter().filter_map(|d| d.as_wire_domain()).collect();
        assert_eq!(actual, vec!["iam.paigasus.io".to_string(), "gateway.paigasus.io".to_string()]);
    }

    #[test]
    fn every_reason_round_trips() {
        for reason in all_reasons() {
            let Some(wire) = reason.as_wire_reason() else {
                continue; // the Unspecified sentinel, covered by its own test
            };
            assert_eq!(ErrorReason::from_wire_reason(&wire), Some(reason), "round-trip failed for {wire}");
        }
    }

    #[test]
    fn every_domain_round_trips() {
        for domain in all_domains() {
            let Some(wire) = domain.as_wire_domain() else {
                continue;
            };
            assert_eq!(ErrorDomain::from_wire_domain(&wire), Some(domain), "round-trip failed for {wire}");
        }
    }

    /// The zero sentinel exists only to satisfy buf's ENUM_ZERO_VALUE_SUFFIX lint rule and is
    /// never emitted. It is reachable because it is prost's `Default`, so both directions
    /// refuse it rather than silently inventing an "unspecified" code.
    #[test]
    fn the_unspecified_sentinel_is_not_a_code() {
        assert_eq!(ErrorReason::Unspecified.as_wire_reason(), None);
        assert_eq!(ErrorDomain::Unspecified.as_wire_domain(), None);
        assert_eq!(ErrorReason::from_wire_reason("unspecified"), None);
        assert_eq!(ErrorDomain::from_wire_domain("unspecified.paigasus.io"), None);
    }

    #[test]
    fn every_wire_reason_is_a_well_formed_token() {
        for reason in all_reasons() {
            let Some(wire) = reason.as_wire_reason() else { continue };
            assert!(!wire.is_empty(), "empty wire string");
            assert!(!wire.contains('_'), "{wire} contains an underscore");
            assert!(wire.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{wire} has a non-token character");
            assert!(!wire.starts_with('-') && !wire.ends_with('-'), "{wire} has a leading or trailing hyphen");
            assert!(!wire.contains("--"), "{wire} has a doubled hyphen");
        }
    }

    /// Strictness matters: a lenient parser would widen SMA-507's "emitted subset of registry"
    /// gate, letting a misspelled emitted code pass. The Unicode cases are the sharp ones —
    /// `str::to_uppercase` folds U+0131 to 'I' and U+017F to 'S', so without an ASCII-only
    /// positive check these reconstruct a valid proto name and resolve.
    #[test]
    fn from_wire_reason_rejects_malformed_input() {
        for bad in ["slug_conflict", "SLUG-CONFLICT", "Slug-Conflict", "", "-slug", "slug-", "slug--conflict", "no-such-code", "ınternal", "ſlug-conflict"] {
            assert_eq!(ErrorReason::from_wire_reason(bad), None, "{bad:?} must not resolve");
        }
    }

    #[test]
    fn from_wire_domain_requires_the_suffix() {
        assert_eq!(ErrorDomain::from_wire_domain("iam"), None);
        assert_eq!(ErrorDomain::from_wire_domain("iam.example.com"), None);
        assert_eq!(ErrorDomain::from_wire_domain("IAM.paigasus.io"), None);
        assert_eq!(ErrorDomain::from_wire_domain("iam.paigasus.io"), Some(ErrorDomain::Iam));
    }

    /// The numbering ranges are what lets SMA-507 decide which service may emit which code.
    #[test]
    fn every_reason_number_is_in_a_declared_range() {
        for reason in all_reasons() {
            let n = reason as i32;
            if n == 0 {
                continue; // the sentinel
            }
            assert!(
                (1..=299).contains(&n) || (300..=599).contains(&n) || (900..=999).contains(&n),
                "{reason:?} has number {n}, outside the IAM / gateway / shared ranges"
            );
        }
    }

    /// ADR-0019 quotes these spellings directly; they are the ones a reader will check first.
    #[test]
    fn the_adr_examples_are_spelled_as_documented() {
        assert_eq!(ErrorReason::SlugConflict.as_wire_reason().as_deref(), Some("slug-conflict"));
        assert_eq!(ErrorReason::ParentArchived.as_wire_reason().as_deref(), Some("parent-archived"));
        assert_eq!(ErrorReason::NothingToRename.as_wire_reason().as_deref(), Some("nothing-to-rename"));
        assert_eq!(ErrorDomain::Iam.as_wire_domain().as_deref(), Some("iam.paigasus.io"));
    }
}
```

Register the module by appending to `rs/crates/libs/paigasus-proto/src/lib.rs`:

```rust
/// Wire-string helpers for the canonical error registry (`common::v1::ErrorReason`/`ErrorDomain`).
pub mod error;
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto
```

Expected: **compile failure**, `no method named as_wire_reason found for enum ErrorReason`. That is the correct failure — the helpers do not exist yet.

- [ ] **Step 3: Write the implementation**

Insert this **above** the `#[cfg(test)] mod tests` block in `rs/crates/libs/paigasus-proto/src/error.rs`, directly after the module doc comment. Extend the module doc to the full text shown here:

```rust
//! Wire-string helpers for the canonical error registry (ADR-0019, SMA-498).
//!
//! [`ErrorReason`] and [`ErrorDomain`] are REGISTRIES, never wire types: the wire carries
//! `google.rpc.ErrorInfo.reason` / `.domain` as strings. These helpers convert between the
//! generated enum and that string, deriving BOTH directions from prost's `as_str_name` /
//! `from_str_name` rather than a match table, so the kebab spellings exist in exactly one
//! place — `contracts/proto/paigasus/common/v1/error.proto`. A table here would be a second
//! copy of the registry, which is the "three unlinked places" drift ADR-0019 cites.

use crate::paigasus::common::v1::{ErrorDomain, ErrorReason};

/// The suffix every canonical error domain carries.
const DOMAIN_SUFFIX: &str = ".paigasus.io";

/// The proto-name prefix buf's `ENUM_VALUE_PREFIX` lint rule requires on every reason.
const REASON_PREFIX: &str = "ERROR_REASON_";

/// The proto-name prefix buf's `ENUM_VALUE_PREFIX` lint rule requires on every domain.
const DOMAIN_PREFIX: &str = "ERROR_DOMAIN_";

/// The bare proto-name suffix of the zero sentinel, which is never a code.
const UNSPECIFIED: &str = "UNSPECIFIED";

/// Does `s` match `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`?
///
/// Hand-written rather than pulled from a regex crate to keep `paigasus-proto` free of
/// non-codegen dependencies. Validation is POSITIVE (an allow-list of shapes) rather than a
/// deny-list of characters, and strictly ASCII: `str::to_uppercase` folds `ı` (U+0131) to `I`
/// and `ſ` (U+017F) to `S`, so a deny-list would let those reconstruct a valid proto name and
/// resolve to a real code.
fn is_wire_token(s: &str) -> bool {
    if !s.starts_with(|c: char| c.is_ascii_lowercase()) || s.ends_with('-') {
        return false;
    }
    let mut prev_hyphen = false;
    for c in s.chars() {
        match c {
            'a'..='z' | '0'..='9' => prev_hyphen = false,
            '-' if !prev_hyphen => prev_hyphen = true,
            _ => return false,
        }
    }
    true
}

impl ErrorReason {
    /// The canonical wire `reason` for this value, e.g. `"slug-conflict"`.
    ///
    /// `None` for [`ErrorReason::Unspecified`]: that sentinel exists only to satisfy buf's
    /// `ENUM_ZERO_VALUE_SUFFIX` lint rule and is emitted by no surface. It is reachable because
    /// it is prost's `Default`, so returning `None` makes emitting it a caller-visible decision
    /// rather than a silent one.
    ///
    /// Returns an owned `String` deliberately: a borrowed `&'static str` would need a const
    /// table, i.e. the second copy of the registry this module exists to avoid. The allocation
    /// happens once per error response, and `tonic_types::ErrorDetails::with_error_info` takes
    /// `impl Into<String>` anyway.
    pub fn as_wire_reason(&self) -> Option<String> {
        let name = self.as_str_name().strip_prefix(REASON_PREFIX)?;
        if name == UNSPECIFIED {
            return None;
        }
        Some(name.to_ascii_lowercase().replace('_', "-"))
    }

    /// Parses a canonical wire `reason` back into a registry value; the exact inverse of
    /// [`ErrorReason::as_wire_reason`].
    ///
    /// Validates BEFORE transforming (see [`is_wire_token`]), so `"slug_conflict"`,
    /// `"SLUG-CONFLICT"` and Unicode look-alikes are rejected rather than folded into a valid
    /// name. A lenient parser would widen SMA-507's "emitted ⊆ registry" gate, which is the one
    /// thing that gate exists to prevent.
    pub fn from_wire_reason(reason: &str) -> Option<Self> {
        if !is_wire_token(reason) {
            return None;
        }
        let name = format!("{REASON_PREFIX}{}", reason.to_ascii_uppercase().replace('-', "_"));
        match Self::from_str_name(&name)? {
            Self::Unspecified => None,
            value => Some(value),
        }
    }
}

impl ErrorDomain {
    /// The canonical wire `domain` for this value, e.g. `"iam.paigasus.io"`. `None` for the
    /// zero sentinel, for the same reason as [`ErrorReason::as_wire_reason`].
    pub fn as_wire_domain(&self) -> Option<String> {
        let name = self.as_str_name().strip_prefix(DOMAIN_PREFIX)?;
        if name == UNSPECIFIED {
            return None;
        }
        Some(format!("{}{DOMAIN_SUFFIX}", name.to_ascii_lowercase().replace('_', "-")))
    }

    /// Parses a canonical wire `domain`; the exact inverse of [`ErrorDomain::as_wire_domain`].
    /// The `.paigasus.io` suffix is required, and the label is validated with the same positive
    /// ASCII check the reason parser uses.
    pub fn from_wire_domain(domain: &str) -> Option<Self> {
        let label = domain.strip_suffix(DOMAIN_SUFFIX)?;
        if !is_wire_token(label) {
            return None;
        }
        let name = format!("{DOMAIN_PREFIX}{}", label.to_ascii_uppercase().replace('-', "_"));
        match Self::from_str_name(&name)? {
            Self::Unspecified => None,
            value => Some(value),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto
```

Expected: PASS, 10 new tests plus the 3 pre-existing ones. If `the_registry_contains_exactly_the_expected_reasons` fails, the proto and the test list disagree — read the `missing:`/`unexpected:` lists and fix whichever is wrong.

- [ ] **Step 5: Check formatting and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-proto --all-targets -- -D warnings
```

Expected: both exit 0. If `cargo fmt --check` reports a diff, run `cargo fmt` and re-run.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-proto/src/error.rs rs/crates/libs/paigasus-proto/src/lib.rs
git commit -F - <<'EOF'
feat(rs): derive canonical error wire strings from the registry (SMA-498)

No language's codegen produces the kebab wire string from an enum value: prost
gives back the full proto name, protobuf-es and betterproto2 give nothing. So
the ERROR_REASON_X_Y to x-y transform has to live somewhere.

It is derived from as_str_name/from_str_name rather than tabulated, so the
spellings exist only in error.proto. A match table would be a second copy of
the registry inside Rust, which is the drift ADR-0019 warns about.

Parsing validates against a positive ASCII token shape before transforming.
A deny-list would not do: to_uppercase folds U+0131 to I and U+017F to S, so
"ınternal" would otherwise reconstruct ERROR_REASON_INTERNAL and resolve.
EOF
```

---

### Task 4: Prove the registry covers every tenancy code (AC2)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (append to its existing `#[cfg(test)] mod tests`)

This lives in `convert.rs`, **not** `application/error.rs`: the application layer imports only `paigasus_iam_core` and must stay transport-agnostic per the repo's hexagonal-architecture rule, while `convert.rs` already imports `paigasus_proto`.

**Interfaces:**
- Consumes: `ErrorReason::from_wire_reason` (Task 3), `TenancyError::code()` (existing).
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Write the failing test**

**This is what actually shipped, after two disproved attempts — do not build either of those
attempts, even though a draft of this plan once presented one as the design.**

**Disproved constructions (recorded so nobody copies one into SMA-507's drift gate):**

1. A hand-listed array of variant instances guarded by a wildcard-free `match`
   (`assert_variant_is_known`) intended to fail *compilation* when a new variant is added. This
   does not close the gap it targets: the exhaustive match forces a developer to write a match
   *arm* for the new variant, but nothing forces them to also add an *instance* of it to the
   array being iterated. Adding a variant and making the minimal edit that clears the resulting
   `E0004` leaves the test compiling, passing, and blind to the new variant's unregistered code
   — the exact gap the test exists to close.
2. A second attempt mapped each variant to a unique numeric "slot" and asserted the slots formed
   a bijection. Disproved for the same underlying reason: the slot function is only ever
   evaluated for members of the hand-written list, so an unlisted variant is invisible to it too.

**What shipped instead:** enumerate variants from the type itself via `strum::EnumIter`, gated
`#[cfg(test)]` so `strum_macros` never enters the shipped binary. This costs a dev-only
dependency rather than being dependency-free (see Task 5 Step 3 and "Out of scope" below, both
corrected to reflect this).

In `rs/crates/services/paigasus-iam/src/application/error.rs`, add the derive to `TenancyError`:

```rust
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum TenancyError {
    // ...unchanged variants
}
```

In `rs/crates/services/paigasus-iam/Cargo.toml`, under `[dev-dependencies]`:

```toml
# `adapters::grpc::convert`'s AC2 registry-coverage test enumerates every TenancyError
# variant, which safe Rust cannot do without a derive. Dev-only: the derive is gated behind
# cfg(test) so strum_macros never enters the shipped binary (SMA-498).
strum = { version = "0.26", features = ["derive"] }
```

Append to the existing `#[cfg(test)] mod tests` block at the bottom of
`rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs`:

```rust
/// AC2: every code `TenancyError::code()` value appears unchanged, kebab retained deliberately
///
/// Coverage is checked by enumerating every `TenancyError` variant via `strum::EnumIter`
/// (`#[cfg_attr(test, derive(strum::EnumIter))]` on the enum) rather than a hand-maintained
/// list: `TenancyError::iter()` yields one instance per variant straight from the type
/// itself, so a new variant is included automatically — there is no second list that can be
/// left un-extended. Assertions run through `code()` rather than string literals, so an
/// unregistered rename fails too.
#[test]
fn every_tenancy_code_is_declared_in_the_canonical_registry() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    use strum::IntoEnumIterator;

    for err in TenancyError::iter() {
        let code = err.code();
        assert!(
            ErrorReason::from_wire_reason(code).is_some(),
            "TenancyError::{err:?} emits {code:?}, which is not declared in common/v1/error.proto"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Temporarily prove the assertion bites by breaking one registry entry: in `contracts/proto/paigasus/common/v1/error.proto`, rename `ERROR_REASON_SLUG_CONFLICT` to `ERROR_REASON_SLUG_CONFLICTX`, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run contracts:generate
cd rs && cargo nextest run -p paigasus-iam every_tenancy_code_is_declared
```

Expected: FAIL with `TenancyError::SlugConflict emits "slug-conflict", which is not declared in common/v1/error.proto`.

Note: this deliberate break also fails Task 3's `the_registry_contains_exactly_the_expected_reasons`
(it will report `slug-conflict` missing and `slug-conflictx` unexpected). That is correct and
expected — which is why this step runs only the one IAM test by name. Both go green again after
Step 3.

- [ ] **Step 3: Restore the registry**

Change `ERROR_REASON_SLUG_CONFLICTX` back to `ERROR_REASON_SLUG_CONFLICT` and regenerate:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run contracts:generate
git status --short
```

Expected: `git status --short` shows only `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` modified. If a generated file still shows as modified, the restore was incomplete — fix it before continuing.

**Note:** `mv`-style restores roll a file's mtime *backwards*, which makes cargo reuse the binary built from the broken edit. Always restore by editing the file (as above), never by moving a backup over it.

- [ ] **Step 4: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam every_tenancy_code_is_declared
```

Expected: PASS, 1 test run.

- [ ] **Step 5: Run the whole IAM suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam -p paigasus-proto --retries 2
```

Expected: PASS. `CI=1` is required — the Docker-gated container suites otherwise `return` early and report PASS in under a second having run nothing. They are also genuinely flaky under parallel load, hence `--retries 2`. If a container test fails, re-run on an unmodified `origin/main` before assuming this change caused it.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -F - <<'EOF'
test(rs): assert every tenancy code is in the canonical registry (SMA-498)

Turns AC2 from something verified by reading the diff into something CI checks.

The wildcard-free match makes adding a TenancyError variant a COMPILE error
here until the new code is registered, so the usual weakness of a hand-written
list does not apply. Assertions run through code() rather than string literals,
so an unregistered rename fails too.

Lives in the gRPC adapter, not application/error.rs: the application layer
imports only paigasus-iam-core and stays transport-agnostic.
EOF
```

---

### Task 5: Full-graph verification

No new code. This task exists because the per-project Moon tasks do **not** run the repo-level gates, and the codegen-drift gate is not a Moon task at all — so nothing so far has actually checked AC4.

**Files:** none modified (unless a gate fails).

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: evidence that the branch is CI-green.

- [ ] **Step 1: Verify the codegen-drift gate (AC4)**

This reproduces `.github/workflows/ci.yml`'s "Codegen drift gate" step, which is a workflow step rather than a Moon task and is therefore absent from the `moon ci` target list:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run contracts:generate
git diff --exit-code -- \
  rs/crates/libs/paigasus-proto/src/generated \
  py/packages/paigasus-proto/src/paigasus_proto/generated \
  ts/packages/paigasus-proto/src/generated
```

Expected: exit 0 with no diff. A non-empty diff means committed generated code is stale — commit the regenerated output and re-run.

- [ ] **Step 2: Run the full CI graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: exit 0.

- [ ] **Step 3: Diagnose any failure**

Moon reports an unattributed "N failed" without naming the task. To find out which:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

Expected outcomes for the gates most likely to react to this change:
- `contracts:fmt` — you skipped `buf format -w`. Run it, regenerate, commit.
- `contracts:breaking` — should pass; Task 1 only widened what is tolerated.
- `repo:affected-smoke` — should pass. Its `contracts->proto` case probes a hardcoded synthetic path (`contracts/proto/paigasus/gateway/v1/health.proto`), so adding a file does not change its strict-equality set.
- `repo:deny` / `repo:machete` — should pass; `strum`/`strum_macros` landed as a `cfg(test)`-gated
  dev-dependency (Task 4 Step 1), MIT-licensed and already covered by `rs/deny.toml`'s allow list,
  so neither gate needs a waiver.

- [ ] **Step 4: Confirm the working tree is clean**

```bash
git status --short
```

Expected: no output. Any leftover probe file (`zz_probe.proto`, `zz_probe.rs`) or un-reverted experiment is a bug — remove it and re-run Step 1.

---

## Definition of done

| AC | Verified by |
| --- | --- |
| 1. `error.proto` generates cleanly to Rust, Python and TypeScript | Task 2 Steps 3–5 |
| 2. Every `TenancyError::code()` value appears unchanged, kebab retained | Task 4 Steps 2–4 |
| 3. Doc comment states append-only + removal is breaking | Task 2 Step 1 (file-level comment) |
| 4. Committed generated output in sync | Task 5 Step 1 |

## Out of scope — do not implement

- Emitting `google.rpc.ErrorInfo`, removing the in-band `"{code}: {message}"` prefix, the 17 renames in spec D7, `retryable`/`correlation_id` — **SMA-504**.
- The two-way drift gate — **SMA-507**.
- TypeScript/Python `as_wire_reason` equivalents and the `@paigasus/proto` barrel export — the SDK issue.
- Adding `tonic-types` to the workspace — SMA-504 needs it. (This branch does add one dependency of
  its own: `strum`, `cfg(test)`-gated, dev-only — see Task 4 Step 1. `tonic-types` would be a
  *production* dependency, a different story.)
