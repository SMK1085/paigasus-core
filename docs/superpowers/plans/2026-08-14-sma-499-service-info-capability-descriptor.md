# ServiceInfo Capability Descriptor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Define a `ServiceInfo` capability descriptor in `paigasus.common.v1` — service name, version, and append-only capability keys — generating to Rust, Python and TypeScript, and supersede the `iam.v1.ServiceInfo` placeholder.

**Architecture:** One new proto file declares the descriptor message, a `Capability` **registry enum** (never a wire type — the wire carries `repeated string`), request/response wrappers, and a shared `ServiceInfoService`. Hand-written transform modules in Rust and TypeScript **derive** the wire key from the generated enum by a mapping rule rather than tabulating it, so there is no second copy of the registry to drift. Nothing serves the descriptor — that is SMA-505.

**Tech Stack:** protobuf 3 + buf (lint/format/breaking/generate), prost 0.14 + tonic 0.14 (Rust), protobuf-es v2 (TypeScript), betterproto2 (Python), Moon task runner, vitest, pytest, cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md`

## Global Constraints

- **Worktree:** all work happens in `/Users/smaschek/dev/paigasus/paigasus-core-sma499` on branch `feature/sma-499-contracts-serviceinfo-capability-descriptor`. Never work in `/Users/smaschek/dev/paigasus/paigasus-core`.
- **PATH:** every shell command must start from a shell that has run `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Without it `moon`, `buf`, `uv` and `nextest` resolve to the wrong binaries or not at all.
- **SPDX:** every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). No exceptions.
- **`buf format -w` before every commit that touches a `.proto`.** Skipping it reds `contracts:fmt` in `moon ci` **silently**.
- **Never hand-edit generated code.** Regenerate with `moon run contracts:generate` and commit the result.
- **Rust:** edition 2024, rust-version 1.95. `[workspace.lints.rust] warnings = "deny"` — an unused item is a hard **compile error**, not a warning, so never add a module without wiring it up in the same task.
- **No new dependency in any workspace.** In particular the key-grammar check is hand-rolled; do **not** add the `regex` crate.
- **Commit messages:** conventional commits with a workspace scope (`feat(contracts):`, `feat(rs):`). Subject must **start lowercase** and be **≤100 chars**. Never put `#NNN` in the body — write "SMA-499" instead — and keep the footer contiguous, or commitlint reds CI.
- **Do not use `git commit --no-verify`.** The commit-msg hook is the local half of a CI gate.
- **Capability keys are append-only and permanent.** Do not add, rename or reorder a key that this plan does not specify.

## Merge-order constraint (read before starting)

The spec's R1: **SMA-499 must not merge before SMA-498.** Until SMA-498's `contracts/buf.yaml` change is on `main`, `ENUM_VALUE_NO_DELETE` is active and every `Capability` spelling committed here is permanent. This plan therefore makes **no `buf.yaml` change**. If asked to invert the merge order, stop and escalate — the spec must be revised first.

## File Structure

| File | Responsibility |
| --- | --- |
| `contracts/proto/paigasus/common/v1/service_info.proto` | The whole contract: descriptor message, `Capability` registry, RPC, and every normative rule as doc comments |
| `contracts/proto/paigasus/iam/v1/iam.proto` | Existing — the placeholder message gains a supersession comment and `option deprecated` |
| `rs/crates/libs/paigasus-proto/src/capability.rs` | Rust wire-key transform + its tests. Derived from prost's `as_str_name()`, never tabulated |
| `rs/crates/libs/paigasus-proto/src/lib.rs` | Existing — adds the `common/v1` tonic include and `pub mod capability;` |
| `ts/packages/paigasus-proto/src/capability.ts` | TypeScript wire-key transform (forward direction only) |
| `ts/packages/paigasus-proto/src/capability.test.ts` | Its tests, plus a service-descriptor assertion |
| `ts/packages/paigasus-proto/src/index.ts` | Existing — barrel re-exports, split by type vs value |
| `py/packages/paigasus-proto/tests/test_service_info_smoke.py` | Proves the descriptor and registry generate usably in Python |
| Three generated trees | Regenerated; committed; never hand-edited |

---

### Task 1: The proto contract and its bindings

Delivers AC1: the descriptor is defined in `common/v1` and generates to all three languages. Also delivers AC2's and AC3's in-proto documentation.

**Files:**
- Create: `contracts/proto/paigasus/common/v1/service_info.proto`
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs` (the `common::v1` module block)
- Generated (do not hand-edit): `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`, `…/paigasus.common.v1.tonic.rs` (new), `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts` (new), `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py`

**Interfaces:**
- Consumes: nothing.
- Produces, for Tasks 3–5:
  - Rust: `crate::paigasus::common::v1::{ServiceInfo, Capability, GetServiceInfoRequest, GetServiceInfoResponse}`. `Capability` variants are `Unspecified`, `IamAuthzCedar`, `IamApikeys`, `IamAudit`, `GatewayChatStream`, with `as_str_name() -> &'static str` returning the full proto name (e.g. `"CAPABILITY_IAM_AUDIT"`) and `from_str_name(&str) -> Option<Self>`.
  - TypeScript: `Capability.{UNSPECIFIED,IAM_AUTHZ_CEDAR,IAM_APIKEYS,IAM_AUDIT,GATEWAY_CHAT_STREAM}` (**the `CAPABILITY_` prefix is stripped by protobuf-es**), plus `ServiceInfoSchema`, `CapabilitySchema`, `ServiceInfoService`, and the type `ServiceInfo`.
  - Python: `Capability.{UNSPECIFIED,IAM_AUTHZ_CEDAR,IAM_APIKEYS,IAM_AUDIT,GATEWAY_CHAT_STREAM}` and `ServiceInfo`, from `paigasus_proto.generated.paigasus.common.v1`.

- [ ] **Step 1: Create the proto file**

Create `contracts/proto/paigasus/common/v1/service_info.proto` with exactly this content. The long comment block after `package` is a **detached file-level comment** — the blank line before `message ServiceInfo` is what detaches it. Keep it.

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.common.v1;

// Service capability discovery (ADR-0020).
//
// Every Paigasus service describes itself with `ServiceInfo`: which service it
// is, which build is running, and which features that build actually has
// ENABLED. A deployment may run IAM alone, IAM plus the gateway, or more
// services later, and self-hosters upgrade components independently — so a
// client that hard-codes which features a service has is wrong after any
// partial upgrade.
//
// ── TRANSPORT ────────────────────────────────────────────────────────────────
//
// Services expose the descriptor over `ServiceInfoService.GetServiceInfo`
// below, or — for a service with no gRPC server — over the equivalent HTTP
// route. A service MUST offer at least one, and SHOULD offer the one its
// clients already speak. Today IAM serves gRPC; the gateway serves HTTP.
//
//     GET /v1/service-info
//     200, Content-Type: application/json
//     body: canonical protojson of ServiceInfo — the BARE message, not
//           GetServiceInfoResponse, which exists only to satisfy buf lint.
//
// The HTTP encoder MUST emit default values, so `capabilities` is always
// present, as `[]` when empty. Canonical protojson omits default-valued fields,
// which would otherwise drop the list entirely and turn "no features enabled"
// into a client-side type error rather than "every feature off". Clients MUST
// nonetheless treat an absent `capabilities` as an empty list.
//
// The route MUST require authentication equivalent to any other authenticated
// route of that service. Capability discovery introduces NO new unauthenticated
// surface (ADR-0020 D4). Errors keep the service's existing error envelope and
// status codes — envelopes are per-surface (ADR-0019).
//
// ── CAPABILITY KEYS ──────────────────────────────────────────────────────────
//
// `capabilities` carries wire STRINGS, never enum numbers. The `Capability`
// enum below is the REGISTRY for those strings and is never used as a field
// type. The wire key is the enum value name with `CAPABILITY_` stripped,
// lowercased, and `_` replaced by `.`:
//
//     CAPABILITY_IAM_AUTHZ_CEDAR  ->  "iam.authz.cedar"
//
// Because that rule consumes every underscore, a key is always lowercase
// alphanumeric segments separated by dots — `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`.
// A key such as "gateway.chat-stream" is unreachable through the rule and MUST
// NOT be proposed.
//
// The registry is APPEND-ONLY. Removing a value is a breaking change.
// `buf breaking` catching a deleted enum value is a bonus, NOT the guard: it
// cannot see the wire strings, cannot tell whether a key is still advertised by
// a service or consumed by a client, and never runs against Rust or TypeScript.
// Review discipline is the guard.
//
// The vocabulary is CLOSED and central: every key a Paigasus service advertises
// is registered here. There is no reserved vendor prefix — a fork advertising
// "acme.custom.thing" is simply an unknown key, which every conforming client
// ignores.
//
// ── CLIENT CONTRACT ──────────────────────────────────────────────────────────
//
//   * An UNKNOWN key is IGNORED.
//   * An ABSENT capability means the feature is OFF.
//   * The list is unordered and duplicates MUST be ignored. Build a set from
//     it; do not index it.
//   * Capability gating is COSMETIC, exactly like the console's can() helper.
//     The server remains authoritative and must handle an unimplemented call
//     gracefully. A client treating this list as a security boundary is wrong.
//   * "Degraded" is NOT expressible here and MUST NOT be inferred from the
//     payload, which distinguishes only present from absent. ADR-0020's third
//     UI state is derived client-side, from a failed, timed-out or stale-cache
//     GetServiceInfo.

message ServiceInfo {
  // Which service this is: a bare slug matching the prefix of its own
  // capability keys — "iam", "gateway".
  //
  // ADVISORY, and never a cache key. ADR-0020 caches discovery under
  // `svcinfo:<service>`; that `<service>` MUST be the client's own deployment
  // configuration identifier for the service it dialled, never this
  // server-reported value — otherwise a misconfigured or hostile service could
  // poison another service's cache entry. A mismatch is worth logging and
  // nothing more.
  string service = 1;

  // The running build, as SemVer 2.0. MAY carry pre-release and build metadata
  // ("1.4.0-rc1+abc123").
  //
  // For display and for ADR-0020's N-1-minor skew reporting ONLY. NEVER an
  // input to a feature decision — `capabilities` is the only sanctioned input.
  // Gating on a version range reintroduces exactly the version-skew bug this
  // descriptor exists to prevent. Clients MUST tolerate an unparseable value by
  // suppressing skew reporting rather than erroring.
  string version = 2;

  // The capability keys this build actually has ENABLED — not what the binary
  // could do if it were configured differently.
  repeated string capabilities = 3;
}

// Registry of capability keys. NEVER used as a field type: `ServiceInfo.capabilities`
// carries the wire strings this enum documents. See the file comment for the
// mapping rule, the append-only rule, and the client contract.
enum Capability {
  // Not a capability. Exists only to satisfy buf's ENUM_ZERO_VALUE_SUFFIX rule.
  // No service advertises it and no client parses it.
  CAPABILITY_UNSPECIFIED = 0;

  // "iam.authz.cedar" — Cedar policy evaluation is enabled: authorization
  // decisions, policy administration and role grants are available.
  CAPABILITY_IAM_AUTHZ_CEDAR = 1;

  // "iam.apikeys" — service-account API key issuance and introspection are
  // available.
  CAPABILITY_IAM_APIKEYS = 2;

  // "iam.audit" — the append-only audit log is queryable.
  CAPABILITY_IAM_AUDIT = 3;

  // "gateway.chat.stream" — chat completions can be streamed.
  CAPABILITY_GATEWAY_CHAT_STREAM = 4;
}

message GetServiceInfoRequest {}

message GetServiceInfoResponse {
  ServiceInfo service_info = 1;
}

// Implemented by every Paigasus service. See the file comment for the HTTP
// route a service with no gRPC server serves instead.
service ServiceInfoService {
  rpc GetServiceInfo(GetServiceInfoRequest) returns (GetServiceInfoResponse);
}
```

- [ ] **Step 2: Format, lint and breaking-check the proto**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/contracts
buf format -w
buf lint
buf breaking --against '../.git#branch=main,subdir=contracts'
```

Expected: all three produce **no output**. `buf format -w` may rewrite the file; that is fine and must be committed. If `buf lint` complains about request/response naming, you renamed something — the wrappers must be exactly `GetServiceInfoRequest` / `GetServiceInfoResponse`.

- [ ] **Step 3: Generate the bindings**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run contracts:generate
```

Expected: succeeds. A warning `duplicate generated file name "paigasus/common/v1/__init__.py"` is **pre-existing** and harmless — it appears on `main` too. Ignore it.

Confirm the three new files exist:

```bash
ls rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.tonic.rs \
   ts/packages/paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts
git status --short
```

Expected: the `.tonic.rs` and `service_info_pb.ts` show as untracked (`??`); the Rust, TS and Python `common/v1` files show as modified.

- [ ] **Step 4: Wire the new tonic file into the crate**

`paigasus.common.v1.tonic.rs` is generated but not compiled until `lib.rs` includes it. In `rs/crates/libs/paigasus-proto/src/lib.rs`, replace exactly:

```rust
            // Only the prost file: audit.proto declares no service, so
            // neoeinstein-tonic emits no `.tonic.rs` for this package.
            include!("generated/paigasus/common/v1/paigasus.common.v1.rs");
```

with:

```rust
            // service_info.proto declares ServiceInfoService, so neoeinstein-tonic
            // now emits a `.tonic.rs` for this package alongside the prost file
            // (SMA-499). Before that, audit.proto declared no service and this
            // package was prost-only.
            include!("generated/paigasus/common/v1/paigasus.common.v1.rs");
            include!("generated/paigasus/common/v1/paigasus.common.v1.tonic.rs");
```

- [ ] **Step 5: Verify the crate compiles**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/rs
cargo build -p paigasus-proto
```

Expected: `Finished` with no errors. (This was verified during planning — if it fails, the include path or the module block is wrong, not the generated code.)

- [ ] **Step 6: Verify the codegen-drift gate is satisfied**

Reproduce the CI step exactly. A plain `git diff` is **not** a substitute: two of the generated files are brand new and untracked, and `git diff` reports them clean.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run contracts:generate
git add --intent-to-add -- \
    rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
git diff --exit-code -- \
    rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
echo "drift gate exit: $?"
```

Expected: `drift gate exit: 0`. A non-zero exit means generation is not idempotent — stop and investigate; do not hand-edit the output.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git add contracts/proto/paigasus/common/v1/service_info.proto \
        rs/crates/libs/paigasus-proto/src/lib.rs \
        rs/crates/libs/paigasus-proto/src/generated \
        ts/packages/paigasus-proto/src/generated \
        py/packages/paigasus-proto/src/paigasus_proto/generated
git commit -m "feat(contracts): define the ServiceInfo capability descriptor in common/v1 (SMA-499)"
```

---

### Task 2: Supersede the `iam.v1.ServiceInfo` placeholder

Delivers AC4. Kept separate from Task 1 because a reviewer could reasonably accept the new descriptor and reject this reconciliation.

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:18-22`
- Generated: the `iam/v1` files in all three trees (descriptor bytes, plus TS JSDoc and a Python `DeprecationWarning`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: nothing. No later task depends on this.

**Background — why the message is not deleted.** `contracts/buf.yaml` uses the `FILE` breaking category, which includes `MESSAGE_NO_DELETE`. Deleting the message reds `contracts:breaking` with `Previously present message "ServiceInfo" was deleted from file.` Superseding in place is the whole point of this task.

- [ ] **Step 1: Rewrite the placeholder's doc comment and deprecate it**

In `contracts/proto/paigasus/iam/v1/iam.proto`, replace exactly:

```proto
// Placeholder so the package generates a concrete type in all three languages.
// Replaced by real messages in M1; carries a service PRN string for now.
message ServiceInfo {
  string prn = 1;
}
```

with:

```proto
// DEPRECATED — superseded by `paigasus.common.v1.ServiceInfo`, the real
// capability descriptor (ADR-0020, SMA-499). Do not use: nothing serves this
// message, and no RPC accepts or returns it.
//
// It is retained, rather than removed, only because buf's MESSAGE_NO_DELETE
// forbids deleting a published message — removal would red `contracts:breaking`.
// This deprecation is PERMANENT; there is no follow-up that retires it.
message ServiceInfo {
  option deprecated = true;

  string prn = 1;
}
```

- [ ] **Step 2: Format, lint and breaking-check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/contracts
buf format -w
buf lint
buf breaking --against '../.git#branch=main,subdir=contracts'
```

Expected: no output from any of them. `option deprecated = true` is **not** a breaking change — this was verified during planning.

- [ ] **Step 3: Regenerate and inspect what each language did**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run contracts:generate
grep -rn -i "deprecated" \
  ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts \
  py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/iam/v1/__init__.py \
  | head
```

Expected, and this asymmetry is intentional:
- **TypeScript:** two `@deprecated` JSDoc tags, on `ServiceInfo` and `ServiceInfoSchema`.
- **Python:** `warnings.warn("ServiceInfo is deprecated", DeprecationWarning)` inside `__post_init__`. Inert — no Python code constructs the type and the workspace sets no `filterwarnings = error`.
- **Rust:** **nothing.** prost emits no `#[deprecated]`; the only Rust change is `FILE_DESCRIPTOR_SET` bytes. That is what makes AC4's "without breaking the generated Rust" hold.

- [ ] **Step 4: Verify Rust still compiles**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/rs && cargo build -p paigasus-proto
```

Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git add contracts/proto/paigasus/iam/v1/iam.proto \
        rs/crates/libs/paigasus-proto/src/generated \
        ts/packages/paigasus-proto/src/generated \
        py/packages/paigasus-proto/src/paigasus_proto/generated
git commit -m "feat(contracts): supersede and deprecate the iam.v1.ServiceInfo placeholder (SMA-499)"
```

- [ ] **Step 6: Verify the codegen-drift gate — AFTER committing, not before**

The gate compares regenerated output against **committed** state. Run it on a clean tree; running it with uncommitted generated changes present compares against a stale index and reports a failure that is an artefact of the ordering, not real drift.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git status --short   # must be empty before proceeding
moon run contracts:generate
git add --intent-to-add -- rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
git diff --exit-code -- rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
echo "drift gate exit: $?"
```

Expected: `drift gate exit: 0`. Non-zero here, on a clean tree, is a genuine drift bug — stop and investigate rather than hand-editing the output.

---

### Task 3: Rust wire-key transform

Delivers the Rust half of AC2 — the four spellings become a fact CI checks.

**Files:**
- Create: `rs/crates/libs/paigasus-proto/src/capability.rs`
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs` (append one module declaration)

**Interfaces:**
- Consumes, from Task 1: `crate::paigasus::common::v1::Capability`, with variants `Unspecified`, `IamAuthzCedar`, `IamApikeys`, `IamAudit`, `GatewayChatStream`; `as_str_name(&self) -> &'static str` returning e.g. `"CAPABILITY_IAM_AUDIT"`; `from_str_name(&str) -> Option<Self>`. The derive `::prost::Enumeration` also provides `Default` (yielding `Unspecified`) and `TryFrom<i32>` (erroring `UnknownEnumValue`) — both verified during planning.
- Produces, for SMA-505: `Capability::as_wire_key(self) -> Option<String>` and `Capability::from_wire_key(&str) -> Option<Capability>`.

**Why derived, not tabulated:** a four-arm `match` would be a second copy of the registry inside Rust with nothing keeping it in step with the proto. Do not write one, even though it would be shorter.

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/libs/paigasus-proto/src/capability.rs` containing **only** the header and the test module for now, so the tests genuinely fail to compile against absent functions:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Wire-key transform for the `Capability` registry in
//! `paigasus/common/v1/service_info.proto`.

use crate::paigasus::common::v1::Capability;

#[cfg(test)]
mod tests {
    use super::Capability;

    /// Every registered capability. Deliberately explicit: prost generates no
    /// variant iterator. `adding_a_capability_forces_updating_these_tests`
    /// below is what stops this list going stale.
    const ALL: [Capability; 4] = [
        Capability::IamAuthzCedar,
        Capability::IamApikeys,
        Capability::IamAudit,
        Capability::GatewayChatStream,
    ];

    #[test]
    fn every_capability_round_trips() {
        for cap in ALL {
            let key = cap.as_wire_key().expect("a registered capability has a wire key");
            assert_eq!(Capability::from_wire_key(&key), Some(cap), "round-trip failed for {key}");
        }
    }

    #[test]
    fn the_registry_spells_the_adr_keys_exactly() {
        assert_eq!(Capability::IamAuthzCedar.as_wire_key().unwrap(), "iam.authz.cedar");
        assert_eq!(Capability::IamApikeys.as_wire_key().unwrap(), "iam.apikeys");
        assert_eq!(Capability::IamAudit.as_wire_key().unwrap(), "iam.audit");
        assert_eq!(Capability::GatewayChatStream.as_wire_key().unwrap(), "gateway.chat.stream");
    }

    #[test]
    fn the_zero_sentinel_has_no_wire_key_in_either_direction() {
        assert_eq!(Capability::Unspecified.as_wire_key(), None);
        assert_eq!(Capability::from_wire_key("unspecified"), None);
        // Unspecified is prost's Default, so this is the realistic hazard: a
        // default-initialised descriptor must not advertise "unspecified".
        assert_eq!(Capability::default().as_wire_key(), None);
    }

    #[test]
    fn wire_keys_match_the_documented_grammar() {
        for cap in ALL {
            let key = cap.as_wire_key().unwrap();
            assert!(key.starts_with(|c: char| c.is_ascii_lowercase()), "{key} must start with a letter");
            for segment in key.split('.') {
                assert!(!segment.is_empty(), "{key} has an empty segment");
                assert!(
                    segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                    "segment {segment} of {key} is not [a-z0-9]",
                );
            }
        }
    }

    #[test]
    fn from_wire_key_rejects_malformed_input() {
        for bad in [
            "iam_audit",   // wrong separator: uppercases into a real proto name
            "IAM.AUDIT",   // wrong casing
            "Iam.Audit",
            "unspecified", // the zero sentinel is not a key
            "",
            ".iam.audit",  // leading dot
            "iam.audit.",  // trailing dot
            "iam..audit",  // empty segment
            "iam.unknown", // well-formed but unregistered
            "ıam.audit",   // U+0131 dotless i: to_uppercase folds it to 'I'
            "ſervice.x",   // U+017F long s: folds to 'S'
        ] {
            assert_eq!(Capability::from_wire_key(bad), None, "{bad:?} must not resolve");
        }
    }

    #[test]
    fn adding_a_capability_forces_updating_these_tests() {
        // ALL covers discriminants 1..=4. Registering a fifth value fails here,
        // which is the signal to extend ALL and the literals test above.
        assert!(Capability::try_from(5).is_err());
    }
}
```

Then append to the end of `rs/crates/libs/paigasus-proto/src/lib.rs`:

```rust
/// Wire-key transform for the `Capability` registry in `common::v1`.
pub mod capability;
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/rs
cargo test -p paigasus-proto --lib capability
```

Expected: **compile failure**, `no method named as_wire_key found for enum Capability` and `no function or associated item named from_wire_key`. If it instead fails on an unused-import warning for `Capability`, that is the `warnings = "deny"` lint — it confirms the constraint and is resolved by Step 3.

- [ ] **Step 3: Write the implementation**

In `rs/crates/libs/paigasus-proto/src/capability.rs`, insert this **between** the `use` line and `#[cfg(test)] mod tests`:

```rust
/// The prefix every generated `Capability` value name carries.
const PREFIX: &str = "CAPABILITY_";

impl Capability {
    /// This capability's wire string, or `None` for the zero sentinel.
    ///
    /// Derived from prost's `as_str_name()` by the registry's mapping rule —
    /// strip `CAPABILITY_`, lowercase, `_` to `.` — never tabulated, so there is
    /// no second copy of the registry to drift against the proto.
    ///
    /// Returns `None` for [`Capability::Unspecified`] because that variant is
    /// prost's `Default`: a default-initialised or out-of-range-decoded value
    /// would otherwise silently advertise `"unspecified"` to every client.
    pub fn as_wire_key(self) -> Option<String> {
        let name = self.as_str_name().strip_prefix(PREFIX)?;
        if name == "UNSPECIFIED" {
            return None;
        }
        Some(name.to_ascii_lowercase().replace('_', "."))
    }

    /// The capability a wire string names, or `None` if it is not a registered key.
    ///
    /// The grammar is checked **positively, before** any transformation. A
    /// negative filter — rejecting `_` and ASCII uppercase — is not sufficient:
    /// `str::to_uppercase` folds U+0131 (dotless i) to `I`, so `"ıam.audit"`
    /// would otherwise resolve to a real capability.
    pub fn from_wire_key(key: &str) -> Option<Self> {
        if !is_wire_key(key) {
            return None;
        }
        let name = format!("{PREFIX}{}", key.to_ascii_uppercase().replace('.', "_"));
        match Self::from_str_name(&name)? {
            // "unspecified" satisfies the grammar, so reject the sentinel here.
            Self::Unspecified => None,
            capability => Some(capability),
        }
    }
}

/// `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`, hand-rolled to avoid a `regex` dependency.
fn is_wire_key(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_lowercase())
        && key.split('.').all(|segment| {
            !segment.is_empty()
                && segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/rs
cargo test -p paigasus-proto --lib capability
```

Expected: `test result: ok. 6 passed; 0 failed`.

- [ ] **Step 5: Check formatting and lints**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499/rs
cargo fmt --check
cargo clippy -p paigasus-proto -- -D warnings
```

Expected: no output from either. If `cargo fmt --check` prints a diff, run `cargo fmt` and re-run.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git add rs/crates/libs/paigasus-proto/src/capability.rs rs/crates/libs/paigasus-proto/src/lib.rs
git commit -m "feat(rs): derive capability wire keys from the generated registry (SMA-499)"
```

---

### Task 4: TypeScript wire-key transform and barrel exports

**Files:**
- Create: `ts/packages/paigasus-proto/src/capability.ts`
- Create: `ts/packages/paigasus-proto/src/capability.test.ts`
- Modify: `ts/packages/paigasus-proto/src/index.ts`

**Interfaces:**
- Consumes, from Task 1: `Capability` (members `UNSPECIFIED`, `IAM_AUTHZ_CEDAR`, `IAM_APIKEYS`, `IAM_AUDIT`, `GATEWAY_CHAT_STREAM` — protobuf-es **strips** the `CAPABILITY_` prefix, unlike prost), plus `ServiceInfoSchema`, `ServiceInfoService` and the type `ServiceInfo`, all from `./generated/paigasus/common/v1/service_info_pb.js`.
- Produces: `capabilityWireKey(capability: Capability): string | undefined`.

**Two constraints that will bite if ignored:**
1. `ts/tsconfig.base.json` sets `verbatimModuleSyntax: true`. `ServiceInfo` is a **type** and must be exported with `export type`; `ServiceInfoSchema`, `ServiceInfoService` and `Capability` are **values**. Mixing them in one `export {}` is a hard `tsc` error.
2. There is **no reverse parser** here, deliberately. A console compares advertised strings against known keys; it never needs to turn an arbitrary string into an enum, because an unknown key is ignored by contract.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-proto/src/capability.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { capabilityWireKey } from './capability.js';
import {
  Capability,
  ServiceInfoService,
} from './generated/paigasus/common/v1/service_info_pb.js';

describe('capabilityWireKey', () => {
  it('spells the ADR-0020 keys exactly', () => {
    expect(capabilityWireKey(Capability.IAM_AUTHZ_CEDAR)).toBe('iam.authz.cedar');
    expect(capabilityWireKey(Capability.IAM_APIKEYS)).toBe('iam.apikeys');
    expect(capabilityWireKey(Capability.IAM_AUDIT)).toBe('iam.audit');
    expect(capabilityWireKey(Capability.GATEWAY_CHAT_STREAM)).toBe('gateway.chat.stream');
  });

  it('has no wire key for the zero sentinel', () => {
    expect(capabilityWireKey(Capability.UNSPECIFIED)).toBeUndefined();
  });
});

describe('generated ServiceInfoService', () => {
  it('is declared in paigasus.common.v1', () => {
    expect(ServiceInfoService.typeName).toBe('paigasus.common.v1.ServiceInfoService');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run paigasus-proto-ts:test
```

Expected: FAIL — `Failed to load ./capability.js` or `Cannot find module`, because `capability.ts` does not exist yet.

- [ ] **Step 3: Write the implementation**

Create `ts/packages/paigasus-proto/src/capability.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { Capability } from './generated/paigasus/common/v1/service_info_pb.js';

/**
 * The wire string a capability is advertised as, or `undefined` for the zero
 * sentinel.
 *
 * Derived from the generated enum's member names rather than tabulated, so
 * there is no second copy of the registry to drift against the proto. Note the
 * asymmetry with the Rust helper: protobuf-es already strips the `CAPABILITY_`
 * prefix from member names, so only the lowercase-and-dot half of the mapping
 * rule remains here.
 *
 * There is deliberately no reverse parser. A client compares advertised strings
 * against keys it knows; it never needs to resolve an arbitrary string, because
 * an unknown key is ignored by contract.
 */
export function capabilityWireKey(capability: Capability): string | undefined {
  const name: string | undefined = Capability[capability];
  if (name === undefined || name === 'UNSPECIFIED') {
    return undefined;
  }
  return name.toLowerCase().replace(/_/g, '.');
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run paigasus-proto-ts:test
```

Expected: 3 tests passing (plus the pre-existing `audit.test.ts` and `health.test.ts`).

- [ ] **Step 5: Add the barrel exports**

Append to `ts/packages/paigasus-proto/src/index.ts`:

```ts
export { capabilityWireKey } from './capability.js';
export {
  Capability,
  ServiceInfoSchema,
  ServiceInfoService,
} from './generated/paigasus/common/v1/service_info_pb.js';
export type { ServiceInfo } from './generated/paigasus/common/v1/service_info_pb.js';
```

- [ ] **Step 6: Typecheck and format**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run paigasus-proto-ts:typecheck
moon run ts:fmt
```

Expected: both clean. `ts:fmt` is Prettier over the whole tree and is a **separate CI gate** from lint and tsc — if it rewrites your files, commit the result. If `typecheck` errors with "… is a type and must be imported using a type-only import", you merged the two export statements; keep them separate.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git add ts/packages/paigasus-proto/src/capability.ts \
        ts/packages/paigasus-proto/src/capability.test.ts \
        ts/packages/paigasus-proto/src/index.ts
git commit -m "feat(ts): derive capability wire keys and export the descriptor (SMA-499)"
```

---

### Task 5: Python codegen smoke test

Completes AC1's "generates to all three languages" with an assertion about contents, not just file existence. Follows the precedent of `py/packages/paigasus-proto/tests/test_health_smoke.py`, which did exactly this for the previous first-service-in-a-package.

**Files:**
- Create: `py/packages/paigasus-proto/tests/test_service_info_smoke.py`

**Interfaces:**
- Consumes, from Task 1: `paigasus_proto.generated.paigasus.common.v1.{ServiceInfo, Capability}`. betterproto2 strips the `CAPABILITY_` prefix from member names and exposes the full proto names via the classmethod `betterproto_value_to_renamed_proto_names() -> dict[int, str]`.
- Produces: nothing.

- [ ] **Step 1: Write the test**

Create `py/packages/paigasus-proto/tests/test_service_info_smoke.py`:

```python
# SPDX-License-Identifier: Apache-2.0
from paigasus_proto.generated.paigasus.common.v1 import Capability, ServiceInfo


def test_service_info_carries_a_capability_list() -> None:
    info = ServiceInfo(service="iam", version="1.4.0", capabilities=["iam.audit"])
    assert info.service == "iam"
    assert info.capabilities == ["iam.audit"]


def test_service_info_defaults_to_no_capabilities() -> None:
    # "absent capability -> feature off" starts from an empty list, not None.
    assert ServiceInfo().capabilities == []


def test_capability_registry_keeps_the_proto_names() -> None:
    names = Capability.betterproto_value_to_renamed_proto_names()
    assert names[Capability.IAM_AUTHZ_CEDAR.value] == "CAPABILITY_IAM_AUTHZ_CEDAR"
    assert names[Capability.IAM_APIKEYS.value] == "CAPABILITY_IAM_APIKEYS"
    assert names[Capability.IAM_AUDIT.value] == "CAPABILITY_IAM_AUDIT"
    assert names[Capability.GATEWAY_CHAT_STREAM.value] == "CAPABILITY_GATEWAY_CHAT_STREAM"
```

- [ ] **Step 2: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run py:test
```

Expected: 3 passed (plus the pre-existing audit and health tests).

Note the task id: Python `test`/`lint`/`fmt` are defined on the **root `py` project** (`.moon/tasks/python.yml`), not per package — `paigasus-proto-py` defines only `build`. `moon run paigasus-proto-py:test` does not exist.

If `ServiceInfo().capabilities` is `None` rather than `[]`, do **not** change the assertion to match — report it. It would mean betterproto2 models a repeated field as nullable, which changes what the spec's client contract must say.

- [ ] **Step 3: Lint and format the Python**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run py:lint
moon run py:fmt
```

Expected: clean. These are root-`py` tasks covering the whole tree, for the reason given in Step 2.

- [ ] **Step 4: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git add py/packages/paigasus-proto/tests/test_service_info_smoke.py
git commit -m "test(py): smoke-test the generated ServiceInfo descriptor and registry (SMA-499)"
```

---

### Task 6: Full-graph verification

No new files. This is the gate that catches what per-project tasks do not: per CLAUDE.md, `<proj>:build/test/lint/fmt` do **not** run the repo-level gates.

**Files:** none.

**Interfaces:** consumes the output of Tasks 1–5; produces nothing.

- [ ] **Step 1: Confirm the working tree is clean and the commits are present**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
git status --short
git log --oneline origin/main..HEAD
```

Expected: no output from `git status`; five feature commits plus the two spec/plan commits.

- [ ] **Step 2: Re-run the codegen-drift gate from a clean tree**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon run contracts:generate
git add --intent-to-add -- rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
git diff --exit-code -- rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
echo "drift gate exit: $?"
```

Expected: `drift gate exit: 0`.

- [ ] **Step 3: Run the full CI graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma499
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all tasks pass. Specifically:
- `:breaking` — green. A new file is additive and `option deprecated` is not breaking.
- `:affected-smoke` — green. No `dependsOn` edge was added, so the strict-equality expectation in `ci/affected-graph/run.sh` is unchanged.
- `:deny` / `:machete` — green. No new dependency in any workspace.

If Moon reports a bare "N failed" without naming the task, diagnose with:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

- [ ] **Step 4: Reviewer checklist — the part CI cannot prove**

AC3, and the "meaning of each key" half of AC2, are doc-comment properties no test can read. Open `contracts/proto/paigasus/common/v1/service_info.proto` and confirm the file header states, verbatim:

- [ ] the append-only rule, and that `buf breaking` is **not** its guard
- [ ] the mapping rule and the key grammar
- [ ] **an unknown key is ignored**
- [ ] **an absent capability means the feature is off**
- [ ] capability gating is cosmetic; the server stays authoritative
- [ ] each of the four values' doc comments opens with its literal wire key and says what the capability means

- [ ] **Step 5: Confirm the spec's human action is outstanding**

The spec's D10 requires an **ADR-0020 amendment** in Notion recording D1 (a generated enum where the ADR said doc comment) and D3 (the shared RPC plus a normative HTTP endpoint on every service). This is a human action, not an implementation step. Report it as outstanding in the handoff; do not attempt to edit Notion.

---

## Self-Review

**Spec coverage.** D1 → Task 1 (enum registry, string wire). D2 → no `buf.yaml` change, plus the merge-order constraint stated up front. D3 → Task 1's file comment (transport table) and the `service{}` declaration. D4 → Task 2. D5 → Task 1's comment block, enforced by Task 3's grammar test. D6 → Task 1's field comments. D7 → Task 3. D8 → Task 3's `is_wire_key` and rejection test. D9 → Task 4. D10 → Task 6 Step 5. §6 testing → Tasks 3, 4, 5. §7 AC mapping → Task 6 Step 4 carries the review-only items. §8 verification → Task 6.

**Placeholder scan.** No TBD/TODO. Every code step carries literal content; every command carries its expected output.

**Type consistency.** `as_wire_key` returns `Option<String>` in Task 3's tests, its implementation, and the Interfaces block. `from_wire_key` takes `&str` and returns `Option<Capability>` throughout. `capabilityWireKey` returns `string | undefined` in Task 4's test, implementation and Interfaces block. Rust variants are CamelCase (`IamApikeys`), TypeScript and Python members are UPPER_SNAKE with the prefix stripped (`IAM_APIKEYS`) — the asymmetry is called out in both places it matters.

**Verified during planning, so the plan does not rest on assumption:** the `common/v1` tonic include compiles; `Capability::default()` is `Unspecified`; `TryFrom<i32>` exists and errors on 5; deleting the placeholder reds `contracts:breaking`; `option deprecated` passes all four buf gates with the per-language asymmetry described; and the exact generated symbol names in all three languages.
