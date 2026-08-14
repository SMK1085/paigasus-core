# SMA-499 — `ServiceInfo` capability descriptor in `common/v1`

Linear: [SMA-499](https://linear.app/smaschek/issue/SMA-499/contracts-serviceinfo-capability-descriptor-in-commonv1)
ADR: [ADR-0020 — Service capability discovery](https://app.notion.com/p/3bb830e8fbaa8113b9f3da910893aaa8) (accepted 2026-08-13)
Design context: [Frontend Architecture Scoping](https://app.notion.com/p/3bb830e8fbaa8188adf2c6bfebf67ed5) § 3–4
Blocks: [SMA-505](https://linear.app/smaschek/issue/SMA-505) (iam+gateway serve the descriptor),
[SMA-509](https://linear.app/smaschek/issue/SMA-509) (console capability-discovery client)
**Ordered after: [SMA-498](https://linear.app/smaschek/issue/SMA-498) — a hard constraint, see § 10.**

## 1. Context

### 1.1 What exists today

`contracts/proto/paigasus/iam/v1/iam.proto` opens with a stub:

```proto
// Placeholder so the package generates a concrete type in all three languages.
// Replaced by real messages in M1; carries a service PRN string for now.
message ServiceInfo {
  string prn = 1;
}
```

Nothing hand-written consumes it. Its only references anywhere in the repo are the three committed
generated trees plus three historical plan and spec documents.

`contracts/proto/paigasus/common/v1/` holds `audit.proto` and `auditable_example.proto`. Neither
declares a service, and `rs/crates/libs/paigasus-proto/src/lib.rs` records that fact in a comment.

### 1.2 What this issue delivers

ADR-0020 decision 2 grows the stub into a real descriptor in `common/v1`: service name, version, and
a list of capability keys. This issue builds **only the contract** — the message, the registry, the
RPC and route shape, and the Rust and TypeScript transform helpers. No service serves it until
SMA-505. One caveat to that boundary is recorded honestly in § 4.2 and § 9.

## 2. Findings that constrain the design

Seven facts were established experimentally in the worktree before designing. Each is recorded with
its evidence, because five of them contradict a plain reading of the issue's scope notes.

### 2.1 Deleting the placeholder reds `contracts:breaking`

AC4 offers "replaced or superseded". Replacement was probed by deleting the message and running the
gate:

```
proto/paigasus/iam/v1/iam.proto:1:1:Previously present message "ServiceInfo" was deleted from file.
```

`contracts/buf.yaml` uses the `FILE` breaking category, which includes `MESSAGE_NO_DELETE`. The file
already carries a documented exception for `FIELD_NO_DELETE`, but that exception is field-scoped and
does not reach message deletion. **Therefore: supersede, do not delete** (D4).

### 2.2 The three languages disagree about `option deprecated`

`option deprecated = true` on the placeholder was probed through all four buf gates — `lint`,
`format --exit-code`, `breaking`, `generate` — all of which pass. The generated output differs
per language:

| Language | Emitted for a deprecated message |
| --- | --- |
| Rust (prost) | **Nothing.** The struct is byte-identical; only `FILE_DESCRIPTOR_SET` changes |
| TypeScript (protobuf-es) | `@deprecated` JSDoc on both `ServiceInfo` and `ServiceInfoSchema` |
| Python (betterproto2) | A runtime `warnings.warn("ServiceInfo is deprecated", DeprecationWarning)` in `__post_init__` |

The Python behaviour is a genuine runtime change rather than an annotation. It is inert here: no
Python code constructs the type, and the `py` workspace configures no `filterwarnings = error`.

Because prost emits no `#[deprecated]`, AC4's "without breaking the generated Rust that references
it" is satisfied by construction rather than by care.

### 2.3 A service in `common/v1` emits a tonic file for the first time

A probe proto declaring `service ServiceInfoService` in `paigasus.common.v1` generated
`rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.tonic.rs` — a
file that does not exist today.

`lib.rs` currently asserts the opposite:

```rust
// Only the prost file: audit.proto declares no service, so
// neoeinstein-tonic emits no `.tonic.rs` for this package.
include!("generated/paigasus/common/v1/paigasus.common.v1.rs");
```

That comment becomes false, and without a second `include!` the generated client never compiles into
the crate. This is the single easiest step to miss in the whole change.

### 2.4 `:affected-smoke` is not at risk

`ci/affected-graph/run.sh:106-107` asserts that a `contracts` proto edit affects exactly
`contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs`,
by strict equality. This change adds no `dependsOn` edge in any workspace, so the affected set is
unchanged. The guard needs no update.

### 2.5 `ENUM_VALUE_NO_DELETE` is active, and SMA-498 already fixes it

`contracts/buf.yaml:15-26` swaps only the `FIELD_NO_DELETE` siblings for their reserve-tolerant
counterparts. No `ENUM_VALUE_*` rule appears anywhere in `contracts/`, so `FILE`-category
`ENUM_VALUE_NO_DELETE` is active and rejects even a `reserved`-and-retired enum value.

Because D1 makes the registry an enum, **a mis-spelled capability key would be permanent from first
commit** — retractable only by a two-PR waiver sequence against `main`. `iam.apikeys` versus
`iam.api.keys` is exactly the word-boundary judgement that gets revisited.

SMA-498 reached the identical conclusion for the error registry and has **already landed the fix** on
its branch (commit `c9cb12d`, "let a reserved enum value pass buf breaking"), citing an ADR-0019
amendment:

```yaml
    - ENUM_VALUE_NO_DELETE_UNLESS_NAME_RESERVED
    - ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED
  except:
    - FIELD_NO_DELETE
    - ENUM_VALUE_NO_DELETE
```

This spec does **not** duplicate that change. It takes a hard ordering dependency on SMA-498
instead — see D2 and § 10.

### 2.6 `google.api.http` breaks the TypeScript output

D3 specifies the HTTP route in prose. The machine-readable alternative —
`option (google.api.http) = {get: "/v1/service-info"}` — was probed, since `contracts/buf.yaml:6-7`
already pins `buf.build/googleapis/googleapis` and an *option* is a different mechanism from the
*type* reference SMA-498 § 2.1 ruled out.

`buf generate` exits 0 and emits, into `service_info_pb.ts`:

```ts
import { file_google_api_annotations } from "../../../google/api/annotations_pb.js";
```

That file is not generated. The failure is identical to SMA-498's type-reference finding: silent at
generation time, fatal at `tsc`. Generating the googleapis module to satisfy it would pull
`annotations.proto` and its transitive dependencies into all three committed binding trees to
express one route string.

**Therefore: the HTTP contract stays prose in the file doc comment** (D3).

### 2.7 protojson omits empty repeated fields

Canonical proto3 JSON omits fields holding their default value unless the serializer is explicitly
told otherwise. For `ServiceInfo` that means a service advertising **no** capabilities serializes as:

```json
{"service":"gateway","version":"1.2.3"}
```

with `capabilities` absent entirely. A hand-written console client doing
`info.capabilities.includes(k)` then throws `TypeError` rather than rendering "feature off" — which
breaks AC3's "absent capability → feature off" on the exact transport the console uses for the
gateway. D3 fixes this on both sides.

## 3. Decisions

### D1 — The registry is a proto `enum`; the wire stays `repeated string`

ADR-0020 decision 3 says capability keys are "strings, append-only, with the registry maintained as a
doc comment in the proto". Read literally that means a structured comment block and no declarations.
Rejected because it generates *nothing*: AC1 ("generates to all three languages") would be true of
the message and vacuous of the registry, the console and SDK would get no symbols, and any future
drift gate would have to parse proto comments.

The alternative that genuinely competes is not the comment block but a hand-written constant table —
`pub const IAM_AUDIT: &str = "iam.audit";` in `capability.rs`. It gives Rust identical symbols at
zero proto-churn cost. It is rejected because the constants would be a *second copy* of the registry,
maintained by hand alongside the doc comment, with nothing keeping the two in step — the "three
unlinked places" failure ADR-0019 cites from the observability metrics. The enum makes the proto the
single source and derives every language's symbols from it.

`Capability` is declared as an enum and is a **registry, never a wire type**. No field in any `.proto`
has type `Capability`. `ServiceInfo.capabilities` is `repeated string`, exactly as ADR-0020 requires.

This preserves the ADR's forward-compatibility argument, which is entirely about the wire: a new key
must not force a bindings regen on every consumer before it can be *sent*, and an old console must
receive something it can log verbatim. Both hold.

The cost that survives: adding a key regenerates all three binding trees and the codegen-drift gate
requires committing that churn. Accepted — keys are added rarely.

### D2 — `buf breaking` is a bonus, not the guard; retractability comes from SMA-498

Because the registry is an enum, `buf breaking` now catches a *deleted enum value*. That is a welcome
side effect, not the protection. It cannot see the dotted wire strings, cannot tell whether a key is
still advertised by a service or consumed by the console, and does not run against non-proto sources.
The file's doc comment says so explicitly, so nobody mistakes a green `:breaking` for a checked
registry.

Per 2.5, the enum form also makes a mis-spelled key permanent unless `buf.yaml` carries the
reserve-tolerant enum rules. **This spec takes that as an ordering dependency rather than duplicating
the config**: SMA-498 must merge first, and SMA-499 rebases onto it. Stated as a hard constraint in
§ 10 because it is the one thing that can make this change unsafe in a way review will not catch.

### D3 — A shared RPC in `common/v1`, plus a fully specified HTTP route

`paigasus-iam` runs a tonic server; `paigasus-gateway` does not. The gateway is axum-only
(`adapters/http/mod.rs:73-81` — exactly `/healthz`, `/readyz`, `/v1/chat/completions`) and merely
*dials* IAM over gRPC through a tonic `Channel` (`adapters/iam/client.rs:22`).

A gRPC-only contract would oblige SMA-505 to give the gateway a tonic server, a second listening
port, and matching Helm and ingress entries — a new deployment surface for every self-hoster, which
exceeds ADR-0020's "deliberate, small tax on service authors".

`common/v1` therefore declares one shared service:

```proto
service ServiceInfoService {
  rpc GetServiceInfo(GetServiceInfoRequest) returns (GetServiceInfoResponse);
}
```

The `GetServiceInfoRequest` / `GetServiceInfoResponse` wrappers exist only to satisfy buf lint
STANDARD's `RPC_REQUEST_STANDARD_NAME` and `RPC_RESPONSE_STANDARD_NAME`. They carry no meaning.

Two servers both serving `paigasus.common.v1.ServiceInfoService` is not a name collision: gRPC
service names are scoped per server, and `paigasus-proto` yields one generated server trait that each
service implements independently. The real consequence of a `service{}` in `common/v1` is that
`FILE`-category `SERVICE_NO_DELETE` and `RPC_NO_DELETE` now bind this package permanently — the
intended append-only posture.

**The HTTP equivalent, specified normatively in the file doc comment:**

| Aspect | Contract |
| --- | --- |
| Route | `GET /v1/service-info` |
| Success | `200`, `Content-Type: application/json` |
| Body | Canonical protojson of the **bare `ServiceInfo`**, not the RPC response wrapper |
| Empty fields | The encoder **MUST** emit default values, so `capabilities` is always present, as `[]` (2.7) |
| Auth | **MUST** require authentication equivalent to any other authenticated route of that service. No new unauthenticated surface (ADR-0020 D4, SMA-505 AC2) |
| Errors | The service's existing error envelope and status codes, unchanged — per ADR-0019, envelopes are per-surface |

Clients **MUST** additionally treat an absent `capabilities` key as an empty list, so a
non-conforming encoder degrades to "all features off" rather than a `TypeError`. Belt and braces:
the server is required to emit it, and the client is required not to depend on that.

The bare-`ServiceInfo` body means the two transports return different JSON — `{...}` versus
`{"serviceInfo":{...}}`. This asymmetry is deliberate: the wrapper is a lint artefact, and the
console already has two distinct call sites, so nothing handles both shapes at one site.

**Which service serves which:** a service **MUST** expose the descriptor on at least one transport,
and **SHOULD** use the one its clients already speak. IAM serves gRPC; the gateway serves HTTP.
Serving both is permitted, never required. SMA-505 makes the final call per service.

Note for SMA-505: the gateway's existing `require_iam_auth` introspects a `pgs_sk_` API key, not a
console session, so neither of its current route groups fits `/v1/service-info` as-is. The contract
states the requirement; the mechanism is SMA-505's.

### D4 — The placeholder is superseded in place, not replaced

Per 2.1 the message stays. It keeps its name and field 1, gains `option deprecated = true`, and its
doc comment is rewritten to name `paigasus.common.v1.ServiceInfo`, cite ADR-0020, and state **why it
still exists** — that `MESSAGE_NO_DELETE` forbids removing it. Without that last sentence the next
reader finds a dead message and assumes it was forgotten.

The deprecation is **permanent**, not a staging step: the doc comment says so. Retiring it would take
the two-PR waiver sequence described in 2.1, which buys nothing once the message is deprecated,
unreferenced, and documented. Recorded so the next reader does not re-derive the analysis.

### D5 — Two mapping rules and one client contract, stated normatively in the proto

- **Key mapping:** strip the `CAPABILITY_` prefix, lowercase, replace `_` with `.`.
- **Key grammar:** `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$` — lowercase alphanumeric segments separated by `.`.

The grammar is a consequence of the mapping, not an independent choice: because `_` → `.` consumes
*every* underscore, a key can never contain a hyphen or underscore inside a segment.
`gateway.chat-stream` is unreachable through the rule. All four keys in the initial registry comply.
Stating the grammar prevents a future key being proposed that the transform cannot express.

Every value's doc comment repeats its resulting literal verbatim as the first token, followed by what
the capability means. Doc comments reach all three generated languages (Rust `///`, TS JSDoc, Python
docstring), so the comment is the human-readable contract and the rule is the machine-readable one.

The file doc comment states the client contract in the exact terms AC3 requires:

- **An unknown key is ignored.**
- **An absent capability means the feature is off.**
- Capability gating is **cosmetic**, exactly like the console's `can()` helper. The server remains
  authoritative and must handle an unimplemented call gracefully. A client treating the capability
  list as a security boundary would be wrong.
- The registry is **append-only**; removing a value is a breaking change, and `buf breaking` is not
  what protects it (D2).
- The list is **unordered**, duplicates are permitted and MUST be ignored, and clients MUST tolerate
  both unknown and repeated entries. Build a set from it; do not index it.
- The vocabulary is **closed and central**: every key a Paigasus service advertises is registered
  here. There is no reserved vendor prefix, and a fork advertising `acme.custom.thing` is simply an
  unknown key that every conforming client ignores. If a vendor-extension convention is ever wanted,
  it is an ADR, not an implementation detail.
- **"Degraded" is not expressible in this message and must not be inferred from it.** ADR-0020's
  third UI state is derived entirely client-side, from a failed, timed-out, or stale-cache
  `GetServiceInfo` — never from the payload, which can only distinguish present from absent.

### D6 — `service` is a bare slug; `version` is SemVer and never gates a feature

`ServiceInfo.service` carries `iam` / `gateway`, not the `iam.paigasus.io` form that SMA-498's
`ErrorDomain` uses. It is the prefix of its own capability keys, so it should be self-consistent with
this file. SMA-498 is unmerged; coupling this field to its vocabulary would be premature. The overlap
is noted in prose so it is visible rather than accidental.

**The field is advisory, and must not be used as a cache key.** ADR-0020 D4 caches discovery under
`svcinfo:<service>`; that `<service>` **MUST** be the console's own deployment-configuration
identifier for the service it dialled, never the value the server reported. Keying on server-reported
data would let a misconfigured — or hostile — service poison another's cache entry. A mismatch
between the two is worth logging and nothing more.

`ServiceInfo.version` **MUST** be SemVer 2.0 and MAY carry pre-release and build metadata
(`1.4.0-rc1+abc123`). It exists for display and for ADR-0020's N-1-minor skew reporting, and is
**never an input to a feature decision** — the capability list is the only sanctioned input. Without
that rule someone eventually writes `if version >= "1.4"`, reintroducing precisely the version-skew
bug ADR-0020 exists to prevent. Clients **MUST** tolerate an unparseable value by suppressing skew
reporting rather than erroring.

### D7 — The Rust transform lives in `paigasus-proto`, derived rather than tabulated

`rs/crates/libs/paigasus-proto/src/capability.rs`, a hand-written module following the existing
`src/audit.rs` precedent, providing **inherent impls** on the generated enum (legal because the enum
is local to this crate — no extension trait):

```rust
Capability::IamAuthzCedar.as_wire_key()   // Some("iam.authz.cedar")
Capability::from_wire_key("iam.audit")    // Some(Capability::IamAudit)
```

Both directions are **derived** from prost's `as_str_name()` / `from_str_name()` by applying the D5
rule, not written as a four-arm match. A match would be the second copy of the registry that D1
exists to avoid. `as_str_name()` returns the full unmangled proto name, which is what makes the
derivation exact.

**Both directions return `Option`, and the two are exact inverses.** `as_wire_key` returns
`Option<String>`, yielding `None` for `Capability::Unspecified`. This is not fastidiousness:
`Unspecified` is prost's `Default`, so a `Default`-initialised or out-of-range-decoded value would
otherwise silently advertise the string `"unspecified"` to every console — a string D8 then refuses
to parse back. The sentinel exists only to satisfy buf's `ENUM_ZERO_VALUE_SUFFIX` lint rule and is
not a key any service advertises.

`String` rather than `&'static str`: a borrowed static would require a const table, which is the
second list this decision exists to avoid.

Without this module SMA-505 must invent the transform itself, and will be tempted to write
`"iam.audit"` as a literal where it builds the descriptor — exactly the drift the registry prevents.

### D8 — `from_wire_key` validates positively, before transforming

Listing forbidden characters is not sufficient. An implementer writing
`input.to_uppercase().replace('.', "_")` satisfies every obvious rejection and still resolves
`"ıam.audit"` (U+0131 dotless i), because `str::to_uppercase` folds it to `I`. Three of the four
registered keys begin with `i`, so the hole is directly reachable.

Therefore `from_wire_key`:

1. **Matches the D5 grammar positively** — `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$` — *before* any
   transformation. This rejects `_`, every uppercase form, empty and leading/trailing-dot inputs, and
   every non-ASCII homoglyph in one rule rather than four.
2. Uses `to_ascii_uppercase`, never `to_uppercase`.
3. Rejects `"unspecified"` explicitly (D7).

A lenient parser would quietly widen any future "advertised ⊆ registry" gate, letting a misspelled or
homoglyph key pass it.

### D9 — A TypeScript transform, and a type/value-split barrel export

Exporting `Capability` without a transform would hand the console `Capability.IAM_AUDIT === 3` and no
way to recover `"iam.audit"` — the literal-typing risk D7 exists to prevent, in the one place it is
most likely to bite: the console's gateway client is hand-written `fetch`, not a generated stub.

So `ts/packages/paigasus-proto/src/capability.ts` adds the ~6-line mirror, deriving the wire key from
the generated `CapabilitySchema` descriptor's value names by the same D5 rule.

This diverges from SMA-498, whose revised spec leaves TypeScript to the SDK issue and states the
barrel is *not* touched. The divergence is deliberate: error codes are produced by Rust and only
*read* in TypeScript, whereas capability keys are *compared* in TypeScript against a hand-written
client's string.

`ts/packages/paigasus-proto/src/index.ts` re-exports `ServiceInfoSchema`, `ServiceInfoService`,
`Capability` and the transform as **values**, and `ServiceInfo` as a **type**. `ts/tsconfig.base.json`
sets `verbatimModuleSyntax: true` and the barrel already splits `export {…}` from `export type {…}`;
a naive `export { ServiceInfo }` is a hard `tsc` error, since protobuf-es v2 emits `ServiceInfo` as a
type alias and `ServiceInfoSchema` / `Capability` / `ServiceInfoService` as values.

No Python transform: nothing in the Python workspace consumes capabilities today.

### D10 — ADR-0020 needs an amendment note before implementation

`CLAUDE.md` requires that significant choices get a Notion ADR before code. Two decisions here go
beyond accepted ADR-0020:

- **D1** adds generated enum symbols where decision 3 prescribes "registry maintained as a doc
  comment". Modest — D1 *keeps* the doc comments and only adds symbols — but it is a departure.
- **D3** is a genuinely new architectural decision, absent from ADR-0020 entirely: a shared
  `service{}` in `common/v1` plus a normative HTTP endpoint on every Paigasus service, carrying
  ingress, Helm and auth consequences for every self-hoster.

SMA-498 set the precedent, citing "ADR-0019 amendment A1.2" in the `buf.yaml` it landed. ADR-0020
needs the equivalent: an amendment recording D1 and D3, or a sign-off recorded against this spec.
**This is a human action on a Notion page and is not performed by the implementation.**

## 4. The contract

`contracts/proto/paigasus/common/v1/service_info.proto`, package `paigasus.common.v1`, **no imports**
(2.6), opening with the mandatory `// SPDX-License-Identifier: Apache-2.0` header. A separate file
rather than an addition to `audit.proto`, which is about auditable entities; the split also keeps the
TypeScript output in its own `service_info_pb.ts`.

### 4.1 Messages and service

```proto
message ServiceInfo {
  string service = 1;
  string version = 2;
  repeated string capabilities = 3;
}

message GetServiceInfoRequest {}
message GetServiceInfoResponse {
  ServiceInfo service_info = 1;
}

service ServiceInfoService {
  rpc GetServiceInfo(GetServiceInfoRequest) returns (GetServiceInfoResponse);
}
```

### 4.2 `Capability` — 4 values

Verbatim from ADR-0020 decision 2; nothing invented.

| Proto value | Wire key | Meaning |
| --- | --- | --- |
| `CAPABILITY_IAM_AUTHZ_CEDAR` | `iam.authz.cedar` | Cedar policy evaluation is available |
| `CAPABILITY_IAM_APIKEYS` | `iam.apikeys` | Service-account API key issuance is available |
| `CAPABILITY_IAM_AUDIT` | `iam.audit` | The audit log is queryable |
| `CAPABILITY_GATEWAY_CHAT_STREAM` | `gateway.chat.stream` | Streaming chat completions are available |

Numbered sequentially from 1, preceded by the mandatory `CAPABILITY_UNSPECIFIED = 0` sentinel.

**Known gap, and it crosses the issue boundary.** None of the four maps to a single boolean in either
service's config. `rs/crates/services/paigasus-iam/src/config.rs` offers `authz.enforce_tenancy:155`,
`audit.retention.enabled:319`, `authn.jit_provisioning:136`, `outbox.relay_enabled:358` and
`metrics.enabled:622`; none corresponds to a registered key. The gateway is worse —
`rs/crates/services/paigasus-gateway/src/config.rs` has only `stream_idle_timeout_secs`, no streaming
toggle at all, so `gateway.chat.stream` is a compile-time constant today.

SMA-505's AC3 requires a test that flips a config flag and asserts a key disappears. It will
therefore either derive these four from live state that is not a single flag, or **append a key,
reopening `contracts/`, regenerating all three trees, and editing § 6's literal test**. That is a
real crack in the "this issue is contract-only" boundary, stated plainly rather than discovered
later. Registering a speculative flag-shaped key *now* was rejected: in an append-only registry a
wrong guess is permanent, and appending later is cheap by construction.

### 4.3 File-level doc comment

States, normatively: the D5 mapping rule and grammar; that each value's doc comment repeats its
literal; the full client contract (unknown key ignored, absent capability means off, gating is
cosmetic, unordered, duplicates ignored, closed vocabulary, degraded is client-side only); that the
registry is append-only and `buf breaking` is not its guard (D2); the complete HTTP route contract
from D3's table, including the MUST-emit-defaults rule; and D6's rules for `service` and `version`.

## 5. Files touched

| Path | Change |
| --- | --- |
| `contracts/proto/paigasus/common/v1/service_info.proto` | new — SPDX header required |
| `contracts/proto/paigasus/iam/v1/iam.proto` | doc comment rewrite plus `option deprecated = true` (D4) |
| `rs/.../paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` | regenerated — prost emits one file per *package*, so the descriptor lands beside `AuditMetadata` |
| `rs/.../paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.tonic.rs` | **new (generated)** — first service in this package (2.3) |
| `rs/.../paigasus-proto/src/generated/paigasus/iam/v1/paigasus.iam.v1.rs` | regenerated — `FILE_DESCRIPTOR_SET` bytes only |
| `ts/.../paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts` | new (generated) |
| `ts/.../paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts` | regenerated — `fileDesc` plus `@deprecated` JSDoc |
| `py/.../paigasus_proto/generated/paigasus/common/v1/__init__.py` | regenerated — betterproto2 merges by package |
| `py/.../paigasus_proto/generated/paigasus/iam/v1/__init__.py` | regenerated — adds the `DeprecationWarning` (2.2) |
| `rs/.../paigasus-proto/src/capability.rs` | new, hand-written, SPDX header (D7) — implementation and tests |
| `rs/.../paigasus-proto/src/lib.rs` | `pub mod capability;`, the tonic `include!`, and the corrected comment (2.3) |
| `ts/packages/paigasus-proto/src/capability.ts` | new, hand-written, SPDX header (D9) |
| `ts/packages/paigasus-proto/src/capability.test.ts` | new (§ 6) |
| `ts/packages/paigasus-proto/src/index.ts` | re-exports, type/value split (D9) |
| `py/packages/paigasus-proto/tests/test_service_info_smoke.py` | new (§ 6) |

**No `contracts/buf.yaml` change** — the enum-value rules arrive with SMA-498 (D2, § 10). No
`Cargo.toml`, `buf.gen.yaml` or `moon.yml` change, and no new dependency in any workspace, so
`cargo-deny` needs no waiver and `cargo-machete` no allowlist entry.

## 6. Testing

**Rust, in `capability.rs`:**

- **Round-trip.** For every `Capability` variant except `Unspecified`,
  `from_wire_key(v.as_wire_key().unwrap()) == Some(v)`.
- **Sentinel.** `Capability::Unspecified.as_wire_key()` is `None`, and
  `from_wire_key("unspecified")` is `None`. Guards D7's `Default` hazard in both directions.
- **Grammar.** Every `as_wire_key()` output matches D5's regex — which subsumes non-empty,
  ASCII-lowercase, no `_`, no empty segment, no leading or trailing `.`.
- **The four literals, verbatim.** `iam.authz.cedar`, `iam.apikeys`, `iam.audit`,
  `gateway.chat.stream`. With exactly four keys this is complete coverage of the vocabulary against
  the ADR that defined it.
- **D8 rejections.** `from_wire_key` returns `None` for `"iam_audit"`, `"IAM.AUDIT"`, `"unspecified"`,
  `""`, `".iam.audit"`, `"iam.audit."`, an unregistered key, and the Unicode folds `"ıam.audit"`
  (U+0131) and `"ſervice.x"` (U+017F).

**TypeScript, in `capability.test.ts`:** the same round-trip and four-literal assertions against the
transform, plus `ServiceInfoService.typeName === "paigasus.common.v1.ServiceInfoService"`.

**Python, in `test_service_info_smoke.py`:** a ~8-line import-and-construct smoke test asserting the
four `Capability` members exist with their expected proto names.

The TS and Python tests follow the repo's established precedent for proving per-language codegen —
`ts/packages/paigasus-proto/src/health.test.ts` and
`py/packages/paigasus-proto/tests/test_health_smoke.py` did exactly this for the previous
first-service-in-a-package. Without them AC1 rests on `tsc` and the drift gate, which prove the files
exist but assert nothing about their contents.

**No serving-side test.** Nothing implements the RPC until SMA-505, so an end-to-end assertion would
have nothing to call. Stated so the two issues do not appear redundant.

## 7. Acceptance-criteria mapping

Being explicit about what CI proves and what only review proves, because three of the four ACs are
partly about doc comments that no test can read.

| AC | Verified by | Mechanism |
| --- | --- | --- |
| 1 — defined in `common/v1`, generates to all three languages | **CI** | Codegen-drift gate; `cargo nextest`; `capability.test.ts`; `test_service_info_smoke.py` |
| 2 — registry documented in-proto, append-only rule and each key's meaning stated | **CI, partly** | The four-literal tests prove the *spellings*. The append-only rule and each key's *meaning* are prose — **review-verified only** |
| 3 — doc comment states unknown key → ignore, absent capability → feature off | **Review only** | No test can read a doc comment. The reviewer must read § 4.3's header |
| 4 — `iam.proto` placeholder reconciled without breaking generated Rust | **CI** | `contracts:breaking` green; `cargo build` green; the Rust diff is descriptor bytes only (2.2) |

**Reviewer checklist item:** open `service_info.proto` and confirm the file header carries the
append-only rule, each of the four values' meaning, and the unknown-key / absent-capability sentences
verbatim. AC2's documentation half and all of AC3 rest on that read.

## 8. Verification

In order. Two of these steps are traps documented in `CLAUDE.md` and in SMA-498's spec.

1. `buf format -w` in `contracts/`. Mandatory before commit, or `contracts:fmt` reds `moon ci`
   **silently**.
2. The codegen-drift gate, reproduced exactly as `.github/workflows/ci.yml:194-207` runs it. It is a
   **workflow step, not a Moon task**, so the documented full-graph command does not cover it — and a
   plain `git diff` is not a valid substitute, because this change adds three brand-new *untracked*
   generated files that `git diff` reports clean:

   ```
   moon run contracts:generate
   git add --intent-to-add -- \
       rs/crates/libs/paigasus-proto/src/generated \
       py/packages/paigasus-proto/src/paigasus_proto/generated \
       ts/packages/paigasus-proto/src/generated
   git diff --exit-code -- <same three paths>
   ```

3. `cargo nextest run -p paigasus-proto` in `rs/`.
4. `moon run ts:test` and `moon run ts:fmt`. Prettier is its own whole-tree gate, decoupled from
   `ts:lint` and `tsc`.
5. `uv run pytest` for the Python smoke test in `py/`.
6. The full CI graph as documented in `CLAUDE.md`:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool
   :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts
   --base origin/main --include-relations`.

`:breaking` is expected green — a new file is additive, and `option deprecated` is not a breaking
change. Both were probed (2.1, 2.2). `:affected-smoke` is expected green per 2.4.

## 9. Out of scope

- **Serving the descriptor** — SMA-505, in both services, including deriving capabilities from live
  config and wiring `version` to the build. See § 4.2 for the boundary crack.
- **A drift gate over the capability vocabulary.** The analogue of SMA-507, which covers error codes
  only. No equivalent issue exists for capability keys; until one does, D2's protection is review
  discipline plus § 6's tests. Worth filing once SMA-505 makes an "advertised" side exist to check
  against — before then the gate would have only one side.
- **A Python transform helper** — nothing in the Python workspace consumes capabilities (D9).
- **Console-side consumption** — SMA-509, which owns the three nav states and `<Capability need="…">`.
- **The ADR-0020 amendment itself** — a Notion edit, per D10.

## 10. Risks and sequencing

- **R1 — Ordering dependency on SMA-498 (hard).** Until SMA-498's `contracts/buf.yaml` change is on
  `main`, `ENUM_VALUE_NO_DELETE` is active and every `Capability` spelling this issue commits is
  permanent (2.5, D2). **SMA-499 must not merge before SMA-498.** If that ordering has to be
  inverted, this spec must be revised to duplicate the `buf.yaml` change instead — it is not a
  detail the implementation can decide.
- **R2 — Irretractable spellings until R1 clears.** `iam.apikeys` versus `iam.api.keys` is a genuine
  word-boundary judgement. Mitigation: R1, plus reviewing § 4.2 as a wire contract rather than a
  draft.
- **R3 — § 4.2's config gap reopens `contracts/` in SMA-505.** Accepted deliberately; see § 4.2.
- **R4 — D10's ADR amendment is a human action.** If it does not happen, the repo's own "ADR before
  code" rule is violated by D3 in particular.

**Relationship to SMA-498.** Beyond R1 the two are independent: this issue imports nothing from
`error.proto`. The only file both touch is `rs/crates/libs/paigasus-proto/src/lib.rs`
(`pub mod error;` versus `pub mod capability;`), which will conflict trivially. There is **no**
TypeScript barrel conflict — SMA-498's revised spec states the barrel is not touched.

The design mirrors SMA-498 where the problems are the same — enum registry with a string wire (D1), a
derived rather than tabulated transform (D7), a positively-validating parser (D8) — and diverges
where they differ, explicitly (D9).
