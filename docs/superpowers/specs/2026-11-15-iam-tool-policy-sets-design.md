# IAM: tool catalog, tool policy sets, compile on save, and policy sync to the gateway

- **Linear:** SMA-751 (the parent of the stack). Sub-issues for PR 1 and PR 2 are made at
  planning time.
- **ADR:** ADR-0027, decisions (d), (e) and (f), as amended on 2026-11-09 ("A-restricted"). This
  design needs one more amendment, for the gateway service identity (§9.4).
- **Depends on:** SMA-748, the Dogwood spike (Done). Its result comment is the source for every
  Dogwood fact in this document.
- **Consumers:** SMA-752 (the gateway Cedar decider) and SMA-753 (the gateway history store). This
  document defines the contract they read.
- **Date:** 2026-11-15. Revised the same day after the builder check and the adversarial challenge
  (see the changelog at the end).

## 1. Intent

The gateway can now see tool calls (SMA-749, SMA-769), but it has no rules to apply. ADR-0027
puts the rules in IAM. A tenant keeps a catalog of its tools and writes tool-call policies. A
platform operator keeps a floor of `forbid` policies that no tenant can loosen. IAM compiles
each policy set when it is saved. The gateway fetches the compiled result and evaluates it
itself. IAM does not get a call for each tool call.

Tenants write policies in Dogwood syntax, so they can write rules over the history of tool
calls. The gateway must never run the Dogwood interpreter. So IAM uses Dogwood only to validate
and lower. IAM translates each history rule into a small Paigasus-owned format, and the gateway
evaluates that format.

### Goals

1. A per-organization tool catalog, with import from OpenAI `tools` JSON and from an MCP tool
   list.
2. A per-organization tool policy set and one platform floor set, kept apart from IAM's own
   authorization policies.
3. Compile on save: validate, lower, check the allowed history shapes, translate, validate again
   with the gateway's Cedar version, then store. A save that fails a step is rejected with a
   precise reason.
4. A fetch RPC that returns an organization's compiled set by version, a fetch RPC for the floor,
   and NATS change events.
5. A per-organization setting for the mode and the fail mode, and an allow-everything starter
   policy.
6. A re-compile job for a compiler or schema change. It applies identical results and holds
   different ones for an admin.

### Non-goals

- Evaluation of tool calls. That is SMA-752 (the Cedar decider) and SMA-753 (the history store).
- The gateway's NATS consumer, including fan-out to every gateway replica. That is SMA-492. This
  work only publishes the events.
- `count_distinct_within`, `previous`, `since`, and every other history shape not in §5.3.
  ADR-0027 says to add shapes one at a time. Each new shape also needs a gateway evaluator.
- A temporal block that combines two shapes, and `error` event patterns (§5.3).
- A console UI for the catalog or the policies.
- Floor policies that name a specific tool (§3.3).
- A delete RPC for a tenant set. An admin sets the mode to `off`, or saves an empty set.

### Anti-goals

- A compiler or schema change that silently changes a running policy. Such a change must never
  change what the gateway enforces without an admin action, unless the output is identical.
- A catalog change that silently breaks a running policy in the gateway.
- A tenant action that weakens the platform floor. This includes the tenant mode and fail mode.
- Dogwood, `rhai` or `cedar-policy` 4.11 inside the `rs/` workspace or the IAM process.
- A gateway that accepts a compiled set it cannot fully evaluate. A missing slot or a malformed
  set always fails closed.
- A compiler child process that can read IAM's secrets, or can stop IAM through memory or
  process exhaustion. §5.2 names the controls and their limits.

## 2. Constraints

- Dogwood (`github.com/dogwood-policy/dogwood`, Apache-2.0, commit
  `ff158506cfd9c7871385ed3a87d7435505fd24b1`, version 1.1.1) pins `cedar-policy` 4.11.0 exactly.
  It does not compile on 4.14. `rs/` resolves `cedar-policy` to 4.14.1. So Dogwood must live in a
  separate cargo workspace.
- Dogwood pulls about 135 crates, including `rhai` and `pest`. `cargo deny` rejects `tiny-keccak`
  (CC0-1.0), which `rhai` pulls in.
- Dogwood's slot definitions exist only as an in-memory Rust syntax tree with no serde support.
  So the translation must run in the same process as Dogwood.
- One slot is one `Bool` for a whole `temporal { … }` block, with its threshold.
- Default slot names (`policy_<i>__temporal_<j>`) change when a rule is deleted.
  One `lower_with_distincter` call over the whole set does not keep them stable: it numbers
  rules by position in the call (Dogwood `cedarify/mod.rs:183-197`). One `lower_many` call, with
  one entry for each policy and a distincter made from its `@id`, keeps them stable (D1).
- Dogwood's history partitioning (event-schema pins) applies to the whole policy set. IAM must
  implement `@scope` for each policy. Annotations (`@scope`, `@id`) survive lowering verbatim.
- Dogwood's MCP schema generator breaks on a nested namespace. IAM generates the schema itself.
- The real temporal syntax is `when temporal { formerly within 1h <Ns>::Action::"x"::response{…} }`.
  `when formerly within 1h { … }` does not parse.
- The semantics that the gateway must match (from the spike): the window is closed,
  `[now - W, now]`; the event being decided is recorded before evaluation, so it counts toward
  `count_within`; only `request` events are decided.
- The spike did not verify the `previous` and `since` shapes, large per-tenant schemas, or a real
  git dependency with the full `cargo deny` checks.
- The IAM image is built by `rs/Dockerfile` with the build context `rs/`.
- The gateway has no IAM credential of its own today. It sends only the caller's own key (the D9
  self-query rule in `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs`).
- The repo rules: SPDX headers, edition 2024 and rust-version 1.95, literal workspace `members`
  (SMA-663), new error codes in `error.proto` with a `MANIFEST` row in
  `ci/error-registry/check.py`, new events in `ops/nats/subjects.env`, and capability keys that
  match `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`.

## 3. Approaches

### 3.1 How IAM runs Dogwood (decided: A)

- **A. A CLI binary that IAM starts for each compile. Chosen.** A separate cargo workspace builds
  `paigasus-policy-compiler`. IAM writes a JSON request to its stdin and reads a JSON result from
  its stdout, with a timeout. The binary ships in the IAM image. A Dogwood crash or hang does not
  reach IAM, given the limits in §5.2. There are no extra deployments. One compile costs a
  process start.
- **B. A separate compiler service over gRPC.** Rejected. It needs a new deployment, Helm
  projection, network policy and health checks, for a call that happens only on a save.
- **C. A linked static library.** Rejected. It puts two `cedar-policy` versions into one binary,
  across an FFI boundary.
- **D. A WASM module run by `wasmtime` in IAM.** Rejected. It adds a large runtime to IAM for no
  gain over A.

### 3.2 Delivery (decided: a stack of three PRs)

- **PR 1:** the compiler workspace and its CI, the catalog, the tenant set, the floor set, compile
  on save, the starter policy text, and the audit entries.
- **PR 2:** the settings row, the starter policy on first enable, the gateway service identity,
  the fetch RPCs and the change events.
- **PR 3:** the re-compile job and the held-version RPCs. Only PR 3 closes SMA-751.

PR 1 and PR 2 each get a sub-issue of SMA-751, and their branches use the sub-issue key. In an
earlier stack, Linear closed a parent issue when PR 1 named the parent key. So the title, the body
and every commit of PR 1 and PR 2 must not contain the text `SMA-751`. Refer to the parent by its
title. After each merge, check that SMA-751 is still In Progress.

No Dogwood bump and no compiler output change may merge before PR 3. Without the job, such a
change leaves old live versions with no re-compile.

### 3.3 What a floor policy can name (decided: generic only)

- **Generic only. Chosen.** A floor policy uses only `Action::"InvokeTool"`,
  `Action::"Unregistered"`, the principal and resource types, and the shared context. It is
  validated once against the base schema, which has no tenant tools. A tenant catalog change can
  never break the floor.
- **Named tools.** Rejected. The floor would have to validate against each tenant's schema, and it
  needs a rule for a tenant that does not have the named tool.

### 3.4 The floor and the tenant mode (decided: the floor is always enforced)

- **Always enforced. Chosen.** The tenant `mode` and `fail_mode` apply only to the tenant set. A
  floor `forbid` blocks a call in every tenant mode, `off` included. A deny that a floor policy
  decided is always enforced. A deny that only tenant policies decided follows the tenant mode.
  When the floor cannot be loaded, the gateway's global `on_policy_unavailable` decides (§9.3),
  never the tenant setting. SMA-752 made the same decision (D33), and removed its `floor_mode`.
- **A platform-level floor mode.** Rejected. It allows a log-only roll-out of a floor rule, but it
  adds a setting and an RPC.

### 3.5 What the re-compile job does with a result (decided: option 1)

- **Apply identical, hold different. Chosen.** See §8.
- **Report only.** Rejected: an operator must re-save every set by hand after each bump.
- **Apply all, report afterwards.** Rejected: it breaks the first anti-goal in §1.

### 3.6 How the gateway authenticates the fetch (decided: a dedicated service account)

- **A dedicated service account. Chosen.** See §9.4.
- **Grant the read action to `gateway_user`.** Rejected. Every API-key holder could then read the
  compiled policies of its organization.

### 3.7 Where the Dogwood source comes from (decided: upstream)

- **The upstream repository, pinned to a full commit. Chosen.** `Cargo.toml` pins
  `git = "https://github.com/dogwood-policy/dogwood"` and `rev = "<full sha>"`. `rs/Dockerfile`
  adds a cache mount for the cargo git directory (`/usr/local/cargo/git`), next to the existing
  registry cache mount.
- **A fork under the project's control.** Rejected for now. A fork removes the network risk of an
  upstream outage, but it must be maintained.

### 3.8 How to lower `@scope` (my call: lower once)

- **Lower the whole set once. Chosen, subject to a proof.** The spike shows that `@scope` survives
  lowering, and the distincter maps each slot to its policy. So the compiler reads `@scope` from
  each lowered policy and writes it into `HistoryCondition.scope`. The gateway partitions history
  by that field. The Dogwood event-schema pin stays the same for all scopes.
- **Lower each scope group separately.** Rejected. It needs a merge of several lowered schemas
  and policy sets. Dogwood has no such merge, and nothing could check it.
- **The proof.** The first task of the PR 1 plan shows that the lowered output of a policy does
  not depend on the scope of the other policies in the set. If that proof fails, stop and bring
  it back to Sven before any more work.

## 4. Data model

All new tables are in the IAM Postgres database, as SeaORM migrations from `m0013` on. They are
apart from the `policy`, `role` and `role_grant` tables. "Tenant" in this document means an
organization (`organization`, migration `m0002`).

### 4.1 Tool names

A tool name has an optional namespace and a name: `<namespace>.<name>` or `<name>`. This is the
qualified name that `ToolCall` in `paigasus-gateway-core` already builds from `namespace` and
`name`.

- Each part must match `^[A-Za-z0-9_-]{1,64}$`. This is the OpenAI function-name grammar. MCP
  tool names that do not match are rejected at import, with the reason.
- The match key (`name_key`) is the qualified name in ASCII lowercase. With this grammar, trim
  and Unicode NFC do nothing, so the ADR-0027 (d) rule ("trim, NFC, compare case-insensitively")
  holds with no Unicode dependency.
- The normaliser is the one that SMA-752 defines (trim, NFC, lowercase, NFC), in the kernel
  (ADR-0005). IAM calls the same function. The ASCII grammar is stricter than the normaliser, so
  every catalog name is already in normal form after lowercase.
- `invoketool` and `unregistered` are reserved (SMA-752) and are rejected as tool names.
- Two entries with the same `name_key` are a conflict.
- A call to `SEARCH` and a call to `search` get the same policies. ADR-0027 (d) accepts this risk.

### 4.2 Tool catalog

`tool_catalog_entry`:

| Column | Meaning |
|---|---|
| `org_id` | the organization, with a foreign key to `organization`. Part of the key. |
| `name_key` | the match key (§4.1). Part of the key. |
| `display_name` | the qualified name as given. It is shown to people only. The Cedar action id is the `name_key`. |
| `kind` | `function` (a JSON object input) or `custom` (a free-text input). |
| `input_schema` | the original JSON Schema of the arguments, as given. Null for `custom`. |
| `cedar_input_type` | the mapped Cedar record type (§4.3), as JSON. |
| `dropped_fields` | the argument paths that did not map, with the reason for each. |
| `source` | `manual`, `openai` or `mcp`. |
| `created_at`, `updated_at` | timestamps. |

The per-organization state row (§4.5) holds the `catalog_revision`. Every catalog write
increments it.

Limits (my call, set in `iam.toml`): at most 500 tools per organization, 64 KiB per input schema,
and a nesting depth of 8. A write over a limit is rejected.

**The catalog size cap.** `max_catalog_response_bytes` (default 2 MiB = 2 097 152 bytes) limits
the whole catalog of one organization. IAM measures the catalog after the write as one
`ListToolsResponse` with all its tools, encoded with prost. The gRPC adapter uses the same `Tool`
conversion. A write over the cap is a `tool-catalog-invalid` rejection. Its message names the size
and the cap. The size check uses the second of the write time with the largest nanosecond value.
So the stored catalog is never larger than the measured one. The value must be above 0 and below
4 MiB, the default decode limit of tonic. So every `ListTools` page decodes in a default client.

The cap also bounds the compile request and the compile result (§5.2). Measured with the release
compiler and a catalog at the cap, the largest result is 2 233 057 bytes (string properties). The
3 MiB `max_result_bytes` holds it. At a 3 MiB cap the same form gives a 3 262 597-byte result,
which is over `max_result_bytes`. So the default is 2 MiB, not 3 MiB. An operator who raises the
cap must also raise `max_result_bytes`.

**A consistent list.** `ListTools` reads the tools and the catalog revision in one repository
call (`catalog_snapshot`). Postgres runs it as one `REPEATABLE READ`, read-only transaction. So a
catalog write that commits during the read is in both values or in neither. Two plain reads could
return the new tools with the old revision.

**Built-in tools.** The gateway sees more kinds of call than JSON functions (`Computer`,
`LocalShell`, `Shell`, `ApplyPatch`, `ToolSearch`, `McpApprovalRequest` and others). An admin can
add such a tool manually by the name that the gateway puts in `ToolCall`. It gets an empty input
record. A built-in tool that is not in the catalog is `Unregistered`, like any other tool. The
SMA-752 author confirms the name the gateway uses for each kind.

**Import.** `ImportTools` accepts three forms:

- the OpenAI Chat Completions form: `[{"type": "function", "function": {"name", "description",
  "parameters"}}]`;
- the OpenAI Responses form: `[{"type": "function", "name", "description", "parameters"}]`, and
  `{"type": "custom", "name", …}` as a `custom` tool;
- an MCP `tools/list` result: `{"tools": [{"name", "inputSchema", …}]}`, with an optional
  namespace given in the request.

An entry of another type (`web_search`, `file_search` and others) is skipped and listed in the
result. An import is all-or-nothing: one transaction and one coupling compile (§4.6) for the whole
batch. With `replace = true`, the import also removes the tools that are not in it.

### 4.3 Schema generation

IAM generates the tenant Cedar schema from a fixed base and the catalog. IAM owns both. The
gateway does not validate requests against a schema (SMA-752 §6.1.2), so the schema is used only
by IAM, on save.

The names below come from the approved SMA-752 spec
(`docs/superpowers/specs/2026-11-15-sma-752-tool-invoke-decisions-design.md`, §6.1.5, D20 and
D22). They replace the `Paigasus::Tool` namespace in the SMA-751 issue text.

- **Principal:** the entity type of the caller's PRN. Every IAM principal PRN has the resource
  type `principal`, so the type is `Pgs::Iam::Principal` for users and service accounts (D5).
  No parents.
- **Resource:** `Pgs::Iam::Organization::"<org uuid>"`, the organization of the scope. No parents.
- **Action:** `Pgs::Gateway::Action::"<name_key>"` for every call. The action id does not depend
  on the catalog. A catalog tool has the parent group `Pgs::Gateway::Action::"InvokeTool"`. Any
  other tool has the parent group `Pgs::Gateway::Action::"Unregistered"`.
- **Context:**
  - `input`: a record of the arguments;
  - `input_text`: an optional `String`, present only when the arguments are not a JSON object;
  - `findings`: a `Set<String>` of finding kinds (`name_collision`, `unparsed_arguments`, and
    more from SMA-754);
  - `call`: `{ declared: String }`, one of `yes`, `no` or `unknown`;
  - `request`: `{ scope: String, ingress: String, model: String }`;
  - `session`: `{ id: String, turn: Long }`;
  - the history slots that lowering adds (SMA-753).

Any change to these names is a change to both specs. The PR 1 plan reviews the base schema with
the SMA-752 author.

**How the gateway maps arguments.** The gateway maps JSON by value, not by the schema (SMA-752).
An integral number in the `i64` range becomes `Long`. Another exact number with at most four
fraction digits becomes `decimal`. Any other number becomes a `String`. If the model sends a
value of a type that is different from the schema, a comparison is false, or an operator errors.
An errored `permit` is skipped. An errored `forbid` gives Deny (SMA-752 D26). So a type mismatch
never allows a call that the policy would deny.

**Policy source rules.** The compiler rejects an action literal that is not in normal form, and
any literal `invoketool` or `unregistered` that is not one of the two groups. The gateway treats
these as compile errors too (SMA-752).

**The starter and the floor against an empty catalog.** The starter (`action in InvokeTool`) and
a generic floor must validate when the catalog is empty. Cedar's validator can report that no
action matches a policy scope. The base schema must declare `InvokeTool` and `Unregistered` so
that this does not happen. PR 1 starts with tests that compile the starter against an empty
catalog and a generic floor against the base schema.

Each catalog tool adds one action `Pgs::Gateway::Action::"<name_key>"`, a member of
`InvokeTool`. Its `context.input` has the tool's mapped type. A `custom` tool has an empty
`input` record and a required `input_text`. A policy scoped to `action in Unregistered` sees an
empty `input` record, so it cannot read the arguments of an unknown tool.

JSON Schema maps to Cedar as follows:

| JSON Schema | Cedar |
|---|---|
| `string` (any `format`) | `String` |
| `integer` | `Long` |
| `boolean` | `Bool` |
| `array` with a mappable `items` that do not allow `null` | `Set<…>` |
| `object` with `properties` | a record. A property is required only if it is in `required`. |
| `["T", "null"]`, or `anyOf: [T, {"type": "null"}]` | an optional attribute of type T |
| `number`, other `oneOf` / `anyOf` / `allOf`, `$ref`, `additionalProperties` with no `properties`, `null` alone, other type lists, an `array` whose `items` allow `null` | not mapped |

In the `anyOf` row, the keywords next to `anyOf` apply to T too, as JSON Schema says. So
`properties` next to `anyOf` stay in the record of an object T. A `type` next to `anyOf` must
allow the type of T, or the field is not mapped. The attribute is optional only when that `type`
allows `null` or is absent.

The nullable row matters, because OpenAI strict mode writes every optional field as
`["string", "null"]`. A Cedar set cannot hold `null`, so an array whose `items` allow `null`
(for example `["integer", "null"]`) is not mapped.

A field that does not map is left out of the record. It is listed in `dropped_fields`, and the
import result returns it. A policy that reads such a field fails validation. The rejection then
adds the `dropped_fields` reason for that path, so the tenant can see why the field is not
available. A nested record that is empty after this step stays as an empty record.

The schema does not use Dogwood's MCP generator (§2).

### 4.4 Tool policy sets

There are two kinds of set: one tenant set per organization, and one platform floor set.

`tool_policy_set_version` holds one immutable row for each version:

| Column | Meaning |
|---|---|
| `kind` | `tenant` or `floor`. |
| `org_id` | the organization, with a foreign key. Null for `floor`. A CHECK constraint ties it to `kind`. |
| `version` | 1, 2, 3 … for each set. Part of the key, with `kind` and `org_id`. |
| `source` | the Dogwood source of the whole set, as saved. |
| `compiled` | the compile result (§5.4), as the proto bytes of `CompiledToolPolicySet`. |
| `compiler_generation` | the compiler generation (§6.2). |
| `schema_revision` | the IAM schema revision (§6.2). |
| `catalog_revision` | the catalog revision it was compiled against. Null for `floor`. |
| `status` | `live`, `superseded`, `held` or `discarded` (§8). |
| `origin` | `save`, `starter`, `catalog_change`, `recompile` or `accept`. |
| `base_version` | for a `held` row: the live version it was compiled from. |
| `report` | for a `held` row: the diff report (§8). |
| `created_by`, `created_at` | who and when. |

Partial unique indexes enforce exactly one `live` row for each set, and at most one `held` row for
each set. Versions are kept forever (my call). A set has few versions, and the history is the
audit trail of the rules.

One save replaces the whole set and makes a new version. The whole set compiles as one unit,
because validation and stable slot names work at set level. A save of a source that is
byte-identical to the live source makes no new version and returns the live version. IAM reads
the state and the live row in two reads. If the live row is newer than `expected_version`, the
save is a `ToolPolicyVersionConflict`. There is one
exception. If the compiler generation or the schema revision of the live version is not current,
the save compiles the source again and makes a new version. In PR 1, this is the only way to clear
`tool-policy-recompile-pending`.

An empty source is a valid tenant set. It permits nothing, so every tool call is blocked in
`enforce` mode.

### 4.5 State rows and concurrency

`tool_policy_state` has one row for each organization and one row for the floor. It holds the
`catalog_revision` (tenant only) and the `live_version`. It is also the lock row.

A compile takes up to 10 s. So no compile runs inside a database transaction. A write works in
four steps:

1. Read the state row, and the inputs: the source, the catalog and the floor source.
2. Compile, with no transaction open.
3. Open a short transaction. Lock the state row with `SELECT … FOR UPDATE`. Check that
   `live_version` and `catalog_revision` still have the values from step 1.
4. If they do, write the rows, the outbox event and the audit entry, then commit. If they do not,
   roll back and return `tool-policy-version-conflict`.

The write RPCs carry the expected value: `expected_version` on `PutToolPolicySet` and
`PutFloorPolicySet`, and `expected_catalog_revision` on every catalog write. A request whose
expected value is not current fails at once with `tool-policy-version-conflict`. So a second
admin cannot replace a first admin's save without seeing an error.

### 4.6 Catalog and set coupling

A catalog write changes the tenant schema. With the steps in §4.5, IAM compiles the live tenant
source against the new catalog:

- If the compile succeeds and the output differs, IAM stores it as a new live version with
  `origin = catalog_change`, in the same transaction as the catalog write.
- If the output is identical (§8.1), IAM does not change the set.
- If the compile fails, IAM rejects the catalog write. The rejection names each policy id that
  fails and the reason. The admin must change the policies first, then change the catalog.

The rule applies in every mode, `off` included. A live set is still the set that `log_only` and
`enforce` would use.

An organization that has no tenant set can change its catalog freely.

**A live set from an older compiler.** If the live version's `compiler_generation` or
`schema_revision` is not the running one, the coupling compile would also apply that difference.
So IAM rejects the catalog write with `tool-policy-recompile-pending`. The admin first accepts the
held version (§8), or saves the set again. A save always uses the running compiler and is a
reviewed action.

## 5. The compiler

### 5.1 Workspace

The compiler is a separate cargo workspace at `rs/tools/paigasus-policy-compiler/` (my call). It
is under `rs/` for three reasons:

- `rs/Dockerfile` uses the build context `rs/`, so the IAM image build can reach it.
- `rs/.cargo/config.toml` applies to it, because cargo reads config files by directory.
- It sits next to the code that calls it.

It is not a member of the `rs/` workspace. `rs/Cargo.toml` lists it under `exclude`. It has its
own `Cargo.toml` with a `[workspace]` table, its own `Cargo.lock` and its own `deny.toml`. It uses
`rs/rust-toolchain.toml` and adds no toolchain file of its own. Its `Cargo.toml` restates the
edition, the rust-version and the lints, because it cannot inherit `[workspace.package]` from
`rs/`.

Its `deny.toml` starts as a copy of `rs/deny.toml`, with two additions:

- the license exception `{ name = "tiny-keccak", allow = ["CC0-1.0"] }`;
- an `allow-git` entry for the Dogwood repository, because `rs/deny.toml` sets
  `unknown-git = "deny"`.

Each other exception must have a written reason, in the same style as `rs/deny.toml`.

The compiler is a Moon project. `.moon/workspace.yml` gets a literal entry for it. Its `moon.yml`
defines `build`, `test`, `lint` (clippy with `-D warnings`), `fmt`, `deny` and `machete` tasks.
These run in its own directory, with its own `Cargo.lock` and `Cargo.toml` as inputs. The shared
`.moon/tasks/rust.yml` gives every Rust project the inputs `/rs/Cargo.lock` and `/rs/Cargo.toml`,
so the compiler project must override or add to them.

These places read only the `rs/` lockfile today. Each one must learn the second lockfile, or the
plan must say why it does not apply:

- the repo-level `:deny` and `:machete` gates;
- the Dependabot `cargo` entry, which covers only `/rs`;
- the `images.yml` path filter, which lists only `rs/Cargo.{lock,toml}`;
- the `ci.yml` cargo-lock-integrity step;
- `rs/.dockerignore`, whose `target/` rule matches only the top-level directory. It needs
  `**/target/`.

`:osv` does not cover Rust by design, so advisories come from `:deny` and the scheduled security
scan.

### 5.2 Protocol and process limits

IAM starts the binary as `paigasus-policy-compiler compile`. It writes one JSON request to stdin
and closes stdin. The binary writes one JSON result to stdout and exits.

- Exit 0: the result is a success or a rejection. A rejection is a normal result.
- Any other exit, a timeout, or stdout that does not parse: IAM reports
  `tool-policy-compiler-unavailable` (§7). It never stores anything in this case.

The request holds the set kind (`tenant` or `floor`), the Dogwood source, the generated schema in
Cedar JSON schema form, the window limit, and a protocol version. The result holds the compiled
set (§5.4) or a list of rejections. Each rejection has the policy id, a source location if Dogwood
gives one, a reason code (§7) and a message.

`paigasus-policy-compiler revision` prints the compiler generation and the revision string
(§6.2).

The protocol is versioned. IAM checks the version on every result. Each side defines the JSON
types once. A shared fixture file of request and result examples is read by the tests on both
sides. That fixture is the drift control for the protocol. The decision corpus (§10) is the drift
control for the meaning.

The child process runs with these limits. Each one has a test.

- `env_clear()`: the child gets no environment of its own. This control alone does not hide the
  database, NATS and Redis credentials of IAM. On Linux, a process under the same UID can read
  `/proc/<IAM pid>/environ` and `/proc/<IAM pid>/mem` while IAM is dumpable.
- IAM is not dumpable. When the feature is on, IAM calls `prctl(PR_SET_DUMPABLE, 0)` on Linux
  before the boot probe starts the first child. Then the kernel refuses `/proc/<IAM pid>/environ`,
  `/proc/<IAM pid>/mem` and `ptrace` to a process without `CAP_SYS_PTRACE`, also under the same
  UID. If the call fails, IAM logs an error and keeps the feature off. IAM serves its other RPCs,
  in the same way as after a failed probe: only the compiler child needs this control. IAM logs
  the result once at boot.
  - Side effects: the kernel writes no core dump of IAM, and the `/proc/<IAM pid>` files belong to
    root. A debugger or a profiler that attaches to IAM needs `CAP_SYS_PTRACE`.
  - Limits: the control does not stop a process with `CAP_SYS_PTRACE` or root with that
    capability. The container must not grant it. macOS has no `PR_SET_DUMPABLE`. There, IAM logs a
    warning, and the control is absent. macOS is a development host only.
  - A test proves it on Linux. It runs a fake compiler that reads `/proc/$PPID/environ` through
    the production spawn code. The read succeeds under a dumpable parent (the positive control)
    and gives zero bytes under a non-dumpable parent. Each case runs in its own test process,
    because the call cannot be undone.
- An empty, temporary working directory, and no inherited file descriptors except the three
  pipes. In `pre_exec`, IAM sets `FD_CLOEXEC` on every descriptor from 3 up, so `exec` closes
  each descriptor that IAM itself got without the flag.
- `RLIMIT_AS` (default 1 GiB) and `RLIMIT_CPU` (default 8 s), set in `pre_exec`. The CPU limit
  must be below the 10 s timeout. CPU time does not run faster than the wall clock, so a CPU limit
  at or above the timeout never fires. IAM refuses such a config at boot.
- A global semaphore (default 2 permits) in front of the compiler. When it is full, the RPC
  returns `tool-policy-compiler-busy`, which is retryable.
- IAM writes stdin and reads stdout and stderr at the same time, so the pipes cannot deadlock.
- On a timeout, IAM kills and reaps the child (`kill_on_drop`).
- Limits on the size: a 256 KiB source, a 3 MiB result, and a 10 s timeout. The defaults are set
  in `iam.toml`. The catalog size cap (§4.2) bounds the schema in the request and the result.
  Measured at the 2 MiB cap: a compile request of up to 1 278 659 bytes, a result of up to
  2 233 057 bytes, and 1.6 s of wall time for the release compiler on the development Mac. A
  debug build needs about 22 s, so it is over the 8 s CPU limit. The image ships a release build.
- Stderr is captured, truncated and logged. It is never returned to the caller.

### 5.3 Compile steps

1. Parse the source. Every policy must have a unique `@id`. A missing or duplicate `@id` rejects
   the save. A policy with more than one `@id` annotation also rejects the save, because the
   emitted Cedar keeps only one of them. The `@id` grammar is `^[a-z0-9][a-z0-9_-]{0,63}$`. Floor `@id` values must start with
   `floor-`, and tenant `@id` values must not. So the slot names of the two sets cannot collide.
   The `@id` gives the distincter of the policy's `lower_many` entry (D1).
2. Read `@scope` on each policy: `session` (the default), `principal` or `key`. Any other value
   rejects the save. A policy with more than one `@scope` annotation also rejects the save,
   because the emitted Cedar keeps only one of them.
3. Validate the set with Dogwood against the schema.
4. Lower the whole set once: one `lower_many` call, one entry for each policy (§3.8, D1). A failed lowering rejects the
   save.
5. Translate each temporal block into one history condition. Only these shapes are accepted:
   - `formerly within W` over a predicate on events;
   - `count_within W` with a comparison (`<`, `<=`, `==`, `>=`, `>`, `!=`) to a constant;
   - `sum_within W` of one `Long` argument field, with a comparison to a constant.
   A block with any other shape, or with more than one shape, rejects the save. The rejection
   gives the policy id and the shape name.
6. Validate the lowered Cedar again with `cedar-policy` 4.11 (Dogwood's own) against the lowered
   schema. This finds a lowering defect early.
7. For the floor, check that every lowered policy has the `forbid` effect.
8. Check the window: `W` must not be more than the window limit in the request. IAM's
   `tool_policies.max_window` defaults to 24 h. It cannot be set above 24 h, because Dogwood caps
   every window at 24 h. A larger limit can never take effect. ADR-0027 (g) permits 7 days. This
   spec narrows that limit to 24 h.

The predicate in a history condition may use only:

- the event kind: `request` (a call, in ADR-0027 (g)) or `response` (a result). An `error` pattern
  rejects the save in v1;
- the tool action: one tool, or `InvokeTool`;
- equality and comparison of argument fields to constants;
- `&&` and `!`.

Dogwood temporal source has no `||`. The proto contract keeps an `AnyOf` arm for a later version,
but no Dogwood source can reach it in v1.

Any other expression in a predicate rejects the save. The plan lists the exact Dogwood syntax-tree
nodes that map to this subset, from the Dogwood source.

The plan also checks what Dogwood uses `rhai` for. If tenant source can reach `rhai` during
validation or lowering, the plan sets operation limits for it, or the compiler rejects that
syntax.

IAM then does three more checks in its own process, with the `cedar-policy` 4.14 that
`paigasus-iam-core` already links:

9. Parse and validate `cedar_policies` against `cedar_schema`. A failure rejects the save with
   `tool-policy-invalid`. This catches a policy that passes under 4.11 but fails under the
   gateway's Cedar version.
10. For the floor, check the `forbid` effect a second time.
11. Check the depth of each history condition predicate. A predicate deeper than 32 nodes rejects
    the save with `tool-policy-invalid`. Each `&&` term and each event argument adds one node.
    prost does not decode a message that nests deeper than 100 levels, and each `all_of` adds two
    levels. So a deeper set could be stored, but IAM and the gateway could not read it again.

### 5.4 Output: `CompiledToolPolicySet`

A new proto message in `contracts/proto/paigasus/iam/v1/`. It is the contract with SMA-752 and
SMA-753. It holds:

- `kind`, `org_id`, `version`, `compiler_generation`, `schema_revision`, `catalog_revision`;
- `cedar_policies`: the lowered Cedar text of the whole set;
- `cedar_schema`: the lowered schema in Cedar JSON schema form, which includes the slot
  attributes. Every slot attribute has the type `Bool`;
- `conditions`: one `HistoryCondition` for each slot, sorted by `slot`;
- `max_window_ms`: the window limit it was compiled with;
- `catalog_names`: the sorted `name_key` list of the catalog it was compiled against. The gateway
  needs it to give each call the parent `InvokeTool` or `Unregistered`. Empty for the floor.

A `HistoryCondition` holds:

- `slot`: the stable slot name, as it appears in the lowered Cedar context;
- `policy_id`: the `@id` it came from;
- `scope`: `SESSION`, `PRINCIPAL` or `KEY`;
- `window_ms`: the window length;
- `shape`: a `oneof` of `Formerly { predicate }`, `CountWithin { predicate, op, threshold }` and
  `SumWithin { predicate, field_path, op, threshold }`;
- `predicate`: a small expression tree: event kind, tool match, field comparison, and, or, not.

The message uses no `map` fields, so its encoding is deterministic.

The doc comments of these messages state the evaluation rules. The gateway must follow them:

- The window is closed: `[now - W, now]`. Only the gateway clock sets event times.
- The event being decided is recorded before evaluation. It counts toward `count_within` and
  `sum_within`. A denied call is still recorded, so it counts too.
- Whether `formerly` sees the event being decided is set by Dogwood's semantics. The decision
  corpus (§10) settles it, and the doc comment states the result.
- Tool matching uses the `name_key` (§4.1).
- `sum_within`: an event with no value at `field_path` adds 0. A negative value adds 0 (my call).
  This differs from Dogwood, because the arguments come from the model, and a negative amount
  must not lower a budget. The sum saturates at the `Long` limits.
- `KEY` scope: a caller with no API key (an OIDC bearer token) uses its principal as the key.
- The gateway must keep history for at least `max_window_ms`. If its retention is shorter, it
  fails closed for that set.
- A missing slot, an unknown shape, or an unknown enum value fails closed.

The proto change adds new messages only, so it passes `repo:breaking`.

**The combination with the floor.** This follows the approved SMA-752 spec. The gateway builds
one merged Cedar policy set from the floor and the tenant set. It prefixes the policy ids with
`floor/` and `tenant/` internally. Cedar's rule that a `forbid` wins over any `permit` means that a
tenant cannot loosen the floor. The floor is compiled and cached apart from the tenant set, so a
tenant set that fails to load does not remove the floor. The `floor-` `@id` prefix (§5.3) keeps
the slot names of the two sets apart in the one merged context. The proto doc comments state
this rule.

## 6. Storage, revisions and the starter policy

### 6.1 What is stored

Each version stores the source, the compiled message, the compiler generation, the schema
revision and the catalog revision (§4.4). This meets the ADR-0027 (f) rule: "store the source,
the compiled Cedar, the translated conditions and the Dogwood revision".

### 6.2 Generations and revisions

Two hand-maintained integers decide when a set must be compiled again:

- **`compiler_generation`**, a `u32` constant in the compiler crate. It changes when the compiler
  output can change: a Dogwood bump, a translation change, or a protocol change. A golden-output
  test pins the output of a fixed corpus to the current generation. The test fails when the
  output changes and the integer does not. `EXPECTED_STARTER_CONTENT_HASH` is the precedent.
- **`schema_revision`**, a `u32` constant in IAM. It changes when the base schema or the JSON
  Schema mapping changes. A pin test works in the same way.

`CARGO_PKG_VERSION` cannot do this, because the crate versions do not move with the output. The
`STARTER_POLICY_REVISION` docs in `roles.rs` record the same trap.

The compiler also prints a revision string for logs:
`<generation>+dogwood.<12-char commit>+proto.<protocol version>`. The integers are the only values
that the job compares.

IAM reads the compiler generation once at boot, with `paigasus-policy-compiler revision`. The
probe tries a transient failure up to 3 times, with 1 s between the attempts. Each attempt has a
limit of 3 s, or the compile timeout if that is shorter. So a binary that hangs delays readiness by
about 11 s at most. If the binary is missing or the command fails, IAM still boots. The tool-policy write RPCs then return
`capability-disabled`, and `ServiceInfo` does not advertise the capability. IAM's other features
are not affected.

The capability is `CAPABILITY_IAM_TOOL_POLICIES`, with the key `iam.tool.policies`. The config
toggle `tool_policies.enabled` (default `false`) lets an operator turn the feature off without a
deploy, in the same way as `authz.admin_enabled`.

### 6.3 Starter policy

The starter policy is a fixed source that IAM owns:

```cedar
@id("starter-allow-all")
permit (principal, action in Pgs::Gateway::Action::"InvokeTool", resource);
```

It does not permit `Unregistered`. A tool that is not in the catalog stays blocked (ADR-0027 (d)).
In log-only mode, such a block is only logged. PR 1 adds the source and a test that compiles it
against an empty catalog. PR 2 saves it (§9.2).

## 7. Errors

**A rejection is a successful response.** `PutToolPolicySet`, `PutFloorPolicySet` and each catalog
write return `oneof result { accepted; rejected }`. `accepted` holds the new version. `rejected`
holds every rejection: the policy id, the location, the reason and the message. This agrees with
§5.2, and the HTTP error envelope (`{"error": {"code", "message"}}`) does not change.

The other failures are errors. New codes go in the IAM block (1-299) of `error.proto`, with the
next free numbers. The reason codes of a rejection are registered too, so that one registry
covers every machine-readable reason.

| Reason | Form | gRPC / HTTP | When |
|---|---|---|---|
| `tool-policy-invalid` | rejection | - | Dogwood validation or Cedar validation failed. |
| `tool-policy-shape-unsupported` | rejection | - | a block is not one of the three shapes, has two shapes, or uses a predicate node outside the subset. |
| `tool-policy-lowering-failed` | rejection | - | Dogwood's `lower` failed. |
| `tool-policy-floor-permit` | rejection | - | a floor set contains a `permit`. |
| `tool-policy-id-invalid` | rejection | - | a missing, duplicate or malformed `@id`, a wrong `floor-` prefix, or an unknown `@scope`. |
| `tool-policy-window-too-long` | rejection | - | a window is longer than the limit. |
| `tool-catalog-breaks-policy` | rejection | - | a catalog write would break the live tenant set (§4.6). |
| `tool-catalog-invalid` | rejection | - | a tool name breaks the grammar, or a write breaks a limit. |
| `tool-catalog-conflict` | error | `ALREADY_EXISTS` / 409 | two tool names have the same match key. |
| `tool-policy-version-conflict` | error | `ABORTED` / 409 | the expected version or catalog revision is not current (§4.5). |
| `tool-policy-recompile-pending` | error | `FAILED_PRECONDITION` / 409 | the live set is from an older compiler or schema (§4.6). |
| `tool-policy-compiler-unavailable` | error | `UNAVAILABLE` / 503 | the compiler crashed, timed out, or broke the protocol. |
| `tool-policy-compiler-busy` | error | `RESOURCE_EXHAUSTED` / 429 | the compiler semaphore is full. Retryable. |
| `tool-policy-version-not-found` | error | `NOT_FOUND` / 404 | a fetch asked for a version that does not exist, or that is not `live` or `superseded`. |

An existing code is used where it fits: `not-found`, `forbidden`, and `capability-disabled` when
the binary is missing at boot or the feature is turned off. Each new emission site gets a
`MANIFEST` row and a membership test, as `repo:error-code-single-site` requires.

## 8. Re-compile job (PR 3)

The job runs once at boot, after migrations, and on demand through an admin RPC. It takes a
Postgres advisory lock, so one replica runs it at a time. A replica does not run it if a stored
`compiler_generation` or `schema_revision` is higher than its own. This follows the
`STARTER_POLICY_REVISION` precedent for an older replica during a rollout.

The job selects each live version whose `compiler_generation` or `schema_revision` is not the
running one. It skips a set that already has a `held` row for the running values. For each other
set:

1. Compile the live source against the current catalog, with the running compiler, using the
   steps in §4.5.
2. **Identical output (§8.1):** store a new live version with `origin = recompile`. The gateway
   sees a new version with the same meaning.
3. **Different output, or a failed compile:** keep the old version live. Store the new output as
   a `held` row, with `base_version` and a diff report. The report lists the added, removed and
   changed policies, conditions and schema entries, or the compile failure.

The job emits a log line and a metric for each result: `identical`, `held_different` or
`held_failed`.

**Held rows.**

- Any new live version, from any origin, sets the held row of that set to `discarded`.
- Accept checks that `base_version` is still live and that the catalog revision is current. It
  then copies the held output to a new version number with `origin = accept`. If a check fails,
  the held row is stale. Accept then returns `tool-policy-version-conflict`, and the admin runs
  the job again.
- Discard sets the row to `discarded`. The live set stays at the old generation. Catalog writes
  stay blocked (§4.6) until an admin saves the set again.
- The floor follows the same rules. Accept and discard of a floor row need the platform-admin
  action.

### 8.1 Identical output

There is one comparator for §4.6 and §8. It compares `cedar_policies`, `cedar_schema` and
`conditions` only, after canonical serialisation. It does not compare `version`, the generations
or the revisions, which always differ.

## 9. Settings, the gateway contract, RPCs and events

### 9.1 RPCs

A new `ToolPolicyService` in `iam.proto`. All methods are new, and no existing method changes.

PR 1:

- `PutTool`, `DeleteTool`, `ListTools`, `ImportTools` (§4.2). Each write carries
  `expected_catalog_revision` and returns the result `oneof` (§7).
- `PutToolPolicySet` (org, source, `expected_version`) and `GetToolPolicySet` (org, optional
  version).
- `PutFloorPolicySet` (source, `expected_version`) and `GetFloorPolicySet`.

PR 2:

- `GetToolPolicySettings` and `PutToolPolicySettings`.
- `GetCompiledToolPolicies` (org, optional version). It returns the tenant `CompiledToolPolicySet`
  at that version (default: live), the settings, and the live floor version number. An org with
  no tenant set gets an empty set with `version = 0`. A `held` or `discarded` version is never
  returned.
- `GetCompiledFloorPolicies` (optional version). The floor is fetched separately, so that one
  response stays below the default tonic limit of 4 MiB. A platform with no floor gets an empty
  set with `version = 0`.

PR 3:

- `ListHeldToolPolicyVersions`, `AcceptHeldToolPolicyVersion`, `DiscardHeldToolPolicyVersion`,
  `RunToolPolicyRecompile`.

Each RPC also has an HTTP route in `adapters/http/`, in the same way as the existing policy
routes.

### 9.2 Settings (PR 2)

`tool_policy_settings`, one row for each organization:

- `mode`: `off` (the default when no row exists), `log_only` or `enforce`;
- `fail_mode`: `open` or `closed`. The default is `closed`.

The first change away from `off` must be to `log_only`. IAM then saves the starter policy as
version 1 with `origin = starter`, if the organization has no tenant set. A later change from
`off` to `enforce` is allowed when a tenant set exists. This follows ADR-0027 (d): "a tenant starts
in log-only mode".

The mode and the fail mode apply only to the tenant set (§3.4).

### 9.3 Failure handling at the gateway

This agrees with the SMA-752 spec (D33 and D34, settled by Sven on 2026-11-15).

- **The gateway's `on_policy_unavailable`** is global and required. It decides when the floor
  cannot be loaded: `closed` blocks every call, and `open` allows and reports.
- **The tenant `fail_mode` from IAM** replaces the gateway's per-org config override
  (`[[tool_policy.org]]`). It decides when the tenant set cannot be loaded. When the tenant set is
  unavailable, its `fail_mode` is unavailable too. So the gateway uses the last known value for
  that tenant, and the global value when it has none. For this reason, `GetCompiledToolPolicies`
  returns the settings in the same response as the compiled set (§9.1), and the gateway caches
  them together.
- **`tool_interception.on_failure`** covers only the hold limits, malformed or truncated streams,
  the decide timeout and a decider panic.
- **Always closed:** a missing slot, a malformed set, an unknown shape, and a history retention
  shorter than `max_window_ms`.

### 9.4 Gateway service identity (PR 2)

The fetch runs with no caller, after a NATS event or on a refresh timer. So the gateway needs an
IAM identity of its own.

- A new system role `tool_policy_reader`. It has `ReadCompiledToolPolicies` and nothing else. It
  is granted at Root.
- A dedicated gateway service account, provisioned in the same way as the other system service
  accounts. It holds the `tool_policy_reader` grant.
- A gateway config key for the account's credential file (`iam.service_credential_file`). The
  gateway sends this credential only on the two fetch RPCs. The self-query `IsAuthorized` keeps
  sending the caller's own credential, as D9 requires.
- Rotation: the gateway reads the credential file again when it changes. A rotation needs no
  restart.
- This adds a new system role, so it changes `STARTER_POLICY_IDS` and needs a
  `STARTER_POLICY_REVISION` bump.
- **The gateway adapter is part of PR 2.** It implements SMA-752's `PolicySource` port with the
  two fetch RPCs: `tenant(org)` maps to (`cedar_policies`, `catalog_names`, the version as a
  string), and `floor()` maps to the floor's `cedar_policies`. It caches the settings with the
  tenant set (§9.3). The credential config key and its reload are in this adapter. SMA-752 owns
  the port and the NATS subscribe grant on `iam.policy.>`. If SMA-752 has not merged when PR 2
  starts, the plan stacks PR 2 on it, or delivers the adapter in a follow-up PR.
- **An ADR-0027 amendment is required before PR 2 merges.** It records that the gateway now holds
  one IAM credential, that the credential is limited to the fetch, and that the D9 self-query rule
  is unchanged.

### 9.5 Authorization

New IAM `Action` variants. Each one goes into the `Action` enum, `Action::ALL` and its length
test, `is_write`, the string mapping (`paigasus-iam-core/src/authz/action.rs`), `SCHEMA_SRC`
(`authz/schema.rs`), and the role allow-lists (`paigasus-iam-core/src/authz/roles.rs`).

| Action | Granted to |
|---|---|
| `ManageToolCatalog` | `org_admin` |
| `ReadToolCatalog` | `org_admin`, `org_member` |
| `ManageToolPolicies`, `ReadToolPolicies` | `org_admin` |
| `ManageToolPolicySettings` | `org_admin` |
| `ManageToolPolicyFloor`, `ReadToolPolicyFloor` | `platform_admin` only |
| `ManageToolPolicyRecompile` | `platform_admin` only |
| `ReadCompiledToolPolicies` | `tool_policy_reader` |

The new write actions join the generated `forbid-archived-writes` list. So the
`STARTER_POLICY_REVISION` bump is certain in PR 1.

### 9.6 Audit

Each mutation writes one `AuditEntry` in the same unit of work as the change and its outbox event:
catalog writes, set saves, floor saves, settings changes, recompile results, accept and discard.
The entry holds the set, the version, the origin and the actor. This follows
`RoleService::grant` in `rs/crates/services/paigasus-iam/src/application/roles.rs`. It starts in
PR 1.

### 9.7 Change events (PR 2)

New `EventType` values under `iam.policy.>`, as SMA-752 asks:

- `iam.policy.tool_set.published` (org, version): a new live tenant version, from any origin.
- `iam.policy.tool_floor.published` (version).
- `iam.policy.tool_settings.changed` (org, mode, fail mode).

These have four segments. The builder checks that `EventType::parse` and the wire tests accept
four segments, and otherwise extends them. Every payload carries the org UUID, where one exists,
so a gateway replica can clear one tenant and not all tenants.

Each event is written to the outbox in the same transaction as the change. The subjects are under
`iam.>`, so the publisher's grant covers them. `ops/nats/permissions.md` updates its subject
count, and `tests/nats_permissions.rs` must pass. A catalog change publishes an event only
through the tenant version that it creates. The gateway does not read the catalog.

**Fan-out.** Each gateway replica uses a core NATS subscription on `iam.policy.>` (SMA-752 §9),
not the pull durable `gateway-cache-invalidator`. So every replica sees every event. The NATS
permission for that subscription belongs to the gateway side (SMA-752 or SMA-492). The periodic
refresh stays as the backstop (ADR-0027 (e)). The propagation bound is the refresh interval when a
core NATS message is lost.

## 10. Testing

- **The compiler workspace:**
  - a unit test for each accepted shape and each rejected shape, including a two-shape block and
    an `error` pattern;
  - a test that slot names stay the same when a policy before them is deleted;
  - the scope proof (§3.8);
  - a test that a floor `permit` is rejected in every syntax form;
  - a hyphenated `@id` with a temporal block;
  - golden tests over the shared protocol fixture, and the generation pin test (§6.2).
- **The decision corpus.** The compiler workspace emits a corpus of event sequences and the
  decisions from Dogwood's own replay, for each accepted shape. SMA-753 must replay this corpus
  with its evaluator. The corpus documents each place where Paigasus differs from Dogwood (the
  negative-value rule in §5.4).
- **The cross-version proof:** a test in `rs/` takes the compiler's golden output and checks that
  the lowered Cedar parses and validates under `rs/`'s `cedar-policy` 4.14.
- **IAM:**
  - the schema generator over the three import forms, including every unmapped JSON Schema form
    and the nullable forms;
  - the name grammar, the match key, and the conflicts;
  - the starter against an empty catalog, and a generic floor against the base schema;
  - the catalog and set coupling (§4.6), and the recompile-pending rule;
  - the races: two saves on one version, and a save during a catalog write;
  - the compiler adapter with a fake binary that crashes, hangs, writes too much, writes bad JSON,
    reads a large stdin while it writes a large stdout, and prints its environment (which must be
    empty);
  - the semaphore;
  - one test that runs IAM with the real compiler binary, with the Moon wiring for the build
    dependency;
  - repository tests in Postgres, including the one-live and one-held indexes;
  - RPC tests for every error code and every rejection reason;
  - an audit entry for each mutation.
- **PR 2:** the fetch RPCs by version, with no set, and with a held version; the starter on first
  enable; the mode rules; the service identity and its grant; an `org_admin` who sets `off` and
  still gets the floor; the outbox rows for each event; the NATS permission test.
- **PR 3:** identical, different and failed results; a restart that does not write a second held
  row; the advisory lock; the older-replica rule; accept, stale accept and discard.
- **Mutation proof:** for each gate rule in this design (the shape check, both floor checks, the
  4.14 check, the coupling check, the version check, the one-live index, `env_clear`, the
  non-dumpable parent, the catalog size cap, the consistent list), the plan
  deletes the rule once and shows that a test fails.

## 11. Left to the builder

- The exact base schema (§4.3), written together with the SMA-752 author.
- The Dogwood syntax-tree nodes that map to the predicate subset (§5.3), and the `rhai` check.
- The Rust types on each side of the protocol, and the JSON field names.
- How `rs/Dockerfile` builds the compiler and copies it into the IAM image only, and the config
  key for its path.
- The HTTP routes, which follow the existing policy routes.
- Pagination for the list RPCs, which follows the existing list RPCs.
- How each place in §5.1 covers the second lockfile.
- How the gateway service account is provisioned, in the same way as the existing system service
  accounts.

## 12. Calls I made

These were not discussed with Sven. Each one is my call and can be changed:

- "Tenant" means an organization.
- The compiler lives at `rs/tools/paigasus-policy-compiler/`, excluded from the `rs/` workspace.
- The whole set is lowered once, and `@scope` comes from the annotation (§3.8), subject to a proof.
- Tool names use an ASCII grammar, and the match key is the lowercase qualified name.
- Built-in tools can be added manually with an empty input. A custom tool has `{ text: String }`.
- Unmapped JSON Schema fields are dropped and reported, not rejected.
- A catalog write that breaks the live set is rejected, in every mode.
- `@id` is required, with a grammar, and the floor uses a `floor-` prefix.
- A temporal block may have only one shape. `error` patterns are rejected in v1.
- `sum_within` ignores negative and missing values, and saturates.
- `KEY` scope with no API key uses the principal.
- The default fail mode is `closed`. The default window limit is 24 h.
- `org_member` can read the catalog but not the policies. Only `platform_admin` can read the floor.
- The process and size limits in §4.2 and §5.2.
- Versions are kept forever.
- Without the compiler binary, IAM still boots, with the tool-policy writes disabled.
- An empty source is a valid set. There is no delete RPC for a set.

## Changelog (2026-11-15 review)

The builder check asked 7 questions. The adversarial challenge returned NEEDS REWORK, with 3
blockers, 13 major and 13 minor findings, and 8 questions. Sven decided three items: the gateway
service account (§3.6, §9.4), the floor always enforced (§3.4), and the upstream Dogwood
repository (§3.7). All the other findings were folded in as my calls:

- **Concurrency:** no compile in a transaction, a lock row, expected versions (§4.5).
- **Revisions:** hand-maintained integers with pin tests, and a schema revision (§6.2).
- **The job:** idempotent, stale held rows, one held row, a `discarded` status, and a
  recompile-pending rule for catalog writes (§4.6, §8).
- **Lowering:** lower once, with a proof as the first task (§3.8).
- **The semantics contract:** the event kind, one shape for each block, sums, key scope, the
  retention, and the decision corpus (§5.4, §10).
- **Validation:** a 4.14 check in IAM on every save (§5.3).
- **Process hardening:** env clearing, rlimits, a semaphore, concurrent pipes, kill on timeout
  (§5.2).
- **The API:** a rejection is a success `oneof` (§7), and each error has a status.
- **The catalog:** qualified names, custom and built-in tools, the import forms, nullable types,
  and limits (§4.1 to §4.3).
- **Other additions:** the empty-catalog tests, audit entries (§9.6), the `fail_mode` meaning
  (§9.3), the fan-out (§9.7), the capability key and
  the config toggle (§6.2), the corrected gate list (§5.1), the toolchain, the data model keys,
  one comparator, the size limits, the `@id` grammar, the authorization file list, the empty
  fetch answers, and the test gaps.

No finding was rejected. The challenger's two-call floor proposal was replaced by the SMA-752
merged set, which gives the same result.

**Alignment with SMA-752 (approved spec, 2026-11-15).** The SMA-752 session supplied its facts,
and this spec now follows them:

- the Cedar names: `Pgs::Gateway::Action::"<name_key>"`, the `InvokeTool` and `Unregistered`
  parent groups, `Pgs::Iam::*` principals, and the org resource. These replace `Paigasus::Tool`
  from the issue text;
- the context attributes, including `input_text` for a non-object input;
- one merged policy set, with a separate floor cache;
- the failure settings (D33, D34);
- the events under `iam.policy.>` with the org UUID, and a core NATS subscription for each
  replica;
- `catalog_names` and the settings in the fetch response.

SMA-752 also allows a floor `forbid` that names a tool. IAM stays stricter (generic only, §3.3).
SMA-752 confirmed that this is compatible.

One item stays open by design: the `formerly` rule for the current event is settled by the
decision corpus, not by this document.

**Tenant documentation (PR 2).** A `forbid` on argument values is a deny list, and it is not
robust, because argument keys keep their case and a model can vary values. The tenant docs
recommend `permit` allow lists with `has` guards (SMA-752's request).

## Changelog (2026-11-15 plan review)

Sven confirmed these two corrections after the plan review.

- **D1.** Lowering uses one `lower_many` call with one entry for each policy, not one
  `lower_with_distincter` call. The latter renumbers slots after a delete. PR 1 Task 1 proves both
  facts against the pinned Dogwood commit.
- **D5.** The Cedar principal type is `Pgs::Iam::Principal`, because every IAM principal PRN has the
  resource type `principal`. The earlier examples `Pgs::Iam::User` and `Pgs::Iam::ServiceAccount`
  were wrong.

The final review of PR 1 found five more places where this spec did not agree with the code.

- **§5.3 step 8.** `max_window` cannot be set above 24 h, because Dogwood caps every window at 24 h.
  The 7 days of ADR-0027 (g) is narrowed.
- **§4.4.** An identical save makes a new version when the live version has an old compiler
  generation or schema revision. This is how PR 1 clears `tool-policy-recompile-pending`.
- **§5.3 predicate subset.** `||` is removed. Dogwood source cannot make it. The `AnyOf` arm stays
  only for the proto contract.
- **§5.2.** The default `RLIMIT_CPU` is 8 s, not 15 s, so that it can fire before the 10 s timeout.
- **§6.2.** Each attempt of the boot probe has its own 3 s limit. Before, one attempt could use the
  full 10 s compile timeout, and readiness could wait 32 s.

The first local review of PR 1 found four more gaps.

- **§4.3.** An array whose `items` allow `null` is not mapped. Before, it mapped to a set, and the
  `null` element had no Cedar form.
- **§4.4.** An identical save checks that the live row is not newer than `expected_version`.
  Before, a concurrent save could make it return a newer version with no conflict.
- **§5.3 step 1.** A policy with more than one `@id` rejects the save.
- **§5.3 step 11.** New: a predicate deeper than 32 nodes rejects the save, so that every stored
  set decodes again.

The second local review of PR 1 found three more gaps. Sven approved each fix.

- **§5.2.** IAM makes itself non-dumpable on Linux before the first compiler child starts.
  Before, a child under the same UID could read `/proc/<IAM pid>/environ`, and the spec said that
  the child "cannot read" the credentials.
- **§4.2.** New: `max_catalog_response_bytes` (default 2 MiB) caps the whole catalog as one
  `ListToolsResponse`. Before, a catalog could pass the 4 MiB tonic limit and make every compile
  result too large. A 3 MiB default was measured and is too large for the 3 MiB result limit.
- **§4.2.** `ListTools` reads the tools and the catalog revision in one consistent read. Before,
  two reads could return new tools with an old revision.

## Changelog (2026-11-16 PR review)

The first CodeRabbit round on PR 478 found two more gaps.

- **§4.3.** The keywords next to a nullable `anyOf` apply to its branch. Before, `properties`
  next to `anyOf` were lost with no dropped-field entry, and a `type` next to `anyOf` was ignored.
- **§5.3 step 2.** A policy with more than one `@scope` rejects the save. Before, the history
  condition used the first `@scope`, and the emitted Cedar kept the last one.
- **§5.2.** IAM sets `FD_CLOEXEC` on every descriptor from 3 up before `exec`: `close_range` on
  Linux, an `fcntl` loop elsewhere. Before, CI saw two descriptors without the flag reach the child.
