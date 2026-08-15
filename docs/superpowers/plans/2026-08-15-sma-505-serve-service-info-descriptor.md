# SMA-505 — Serve the `ServiceInfo` capability descriptor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `paigasus-iam` and `paigasus-gateway` each serve a `ServiceInfo` descriptor whose capability list is derived from live configuration, on an authenticated surface, with a version fed from the build.

**Architecture:** A new `paigasus-service-info` library crate owns the wire invariants (drop the `UNSPECIFIED` sentinel, deterministic order, always emit `capabilities`). Each service projects its own config into a small `Capabilities` value type — a pure function, unit-testable with no database — and serves the result. IAM serves both `GET /v1/service-info` and gRPC `ServiceInfoService`; the gateway serves HTTP only. A disabled capability's HTTP routes are not registered and its gRPC RPCs return `UNIMPLEMENTED`.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum 0.8, tonic, prost, serde, figment, Moon, buf.

**Spec:** `docs/superpowers/specs/2026-08-15-sma-505-serve-service-info-descriptor-design.md`
**Worktree:** `/Users/smaschek/dev/paigasus/paigasus-core-sma505`, branch `feature/sma-505-serve-serviceinfo-descriptor`

## Global Constraints

Every task's requirements implicitly include this section.

- **SPDX header.** Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- **Edition 2024, rust-version 1.95.** Inherited via `edition.workspace = true` — never write `edition = "2021"`.
- **`#![deny(warnings)]` is effectively on** (`[lints] workspace = true`). Dead code is a hard **compile error** on the lib target, which breaks every integration test in the package. Consequence for this plan: **every item added in an earlier task that a later task consumes must be `pub` and reachable from `lib.rs`.** A `pub` item in a `pub mod` is never dead code. Do not add private helpers "to wire up later".
- **PATH.** Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — shims first, so moon/buf/nextest resolve to the repo-pinned versions.
- **All commands run from the worktree** `/Users/smaschek/dev/paigasus/paigasus-core-sma505`, never the main checkout.
- **`cargo nextest` needs `--no-tests=pass`** when a target has no tests.
- **Commits:** conventional, workspace-scoped, subject **starts lowercase**, header ≤100 chars, body lines ≤100 chars. Never write `#NNN` in a commit body (it breaks `footer-leading-blank`); write "owner/repo PR NNN". Never `--no-verify`.
- **Capability wire keys** are exactly: `iam.authz.cedar`, `iam.apikeys`, `iam.audit`, `gateway.chat.stream`. Never hand-write these strings in service code — always go through `Capability::as_wire_key`.
- **All four config flags default to `true`.** Existing tests must keep passing untouched.
- **Service-level tests assert capability SETS, never sequences** — the proto declares the list
  unordered and tells clients to build a set from it, so a service test that pins an order encodes a
  property the contract disclaims. The **shared crate's own** tests are the one exception: ordering
  determinism is the guarantee `descriptor()` makes, so Task 1's tests assert `Vec` equality
  deliberately. Nothing outside `paigasus-service-info` may do so.

---

### Task 1: The `paigasus-service-info` crate

**Files:**
- Create: `rs/crates/libs/paigasus-service-info/Cargo.toml`
- Create: `rs/crates/libs/paigasus-service-info/moon.yml`
- Create: `rs/crates/libs/paigasus-service-info/src/lib.rs`
- Modify: `rs/Cargo.toml` (add to `[workspace.dependencies]`)
- Modify: `ci/affected-graph/run.sh:106-107` (the `contracts->proto` expected CSV)

**Interfaces:**
- Consumes: `paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo}`, and `Capability::as_wire_key(self) -> Option<String>` from `paigasus-proto`'s `capability` module.
- Produces:
  - `paigasus_service_info::ROUTE: &str` (`"/v1/service-info"`)
  - `paigasus_service_info::descriptor(service: &str, version: &str, capabilities: &[Capability]) -> ServiceInfo`
  - `paigasus_service_info::ServiceInfoDto` with public fields `service: String`, `version: String`, `capabilities: Vec<String>`, deriving `Serialize`, plus `impl From<&ServiceInfo> for ServiceInfoDto`

- [ ] **Step 1: Create the crate manifest**

`rs/crates/libs/paigasus-service-info/Cargo.toml`:

```toml
[package]
name = "paigasus-service-info"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[dependencies]
# The generated `ServiceInfo`/`Capability` types and `Capability::as_wire_key`, which is the
# single source of the registry's wire-key mapping rule (SMA-499).
paigasus-proto = { workspace = true }
# `ServiceInfoDto` derives `Serialize` — the HTTP body both services return.
serde = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Create the Moon project file**

`rs/crates/libs/paigasus-service-info/moon.yml`. The `deps` are declared **explicitly** rather than relying on the `paigasus-proto` edge, because a project-level `dependsOn` does not propagate task-affected state in Moon:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-service-info-rs'
layer: 'library'
language: 'rust'

tasks:
  build:
    deps: ['contracts:generate']
  test:
    deps: ['contracts:generate']
```

- [ ] **Step 3: Register the crate as a workspace dependency**

In `rs/Cargo.toml`, inside `[workspace.dependencies]`, next to the other `paigasus-*` path entries:

```toml
# paigasus-service-info — the shared `ServiceInfo` builder + HTTP DTO (SMA-505). Owns the wire
# invariants both services must not re-derive: the UNSPECIFIED sentinel is never advertised,
# ordering is deterministic, and `capabilities` is always emitted (as `[]` when empty) so a
# client doing `info.capabilities.includes(k)` cannot throw.
paigasus-service-info = { path = "crates/libs/paigasus-service-info", version = "0.0.0" }
```

- [ ] **Step 4: Write the failing tests**

Create `rs/crates/libs/paigasus-service-info/src/lib.rs` with **only** the test module (the crate will not compile — that is the point of this step):

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_proto::paigasus::common::v1::Capability;

    #[test]
    fn the_unspecified_sentinel_is_never_advertised() {
        let info = descriptor("iam", "1.2.3", &[Capability::Unspecified, Capability::IamAudit]);
        assert_eq!(info.capabilities, vec!["iam.audit".to_string()]);
    }

    #[test]
    fn duplicates_are_removed() {
        let info = descriptor("iam", "1.2.3", &[Capability::IamAudit, Capability::IamAudit]);
        assert_eq!(info.capabilities, vec!["iam.audit".to_string()]);
    }

    #[test]
    fn ordering_is_deterministic_regardless_of_input_order() {
        let forwards = descriptor("iam", "1.2.3", &[Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamAudit]);
        let backwards = descriptor("iam", "1.2.3", &[Capability::IamAudit, Capability::IamApikeys, Capability::IamAuthzCedar]);
        assert_eq!(forwards.capabilities, backwards.capabilities);
    }

    /// AC 4's only assertion that can actually fail while every crate is `version = "0.0.0"`:
    /// it pins that this crate neither rewrites nor substitutes the caller's version string.
    #[test]
    fn the_callers_version_flows_through_verbatim() {
        let info = descriptor("iam", "9.9.9-test-sentinel", &[]);
        assert_eq!(info.version, "9.9.9-test-sentinel");
        assert_eq!(info.service, "iam");
    }

    /// SMA-499 § 2.7: canonical protojson omits an empty repeated field, which would make a
    /// console doing `info.capabilities.includes(k)` throw instead of rendering "feature off".
    #[test]
    fn an_empty_capability_list_serializes_as_an_empty_array() {
        let dto = ServiceInfoDto::from(&descriptor("gateway", "0.0.0", &[]));
        let json = serde_json::to_string(&dto).expect("serialize");
        assert_eq!(json, r#"{"service":"gateway","version":"0.0.0","capabilities":[]}"#);
    }

    #[test]
    fn the_dto_field_names_match_the_protos_canonical_json_names() {
        let dto = ServiceInfoDto::from(&descriptor("iam", "0.0.0", &[Capability::IamAudit]));
        let json = serde_json::to_string(&dto).expect("serialize");
        assert_eq!(json, r#"{"service":"iam","version":"0.0.0","capabilities":["iam.audit"]}"#);
    }

    #[test]
    fn the_route_is_the_path_the_proto_specifies() {
        assert_eq!(ROUTE, "/v1/service-info");
    }
}
```

Add `serde_json` as a dev-dependency in `rs/crates/libs/paigasus-service-info/Cargo.toml`:

```toml
[dev-dependencies]
# The serialization assertions below are the whole point of `ServiceInfoDto` — they prove
# `capabilities` is emitted as `[]` rather than omitted.
serde_json = { workspace = true }
```

- [ ] **Step 5: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo test -p paigasus-service-info
```

Expected: FAIL — `cannot find function 'descriptor' in this scope`, `cannot find type 'ServiceInfoDto'`, `cannot find value 'ROUTE'`.

- [ ] **Step 6: Write the implementation**

Prepend to `rs/crates/libs/paigasus-service-info/src/lib.rs`, above the test module:

```rust
//! The `ServiceInfo` descriptor both Paigasus services serve (ADR-0020, SMA-505).
//!
//! This crate exists to hold the wire invariants in ONE tested place rather than in each
//! service: the `CAPABILITY_UNSPECIFIED` sentinel is never advertised, the list is
//! deterministic and de-duplicated, and `capabilities` is always emitted — as `[]` when empty.
//! That last one is not cosmetic: canonical protojson omits an empty repeated field, and a
//! console doing `info.capabilities.includes(k)` against a missing key throws `TypeError`
//! instead of rendering "feature off" (SMA-499 § 2.7).
//!
//! Each service owns its own config -> `Vec<Capability>` projection; nothing about that is
//! shared, because the two services read entirely different config types.

use paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo};
use serde::Serialize;

/// The HTTP route both services serve the descriptor on, so the path literal cannot drift
/// between them. Specified normatively in `common/v1/service_info.proto`'s file comment.
pub const ROUTE: &str = "/v1/service-info";

/// Build the descriptor from the capabilities a service currently has ENABLED.
///
/// `version` is a parameter rather than this crate's own `CARGO_PKG_VERSION`: each SERVICE
/// must report its own build, and taking it as an argument is what lets the test above prove
/// the value flows through untouched (AC 4 — see that test's doc for why the obvious
/// service-side assertion is vacuous today).
///
/// The `UNSPECIFIED` sentinel is dropped, duplicates are removed, and the result is ordered by
/// enum discriminant. Ordering is an implementation detail for stable output, NOT a contract —
/// the proto states the list is unordered and that clients must build a set from it.
pub fn descriptor(service: &str, version: &str, capabilities: &[Capability]) -> ServiceInfo {
    let mut caps: Vec<Capability> = capabilities.iter().copied().filter(|c| *c != Capability::Unspecified).collect();
    caps.sort_by_key(|c| *c as i32);
    caps.dedup();
    ServiceInfo {
        service: service.to_owned(),
        version: version.to_owned(),
        // `as_wire_key` returns `None` only for the sentinel, already filtered above — so this
        // never silently drops a real capability. It stays the sole source of the mapping rule.
        capabilities: caps.into_iter().filter_map(Capability::as_wire_key).collect(),
    }
}

/// The JSON body of `GET /v1/service-info`: the BARE `ServiceInfo`, not the RPC response
/// wrapper (SMA-499 D3 — the wrapper exists only to satisfy buf lint).
///
/// `capabilities` is a plain `Vec<String>` with no `skip_serializing_if`, so serde emits `[]`
/// for an empty list. That is the MUST-emit-defaults rule holding by construction rather than
/// by anyone remembering it.
#[derive(Debug, Serialize)]
pub struct ServiceInfoDto {
    pub service: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

impl From<&ServiceInfo> for ServiceInfoDto {
    fn from(info: &ServiceInfo) -> Self {
        ServiceInfoDto {
            service: info.service.clone(),
            version: info.version.clone(),
            capabilities: info.capabilities.clone(),
        }
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo test -p paigasus-service-info
```

Expected: PASS, 7 tests.

- [ ] **Step 8: Update the affected-graph expected set**

The new crate depends on `paigasus-proto`, so it now appears in the `contracts->proto` case's affected set. That case uses **strict equality with default-deny**, so it reds until the crate is listed.

In `ci/affected-graph/run.sh`, change the `contracts->proto` expected CSV (line ~107) from:

```
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs"
```

to:

```
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs"
```

Also extend that case's preceding comment to name the new dependent:

```bash
  # contracts proto edit -> proto packages in all three languages + the gateway rebuild + the
  # IAM service crate that consumes paigasus-proto-rs for its gRPC surface (SMA-442) + the
  # shared descriptor crate that consumes the generated ServiceInfo/Capability types (SMA-505).
```

- [ ] **Step 9: Verify the graph and CODEOWNERS gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
moon sync codeowners
moon run repo:affected-smoke
```

Expected: `affected-smoke` PASSes every case. `moon sync codeowners` may rewrite `.github/CODEOWNERS` — that file is Moon-generated, so commit whatever it produces and never hand-edit it.

- [ ] **Step 10: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/libs/paigasus-service-info rs/Cargo.toml rs/Cargo.lock ci/affected-graph/run.sh .github/CODEOWNERS
git commit -m "feat(rs): add paigasus-service-info, the shared ServiceInfo builder (SMA-505)"
```

---

### Task 2: Narrow the two `Capability` doc comments

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/service_info.proto:119-125`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/**` and `ts/packages/paigasus-proto/src/generated/**`
- Modify: `docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md` (§ 4.2 table)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: no new symbols. The wire, the enum values and their numbers are all unchanged — only doc comments move.

- [ ] **Step 1: Narrow the two doc comments**

In `contracts/proto/paigasus/common/v1/service_info.proto`, replace the `CAPABILITY_IAM_AUTHZ_CEDAR` and `CAPABILITY_IAM_APIKEYS` blocks:

```proto
  // "iam.authz.cedar" — Cedar policy ADMINISTRATION is available: policies and
  // role grants can be created, listed and deleted.
  //
  // Scoped to administration deliberately. Authorization DECISIONS are a
  // service-to-service primitive every Paigasus service depends on per request,
  // so `IsAuthorized` and `POST /v1/authz/is-authorized` stay available even
  // when this capability is absent — as does internal tenancy enforcement.
  // A client must not read the absence of this key as "authorization is off".
  CAPABILITY_IAM_AUTHZ_CEDAR = 1;

  // "iam.apikeys" — service-account API key MANAGEMENT is available: keys can be
  // issued, listed and revoked.
  //
  // Scoped to management deliberately. Key INTROSPECTION is a service-to-service
  // primitive the gateway calls on every request, so `IntrospectApiKey` and
  // `POST /v1/authn/api-keys/introspect` stay available even when this capability
  // is absent, and previously issued keys keep authenticating. A client must not
  // read the absence of this key as "API keys do not work here".
  CAPABILITY_IAM_APIKEYS = 2;
```

- [ ] **Step 2: Format, then regenerate all three binding trees**

`contracts:fmt` reds silently if the proto is unformatted. `contracts:generate` has no `outputs:` and can serve stale cache, so `buf generate` is run **directly**:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/contracts
buf format -w
buf generate
```

- [ ] **Step 3: Verify the generated trees actually changed**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git status --short rs/crates/libs/paigasus-proto/src/generated py ts/packages/paigasus-proto/src/generated
```

Expected: modified files in **all three** trees. A comment-only proto edit still shifts the embedded `FILE_DESCRIPTOR_SET`, so an unchanged tree means `buf generate` did not run against the edited file — re-run it.

- [ ] **Step 4: Verify nothing broke, including the breaking-change gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
moon run contracts:fmt contracts:breaking
cd rs && cargo test -p paigasus-proto
```

Expected: `contracts:fmt` PASS, `contracts:breaking` PASS (no field, value or number changed — comments are not breaking), `paigasus-proto` tests PASS including `the_registry_spells_the_adr_keys_exactly`, which must still see the same four wire strings.

- [ ] **Step 5: Update the predecessor spec's now-stale table**

In `docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md` § 4.2, update the two affected rows of the "Proto value / Wire key / Meaning" table:

| `CAPABILITY_IAM_AUTHZ_CEDAR` | `iam.authz.cedar` | Cedar policy **administration** is available (narrowed by SMA-505; `IsAuthorized` stays mounted regardless) |
| `CAPABILITY_IAM_APIKEYS` | `iam.apikeys` | API key **management** is available (narrowed by SMA-505; introspection stays mounted regardless) |

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add contracts rs/crates/libs/paigasus-proto/src/generated py ts docs/superpowers/specs
git commit -F - <<'EOF'
feat(contracts): scope iam.authz.cedar and iam.apikeys to administration (SMA-505)

Authorization decisions and key introspection are service-to-service
primitives other Paigasus services call on every request, not features a
console renders. Scoping the two keys to administration and management
lets those primitives stay permanently mounted, so no capability toggle
can break the gateway.

Comment-only: no field, enum value or number changes, so the wire is
untouched and buf breaking stays green. All three binding trees are
regenerated because the embedded FILE_DESCRIPTOR_SET shifts.
EOF
```

---

### Task 3: IAM config flags and the capability predicate

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` (three flags, three `*Defaults` structs, the boot warn helper)
- Create: `rs/crates/services/paigasus-iam/src/service_info.rs`
- Modify: `rs/crates/services/paigasus-iam/src/lib.rs` (declare `pub mod service_info;`)
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml` (add `paigasus-service-info`)
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example`

**Interfaces:**
- Consumes: `paigasus_service_info::descriptor` (Task 1).
- Produces:
  - `paigasus_iam::config::AuthzConfig.admin_enabled: bool`
  - `paigasus_iam::config::ApiKeyConfig.management_enabled: bool`
  - `paigasus_iam::config::AuditConfig.query_enabled: bool`
  - `paigasus_iam::service_info::SERVICE: &str` (`"iam"`), `paigasus_iam::service_info::VERSION: &str`
  - `paigasus_iam::service_info::Capabilities` with public fields `authz_admin`, `apikeys_management`, `audit_query` (all `bool`), deriving `Debug, Clone, Copy, PartialEq, Eq`
  - `Capabilities::from_config(cfg: &IamConfig) -> Capabilities`
  - `Capabilities::enabled(&self) -> Vec<Capability>`
  - `Capabilities::descriptor(&self) -> ServiceInfo`

- [ ] **Step 1: Add the dependency**

In `rs/crates/services/paigasus-iam/Cargo.toml`, under `[dependencies]`:

```toml
# `service_info` — the shared ServiceInfo builder + HTTP DTO (SMA-505). This crate supplies
# only the wire invariants; the config -> capability projection lives in `src/service_info.rs`.
paigasus-service-info = { workspace = true }
```

- [ ] **Step 2: Add the three config flags**

In `rs/crates/services/paigasus-iam/src/config.rs`, add a field to each of the three structs and to each mirroring `*Defaults` struct. `AuthzConfig`:

```rust
pub struct AuthzConfig {
    /// SMA-505: whether the policy/role-grant ADMINISTRATION surface is served —
    /// `/v1/authz/policies*`, `/v1/authz/role-grants*` and `/v1/authz/system-policies/{id}/retire`
    /// on HTTP, and the six administration RPCs on `AuthorizationService`. Governs the
    /// `iam.authz.cedar` capability key.
    ///
    /// `false` UNMOUNTS those surfaces (404 / `UNIMPLEMENTED`); it does NOT tear down Cedar.
    /// `IsAuthorized`, `POST /v1/authz/is-authorized`, the policy snapshot, its reload task and
    /// `Authorize::check` under `enforce_tenancy` all keep working — so no setting here can
    /// break the gateway. Intended for a deployment whose policies are applied as code at boot
    /// and which wants the runtime mutation surface closed.
    pub admin_enabled: bool,
    pub enforce_tenancy: bool,
    // … existing fields unchanged
}
```

`ApiKeyConfig`:

```rust
    /// SMA-505: whether the API-key MANAGEMENT surface is served —
    /// `/v1/service-accounts/{sa}/api-keys*` on HTTP, and `IssueApiKey`/`RevokeApiKey`/
    /// `ListApiKeys` on `ServiceAccountService`. Governs the `iam.apikeys` capability key.
    ///
    /// `false` UNMOUNTS those surfaces only. Introspection stays mounted
    /// (`/v1/authn/api-keys/introspect`, gRPC `IntrospectApiKey`), `require_bearer`'s API-key
    /// credential path keeps working, and previously issued keys keep authenticating — so the
    /// gateway is unaffected. Service-account lifecycle routes are also untouched: they are
    /// tenancy management, not an API-key concern.
    pub management_enabled: bool,
```

`AuditConfig`:

```rust
    /// SMA-505: whether the audit-log READ surface is served — `GET /v1/audit` on HTTP and the
    /// whole `AuditService` on gRPC. Governs the `iam.audit` capability key.
    ///
    /// `false` unmounts reading only. Every WRITE path continues: the denial audit sink,
    /// `PgAuditLog`, partition maintenance and retention. Intended for a deployment shipping
    /// audit to an external SIEM, where the in-product reader is redundant and its query load on
    /// the partitioned table is unwanted. Logs a startup warning, since writing entries nobody
    /// can read in-product is a misconfiguration in every other case.
    pub query_enabled: bool,
```

Add the matching field to `AuthzDefaults` (`:642`), `ApiKeyDefaults` (`:660`) and `AuditDefaults` (`:675`), and set each to `true` in the corresponding `impl Default`. Also add each field to the `impl Default for AuditConfig` (`:847`) and to any other `impl Default` that reconstructs these structs field-by-field — the compiler will name every site it needs.

- [ ] **Step 3: Write the failing predicate tests**

Create `rs/crates/services/paigasus-iam/src/service_info.rs` with the doc comment, the SPDX header, and **only** the test module:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IamConfig;
    use paigasus_proto::paigasus::common::v1::Capability;
    use std::collections::HashSet;

    fn caps(authz_admin: bool, apikeys_management: bool, audit_query: bool) -> HashSet<Capability> {
        Capabilities { authz_admin, apikeys_management, audit_query }.enabled().into_iter().collect()
    }

    #[test]
    fn all_enabled_advertises_every_iam_capability() {
        assert_eq!(
            caps(true, true, true),
            HashSet::from([Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamAudit])
        );
    }

    #[test]
    fn all_disabled_advertises_nothing() {
        assert!(caps(false, false, false).is_empty());
    }

    /// AC 3's central assertion. Asserting only "the key is absent" would pass against an
    /// implementation returning an empty list unconditionally, so every case ALSO asserts the
    /// siblings survive.
    #[test]
    fn disabling_one_flag_removes_exactly_its_key() {
        assert_eq!(caps(false, true, true), HashSet::from([Capability::IamApikeys, Capability::IamAudit]));
        assert_eq!(caps(true, false, true), HashSet::from([Capability::IamAuthzCedar, Capability::IamAudit]));
        assert_eq!(caps(true, true, false), HashSet::from([Capability::IamAuthzCedar, Capability::IamApikeys]));
    }

    /// R3: the real risk surface is combinations, not single flags. All 8 are cheap here
    /// because this is a pure function.
    #[test]
    fn every_combination_advertises_exactly_its_enabled_keys() {
        for authz in [false, true] {
            for apikeys in [false, true] {
                for audit in [false, true] {
                    let got = caps(authz, apikeys, audit);
                    assert_eq!(got.contains(&Capability::IamAuthzCedar), authz);
                    assert_eq!(got.contains(&Capability::IamApikeys), apikeys);
                    assert_eq!(got.contains(&Capability::IamAudit), audit);
                }
            }
        }
    }

    #[test]
    fn the_projection_reads_the_three_config_flags() {
        let mut cfg = IamConfig::default();
        cfg.authz.admin_enabled = false;
        cfg.api_keys.management_enabled = true;
        cfg.audit.query_enabled = false;
        assert_eq!(
            Capabilities::from_config(&cfg),
            Capabilities { authz_admin: false, apikeys_management: true, audit_query: false }
        );
    }

    #[test]
    fn the_descriptor_names_this_service_and_this_crates_build_version() {
        let info = Capabilities { authz_admin: true, apikeys_management: true, audit_query: true }.descriptor();
        assert_eq!(info.service, "iam");
        // `env!` is expanded HERE, in the test, NOT read back from the module's `VERSION` const.
        // That is the whole point: replacing the const with a literal (`"1.0.0"`) makes the
        // served value diverge from this crate's real `Cargo.toml` version and fails the
        // assertion, where comparing against the const itself would pass trivially.
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        // SemVer-shaped: `major.minor.patch`, all numeric, ignoring any pre-release/build
        // suffix. An empty or malformed version fails loudly instead of being served.
        let core = info.version.split(['-', '+']).next().expect("a version always has a core");
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "version core must be major.minor.patch: {}", info.version);
        assert!(
            parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "version core must be numeric: {}",
            info.version
        );
        // Still NOT proven while every crate is "0.0.0": that this is the SERVICE's version
        // rather than the shared library's. Both strings are identical today (spec § 6.4).
    }

    #[test]
    fn every_advertised_string_is_a_registered_capability_key() {
        let info = Capabilities { authz_admin: true, apikeys_management: true, audit_query: true }.descriptor();
        for key in &info.capabilities {
            assert!(Capability::from_wire_key(key).is_some(), "{key} is not in the registry");
        }
    }
}
```

If `IamConfig` has no `Default` impl, build the config in `the_projection_reads_the_three_config_flags` the way `tests/support/mod.rs` does instead — the point is only that the three flags are read from the three config fields.

- [ ] **Step 4: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo test -p paigasus-iam --lib service_info
```

Expected: FAIL — `cannot find type 'Capabilities' in this scope`.

- [ ] **Step 5: Write the implementation**

Prepend to `rs/crates/services/paigasus-iam/src/service_info.rs`:

```rust
//! IAM's capability projection: which registered keys this build currently has ENABLED.
//!
//! `Capabilities` is a small value type projected out of `IamConfig` ONCE, at wiring time, and
//! then carried on `AppState`. Deliberately not `&IamConfig` on request state: `IamConfig`
//! transitively carries `RawPepper` and every `RedactedUrl`, so storing it would clone the
//! API-key pepper into every HTTP and gRPC worker.
//!
//! `enabled()` is a pure function of three booleans, which is what makes AC 3's central
//! assertion ("flip the flag, the key disappears, the siblings remain") an ordinary unit test
//! with no `AppState`, no Postgres and no Docker.

use paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo};

use crate::config::IamConfig;

/// The bare service slug, matching the prefix of this service's own capability keys.
/// Advisory per the proto — a client must never use it as a cache key.
pub const SERVICE: &str = "iam";

/// This build's version. `env!` is evaluated in THIS crate, so it is `paigasus-iam`'s own
/// `Cargo.toml` version and nothing else's (AC 4). Every crate in the workspace is currently
/// `0.0.0` and release-plz is dormant, so this reports `0.0.0` until releases are cut.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The three capability toggles, projected out of `IamConfig` at wiring time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// `authz.admin_enabled` -> `iam.authz.cedar`.
    pub authz_admin: bool,
    /// `api_keys.management_enabled` -> `iam.apikeys`.
    pub apikeys_management: bool,
    /// `audit.query_enabled` -> `iam.audit`.
    pub audit_query: bool,
}

impl Capabilities {
    #[must_use]
    pub fn from_config(cfg: &IamConfig) -> Self {
        Capabilities {
            authz_admin: cfg.authz.admin_enabled,
            apikeys_management: cfg.api_keys.management_enabled,
            audit_query: cfg.audit.query_enabled,
        }
    }

    /// The registered capabilities this build currently has enabled. Pure — the unit under
    /// test for AC 3.
    #[must_use]
    pub fn enabled(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        if self.authz_admin {
            caps.push(Capability::IamAuthzCedar);
        }
        if self.apikeys_management {
            caps.push(Capability::IamApikeys);
        }
        if self.audit_query {
            caps.push(Capability::IamAudit);
        }
        caps
    }

    /// The descriptor both transports serve. Shared so the HTTP route and the gRPC RPC cannot
    /// drift (spec § 6.5 pins that they agree).
    #[must_use]
    pub fn descriptor(&self) -> ServiceInfo {
        paigasus_service_info::descriptor(SERVICE, VERSION, &self.enabled())
    }
}
```

Declare the module in `rs/crates/services/paigasus-iam/src/lib.rs`:

```rust
pub mod service_info;
```

`pub` is required, not stylistic: a private item unused until Task 4 is dead code, which is a hard compile error under this workspace's lint config.

- [ ] **Step 6: Add the startup warning for a write-only audit log**

In `rs/crates/services/paigasus-iam/src/main.rs`, next to the existing retention-disabled warning:

```rust
    if !cfg.audit.query_enabled {
        tracing::warn!(
            "audit.query_enabled = false: entries are still written but GET /v1/audit and the \
             AuditService gRPC are not served, so nothing can read them in-product"
        );
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo test -p paigasus-iam --lib
```

Expected: PASS — the new `service_info` tests plus every existing lib test. Existing config tests must be unaffected because all three flags default to `true`.

- [ ] **Step 8: Document the flags in the example config**

In `rs/crates/services/paigasus-iam/iam.toml.example`, add each flag to its existing block, e.g. under `[authz]`:

```toml
# Serve the policy / role-grant administration surface (default true). When false,
# /v1/authz/policies*, /v1/authz/role-grants* and /v1/authz/system-policies/{id}/retire are not
# registered and the six AuthorizationService administration RPCs return UNIMPLEMENTED, and the
# iam.authz.cedar capability is not advertised. Authorization DECISIONS are unaffected:
# is-authorized, gRPC IsAuthorized and internal tenancy enforcement all keep working, so this
# cannot break the gateway.
admin_enabled = true
```

Add the equivalent comment + `management_enabled = true` under `[api_keys]` and `query_enabled = true` under `[audit]`, each stating what is unmounted and what deliberately keeps running.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/services/paigasus-iam rs/Cargo.lock
git commit -m "feat(rs): add IAM capability toggles and the config-driven projection (SMA-505)"
```

---

### Task 4: IAM's HTTP surface — conditional routes and `/v1/service-info`

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/service_info.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (`AppState`, `AppState::new`, `app_routes`, the conflict test)
- Create: `rs/crates/services/paigasus-iam/tests/http_service_info.rs`

**Interfaces:**
- Consumes: `paigasus_iam::service_info::Capabilities` (Task 3), `paigasus_service_info::{ROUTE, ServiceInfoDto}` (Task 1).
- Produces: `AppState.capabilities: Capabilities` (public field, like the other `AppState` service fields), and `adapters::http::service_info::router() -> Router<AppState>`.

- [ ] **Step 1: Carry `Capabilities` on `AppState`**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, add to the `AppState` struct:

```rust
    /// The capability toggles this build was configured with (SMA-505), projected once in
    /// `AppState::new`. Read by `app_routes` to decide which sub-routers to merge, by the gRPC
    /// guards, and by both descriptor handlers — one source of truth, derived on demand rather
    /// than cached as a pre-computed key list.
    pub capabilities: crate::service_info::Capabilities,
```

In `AppState::new` (`:310`), populate it near the other `cfg`-derived fields:

```rust
            capabilities: crate::service_info::Capabilities::from_config(cfg),
```

- [ ] **Step 2: Write the failing integration test**

Create `rs/crates/services/paigasus-iam/tests/http_service_info.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-505 AC 1/2/3 over HTTP: the descriptor's shape, its authentication requirement, and the
//! surface half of "flip a flag, the key disappears" — the route is genuinely gone, not merely
//! unadvertised.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop the test skips — the same gating pattern as
//! `tests/authz_enforce_toggle.rs`. The DESCRIPTOR half of AC 3 is proven unconditionally by the
//! pure-predicate unit tests in `src/service_info.rs`, so a skipped daemon never leaves AC 3
//! entirely unproven.

mod support;

use axum::http::StatusCode;
use serde_json::Value;
use support::{app_with_config, provision, send, test_config};

/// Reads the descriptor's capability list as a set (the proto declares the list unordered).
fn capability_set(body: &str) -> std::collections::HashSet<String> {
    let json: Value = serde_json::from_str(body).expect("descriptor is JSON");
    json["capabilities"]
        .as_array()
        .expect("capabilities must be an array, never absent")
        .iter()
        .map(|v| v.as_str().expect("capability keys are strings").to_string())
        .collect()
}

#[tokio::test]
async fn the_descriptor_requires_a_bearer_and_reports_every_enabled_capability() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let cfg = test_config(&idp);
    let (app, state) = app_with_config(db, &cfg).await;

    // AC 2: no credential -> 401, never 200 and never 404.
    let (status, _) = send(&app, "GET", "/v1/service-info", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the descriptor must not be an unauthenticated surface");

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;
    let (status, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // AC 1: exact shape.
    let json: Value = serde_json::from_str(&body).expect("descriptor is JSON");
    assert_eq!(json["service"], "iam");
    assert!(json["version"].as_str().is_some_and(|v| !v.is_empty()), "version must be a non-empty string");
    assert_eq!(
        capability_set(&body),
        std::collections::HashSet::from(["iam.authz.cedar".to_string(), "iam.apikeys".to_string(), "iam.audit".to_string()])
    );
}

/// AC 3, surface half: each flag off removes its own route AND its own key, and leaves the
/// siblings alone. The sibling assertion is what stops this passing against an implementation
/// that returns an empty list unconditionally.
#[tokio::test]
async fn disabling_audit_query_removes_both_the_route_and_the_key() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.audit.query_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let (status, _) = send(&app, "GET", "/v1/audit", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a disabled capability's route must be unmounted, not merely unadvertised");

    let (_, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    let caps = capability_set(&body);
    assert!(!caps.contains("iam.audit"), "the disabled key must be absent: {body}");
    assert!(caps.contains("iam.authz.cedar"), "siblings must survive: {body}");
    assert!(caps.contains("iam.apikeys"), "siblings must survive: {body}");
}

#[tokio::test]
async fn disabling_authz_admin_removes_policy_role_grant_and_retirement_routes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.admin_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    for path in ["/v1/authz/policies", "/v1/authz/role-grants"] {
        let (status, _) = send(&app, "GET", path, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} must be unmounted");
    }
    // is-authorized is a service-to-service primitive and stays mounted regardless — anything
    // other than 404 proves it is still routed (the body may legitimately be a 400/403).
    let (status, _) = send(&app, "POST", "/v1/authz/is-authorized", Some(serde_json::json!({})), Some(token.as_str())).await;
    assert_ne!(status, StatusCode::NOT_FOUND, "is-authorized must stay mounted so the gateway keeps working");

    let (_, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    let caps = capability_set(&body);
    assert!(!caps.contains("iam.authz.cedar"), "{body}");
    assert!(caps.contains("iam.apikeys") && caps.contains("iam.audit"), "siblings must survive: {body}");
}

#[tokio::test]
async fn disabling_apikey_management_removes_management_but_keeps_introspection() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.api_keys.management_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let sa = "00000000-0000-0000-0000-000000000001";
    let (status, _) = send(&app, "GET", &format!("/v1/service-accounts/{sa}/api-keys"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "key management must be unmounted");

    // Introspection is a service-to-service primitive the gateway calls per request.
    let (status, _) = send(&app, "POST", "/v1/authn/api-keys/introspect", Some(serde_json::json!({"token": "nope"})), None).await;
    assert_ne!(status, StatusCode::NOT_FOUND, "introspection must stay mounted so the gateway keeps working");

    let (_, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    let caps = capability_set(&body);
    assert!(!caps.contains("iam.apikeys"), "{body}");
    assert!(caps.contains("iam.authz.cedar") && caps.contains("iam.audit"), "siblings must survive: {body}");
}

/// The empty-list case SMA-499 § 2.7's MUST-emit-defaults rule exists for, and the multi-flag
/// combination R3 warns about: conditional merging must not panic at router registration.
#[tokio::test]
async fn all_capabilities_disabled_serves_an_empty_array_not_a_missing_field() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.admin_enabled = false;
    cfg.api_keys.management_enabled = false;
    cfg.audit.query_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let (status, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(r#""capabilities":[]"#), "capabilities must be emitted as [], never omitted: {body}");
}
```

- [ ] **Step 2b: Check the `support` helper signatures before running**

`send` and `app_with_config` are used above as `send(&app, method, path, body: Option<Value>, bearer: Option<&str>) -> (StatusCode, String)` and `app_with_config(db, &cfg) -> (Router, AppState)`, matching `tests/authz_enforce_toggle.rs`. Read `rs/crates/services/paigasus-iam/tests/support/mod.rs` and adjust the call sites if the real signatures differ — do not change `support/mod.rs`.

- [ ] **Step 3: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
CI=1 cargo nextest run -p paigasus-iam --test http_service_info
```

`CI=1` is required: without it a missing Docker daemon makes these tests `return` early and report a pass having run nothing.

Expected: FAIL — `/v1/service-info` returns 404 (no route), and `cfg.audit.query_enabled` does not exist if Task 3 was skipped.

- [ ] **Step 4: Write the route handler**

Create `rs/crates/services/paigasus-iam/src/adapters/http/service_info.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `GET /v1/service-info` — IAM's capability descriptor (ADR-0020, SMA-505).
//!
//! Merged INSIDE `app_routes`'s `protected` sub-router, so it inherits
//! `auth_middleware::require_bearer`: an OIDC session and a service-account API key both work,
//! and no authorization action is checked. Discovery must not be gated on a permission — a
//! caller who legitimately cannot use a feature still needs to know it exists.
//!
//! Inheriting `require_bearer` also inherits its `Provisioning::Enabled` JIT provisioning and
//! bootstrap-admin seeding, so this `GET` can create a principal row. That is true of every
//! protected IAM route and is not changed here, but it is a write on a read endpoint and is
//! called out rather than left to be discovered.

use axum::{Json, Router, extract::State, routing::get};
use paigasus_service_info::{ROUTE, ServiceInfoDto};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(ROUTE, get(get_service_info))
}

/// Always `200` for an authenticated caller. The body is the BARE `ServiceInfo` (SMA-499 D3),
/// and `capabilities` is always present — as `[]` when nothing is enabled.
async fn get_service_info(State(state): State<AppState>) -> Json<ServiceInfoDto> {
    Json(ServiceInfoDto::from(&state.capabilities.descriptor()))
}
```

Declare it in `adapters/http/mod.rs`'s module list: `mod service_info;`

- [ ] **Step 5: Make `app_routes` conditional**

Replace the `protected` builder in `app_routes` (`:803`):

```rust
fn app_routes(state: AppState) -> Router {
    let caps = state.capabilities;
    let mut protected = Router::new()
        .merge(organizations::router())
        .merge(teams::router())
        .merge(projects::router())
        .merge(memberships::router())
        .merge(users::router())
        .merge(service_accounts::router())
        .merge(dead_letters::router())
        // The descriptor itself is always mounted and always inside the bearer layer (SMA-505).
        .merge(service_info::router());
    // SMA-505: a disabled capability's routes are NOT REGISTERED, so they 404 exactly as they
    // would on a build predating the feature. `is-authorized` is deliberately outside this
    // branch — it is the gateway's per-request primitive, not policy administration.
    if caps.authz_admin {
        protected = protected.merge(authz::admin_router()).merge(system_retirement::router());
    }
    protected = protected.merge(authz::decision_router());
    if caps.apikeys_management {
        protected = protected.merge(api_keys::router());
    }
    if caps.audit_query {
        protected = protected.merge(audit::router());
    }
    let protected = protected
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware::require_bearer))
        .with_state(state.clone());
    // … the rest (authn_api, api_key_introspect_api, the final merge + http_metrics_layer)
    //     is unchanged
}
```

Split `adapters/http/authz.rs`'s `router()` into two, so `is-authorized` can stay mounted while administration goes away:

```rust
/// `POST /v1/authz/is-authorized` — the authorization DECISION endpoint. Always mounted: it is
/// the service-to-service primitive the gateway calls per request, not policy administration,
/// so no `authz.admin_enabled` setting removes it (SMA-505 D8).
pub fn decision_router() -> Router<AppState> {
    Router::new().route("/v1/authz/is-authorized", post(is_authorized))
}

/// Policy and role-grant ADMINISTRATION — gated by `authz.admin_enabled`, and the surface the
/// `iam.authz.cedar` capability key describes.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/v1/authz/policies", post(put_policy).get(list_policies))
        .route("/v1/authz/policies/{policy_id}", delete(delete_policy))
        .route("/v1/authz/role-grants", post(create_role_grant).get(list_role_grants))
        .route("/v1/authz/role-grants/{id}", delete(revoke_role_grant))
}
```

- [ ] **Step 6: Extend the path-conflict test**

axum panics at *registration* time on a path conflict, so a conflict introduced by one flag combination would otherwise surface only at boot. In `adapters/http/mod.rs`'s test module, replace `protected_router_merge_has_no_path_conflicts` with a version covering every combination:

```rust
    /// SMA-469 + SMA-505: axum panics AT REGISTRATION time (inside `.route`/`.merge`), so this
    /// reproduces `app_routes`'s exact `protected` merge chain for all EIGHT capability
    /// combinations. A conflict reachable only under one flag combination fails here rather
    /// than at a customer's boot.
    #[test]
    fn protected_router_merge_has_no_path_conflicts_in_any_capability_combination() {
        for authz_admin in [false, true] {
            for apikeys_management in [false, true] {
                for audit_query in [false, true] {
                    let mut r: Router<AppState> = Router::new()
                        .merge(organizations::router())
                        .merge(teams::router())
                        .merge(projects::router())
                        .merge(memberships::router())
                        .merge(users::router())
                        .merge(service_accounts::router())
                        .merge(dead_letters::router())
                        .merge(service_info::router())
                        .merge(authz::decision_router());
                    if authz_admin {
                        r = r.merge(authz::admin_router()).merge(system_retirement::router());
                    }
                    if apikeys_management {
                        r = r.merge(api_keys::router());
                    }
                    if audit_query {
                        r = r.merge(audit::router());
                    }
                    let _ = r;
                }
            }
        }
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
cargo test -p paigasus-iam --lib
CI=1 cargo nextest run -p paigasus-iam --test http_service_info --retries 2
```

Expected: PASS. `--retries 2` because this crate's Docker-backed suites are genuinely flaky under parallel container startup — a different random subset can fail with "postgres did not accept connections within 60s".

- [ ] **Step 8: Confirm no existing HTTP test regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
CI=1 cargo nextest run -p paigasus-iam --retries 2
```

Expected: PASS. All flags default to `true`, so every existing route stays mounted. If `http_authz.rs` or `http_audit.rs` fails, the conditional merge dropped a route that should still be registered.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): serve the IAM descriptor over HTTP and unmount disabled surfaces (SMA-505)"
```

---

### Task 5: IAM's gRPC surface — `ServiceInfoService` and `UNIMPLEMENTED` guards

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/grpc/service_info.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs` (6 administration RPCs)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/service_accounts.rs` (3 key RPCs)
- Create: `rs/crates/services/paigasus-iam/tests/grpc_service_info.rs`

**Interfaces:**
- Consumes: `AppState.capabilities` (Task 4).
- Produces: `adapters::grpc::service_info::ServiceInfoGrpc::new(state: AppState) -> ServiceInfoGrpc`, implementing the generated `paigasus_proto::paigasus::common::v1::service_info_service_server::ServiceInfoService` trait.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/grpc_service_info.rs`, modelled on `tests/grpc_audit.rs`'s harness (an ephemeral `TcpListener` + `grpc::router(AppState::new(db, &cfg), ..)`):

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-505: `ServiceInfoService.GetServiceInfo` over gRPC, plus the spec § 6.5 transport-
//! agreement assertion — the HTTP body and the RPC response must describe the same build.
//!
//! Docker-gated exactly like `tests/grpc_audit.rs`; see `tests/http_service_info.rs`'s module
//! doc for why a skipped daemon still leaves AC 3 proven.

mod support;

use std::collections::HashSet;

#[tokio::test]
async fn get_service_info_requires_a_bearer() {
    // Build the gRPC server exactly as tests/grpc_audit.rs does, call GetServiceInfo with NO
    // `authorization` metadata, and assert the status code is `Unauthenticated` — proving the
    // path was not added to `grpc::authn::is_exempt`.
}

#[tokio::test]
async fn get_service_info_reports_the_enabled_capabilities() {
    // With a valid bearer and default config, assert the response's `service_info` is
    // populated (never `None`), `service == "iam"`, `version` is non-empty, and the capability
    // SET equals {iam.authz.cedar, iam.apikeys, iam.audit}.
}

#[tokio::test]
async fn the_grpc_and_http_transports_describe_the_same_build() {
    // Build ONE AppState, serve both surfaces from it, and assert `service`, `version` and the
    // capability SET are identical. This is what stops D4's two code paths drifting.
}

#[tokio::test]
async fn a_disabled_capabilitys_rpc_returns_unimplemented() {
    // With `cfg.audit.query_enabled = false`, `AuditService.ListAuditEntries` must return
    // `Code::Unimplemented` (the service is not registered at all).
    // With `cfg.authz.admin_enabled = false`, `AuthorizationService.ListPolicies` must return
    // `Code::Unimplemented` while `IsAuthorized` must NOT — it stays mounted for the gateway.
    // With `cfg.api_keys.management_enabled = false`, `ServiceAccountService.ListApiKeys` must
    // return `Code::Unimplemented` while `ListServiceAccounts` must not.
}
```

Fill each body following `tests/grpc_audit.rs:95-160`'s exact harness shape (it builds `AppState::new(db, &support::test_config(&idp))`, spawns the router over an ephemeral listener, and dials it with a generated client). Do not invent a new harness.

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
CI=1 cargo nextest run -p paigasus-iam --test grpc_service_info
```

Expected: FAIL — `ServiceInfoServiceClient` has nothing to talk to; `GetServiceInfo` returns `Unimplemented` because no server implements it.

- [ ] **Step 3: Implement the gRPC service**

Create `rs/crates/services/paigasus-iam/src/adapters/grpc/service_info.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `ServiceInfoService`: IAM's capability descriptor over gRPC (ADR-0020, SMA-505).
//!
//! Bearer-enforced automatically — `AuthLayer` covers every `:path` absent from
//! `grpc::authn::is_exempt`, and this one is deliberately not added there. No authorization
//! action is checked, matching the HTTP route.
//!
//! Shares `AppState.capabilities.descriptor()` with `adapters::http::service_info`, so the two
//! transports cannot describe different builds (pinned by `tests/grpc_service_info.rs`).

use std::time::Instant;

use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::common::v1::service_info_service_server::ServiceInfoService;
use paigasus_proto::paigasus::common::v1::{GetServiceInfoRequest, GetServiceInfoResponse};
use tonic::{Request, Response, Status};

use crate::adapters::http::AppState;

pub struct ServiceInfoGrpc {
    state: AppState,
}

impl ServiceInfoGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ServiceInfoService for ServiceInfoGrpc {
    /// `service_info` is ALWAYS populated. The proto requires clients to treat an absent
    /// `service_info` as an error rather than as "no capabilities", so a `None` here would be
    /// a server bug, not a representable state.
    async fn get_service_info(&self, _request: Request<GetServiceInfoRequest>) -> Result<Response<GetServiceInfoResponse>, Status> {
        let started = Instant::now();
        let result = Ok(Response::new(GetServiceInfoResponse {
            service_info: Some(self.state.capabilities.descriptor()),
        }));
        record_grpc("ServiceInfo", "GetServiceInfo", started, &result);
        result
    }
}
```

- [ ] **Step 4: Register it, and make `AuditService` conditional**

In `adapters/grpc/mod.rs`, add `pub mod service_info;` and update `router`:

```rust
pub async fn router(state: AppState, timeout: std::time::Duration) -> TonicRouter<Stack<AuthLayer, Identity>> {
    let (_reporter, health) = health_service().await;
    let audit_enabled = state.capabilities.audit_query;
    let mut router = Server::builder()
        .timeout(timeout)
        .layer(AuthLayer::new(state.clone()))
        .add_service(health)
        .add_service(TenancyServiceServer::new(TenancyGrpc::new(state.clone())))
        .add_service(AuthnServiceServer::new(AuthnGrpc::new(state.clone())))
        .add_service(AuthorizationServiceServer::new(AuthzGrpc::new(state.clone())))
        .add_service(ServiceAccountServiceServer::new(ServiceAccountGrpc::new(state.clone())))
        // SMA-505: always served — the descriptor is how a client learns what the rest of this
        // server offers, so it can never itself be capability-gated.
        .add_service(ServiceInfoServiceServer::new(ServiceInfoGrpc::new(state.clone())));
    // `AuditService` is WHOLLY within `iam.audit`, so it is not registered at all when the
    // capability is off — a client then gets `UNIMPLEMENTED`, exactly as it would from a build
    // predating the service. `add_service` returns `Self`, so this does not disturb the
    // concrete `TonicRouter<Stack<AuthLayer, Identity>>` return type.
    if audit_enabled {
        router = router.add_service(AuditServiceServer::new(AuditGrpc::new(state)));
    }
    router
}
```

Add the two imports: `use paigasus_proto::paigasus::common::v1::service_info_service_server::ServiceInfoServiceServer;` and `use service_info::ServiceInfoGrpc;`.

- [ ] **Step 5: Guard the capability-scoped RPCs**

`AuthorizationService` and `ServiceAccountService` each bundle must-stay RPCs with capability-scoped ones, so they cannot be unmounted wholesale. Add a helper to each of the two adapter modules and call it as the first line of the scoped RPCs.

In `adapters/grpc/authz.rs`:

```rust
/// SMA-505: policy/role-grant ADMINISTRATION is gated by `authz.admin_enabled`. `IsAuthorized`
/// is deliberately not — it is the gateway's per-request primitive, and no capability toggle
/// may break it. `UNIMPLEMENTED` is what a client would get from a server that never registered
/// the RPC, so a disabled capability is indistinguishable from a build that never had it.
fn require_authz_admin(state: &AppState) -> Result<(), Status> {
    if state.capabilities.authz_admin {
        Ok(())
    } else {
        Err(Status::unimplemented("capability iam.authz.cedar is not enabled on this service"))
    }
}
```

Call `require_authz_admin(&self.state)?;` as the first statement of `put_policy`, `delete_policy`, `list_policies`, `grant_role`, `revoke_role` and `list_role_grants`. Do **not** add it to `is_authorized`.

In `adapters/grpc/service_accounts.rs`, add the equivalent `require_apikey_management` returning `Status::unimplemented("capability iam.apikeys is not enabled on this service")`, and call it as the first statement of `issue_api_key`, `revoke_api_key` and `list_api_keys` only — never the four service-account lifecycle RPCs.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
CI=1 cargo nextest run -p paigasus-iam --retries 2
```

Expected: PASS, including the existing `grpc_authz.rs`, `grpc_audit.rs` and `api_keys_grpc.rs` suites — all flags default to `true`, so every RPC stays available.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): serve the IAM descriptor over grpc and gate scoped rpcs (SMA-505)"
```

---

### Task 6: Gateway config, capability predicate and the streaming rejection

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs` (`GatewayConfig`, the `Defaults` mirror struct)
- Create: `rs/crates/services/paigasus-gateway/src/service_info.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/lib.rs` (declare `pub mod service_info;`)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (`StreamingDisabled` + `param` threading)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:77`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`AppState.stream_enabled` + 5 construction sites)
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs`, `tests/metrics.rs` (AppState construction)
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example`

**Interfaces:**
- Consumes: `paigasus_service_info::descriptor` (Task 1).
- Produces:
  - `paigasus_gateway::config::GatewayConfig.stream_enabled: bool`
  - `paigasus_gateway::service_info::{SERVICE, VERSION}` and `Capabilities { pub chat_stream: bool }` with `from_config`, `enabled`, `descriptor` — the same shape as IAM's
  - `AppState.stream_enabled: bool`
  - `GatewayError::StreamingDisabled`

- [ ] **Step 1: Add the dependency and the config flag**

Add to `rs/crates/services/paigasus-gateway/Cargo.toml`:

```toml
# `service_info` — the shared ServiceInfo builder + HTTP DTO (SMA-505).
paigasus-service-info = { workspace = true }
```

In `config.rs`, add to `GatewayConfig` (top-level, matching its sibling `stream_idle_timeout_secs`):

```rust
    /// SMA-505: whether streamed (SSE) chat completions are served. `false` rejects a request
    /// carrying `stream: true` with `400` and `param: "stream"` — the OpenAI idiom for an
    /// unsupported request parameter — and withdraws the `gateway.chat.stream` capability.
    /// Non-streaming requests are unaffected. `400` rather than `501` because OpenAI-compatible
    /// SDKs commonly retry 5xx, which would turn a deliberate configuration choice into
    /// repeated load.
    pub stream_enabled: bool,
```

**And to the hand-written `Defaults` mirror struct** (`config.rs:127-137`) plus `impl Default for Defaults` (`stream_enabled: true`). Omitting the mirror field fails figment extraction at **runtime**, not compile time — there is no compiler error to catch it.

- [ ] **Step 2: Write the failing tests**

Create `rs/crates/services/paigasus-gateway/src/service_info.rs` with the SPDX header and only a test module:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_proto::paigasus::common::v1::Capability;

    #[test]
    fn streaming_enabled_advertises_the_capability() {
        assert_eq!(Capabilities { chat_stream: true }.enabled(), vec![Capability::GatewayChatStream]);
    }

    /// AC 3 for the gateway: the flag off removes the key, and the descriptor still serializes
    /// the field as an empty array rather than dropping it.
    #[test]
    fn streaming_disabled_advertises_nothing() {
        assert!(Capabilities { chat_stream: false }.enabled().is_empty());
        let info = Capabilities { chat_stream: false }.descriptor();
        assert!(info.capabilities.is_empty());
        assert_eq!(info.service, "gateway");
    }

    #[test]
    fn the_descriptor_names_this_crates_build_version() {
        let info = Capabilities { chat_stream: true }.descriptor();
        // `env!` expanded HERE, not read back from the module's `VERSION` const — see the
        // identical assertion in paigasus-iam's `service_info` tests for why that distinction
        // is what gives this test content.
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        let core = info.version.split(['-', '+']).next().expect("a version always has a core");
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "version core must be major.minor.patch: {}", info.version);
        assert!(
            parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "version core must be numeric: {}",
            info.version
        );
    }

    #[test]
    fn every_advertised_string_is_a_registered_capability_key() {
        for key in &Capabilities { chat_stream: true }.descriptor().capabilities {
            assert!(Capability::from_wire_key(key).is_some(), "{key} is not in the registry");
        }
    }
}
```

- [ ] **Step 3: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo test -p paigasus-gateway --lib service_info
```

Expected: FAIL — `cannot find type 'Capabilities' in this scope`.

- [ ] **Step 4: Implement the predicate**

Prepend to `rs/crates/services/paigasus-gateway/src/service_info.rs`:

```rust
//! The gateway's capability projection — the same shape as IAM's `service_info`, over
//! `GatewayConfig`. `enabled()` is a pure function, so AC 3's descriptor half is a unit test
//! with no `AppState` and no network.

use paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo};

use crate::config::GatewayConfig;

/// The bare service slug, matching the prefix of this service's own capability keys.
pub const SERVICE: &str = "gateway";

/// This build's version — `env!` evaluated in THIS crate, so it is `paigasus-gateway`'s own
/// `Cargo.toml` version (AC 4). Reports `0.0.0` until release-plz is activated.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// `stream_enabled` -> `gateway.chat.stream`.
    pub chat_stream: bool,
}

impl Capabilities {
    #[must_use]
    pub fn from_config(cfg: &GatewayConfig) -> Self {
        Capabilities { chat_stream: cfg.stream_enabled }
    }

    #[must_use]
    pub fn enabled(&self) -> Vec<Capability> {
        if self.chat_stream { vec![Capability::GatewayChatStream] } else { Vec::new() }
    }

    #[must_use]
    pub fn descriptor(&self) -> ServiceInfo {
        paigasus_service_info::descriptor(SERVICE, VERSION, &self.enabled())
    }
}
```

Declare `pub mod service_info;` in `rs/crates/services/paigasus-gateway/src/lib.rs`.

- [ ] **Step 5: Thread `param` through the error envelope**

In `adapters/http/error.rs`, add the variant:

```rust
    /// Streaming is disabled by configuration and the request asked for it → 400. Carries
    /// `param: "stream"` so the client sees exactly which field was refused (SMA-505 D9).
    StreamingDisabled,
```

Change `parts()` to return a 5-tuple `(StatusCode, &'static str, Option<&'static str>, Option<&'static str>, &'static str)` — status, type, code, **param**, message. Every existing arm gains `None` in the new param position; the new arm is:

```rust
            GatewayError::StreamingDisabled => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("streaming_disabled"),
                Some("stream"),
                "Streamed completions are not enabled on this deployment.",
            ),
```

Update `into_response` to destructure five fields and set `param: param.map(str::to_owned)`.

Correct the `ErrorBody` doc comment, which currently asserts the opposite:

```rust
/// The body of the OpenAI error envelope. `param` names the request field at fault when there is
/// one — only `StreamingDisabled` sets it today (SMA-505 D9); every auth- and egress-path error
/// leaves it `null`, because no request field is at fault in those. `code` is a stable
/// machine-readable diagnostic or `null`. `r#type` serializes as `"type"`.
```

- [ ] **Step 6: Reject `stream: true` when disabled**

In `adapters/http/chat.rs`, immediately after `let stream = dto.stream;` (`:77`):

```rust
    // SMA-505 D9: streaming is a request PARAMETER, not a route, so a disabled capability is
    // enforced here rather than by unmounting. Checked before any egress call, so a refused
    // request never reaches the upstream or its rate limit.
    if stream && !state.stream_enabled {
        return GatewayError::StreamingDisabled.into_response();
    }
```

- [ ] **Step 7: Carry the flag on `AppState` and fix every construction site**

Add to `AppState` in `adapters/http/mod.rs`:

```rust
    /// SMA-505: whether streamed completions are served. `false` makes `chat` reject
    /// `stream: true` with `400` and withdraws `gateway.chat.stream` from the descriptor.
    pub stream_enabled: bool,
```

`AppState` is a public, literal-constructed struct, so the compiler will now error at **five** sites. Add `stream_enabled: cfg.stream_enabled` at `src/main.rs:49`, and `stream_enabled: true` at the four test sites: `adapters/http/mod.rs:206-212`, `tests/metrics.rs:85`, `tests/metrics.rs:135`, `tests/chat_proxy.rs:125`.

- [ ] **Step 8: Add the streaming-rejection integration test**

In `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs`, add:

```rust
/// SMA-505 AC 3, gateway side: with streaming disabled a `stream: true` request is refused with
/// `400` and `param: "stream"`, and the upstream is never called. Needs no database — this
/// crate's harness drives the router via `oneshot` against a fake IAM and a fake upstream.
#[tokio::test]
async fn a_stream_request_is_refused_when_streaming_is_disabled() {
    // Build the harness exactly as the neighbouring tests do, but with `stream_enabled: false`
    // on the AppState. POST /v1/chat/completions with `{"model": "gpt-4", "stream": true,
    // "messages": []}` and a valid bearer.
    // Assert: status == 400; the JSON body's `error.param` == "stream" and `error.code` ==
    // "streaming_disabled"; and the fake upstream recorded ZERO calls.
    //
    // Then repeat with `"stream": false` and assert the request still reaches the upstream —
    // otherwise this passes against an implementation that broke chat entirely.
}
```

Fill the body following the neighbouring tests' exact harness shape.

- [ ] **Step 9: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo nextest run -p paigasus-gateway
```

Expected: PASS, including every existing `chat_proxy.rs` and `metrics.rs` test — `stream_enabled` defaults to `true`.

- [ ] **Step 10: Document the flag**

In `rs/crates/services/paigasus-gateway/gateway.toml.example`, next to `stream_idle_timeout_secs`:

```toml
# Serve streamed (SSE) chat completions (default true). When false, a request carrying
# `stream: true` is rejected with 400 and `param: "stream"`, and the gateway.chat.stream
# capability is not advertised. Non-streaming completions are unaffected.
stream_enabled = true
```

- [ ] **Step 11: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/services/paigasus-gateway rs/Cargo.lock
git commit -m "feat(rs): add the gateway streaming toggle and capability projection (SMA-505)"
```

---

### Task 7: The gateway's `introspect_token` port method and `require_authenticated`

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs` (trait + real impl)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` (new middleware)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs:151-154` (metric description)
- Modify: the six `Iam` impls listed below

**Interfaces:**
- Consumes: nothing from Tasks 1–6 beyond `GatewayError`.
- Produces:
  - `Iam::introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError>`
  - `pub async fn require_authenticated(State(iam): State<Arc<dyn Iam>>, req: Request, next: Next) -> Response`

- [ ] **Step 1: Add the port method**

In `adapters/iam/client.rs`, add to the `Iam` trait:

```rust
    /// Introspect a caller-presented OIDC token (IAM's `AuthnService.Introspect`). **Bearer-
    /// EXEMPT**, exactly like [`Iam::introspect_api_key`]: the token is the request body, so no
    /// `authorization` metadata is attached.
    ///
    /// Used ONLY by the capability-discovery middleware, which must accept a console user's own
    /// session (ADR-0020 D4). The chat path never calls this.
    async fn introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError>;
```

Implement it on `IamClient` alongside `introspect_api_key` (`:96`), using `self.authn.clone().introspect(..)` with `IntrospectRequest { token: token.to_owned() }`, and import `IntrospectRequest, IntrospectResponse` from `paigasus_proto::paigasus::iam::v1`.

- [ ] **Step 2: Update all six `Iam` impls**

Adding a trait method breaks every implementor. Add `introspect_token` to each:

- `UnusedIam` (`src/adapters/http/mod.rs:151`) — `unreachable!("this route must never call IAM")`
- `ProbeIam` (`src/adapters/http/mod.rs:179`) — `unreachable!(..)`
- `FakeIam` (`src/adapters/http/auth.rs:260`) — a recorded, configurable outcome
- `UnusedIam` (`tests/metrics.rs:43`) — `unreachable!(..)`
- `AllowedIam` (`tests/metrics.rs:57`) — a permissive `Ok(..)`
- `FakeIam` (`tests/chat_proxy.rs:88`) — a recorded, configurable outcome

- [ ] **Step 3: Write the failing middleware tests**

In `adapters/http/auth.rs`'s test module:

```rust
    /// AC 2: an API key works on the discovery path, and NO authorization call is made — the
    /// fake's `is_authorized_self` panics, so reaching it fails the test loudly.
    #[tokio::test]
    async fn require_authenticated_accepts_an_api_key_without_authorizing() { /* … */ }

    /// AC 2 + ADR-0020 D4: a console user's OIDC token works. The API-key introspect is tried
    /// first and fails; the token introspect then succeeds.
    #[tokio::test]
    async fn require_authenticated_accepts_an_oidc_token() { /* … */ }

    /// D5's deliberate relaxation, and the one most likely to be "fixed" back into a 401 by a
    /// later reader. IAM returns `PermissionDenied` for a VALIDATED token whose identity has no
    /// local principal; on the discovery path that still counts as authenticated, because the
    /// descriptor is byte-identical for every caller and carries no per-principal data.
    #[tokio::test]
    async fn require_authenticated_accepts_a_validated_but_unprovisioned_identity() { /* … */ }

    /// The relaxation must NOT leak onto the chat path.
    #[tokio::test]
    async fn require_iam_auth_still_rejects_an_unprovisioned_identity() { /* … */ }

    #[tokio::test]
    async fn require_authenticated_rejects_a_missing_bearer_with_401() { /* … */ }

    /// Both introspections failing with a transport error is an IAM outage → 503, and the call
    /// is recorded so the `result="unavailable"` alert can see it.
    #[tokio::test]
    async fn require_authenticated_maps_an_unreachable_iam_to_503() { /* … */ }
```

- [ ] **Step 4: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo test -p paigasus-gateway --lib auth
```

Expected: FAIL — `cannot find function 'require_authenticated'`.

- [ ] **Step 5: Implement the middleware**

In `adapters/http/auth.rs`:

```rust
/// Authenticate a capability-discovery request. Unlike [`require_iam_auth`] this performs NO
/// authorization: discovery must not be gated on a permission, or a caller who legitimately
/// cannot invoke models could never learn that streaming exists — and ADR-0020 D4 forbids
/// provisioning a service credential for the console.
///
/// ## Why both introspections are tried, rather than branching on the token prefix
/// IAM's API-key prefix is an operator knob (`api_keys.key_prefix`), and the gateway has no
/// visibility of its value. Branching on a hardcoded `pgs_sk_` would silently route every
/// service-account key to the OIDC path — and reject it — for any operator who changed that
/// setting, with no boot error. Mirroring the prefix into `GatewayConfig` would instead create a
/// must-match-or-break coupling between two services' configs. Trying both costs one extra RPC
/// on the OIDC path of a low-frequency, client-cached call, and cannot drift.
///
/// ## Why an unprovisioned identity is accepted
/// IAM's `Introspect` resolves with `Provisioning::Disabled`, so a VALIDATED token whose
/// `(issuer, subject)` has no local principal comes back `PermissionDenied`. IAM's own HTTP
/// middleware JIT-provisions instead, so rejecting here would make gateway discovery succeed or
/// fail purely on whether the console happened to call IAM first — breaking exactly the lazy
/// in-user-request flow ADR-0020 D4 specifies. The descriptor is byte-identical for every
/// caller and exposes no per-principal data, so accepting widens nothing. This relaxation is
/// scoped to THIS middleware; `require_iam_auth` is unchanged.
pub async fn require_authenticated(State(iam): State<Arc<dyn Iam>>, req: Request, next: Next) -> Response {
    let Some(token) = bearer(req.headers()) else {
        return GatewayError::MissingBearer.into_response();
    };

    let started = Instant::now();
    match iam.introspect_api_key(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return next.run(req).await;
        }
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => record_iam_call("introspect", iam_result(&err), started),
    }

    let started = Instant::now();
    match iam.introspect_token(&token).await {
        Ok(_) => {
            record_iam_call("introspect_token", "ok", started);
            next.run(req).await
        }
        // A validated-but-unprovisioned identity — see the doc comment. Recorded as "denied"
        // rather than "ok": IAM did reject the RPC, and conflating it with success would hide a
        // genuine provisioning problem from the dashboard.
        Err(IamError::Rpc(ref status)) if status.code() == Code::PermissionDenied => {
            record_iam_call("introspect_token", "denied", started);
            next.run(req).await
        }
        Err(err) => {
            let mapped = introspect_error(err.clone_for_mapping());
            record_iam_call("introspect_token", iam_result(&err), started);
            mapped.into_response()
        }
    }
}
```

If `IamError` is not `Clone`, compute the label *before* mapping (`let label = iam_result(&err); let mapped = introspect_error(err);`) and drop `clone_for_mapping` — do not add a `Clone` impl to `IamError` for this.

- [ ] **Step 6: Update the metric description**

`:observability-drift` only checks that metric *names* resolve, so a new label value going undocumented is invisible to CI. Update `describe_gateway_metrics` in `src/main.rs:151-154` in this same commit:

```rust
    describe_counter!(
        names::GATEWAY_IAM_CALLS_TOTAL,
        "Calls the gateway's auth middleware makes to IAM (introspect/introspect_token/authorize), labeled by operation and result."
    );
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo nextest run -p paigasus-gateway
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): authenticate gateway discovery without authorizing (SMA-505)"
```

---

### Task 8: The gateway's `/v1/service-info` route

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/service_info.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`router`)
- Create: `rs/crates/services/paigasus-gateway/tests/service_info.rs`

**Interfaces:**
- Consumes: `require_authenticated` (Task 7), `AppState.stream_enabled` and `service_info::Capabilities` (Task 6), `paigasus_service_info::{ROUTE, ServiceInfoDto}` (Task 1).
- Produces: nothing later tasks depend on.

- [ ] **Step 1: Write the failing integration test**

Create `rs/crates/services/paigasus-gateway/tests/service_info.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-505 AC 1/2/3 for the gateway's descriptor. No database and no Docker — this crate's
//! harness drives the router via `oneshot` against a fake IAM.

// Assert, following tests/chat_proxy.rs's harness shape:
//
// 1. No Authorization header -> 401 (AC 2).
// 2. Valid API key -> 200; body is {"service":"gateway","version":<non-empty>,
//    "capabilities":["gateway.chat.stream"]}, compared as a SET (AC 1).
// 3. Valid OIDC token (api-key introspect fails, token introspect succeeds) -> 200, against a
//    fake whose `is_authorized_self` panics — proving discovery makes no authorization call.
// 4. Validated-but-unprovisioned identity (token introspect returns PermissionDenied) -> 200
//    (ADR-0020 D4), and the SAME credential against /v1/chat/completions -> 401.
// 5. stream_enabled = false -> 200 with "capabilities":[] — emitted as an empty array, never
//    omitted (AC 3 + SMA-499 § 2.7).
// 6. IAM unreachable (IamError::Connect from both introspections) -> 503, and
//    gateway_iam_calls_total{operation="introspect_token",result="unavailable"} increased by 1.
//    Read the metric by PARSING A NUMBER out of the rendered output — never `contains()` on a
//    `# TYPE` line, which is emitted whether or not the counter was ever incremented.
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo nextest run -p paigasus-gateway --test service_info
```

Expected: FAIL — `/v1/service-info` 404s.

- [ ] **Step 3: Write the handler**

Create `rs/crates/services/paigasus-gateway/src/adapters/http/service_info.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `GET /v1/service-info` — the gateway's capability descriptor (ADR-0020, SMA-505).
//!
//! Protected by [`super::auth::require_authenticated`], NOT by `require_iam_auth`: discovery
//! requires a valid credential but no authorization action. The gateway serves the descriptor
//! over HTTP only — it has no tonic server, and giving it one would mean a second listening
//! port plus Helm and ingress entries for every self-hoster (SMA-499 D3).

use axum::{Json, extract::State};
use paigasus_service_info::ServiceInfoDto;

use super::AppState;
use crate::service_info::Capabilities;

pub async fn get_service_info(State(state): State<AppState>) -> Json<ServiceInfoDto> {
    let caps = Capabilities { chat_stream: state.stream_enabled };
    Json(ServiceInfoDto::from(&caps.descriptor()))
}
```

Declare `pub mod service_info;` in `adapters/http/mod.rs`.

- [ ] **Step 4: Mount it in its own protected group**

In `adapters/http/mod.rs`'s `router`, add a second `route_layer` group — the descriptor must not inherit the chat group's `require_iam_auth` or its body limit:

```rust
    // SMA-505: its own group, because discovery authenticates but does not authorize, and needs
    // no body limit (it is a GET). `route_layer` keeps the middleware off unmatched paths, so a
    // 404 is still a 404 rather than a credential challenge.
    let discovery = Router::new()
        .route(paigasus_service_info::ROUTE, get(service_info::get_service_info))
        .route_layer(axum::middleware::from_fn_with_state(state.iam.clone(), require_authenticated));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(protected)
        .merge(discovery)
        .layer(paigasus_observability::http_metrics_layer("gateway"))
        .with_state(state)
```

Add `require_authenticated` to the `pub use auth::…` re-export line.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs && cargo nextest run -p paigasus-gateway
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): serve the gateway capability descriptor over http (SMA-505)"
```

---

### Task 9: Full verification

**Files:** none — this task changes nothing unless a gate fails.

**Interfaces:** consumes everything; produces nothing.

- [ ] **Step 1: Run the Docker-gated suites with `CI=1`**

This is R6's mitigation and it is not optional. Without `CI=1` these suites `return` early on a machine without a daemon and report a pass in under a second, leaving AC 3's surface half entirely unproven.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
CI=1 cargo nextest run -p paigasus-iam --retries 2
```

Expected: PASS with a real, non-trivial runtime (tens of seconds), not ~0.7s. If it finishes instantly, Docker is not running — start it and re-run. `--retries 2` absorbs this crate's known container-startup flakiness.

- [ ] **Step 2: Run formatting and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 3: Run the full CI graph exactly as CI does**

Per-project Moon tasks do NOT run the repo-level gates, so this is the only command that proves the branch is green:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed "N failed", find the actual failing action with:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
jq '.actions[]|select(.status=="failed")|.label' .moon/cache/ciReport.json
```

Likely failures and their fixes:
- `repo:affected-smoke` — `paigasus-service-info-rs` missing from the `contracts->proto` expected CSV (Task 1 Step 8).
- `contracts:fmt` — `buf format -w` was not run (Task 2 Step 2).
- a codegen-drift failure — `buf generate` was not run directly, or a tree was not committed (Task 2 Step 2/3).
- `repo:deny` / `repo:machete` — should be quiet; this change adds no external dependencies.

- [ ] **Step 4: Verify the descriptor's claims against the running services by hand**

A last sanity check that the two transports agree in a real process, not just in tests. Start IAM with a config setting `audit.query_enabled = false`, then:

```bash
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/v1/service-info
```

Expected: `capabilities` contains `iam.authz.cedar` and `iam.apikeys` but not `iam.audit`, and `GET /v1/audit` returns 404.

- [ ] **Step 5: Commit anything the gates rewrote**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma505
git status --short
# Commit only if a gate rewrote a generated file (e.g. .github/CODEOWNERS):
git add -A && git commit -m "chore(repo): sync generated files after the SMA-505 change (SMA-505)"
```

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: D1/D2 → Tasks 3–6; D3 → Task 5 Step 5; D4 → Tasks 4, 5, 8; D5 → Tasks 4, 7, 8; D6 → Tasks 3, 6 (`VERSION` consts) and § 6.4's three assertions → Task 1 Step 4, Task 3 Step 3, Task 6 Step 2; D7 → Task 1; D8 → Task 2; D9 → Task 6 Steps 5–6. § 4.1's config table → Tasks 3, 6; § 4.2's unmount table → Tasks 4, 5, 6, including system-policy retirement (Task 4 Step 5). § 6.1's two-layer split → pure tests in Tasks 3/6, Docker-gated in Tasks 4/5. § 6.2 → Tasks 4, 5, 7, 8. § 6.5 transport agreement → Task 5 Step 1. § 7's gates → Tasks 1, 2, 9. R3 → Task 4 Step 6. R6 → Task 9 Step 1.

**Type consistency.** `Capabilities` has field names `authz_admin` / `apikeys_management` / `audit_query` in IAM and `chat_stream` in the gateway, used identically in Tasks 3–8. `descriptor(service, version, capabilities)` keeps one signature throughout. `ServiceInfoDto` is constructed only via `From<&ServiceInfo>`. `parts()` becomes a 5-tuple in exactly one place (Task 6 Step 5) and `into_response` is updated in the same step.

**Known gap, deliberate.** Tasks 5, 7 and 8 give test bodies as specified assertion lists rather than literal code, because each must reuse an existing harness (`tests/grpc_audit.rs`'s listener wiring, `tests/chat_proxy.rs`'s fake IAM) whose exact constructors differ per file. Each step names the file and line range to copy from. Do not invent a new harness.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-15-sma-505-serve-service-info-descriptor.md`.
