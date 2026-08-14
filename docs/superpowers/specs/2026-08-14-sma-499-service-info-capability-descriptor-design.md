# SMA-499 — `ServiceInfo` capability descriptor in `common/v1`

Linear: [SMA-499](https://linear.app/smaschek/issue/SMA-499/contracts-serviceinfo-capability-descriptor-in-commonv1)
ADR: [ADR-0020 — Service capability discovery](https://app.notion.com/p/3bb830e8fbaa8113b9f3da910893aaa8) (accepted 2026-08-13)
Blocks: [SMA-505](https://linear.app/smaschek/issue/SMA-505) (iam+gateway serve the descriptor)

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
generated trees plus two historical plan and spec documents.

`contracts/proto/paigasus/common/v1/` holds `audit.proto` and `auditable_example.proto`. Neither
declares a service, and `rs/crates/libs/paigasus-proto/src/lib.rs` records that fact in a comment.

### 1.2 What this issue delivers

ADR-0020 decision 2 grows the stub into a real descriptor in `common/v1`: service name, version, and
a list of capability keys. This issue builds **only the contract** — the message, the registry, the
RPC and route shape, and the Rust transform helper. No service serves it until SMA-505.

## 2. Findings that constrain the design

Four facts were established experimentally in the worktree before designing. Three contradict a
plain reading of the issue's scope notes, so each is recorded with its evidence.

### 2.1 Deleting the placeholder reds `contracts:breaking`

AC4 offers "replaced or superseded". Replacement was probed by deleting the message and running the
gate:

```
proto/paigasus/iam/v1/iam.proto:1:1:Previously present message "ServiceInfo" was deleted from file.
```

`contracts/buf.yaml` uses the `FILE` breaking category, which includes `MESSAGE_NO_DELETE`. The file
already carries a documented exception for `FIELD_NO_DELETE`, but that exception is field-scoped and
does not reach message deletion. Removing the message therefore requires a new `ignore_only` waiver,
and the waiver could only be withdrawn in a follow-up PR — the gate compares against `main`, so the
message must already be absent from `main` before the waiver becomes removable. **Therefore:
supersede, do not delete** (D4).

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

`ci/affected-graph/run.sh` asserts that a `contracts` proto edit affects exactly
`contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs`,
by strict equality. This change adds no `dependsOn` edge in any workspace, so the affected set is
unchanged. The guard needs no update.

## 3. Decisions

### D1 — The registry is a proto `enum`; the wire stays `repeated string`

ADR-0020 decision 3 says capability keys are "strings, append-only, with the registry maintained as a
doc comment in the proto". Read literally that means a structured comment block and no declarations.
Rejected for the same reasons SMA-498's D1 rejected it for error codes: it generates *nothing*,
making AC1 ("generates to all three languages") true only of the message and vacuous of the registry;
it leaves the console and SDK with no symbols; and it forces any future drift gate to parse proto
comments.

`Capability` is declared as an enum and is a **registry, never a wire type**. No field in any `.proto`
has type `Capability`. `ServiceInfo.capabilities` is `repeated string`, exactly as ADR-0020 requires.

This is compatible with the ADR's forward-compatibility argument, which is entirely about the wire: a
new key must not force a bindings regen on every consumer before it can be *sent*, and an old console
must receive something it can log verbatim. Both hold — an old console reads a string it does not
recognise and ignores it, per the client contract in D5.

The cost that does survive: adding a key regenerates all three binding trees and the codegen-drift
gate requires committing that churn. Accepted — keys are added rarely.

### D2 — `buf breaking` is a bonus, not the guard

Because the registry is an enum, `buf breaking` now catches a *deleted enum value*. That is a welcome
side effect, not the protection. It cannot see the kebab-and-dot strings, cannot tell whether a key
is still advertised by a service or consumed by the console, and does not run against non-proto
sources. The file's doc comment says so explicitly, so nobody later mistakes a green `:breaking` for
a checked registry.

The issue's own note — "`buf breaking` does not guard this vocabulary" — stays true of the wire
strings, which are what consumers actually branch on.

### D3 — A shared RPC in `common/v1`, plus a normative HTTP route

`paigasus-iam` runs a tonic server; `paigasus-gateway` does not. The gateway is axum-only
(`/v1/chat/completions`, `/healthz`, `/readyz`) and merely *dials* IAM over gRPC through a tonic
`Channel`. Its own `gateway/v1/health.proto` `HealthService` is not served by anything.

So a gRPC-only contract would oblige SMA-505 to give the gateway a tonic server, a second listening
port, and a matching Helm and ingress entry — a new deployment surface for every self-hoster, which
exceeds ADR-0020's "deliberate, small tax on service authors".

`common/v1` therefore declares one shared service that every Paigasus service implements:

```proto
service ServiceInfoService {
  rpc GetServiceInfo(GetServiceInfoRequest) returns (GetServiceInfoResponse);
}
```

and the file doc comment normatively specifies the equivalent HTTP route for services with no gRPC
server:

```
GET /v1/service-info
200 -> canonical protojson encoding of ServiceInfo
```

A service satisfies the contract over **either** transport. The console uses whichever it already
speaks to that service: gRPC through the generated SDK client for IAM, the hand-written fetch client
for the gateway, per the frontend scoping document sections 4.2 and 4.3.

The `GetServiceInfoRequest` and `GetServiceInfoResponse` wrappers exist only to satisfy buf lint
STANDARD's `RPC_REQUEST_STANDARD_NAME` and `RPC_RESPONSE_STANDARD_NAME`. They carry no meaning.

**The HTTP route returns the bare `ServiceInfo`, not the response wrapper.** The two transports
therefore return different JSON — `{...}` versus `{"serviceInfo":{...}}`. This asymmetry is
deliberate: the wrapper is a lint artefact, and the console already has two distinct call sites for
the two services, so nothing is forced to handle both shapes at one site. Recorded here because it is
the kind of divergence that otherwise looks like an oversight.

### D4 — The placeholder is superseded in place, not replaced

Per 2.1 the message stays. It keeps its name and field 1, gains `option deprecated = true`, and its
doc comment is rewritten to name `paigasus.common.v1.ServiceInfo`, cite ADR-0020, and state **why it
still exists** — that `MESSAGE_NO_DELETE` forbids removing it. Without that last sentence the next
reader finds a dead message and assumes it was forgotten.

### D5 — Two mapping rules and one client contract, stated normatively in the proto

- **Key mapping:** strip the `CAPABILITY_` prefix, lowercase, replace `_` with `.`.
- **Key grammar:** lowercase alphanumeric segments separated by `.`.

The grammar is a consequence of the mapping, not an independent choice: because `_` → `.` consumes
*every* underscore, a key can never contain a hyphen or underscore inside a segment. `gateway.chat-stream`
is unreachable through the rule. All four keys in the initial registry comply. Stating the grammar
prevents a future key being proposed that the transform cannot express.

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

### D6 — `service` is a bare slug; `version` is never parsed

`ServiceInfo.service` carries `iam` / `gateway`, not the `iam.paigasus.io` form that SMA-498's
`ErrorDomain` uses. It is the `svcinfo:<service>` cache key from ADR-0020 decision 4 and the prefix of
its own capability keys, so it should be self-consistent with this file. SMA-498 is unmerged;
coupling this field to its vocabulary would be premature. The relationship is noted in prose so the
overlap is visible rather than accidental.

`ServiceInfo.version` is documented as **display and version-skew reporting only, never an input to a
feature decision**. Without that sentence someone eventually writes `if version >= "1.4"`, which
reintroduces precisely the version-skew bug ADR-0020 exists to prevent. The capability list is the
only sanctioned input to a feature decision.

### D7 — The transform lives in `paigasus-proto`, derived rather than tabulated

`rs/crates/libs/paigasus-proto/src/capability.rs`, a hand-written module following the existing
`src/audit.rs` precedent of layering a thin contract over generated types:

```rust
Capability::IamAuthzCedar.as_wire_key()   // "iam.authz.cedar"
Capability::from_wire_key("iam.audit")    // Some(Capability::IamAudit)
```

Both directions are **derived** from prost's `as_str_name()` / `from_str_name()` by applying the D5
rule, not written as a four-arm match. A match would be a second copy of the registry inside Rust —
the "three unlinked places" failure ADR-0019 cites from the observability metrics, where a metric's
prose drifted across unlinked sites. A derived function has nothing to drift against.

`as_wire_key` returns `String` rather than `&'static str`: a borrowed static would require a const
table, which is the second list this decision exists to avoid.

Without this module SMA-505 must invent the transform itself, and will be tempted to write
`"iam.audit"` as a literal at the point where it builds the descriptor — exactly the drift the
registry exists to prevent.

### D8 — `from_wire_key` is strict

Reconstructing a proto name from an arbitrary string admits inputs that are not valid keys. Three are
rejected explicitly:

- **Wrong separator.** `"iam_audit"` uppercases into `CAPABILITY_IAM_AUDIT` and would otherwise
  resolve. Input containing `_` returns `None`.
- **Wrong casing.** `"IAM.AUDIT"` likewise. Input containing any ASCII uppercase character returns
  `None`.
- **The zero sentinel.** `"unspecified"` must not resolve to `Capability::Unspecified`. The sentinel
  exists only to satisfy buf's `ENUM_ZERO_VALUE_SUFFIX` lint rule; no service advertises it.

A lenient parser would quietly widen any future drift gate, letting a misspelled advertised key pass
an "advertised ⊆ registry" check.

### D9 — Export the registry from the TypeScript barrel

`ts/packages/paigasus-proto/src/index.ts` is a hand-maintained selective barrel; generated files are
not re-exported automatically. `ServiceInfo`, `ServiceInfoSchema`, `Capability` and the
`ServiceInfoService` descriptor are added so `@paigasus/proto` consumers can reach them —
`ServiceInfoService` in particular is what `createClient(ServiceInfoService, transport)` needs.

No TypeScript or Python transform helper is added. SMA-498's D9 left the equivalent to the SDK issue,
and this follows that precedent.

## 4. The contract

`contracts/proto/paigasus/common/v1/service_info.proto`, package `paigasus.common.v1`, **no imports**.
A separate file rather than an addition to `audit.proto`, which is about auditable entities; the split
also keeps the TypeScript output in its own `service_info_pb.ts`.

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

**Known gap, recorded for SMA-505:** none of the four maps to a single boolean in `IamConfig` today.
The nearest config-gated flags are `authz.enforce_tenancy` and `audit.retention.enabled`. SMA-505's
AC3 requires a test that flips a config flag and asserts a key disappears, so it must either derive
these four from live state or append a key that corresponds to a flag. Appending is cheap by
construction; guessing that vocabulary now, in an append-only registry where a wrong guess is
permanent, is not.

### 4.3 File-level doc comment

States, normatively: the D5 key mapping rule and grammar; that each value's doc comment repeats its
literal; the client contract (unknown key ignored, absent capability means off, gating is cosmetic);
that the registry is append-only and `buf breaking` is not its guard (D2); the HTTP route equivalence
and its bare-`ServiceInfo` body (D3); and that `version` is never an input to a feature decision (D6).

## 5. Files touched

| Path | Change |
| --- | --- |
| `contracts/proto/paigasus/common/v1/service_info.proto` | new |
| `contracts/proto/paigasus/iam/v1/iam.proto` | doc comment rewrite plus `option deprecated = true` (D4) |
| `rs/.../paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` | regenerated — prost emits one file per *package*, so the descriptor lands beside `AuditMetadata` |
| `rs/.../paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.tonic.rs` | **new (generated)** — first service in this package (2.3) |
| `rs/.../paigasus-proto/src/generated/paigasus/iam/v1/paigasus.iam.v1.rs` | regenerated — `FILE_DESCRIPTOR_SET` bytes only |
| `ts/.../paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts` | new (generated) |
| `ts/.../paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts` | regenerated — `fileDesc` plus `@deprecated` JSDoc |
| `py/.../paigasus_proto/generated/paigasus/common/v1/__init__.py` | regenerated — betterproto2 merges by package |
| `py/.../paigasus_proto/generated/paigasus/iam/v1/__init__.py` | regenerated — adds the `DeprecationWarning` (2.2) |
| `rs/.../paigasus-proto/src/capability.rs` | new, hand-written (D7) |
| `rs/.../paigasus-proto/src/lib.rs` | `pub mod capability;`, the tonic `include!`, and the corrected comment (2.3) |
| `ts/packages/paigasus-proto/src/index.ts` | four re-exports (D9) |

No `Cargo.toml`, `buf.yaml`, `buf.gen.yaml` or `moon.yml` change. No new dependency in any workspace,
so `cargo-deny` needs no waiver and `cargo-machete` no allowlist entry.

## 6. Testing

All tests live in `capability.rs`. There is no serving-side test, because nothing implements the RPC
until SMA-505 and an end-to-end assertion would have nothing to call — stated so the two issues do
not appear redundant.

- **Round-trip.** For every `Capability` variant except `Unspecified`,
  `from_wire_key(v.as_wire_key()) == Some(v)`.
- **Grammar.** Every `as_wire_key()` output is non-empty, ASCII-lowercase, contains no `_`, has no
  empty segment, and neither starts nor ends with `.`. This is D5's grammar asserted mechanically.
- **The four literals, verbatim.** `iam.authz.cedar`, `iam.apikeys`, `iam.audit`,
  `gateway.chat.stream`. With exactly four keys this is not a spot-check — it is complete coverage of
  the vocabulary against the ADR that defined it, and it is what makes AC2 a fact CI checks rather
  than something verified by reading the diff.
- **D8 rejections.** `from_wire_key` returns `None` for `"iam_audit"`, `"IAM.AUDIT"`,
  `"unspecified"`, `""`, and an unregistered key.

## 7. Verification

In order. Two of these steps are traps documented in `CLAUDE.md` and in SMA-498's spec.

1. `buf format -w` in `contracts/`. Mandatory before commit, or `contracts:fmt` reds `moon ci`
   **silently**.
2. `moon run contracts:generate`, then `git diff --exit-code` over all three generated trees. This is
   the codegen-drift gate, and it is a **step in `.github/workflows/ci.yml`, not a Moon task** — the
   documented full-graph command does not cover it, so it must be run explicitly.
3. `cargo nextest run -p paigasus-proto` in `rs/`.
4. `moon run ts:fmt`. Prettier is its own whole-tree gate, decoupled from `ts:lint` and `tsc`.
5. The full CI graph as documented in `CLAUDE.md`:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool
   :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts
   --base origin/main --include-relations`.

`:breaking` is expected green — a new file is additive, and `option deprecated` is not a breaking
change. Both were probed (2.1, 2.2). `:affected-smoke` is expected green per 2.4.

## 8. Out of scope

- **Serving the descriptor** — SMA-505, in both `paigasus-iam` and `paigasus-gateway`, including
  deriving capabilities from live config and wiring `version` to the build.
- **A drift gate over the capability vocabulary.** The analogue of SMA-507, which covers error codes
  only. No equivalent issue exists for capability keys; until one does, D2's protection is review
  discipline plus the round-trip tests in section 6. Worth filing once SMA-505 makes an "advertised"
  side exist to check against — before then the gate would have only one side.
- **TypeScript and Python transform helpers** — the SDK issue, per D9.
- **Console-side consumption** — [SMA-509](https://linear.app/smaschek/issue/SMA-509), which owns the
  three nav states and `<Capability need="...">`.

## 9. Relationship to SMA-498

The two issues are independent: this one imports nothing from `error.proto` and touches no file
SMA-498 touches, apart from both adding a hand-written module to `paigasus-proto` and both editing
the TypeScript barrel. Those two files will conflict textually if the branches are merged out of
order; the conflicts are additive and mechanical.

The design deliberately mirrors SMA-498's decisions where the problems are the same — enum registry
with a string wire (D1), a derived rather than tabulated transform (D7), a strict parser (D8), and a
barrel re-export (D9) — so that a reader who has understood one registry has understood both.
