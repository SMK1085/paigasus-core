# SMA-787 Event Inbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the durable event inbox of the agent host: CloudEvents ingress over HTTP and NATS,
one fenced lease per inbox, the per-thread claim, a minimal complete, and the tests that prove no
event is lost and no two workers hold a valid claim.

**Architecture:** A new pure crate `paigasus-cloudevents` parses and builds CloudEvents 1.0 JSON.
The core crate gets the revised ports and the `LeaseHandle` rules. The service crate gets one
Postgres adapter for the lease and claim SQL (hand-written, bound parameters), an inbox adapter, a
`LeaseKeeper`, an IAM gRPC adapter, the HTTP route, and a JetStream pull consumer. IAM gets the
action `WriteInboxEvent` and the role `agent_event_writer`.

**Tech Stack:** Rust 2024, axum 0.8, SeaORM 2 (raw statements), tonic 0.14, async-nats 0.53,
tokio-util 0.7 (`CancellationToken`), jiff 0.2, proptest 1, testcontainers (Postgres 16, NATS
2.10), Moon 2.6.5.

**Spec:** `docs/superpowers/specs/2026-11-20-sma-787-event-inbox-design.md` (revision 2, approved
at Gate 1, D6 = T1). Read it before each task. "§x" below means a section of that spec. The ADR
text is `docs/superpowers/specs/2026-11-18-sma-785-agent-host-adr-design.md` §4.

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell,
  Python, TOML and the proptest regression file; after the shebang line in a script).
- New crate `paigasus-cloudevents`: `edition.workspace = true`, `rust-version.workspace = true`
  (the workspace value is `1.95`; the toolchain is `1.100.0`), `[lints] workspace = true`,
  `publish = false`, version `0.0.0`. No I/O, no HTTP and no NATS dependency.
- `rs/Cargo.toml` `members` entries are literal paths. No `*`, `?` or `[` (A11).
- `warnings = deny`. Dead code is a hard error. Each task adds only what it uses or tests. An item
  in a library crate is `pub`, or a test in the same task uses it. Never "add now, wire later".
- A dependency enters in the task that first uses it. A dependency with no use reds
  `repo:machete`. Use workspace pins (`{ workspace = true }`) where the workspace has one.
- The workspace must not link `aws-lc-rs`, `aws-lc-sys`, `openssl-sys` or `native-tls`. tonic uses
  `tls-ring` and `tls-native-roots` (the gateway form). async-nats uses the workspace features.
- The core crate dependency set stays exactly `paigasus-kernel`, `uuid`, `jiff`, `thiserror`,
  `async-trait`, `serde`, `serde_json` (the A1 guard asserts set equality).
- Every time that decides that a lease is valid is the database clock (AG4). `now()` in a short
  transaction, `clock_timestamp()` where a statement can wait on a lock. No worker clock extends a
  lease.
- `inbox_lease.holder` is diagnostic only. It is never in a `WHERE` clause.
- All lease and claim SQL lives in one adapter, `PgLeaseStore` (D4). Every value is a bound
  parameter. The only formatted value is the integer of `SET LOCAL statement_timeout`.
- PRN comparisons compare UUIDs, never strings (§6.3).
- HTTP errors use the envelope `{"error":{"code":"…","message":"…"}}`. Only
  `src/adapters/http/error.rs` and `src/adapters/boot.rs` of the agent host spell a registry code.
  The gate matches a quoted literal, so a string such as `"forbidden"` or `"internal"` in any other
  `src/` file (a log field, a label) reds `repo:error-code-single-site`. Other files read a code
  through `ErrorReason` (the gateway `LazyLock` pattern, `gateway/src/adapters/http/auth.rs:453-480`).
- Metric names live in `paigasus-observability/src/names.rs` and in `names::ALL`, with one
  `describe_*` line each in `describe_agent_host_metrics()` (`main.rs`). A test reads a counter as a
  number (a `metrics_util::debugging::DebuggingRecorder` snapshot), never with `contains()` on a
  `# TYPE` line.
- Docker tests start containers only through `paigasus_test_docker::start_or_skip`. Each new Docker
  test binary goes into the agent-host `docker-containers` override of `rs/.config/nextest.toml` by
  name, and into the binary list of `tests/docker_preflight.rs`.
- Each security or fencing rule has a negative control: the task names the guard line to remove,
  and the named test must fail without it. Remove the line, run the test, see it fail, restore the
  line with an edit (not `git checkout --`, which also reverts the fix), and run it again.
- Shell: run each Rust command from `rs/`. First run
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
  Per crate: `cd rs && cargo nextest run --locked -p <crate>`. Moon project IDs:
  `paigasus-cloudevents-rs`, `paigasus-agent-host-core-rs`, `paigasus-agent-host-rs`,
  `paigasus-iam-core-rs`, `paigasus-iam-rs`.
- Run `/bin/bash ci/affected-graph/run.sh` (system bash 3.2) after every change to a `moon.yml` or
  to the dependency set of a crate. Add only the IDs that a FAIL row names.
- Commits: one per task. Conventional, workspace scope (`feat(rs): …`, `feat(contracts): …`,
  `build(repo): …`), subject ends with `(SMA-787)`, body lines of 100 characters or fewer, no body
  line that starts with `#` and a number, never `--no-verify`. The last line of each message is
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not install host software. Use `docker run ubuntu:24.04` if a tool is missing.
- Do not write the file name of moon's CI report (the JSON file under `.moon/cache`) in a new
  tracked file (`repo:actionlint` check 12).

## Review Focus

1. **A content type with a different case and a parameter**, for example
   `Application/CloudEvents+JSON; charset=UTF-8`. The route must accept it. `application/json` and
   `application/cloudevents+jsonx` must give `415`. Test in Task 14.
2. **`"data": null` against an absent `data`.** A CloudEvent attribute with the JSON value `null`
   is absent. So a duplicate with `"data": null` matches a stored event with no `data`
   (`payload_matches = true`). Tests in Task 1 and Task 5.
3. **A durable with exactly one filter subject.** The NATS server can report it in
   `filter_subject` and leave `filter_subjects` empty. The boot check must read both fields as one
   set. Test in Task 18.
4. **A multi-byte `id` at the length limit.** 256 characters of `é` is valid; 257 is not. The limit
   counts characters, not bytes. Test in Task 1.
5. **A lower-case `bearer` scheme and a token with outer spaces.** `authorization: bearer  key `
   must authenticate as `key`, as the gateway parser does. Test in Task 13.

---

## File map

New crate `rs/crates/libs/paigasus-cloudevents/`: `Cargo.toml`, `moon.yml`, `src/lib.rs`,
`src/event.rs`, `src/parse.rs`, `src/build.rs`, `src/error.rs`, `tests/iam_fixtures.rs`,
`tests/fixtures/iam/{principal_created_micros,organization_created_trailing_zeros,principal_archived_zero_fraction}.json`.

Core crate `rs/crates/libs/paigasus-agent-host-core/`: modify `src/inbox.rs`, `src/run.rs`,
`src/ports.rs`, `src/lib.rs`; create `src/lease.rs`, `src/claim.rs`, `src/mid_run.rs`.

Service crate `rs/crates/services/paigasus-agent-host/`:
- create `src/adapters/persistence/migration/m0002_create_inbox_event_tables.rs`,
  `src/adapters/persistence/time.rs`, `src/adapters/persistence/inbox_repository.rs`,
  `src/adapters/persistence/lease_store/{mod.rs,take.rs,claim.rs,complete.rs}`,
  `src/adapters/id.rs`, `src/application/{mod.rs,ingest.rs,lease_keeper.rs}`,
  `src/adapters/iam/{mod.rs,client.rs}`, `src/adapters/http/{auth.rs,path.rs,bytes.rs,ingress.rs,service_info.rs}`,
  `src/adapters/nats/{mod.rs,channels.rs,tenancy.rs,consumer.rs}`;
- create tests `tests/schema_pg.rs`, `tests/inbox_repository_pg.rs`, `tests/lease_pg.rs`,
  `tests/claim_pg.rs`, `tests/complete_pg.rs`, `tests/keeper_pg.rs`, `tests/lease_property_pg.rs`,
  `tests/lease_property_pg.proptest-regressions`, `tests/lease_stress_pg.rs`,
  `tests/lease_crash_pg.rs`, `tests/ingress_http.rs`, `tests/ingress_pg.rs`,
  `tests/ingress_latency_pg.rs`, `tests/nats_source.rs`, `tests/support/{fake_iam.rs,fake_inbox.rs,inbox.rs}`;
- modify `Cargo.toml`, `moon.yml`, `src/lib.rs`, `src/adapters/mod.rs`, `src/config.rs`,
  `src/main.rs`, `src/adapters/http/{mod.rs,error.rs}`, `src/adapters/persistence/{mod.rs,migration/mod.rs}`,
  `tests/{boot_pg.rs,docker_preflight.rs,support/mod.rs}`, `README.md`, `CHANGELOG.md`,
  `agent-host.toml.example`.

IAM: `rs/crates/libs/paigasus-iam-core/src/authz/{action.rs,schema.rs,roles.rs}`,
`rs/crates/services/paigasus-iam/src/application/bootstrap.rs` (doc text),
`rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs` (fixture test),
`rs/crates/services/paigasus-iam/moon.yml` (test inputs),
`rs/crates/services/paigasus-iam/tests/nats_permissions.rs`.

Contracts and bindings: `contracts/proto/paigasus/common/v1/error.proto`, the three generated trees
for `common/v1`, `rs/crates/libs/paigasus-proto/src/error.rs`,
`ts/packages/paigasus-proto/src/error.test.ts`, `ts/packages/paigasus-sdk/src/errors/presentation.ts`,
`ts/packages/paigasus-sdk/tests/presentation.test.ts`.

Elsewhere: `rs/Cargo.toml`, `rs/Cargo.lock`, `rs/release-plz.toml`, `rs/.config/nextest.toml`,
`rs/crates/libs/paigasus-observability/src/names.rs`, `ci/affected-graph/run.sh`,
`ci/error-registry/check.py`, `ci/http-extractor/check.py` (only if the gate asks),
`ops/nats/{subjects.env,provision.sh,check-subjects.sh,permissions.md,README.md,test/accounts.conf.tmpl}`.

**Decisions this plan makes inside the spec's room (§14):**

- One migration, `m0002_create_inbox_event_tables`, holds all five tables of §7.1. It adds
  `CHECK (status IN ('pending','done','dead'))` on `inbox_event` and a foreign key
  `run_event.event_id → inbox_event(id)` (added after `inbox_event` exists). `run.status` has no
  `CHECK`, because SMA-790 adds values.
- No SeaORM entity module. Every read and write is a raw `Statement` with bound values. Times cross
  the boundary as epoch microseconds (`(extract(epoch FROM x) * 1000000)::bigint`), so the service
  needs no `chrono`. Arrays bind through the sea-orm `postgres-array` feature (added in the
  agent-host manifest). `jsonb` binds as text with `::jsonb` and reads back as `::text`.
- The `acquire_next` candidate limit is 8. The property case count is 64.
- The ingress is on when the `[iam]` section is set. Without it, `/v1/inboxes/…` and
  `/v1/service-info` are not mounted (see the spec gaps in the hand-off report).
- `store-unavailable` gets the number 910 in the shared range (900-999).

---

### Task 1: The `paigasus-cloudevents` crate and the IAM fixtures

**Files:**
- Create: the crate files in the file map.
- Modify: `rs/Cargo.toml` (`members`, `[workspace.dependencies]`), `rs/Cargo.lock`,
  `rs/release-plz.toml`, `ci/affected-graph/run.sh`,
  `rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs` (one test),
  `rs/crates/services/paigasus-iam/moon.yml` (`test` and `test-build` inputs).

**Interfaces:**
- Produces (crate `paigasus_cloudevents`):
  - `pub enum ExtensionValue { String(String), Integer(i64), Boolean(bool) }` (`Debug, Clone, PartialEq, Eq`).
  - `pub struct CloudEvent` (`Debug, Clone, PartialEq`), private fields, with accessors
    `id() -> &str`, `source() -> &str`, `ty() -> &str`, `time() -> Option<jiff::Timestamp>`,
    `subject() -> Option<&str>`, `datacontenttype() -> Option<&str>`, `dataschema() -> Option<&str>`,
    `data() -> Option<&serde_json::Value>`, `extension(name) -> Option<&ExtensionValue>`,
    `schemaversion() -> Option<i64>`, `actorprn() -> Option<&str>`, `correlationid() -> Option<&str>`,
    `threadid() -> Option<&str>` (a typed accessor returns `None` when the value has another type).
  - Builders: `CloudEvent::new(id: uuid::Uuid, source: &str, ty: &str, time: jiff::Timestamp) -> Result<CloudEvent, SchemaError>`;
    `with_subject`, `with_datacontenttype`, `with_dataschema`, each `(self, &str) -> Result<Self, SchemaError>`;
    `with_data(self, serde_json::Value) -> Self`;
    `set_extension(&mut self, name: &str, value: ExtensionValue) -> Result<(), SchemaError>`
    (keeps insertion order; replaces in place); `remove_extension(&mut self, name: &str)`;
    `set_source(&mut self, source: &str) -> Result<(), SchemaError>`.
  - `pub fn parse(bytes: &[u8]) -> Result<CloudEvent, ParseError>`;
    `pub enum ParseError { Json(String), Schema(SchemaError) }`;
    `pub enum SchemaError` (thiserror) with at least `MissingAttribute(&'static str)`,
    `WrongType(&'static str)`, `SpecVersion`, `TooLong { attribute: String, max: usize }`,
    `InvalidExtensionName(String)`, `DataBase64NotSupported`, `InvalidTime`, `Empty(&'static str)`.
  - `impl CloudEvent { pub fn to_json(&self) -> serde_json::Value; pub fn to_bytes(&self) -> Vec<u8> }`.
  - Constants: `ID_MAX_CHARS = 256`, `TYPE_MAX_CHARS = 256`, `URI_MAX_CHARS = 1024` (source,
    subject, dataschema), `EXTENSION_STRING_MAX_CHARS = 1024`, `EXTENSION_NAME_MAX_CHARS = 20`.

- [ ] **Step 1: Write the crate files and register the crate.**
  - `Cargo.toml`: the `[package]` block of `rs/crates/libs/paigasus-agent-host-core/Cargo.toml:1-8`
    with the new name. `[dependencies]`: `serde_json`, `jiff`, `thiserror`, `uuid`, all
    `{ workspace = true }`. No `serde` derive is needed if you write `Serialize` by hand; add
    `serde` only if a derive uses it.
  - `moon.yml`: id `paigasus-cloudevents-rs`, layer `library`, language `rust`, empty `dependsOn`,
    `fileGroups.upstreams: []`, `build` and `test` with `deps: ['^:build']` (the form of
    `rs/crates/libs/paigasus-agent-host-core/moon.yml`, without the kernel lines).
  - `rs/Cargo.toml`: `"crates/libs/paigasus-cloudevents",` after
    `"crates/libs/paigasus-agent-host-core",`. Add
    `paigasus-cloudevents = { path = "crates/libs/paigasus-cloudevents", version = "0.0.0" }` next
    to `paigasus-agent-host-core` (`rs/Cargo.toml:214`).
  - `rs/release-plz.toml`: a `[[package]]` block `name = "paigasus-cloudevents"`, `release = false`,
    `publish = false`, `git_release_enable = false` (form at `:260-263`). Change "thirteen"
    (`:217`, `:221`) to "fourteen".

- [ ] **Step 2: Write the fixture producer test in IAM.**
  In `rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs` tests, add
  `the_cloudevents_crate_fixtures_are_this_encoders_bytes`. It builds three `DomainEvent`s and
  compares `serde_json::to_vec(&CloudEvent::from_domain_event(&ev, "urn:paigasus:iam"))` with
  `include_bytes!("../../../../../libs/paigasus-cloudevents/tests/fixtures/iam/<name>.json")`.
  On a mismatch it prints the actual bytes as UTF-8. The values (`u(n) = Uuid::from_u128(n)`):

  | Fixture | id | type | aggregate (subject) | actor | correlation | occurred_at | payload |
  |---|---|---|---|---|---|---|---|
  | `principal_created_micros` | `u(0x1234)` | `PrincipalCreated` | `prn:pgs:iam:::principal/<u(0xbb)>` | `prn:pgs:iam:::principal/<u(0xaa)>` | `u(9)` | 2026-08-07T12:30:45.123456Z | `{"kind":"user","nested":{"a":1}}` |
  | `organization_created_trailing_zeros` | `u(0x2)` | `OrganizationCreated` | `prn:pgs:iam:::organization/<u(0x500)>` | none | none | 2026-08-07T12:30:45.100000Z | `{"slug":"acme"}` |
  | `principal_archived_zero_fraction` | `u(0x3)` | `PrincipalArchived` | empty (no subject) | none | none | 2026-08-07T12:30:45.000000Z | `{}` |

  Every payload object has its keys in alphabetical order. So the bytes do not depend on the
  `serde_json/preserve_order` feature, which `paigasus-gateway-core` turns on in some builds only.
  Write the three files from the printed output. Each file has no trailing newline. Fixture files
  are data: they carry no SPDX line (a JSON file cannot hold a comment).

  In `rs/crates/services/paigasus-iam/moon.yml`, add
  `'/rs/crates/libs/paigasus-cloudevents/tests/fixtures/**/*'` to the `inputs` of `test` and of
  `test-build` (the SMA-774 pattern at `rs/crates/libs/paigasus-iam-core/moon.yml:28-30`), with a
  comment that names this test.

- [ ] **Step 3: Write the failing crate tests.**

```rust
// src/parse.rs tests
parses_a_minimal_event                       // {"specversion":"1.0","id":"a","source":"s","type":"t"} -> Ok; data() None
rejects_a_missing_required_attribute         // each of specversion/id/source/type removed -> Schema(MissingAttribute(name))
rejects_spec_version_0_3                     // -> Schema(SpecVersion)
rejects_data_base64                          // -> Schema(DataBase64NotSupported)
invalid_json_is_a_json_error                 // b"{" -> ParseError::Json(_); b"[]" -> Schema(_)
a_null_attribute_is_absent                   // Review Focus 2: "subject":null and "data":null -> subject() None, data() None
extensions_are_typed                         // "schemaversion":1 -> Integer(1), schemaversion()==Some(1); "flag":true -> Boolean; "threadid":7 -> threadid()==None
an_extension_name_must_be_lowercase_alnum_up_to_20 // "Bad", "a_b", 21 chars -> InvalidExtensionName; 20 chars Ok
an_object_extension_value_is_rejected        // "ext":{"a":1} -> WrongType
length_limits_count_characters               // Review Focus 4: id "é"x256 Ok, x257 TooLong; type 256/257; source, subject, dataschema 1024/1025; string extension 1024/1025
time_must_be_rfc3339                         // "time":"yesterday" -> InvalidTime
// src/build.rs tests
build_prints_six_fraction_digits_and_truncates // time 12:30:45.123456789Z -> "2026-08-07T12:30:45.123456Z" (truncated, not rounded); .1Z -> ".100000Z"; whole second -> ".000000Z"
build_writes_a_hyphenated_uuid_id            // id() == "00000000-0000-0000-0000-000000001234"
set_extension_keeps_insertion_order          // set schemaversion, actorprn, correlationid -> to_bytes() has them in that order
set_source_checks_the_length_limit           // 1025 chars -> Err(TooLong)
```

```rust
// tests/iam_fixtures.rs (reads the same three files with include_bytes!)
the_crate_parses_every_iam_fixture
    // principal_created_micros: id "00000000-0000-0000-0000-000000001234", ty "iam.principal.created",
    // schemaversion Some(1), actorprn and correlationid Some(..), time 12:30:45.123456Z, data == {"kind":"user","nested":{"a":1}}
    // the two others: subject Some/None as the table says, time .100000 and .000000
the_crate_builds_the_iam_bytes_from_the_same_values
    // CloudEvent::new(u(..), "urn:paigasus:iam", <type>, <time>)?.with_subject?.with_datacontenttype("application/json")?
    // then set_extension schemaversion Integer(1), actorprn, correlationid (only when present), with_data(..)
    // assert_eq!(event.to_bytes(), include_bytes!(..).to_vec()) for all three
```

- [ ] **Step 4: Run and see them fail.**
  Run: `cd rs && cargo build -p paigasus-cloudevents && cargo nextest run --locked -p paigasus-cloudevents`
  Expected: compile errors. (The first build runs without `--locked` because the new crate changes
  `rs/Cargo.lock`.)

- [ ] **Step 5: Implement the crate.**
  - Parse into a `serde_json::Map`, then take each known attribute out; every other key is an
    extension. `null` means absent for every attribute.
  - `Serialize` by hand (or build `to_json` by hand) in this fixed order: `specversion`, `id`,
    `source`, `type`, `subject`, `time`, `datacontenttype`, `dataschema`, the extensions in
    insertion order, `data`. This is IAM's order (`cloud_event.rs:29-46`).
  - Time output: truncate to whole microseconds with `Timestamp::from_microsecond(t.as_microsecond())`,
    then print `%Y-%m-%dT%H:%M:%S` plus a dot, exactly six digits, and `Z`.

- [ ] **Step 6: Run and see them pass.**
  Run: `cd rs && cargo nextest run --locked -p paigasus-cloudevents -p paigasus-iam -E 'test(cloudevents) | package(paigasus-cloudevents)' && cargo clippy --locked -p paigasus-cloudevents --all-targets -- -D warnings && cargo fmt --check`
  Expected: PASS. The IAM test passes, because the fixtures are its own bytes.

- [ ] **Step 7: Update the affected-graph expectations.** Run `/bin/bash ci/affected-graph/run.sh`.
  Expected first: FAIL rows `lockfile->all-lint` and `lockfile->all-lint-ci`
  (`paigasus-cloudevents-rs:lint`). Add exactly those IDs in the list order style. The crate does
  not depend on the kernel, so `kernel->bindings` does not change. Run again: no FAIL row.

- [ ] **Step 8: Commit.**

```bash
git add rs/crates/libs/paigasus-cloudevents rs/Cargo.toml rs/Cargo.lock rs/release-plz.toml ci/affected-graph/run.sh rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs rs/crates/services/paigasus-iam/moon.yml
git commit -m "feat(rs): add the paigasus-cloudevents crate with IAM fixtures (SMA-787)" -m "Spec section 4. CloudEvents 1.0 structured mode with typed extensions, length limits and
six-digit times. IAM's encoder test pins the three byte fixtures that the crate parses and builds." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Core types and the revised ports

**Files:**
- Create: `rs/crates/libs/paigasus-agent-host-core/src/{lease.rs,claim.rs,mid_run.rs}`
- Modify: `src/inbox.rs`, `src/ports.rs`, `src/lib.rs`

**Interfaces:**
- Consumes: SMA-786 IDs (`InboxId`, `EventId`, `RunId`, `ThreadId`), `LeaseToken`, `Inbox`.
- Produces:
  - `inbox`: `NewInboxEvent { pub id: EventId, pub inbox: InboxId, pub source: String, pub ce_id: String, pub ce_type: String, pub thread: Option<ThreadId>, pub envelope: serde_json::Value }`
    (`Debug, Clone, PartialEq`). It has no `status`, `attempts`, `claimed_run`, `received_at` or
    `not_before`. `InboxEvent` gains `pub envelope: serde_json::Value` (keep `data`; it is
    `envelope["data"]` or `Value::Null`).
  - `ports`: `#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)] #[error("store error: {0}")] pub struct StoreError(pub String);`
    `InsertOutcome { Inserted { id: EventId }, Duplicate { id: EventId, payload_matches: bool } }`.
    `InboxRepository`: `get_by_uuid(&self, id: Uuid) -> Result<Option<Inbox>, RepositoryError>`;
    `insert_event(&self, event: NewInboxEvent) -> Result<InsertOutcome, RepositoryError>`. Remove
    the SMA-786 `get(&InboxId)`.
    `LeaseRepository`: `acquire_next(&self, holder: &str, ttl: Duration) -> Result<Option<LeaseHandle>, StoreError>`;
    `acquire(&self, inbox: &InboxId, holder: &str, ttl: Duration) -> Result<Option<LeaseHandle>, StoreError>`;
    `heartbeat(&self, handle: &LeaseHandle, ttl: Duration) -> Result<(), HeartbeatError>`;
    `release(&self, handle: LeaseHandle) -> Result<(), StoreError>`.
    `ClaimRepository`: `claim(&self, handle: &mut LeaseHandle, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError>`;
    `complete(&self, handle: &mut LeaseHandle, claim: Claim) -> Result<Vec<EventId>, CompleteError>`.
    Doc comments: owner SMA-787; `complete` is replaced by `RunCommitter` in SMA-790.
  - `lease`: `LeaseHandle` (`Debug`, NOT `Clone`), private fields `inbox: InboxId, token: LeaseToken,
    holder: String, open_claim: Option<RunId>, poisoned: bool, claimed_before: bool`. Methods:
    `pub fn new(inbox: InboxId, token: LeaseToken, holder: String) -> Self`; `inbox(&self) -> &InboxId`;
    `token(&self) -> LeaseToken`; `holder(&self) -> &str`; `open_claim(&self) -> Option<RunId>`;
    `is_poisoned(&self) -> bool`; `needs_rotation(&self) -> bool` (true after the first claim);
    `check_claim(&self) -> Result<(), ClaimError>` (poisoned → `Poisoned`, open claim → `ClaimOpen`);
    `record_claim(&mut self, token: LeaseToken, run: Option<RunId>)` (sets the token, sets
    `claimed_before`, sets `open_claim` to `run`); `record_complete(&mut self)`; `poison(&mut self)`.
    Errors (all `Debug, Clone, PartialEq, Eq, thiserror::Error`):
    `HeartbeatError { LeaseLost, Store(String) }`, `ClaimError { LeaseLost, ClaimOpen, Poisoned, Store(String) }`,
    `CompleteError { LeaseLost, Store(String) }`. A poisoned handle gives `LeaseLost` from
    `heartbeat` and `complete`, with no SQL.
  - `claim`: `ClaimConfig { pub batch_limit: u32, pub max_attempts: u32, pub thread_ttl: Duration }`;
    `Claim { pub run: RunId, pub thread: ThreadId, pub lease_token: LeaseToken, pub fresh_thread: bool, pub events: Vec<InboxEvent> }`
    (`Debug, Clone, PartialEq`).
  - `mid_run`: `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum MidRunPolicy { Queue, Fold, Interrupt }`.
  - `Duration` is `std::time::Duration` (no new dependency).

- [ ] **Step 1: Write the failing tests.**

```rust
// lease.rs tests (u(n) = Uuid::from_u128(n); h() = LeaseHandle::new(InboxId::new(u(1),u(2)), LeaseToken(1), "w".into()))
a_new_handle_allows_a_claim_and_needs_no_rotation   // check_claim()==Ok(()); !needs_rotation()
after_a_claim_the_handle_needs_rotation              // record_claim(LeaseToken(1), None); needs_rotation()
an_open_claim_refuses_a_second_claim                 // record_claim(LeaseToken(2), Some(run)); check_claim()==Err(ClaimError::ClaimOpen)
record_complete_clears_the_open_claim                // then record_complete(); check_claim()==Ok(()); token()==LeaseToken(2)
a_poisoned_handle_refuses_every_claim                // poison(); check_claim()==Err(ClaimError::Poisoned) also with no open claim
lease_handle_is_not_clone
    // trait AmbiguousIfClone<A> { fn some() {} } impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    // impl<T: ?Sized + Clone> AmbiguousIfClone<u8> for T {}
    // let _ = <LeaseHandle as AmbiguousIfClone<_>>::some;  // compiles only while LeaseHandle is not Clone
// mid_run.rs
mid_run_policy_reads_snake_case                     // serde_json::from_str::<MidRunPolicy>("\"queue\"") == Ok(Queue); "fold", "interrupt" parse too
// lib.rs port test: add assert_port::<dyn LeaseRepository>() and assert_port::<dyn ClaimRepository>()
```

  Negative control for `an_open_claim_refuses_a_second_claim`: remove the `open_claim` arm of
  `check_claim`. The test must fail.

- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host-core`. Expected: compile errors.
- [ ] **Step 3: Implement the three modules and the port changes.** Re-export the new modules as
  `pub mod` from `lib.rs`. Update every existing construction of `InboxEvent` (for example in
  `src/value_object_tests.rs`) with an `envelope`. The A1 guard does not change.
- [ ] **Step 4: Run and see them pass.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host-core && cargo build --locked -p paigasus-agent-host && cargo clippy --locked -p paigasus-agent-host-core --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS (the service crate has no implementor of the old `InboxRepository`, so it still builds).
- [ ] **Step 5: Commit.** `feat(rs): add the lease, claim and inbox port types of the event inbox (SMA-787)`,
  body names §8.1 and §11, then the co-author line.

---

### Task 3: The inbox tables (m0002)

**Files:**
- Create: `src/adapters/persistence/migration/m0002_create_inbox_event_tables.rs`, `tests/schema_pg.rs`, `tests/support/inbox.rs`
- Modify: `src/adapters/persistence/migration/mod.rs`, `tests/boot_pg.rs:36,51,55`,
  `tests/support/mod.rs`, `tests/docker_preflight.rs`, `rs/.config/nextest.toml`

**Interfaces:**
- Produces: the five tables of §7.1, exactly, plus the decisions in the file map section.
- Produces (tests): `tests/support/inbox.rs` with
  `pub async fn migrated_postgres() -> Option<(ContainerAsync<Postgres>, String, DatabaseConnection)>`
  (start, connect, `Migrator::up`) and
  `pub async fn insert_inbox(db: &DatabaseConnection, org: Uuid, id: Uuid, agent: Uuid, principal: Uuid)`.
  Tests may call `Migrator::up` directly; the composition-root guard reads only `main.rs`.

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/schema_pg.rs
the_dedup_key_refuses_a_second_row               // two inbox_event rows (inbox, source, ce_id) equal -> unique violation
the_same_ce_id_from_two_sources_gives_two_rows   // source differs -> both insert
an_event_cannot_name_a_thread_of_another_inbox   // thread in inbox B, event in inbox A with that thread_id -> FK violation (AG5, AG6)
a_run_cannot_name_a_thread_of_another_inbox      // run.thread_id from inbox B, run.inbox_id A -> FK violation
an_event_cannot_name_a_run_of_another_inbox      // claimed_run_id of a run in inbox B -> FK violation
the_status_check_refuses_claimed                 // status 'claimed' -> check violation; 'pending','done','dead' Ok
the_claimable_index_is_partial_on_pending        // pg_indexes.indexdef for inbox_event_claimable contains "WHERE (status = 'pending'::text)"
no_index_names_the_holder_column                 // holder is diagnostic only: pg_indexes has no indexdef on inbox_lease that names holder
```

  Change `tests/boot_pg.rs`: `applied(&db).await` equals `Migrator::migrations().len() as i64`
  (three places), and the messages say "every migration". Add `schema_pg` to the agent-host
  `docker-containers` filter in `rs/.config/nextest.toml` and to the list and the count in
  `tests/docker_preflight.rs`.

  Negative control for `an_event_cannot_name_a_thread_of_another_inbox`: change the composite
  foreign key to `FOREIGN KEY (thread_id) REFERENCES thread (id)`. The test must fail.

- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test schema_pg --test boot_pg`. Expected: FAIL (tables missing).
- [ ] **Step 3: Implement m0002.** One `execute_unprepared` per statement, in this order: `thread`,
  `inbox_lease`, `run`, `run_event`, `inbox_event`, `ALTER TABLE run_event ADD FOREIGN KEY
  (event_id) REFERENCES inbox_event (id)`, the partial index. `down` drops in reverse order. Add
  the migration to `Migrator::migrations()`.
- [ ] **Step 4: Run and see them pass.** Same command, plus `cargo nextest run --locked -p paigasus-agent-host`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the inbox event, thread, lease and run tables (SMA-787)`,
  body names §7.1 and the composite keys, then the co-author line.

---

### Task 4: The inbox adapter and the one insert path

**Files:**
- Create: `src/adapters/persistence/time.rs`, `src/adapters/persistence/inbox_repository.rs`,
  `src/application/mod.rs`, `src/application/ingest.rs`, `tests/inbox_repository_pg.rs`
- Modify: `src/lib.rs`, `src/adapters/persistence/mod.rs`, the service `Cargo.toml` (sea-orm
  feature `postgres-array`; dev `metrics-util`), `src/main.rs` (describe lines),
  `rs/crates/libs/paigasus-observability/src/names.rs`, `rs/.config/nextest.toml`, `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: Task 2 `InboxRepository`, `NewInboxEvent`, `InsertOutcome`; Task 3 tables.
- Produces:
  - `persistence::time::{to_micros(t: Timestamp) -> i64, from_micros(us: i64) -> Result<Timestamp, StoreError>}`.
  - `persistence::inbox_repository::PgInboxRepository { new(db: DatabaseConnection) -> Self }`
    implementing `InboxRepository`. Also `pub(crate) fn row_to_event(row: &QueryResult) -> Result<InboxEvent, StoreError>`
    for Task 6, which reads the same columns.
  - `application::ingest::{Channel, ingest}`: `pub enum Channel { Http, Nats }` with
    `as_label(&self) -> &'static str` (`"http"`, `"nats"`);
    `pub async fn ingest(repo: &dyn InboxRepository, event: NewInboxEvent, channel: Channel) -> Result<InsertOutcome, RepositoryError>`.
  - `names::{AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL = "agent_host_inbox_events_inserted_total",
    AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL = "agent_host_inbox_events_duplicate_total",
    AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL = "agent_host_inbox_events_payload_mismatch_total"}`,
    each with the label `channel`, in `ALL`.

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/inbox_repository_pg.rs (support::inbox::migrated_postgres)
get_by_uuid_returns_the_inbox_with_its_org       // insert_inbox(org u(1), id u(2), agent u(3), principal u(4)); get_by_uuid(u(2)) -> Some; org()==u(1); principal().uuid()==u(4)
get_by_uuid_of_an_unknown_inbox_is_none
an_insert_returns_the_new_id_and_the_row_is_pending // Inserted{id}; row status 'pending', attempts 0, claimed_run_id NULL
a_duplicate_returns_the_stored_id_and_writes_no_row // second insert, new EventId, same key -> Duplicate{id: first id, payload_matches: true}; count(*) == 1
a_duplicate_with_other_data_reports_a_mismatch   // same key, data {"a":2} against {"a":1} -> payload_matches false
a_duplicate_with_another_type_reports_a_mismatch // ce_type differs -> payload_matches false
absent_and_null_data_match                       // Review Focus 2: stored envelope without "data", new envelope with "data": null -> payload_matches true
a_thread_of_another_inbox_is_stored_as_null      // NewInboxEvent.thread = a thread row of inbox B -> Inserted; thread_id IS NULL (no FK error)
a_thread_of_this_inbox_is_kept                   // thread row in this inbox -> thread_id == that id
a_store_error_is_a_repository_store_error        // closed pool (db.close() first) -> Err(RepositoryError::Store(_))
// src/application/ingest.rs unit tests (an in-memory InboxRepository in the test module, a DebuggingRecorder)
ingest_counts_an_insert_per_channel              // counter agent_host_inbox_events_inserted_total{channel="http"} == 1
ingest_counts_a_duplicate_and_a_mismatch         // Duplicate{payload_matches:false} -> duplicate_total == 1 and payload_mismatch_total == 1
```

  Register `inbox_repository_pg` in `nextest.toml` and `docker_preflight.rs`.

  Negative control for `a_thread_of_another_inbox_is_stored_as_null`: replace the thread subselect
  with the raw bound thread id. The test must fail with a foreign-key error.

- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -E 'binary(inbox_repository_pg) | test(ingest)'`. Expected: compile errors.
- [ ] **Step 3: Implement.**
  - Insert (one statement):

```sql
INSERT INTO inbox_event (id, inbox_id, source, ce_id, ce_type, thread_id, envelope)
VALUES ($1, $2, $3, $4, $5,
        (SELECT t.id FROM thread t WHERE t.id = $6 AND t.inbox_id = $2),
        $7::jsonb)
ON CONFLICT (inbox_id, source, ce_id) DO NOTHING
RETURNING id
```

  - No row: read `id`, `ce_type`, `(envelope -> 'data')::text` of the stored row by the key. Compare
    in Rust: `ce_type` equal and `data` equal as `serde_json::Value`, where SQL `NULL` and JSON
    `null` both become `None`.
  - `ingest` logs a mismatch at `warn` with the inbox UUID, `source` and `ce_id`, never the data.
    It increments the counters after the repository call returns `Ok`.
  - Counter tests: a `#[tokio::test]` (current thread) installs a `DebuggingRecorder` with
    `metrics::set_default_local_recorder` and reads the counter value from the snapshot as a
    `u64`. Later tasks read their counters the same way.
  - Add the three `describe_counter!` lines to `describe_agent_host_metrics()`.
- [ ] **Step 4: Run and see them pass.** Same command, then `cargo clippy --locked -p paigasus-agent-host --all-targets -- -D warnings && cargo fmt --check && cargo nextest run --locked -p paigasus-observability`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the inbox insert with dedup and payload comparison (SMA-787)`,
  body names §7.2 and the three counters, then the co-author line.

---

### Task 5: The lease adapter (take, acquire_next, heartbeat, release)

**Files:**
- Create: `src/adapters/persistence/lease_store/{mod.rs,take.rs}`, `tests/lease_pg.rs`
- Modify: `src/adapters/persistence/mod.rs`, `src/main.rs` (describe line), `names.rs`,
  `rs/.config/nextest.toml`, `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: Task 2 `LeaseRepository`, `LeaseHandle`, `HeartbeatError`, `StoreError`.
- Produces:
  - `lease_store::PgLeaseStore { pub fn new(db: DatabaseConnection, heartbeat_statement_timeout: Duration) -> Self }`
    implementing `LeaseRepository` (Task 6 adds `ClaimRepository` to the same struct).
  - `pub const CANDIDATE_LIMIT: u64 = 8;` and `pub const CANDIDATES_SQL: &str` (the candidate
    query; a test runs it in an open transaction).
  - `names::AGENT_HOST_LEASE_LOST_TOTAL = "agent_host_lease_lost_total"` (label `operation`:
    `heartbeat`, `claim`, `complete`, `deadline`), in `ALL`.

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/lease_pg.rs. expire(db, inbox) = UPDATE inbox_lease SET expires_at = now() - interval '1 second' WHERE inbox_id = $1
the_first_take_of_a_new_inbox_has_token_1        // acquire -> Some(handle), token()==LeaseToken(1); no lease row before
a_second_take_while_live_is_none                 // acquire again -> None
a_take_after_expiry_increments_the_token         // expire; acquire -> Some, token 2; row holder is the new holder
tokens_never_repeat_after_release                // acquire(t1), release, acquire -> token 2 (the row is never deleted)
heartbeat_extends_the_lease                      // heartbeat Ok; expires_at grows
heartbeat_after_expiry_is_lease_lost             // expire; heartbeat -> Err(HeartbeatError::LeaseLost); counter operation="heartbeat" == 1
heartbeat_with_a_stale_token_is_lease_lost       // a takes (t1), expire, b takes (t2); heartbeat(a) -> LeaseLost; b's expires_at unchanged
a_database_error_is_store_not_lease_lost         // db.close(); heartbeat -> Err(HeartbeatError::Store(_))
acquire_next_takes_the_inbox_with_the_oldest_pending_event // inboxes A (event at t0) and B (event at t0+1 s, set received_at by SQL): first acquire_next -> A, second -> B, third -> None
acquire_next_skips_an_inbox_with_a_live_lease
acquire_next_skips_an_inbox_whose_events_wait_on_not_before // not_before = now() + 1 h -> None
acquire_next_on_many_inboxes_gives_one_holder_each
    // 20 inboxes with one event each; 8 tasks, each with connect_pinned, loop acquire_next until None;
    // collect InboxId per handle: no inbox twice; all 20 taken
an_open_acquirer_does_not_block_an_insert
    // conn 1: BEGIN; run CANDIDATES_SQL (keep the txn open); conn 2: insert_event into a candidate inbox
    // within tokio::time::timeout(2 s) -> Ok(Inserted); then roll back conn 1
```

  Negative controls:
  - `heartbeat_with_a_stale_token_is_lease_lost`: remove `AND token = $2` from the heartbeat
    `WHERE`. The test must fail.
  - `an_open_acquirer_does_not_block_an_insert`: change `FOR NO KEY UPDATE OF inbox` to
    `FOR UPDATE OF inbox`. The test must time out.
  - `a_second_take_while_live_is_none`: remove `WHERE inbox_lease.expires_at < clock_timestamp()`
    from the take. The test must fail.

- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test lease_pg`. Expected: compile errors.
- [ ] **Step 3: Implement.**
  - The take and the heartbeat are the SQL of §8.3, with `$ttl` bound as microseconds:
    `clock_timestamp() + ($3::bigint * interval '1 microsecond')`. Both read the live clock,
    because each can wait for a row lock (spec §8).
  - `acquire`: the take alone (one statement). `acquire_next`: one transaction. First
    `CANDIDATES_SQL`, then the take for each candidate in order; the first token commits and
    returns. No token: commit and return `None`.

```sql
-- CANDIDATES_SQL ($1 = CANDIDATE_LIMIT)
SELECT i.id, i.org_id
FROM inbox i
WHERE EXISTS (SELECT 1 FROM inbox_event e
              WHERE e.inbox_id = i.id AND e.status = 'pending' AND e.not_before <= now())
  AND NOT EXISTS (SELECT 1 FROM inbox_lease l WHERE l.inbox_id = i.id AND l.expires_at >= now())
ORDER BY (SELECT min(e.received_at) FROM inbox_event e
          WHERE e.inbox_id = i.id AND e.status = 'pending' AND e.not_before <= now())
LIMIT $1
FOR NO KEY UPDATE OF i SKIP LOCKED
```

    The candidate filter has no `attempts < max_attempts` term: `acquire_next` has no
    `ClaimConfig`, and an inbox with only exhausted events must still be taken, so that its claim
    moves them to `dead` (§8.4.1). See spec gap 1 in the hand-off report.
  - `heartbeat`: one transaction: `SET LOCAL statement_timeout = '<ms>ms'` (the constructor value),
    then the §8.3 heartbeat `UPDATE`. Zero rows: `LeaseLost` and the counter. A poisoned handle:
    `LeaseLost`, no SQL, no counter.
  - `release`: `UPDATE inbox_lease SET expires_at = clock_timestamp() - interval '1 microsecond'
    WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp()`. Zero rows is `Ok(())`.
    A poisoned handle: `Ok(())`, no SQL (the lease expires on its own).
- [ ] **Step 4: Run and see them pass.** Same command, then clippy and fmt. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the fenced inbox lease with take, heartbeat and release (SMA-787)`,
  body names §8.3, AG4 and the `FOR NO KEY UPDATE` choice, then the co-author line.

---

### Task 6: The claim (rotation, dead events, thread resolution, grouping)

**Files:**
- Create: `src/adapters/persistence/lease_store/claim.rs`, `src/adapters/id.rs`, `tests/claim_pg.rs`
- Modify: `lease_store/mod.rs`, `src/adapters/mod.rs`, the service `Cargo.toml` (`paigasus-kernel`,
  `rand`), `moon.yml` (`dependsOn: paigasus-kernel-rs`; `upstreams` does not change), `src/main.rs`,
  `names.rs`, `nextest.toml`, `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: Task 5 `PgLeaseStore`; Task 4 `row_to_event`; Task 2 `ClaimRepository`, `ClaimConfig`, `Claim`.
- Produces:
  - `adapters::id::mint_uuid() -> Uuid` (kernel `mint_uuid7` with the host clock and
    `rand::random::<[u8; 10]>()`, as `paigasus-iam/src/adapters/id.rs:16-19`).
  - `impl PgLeaseStore { pub async fn claim_events(&self, handle: &mut LeaseHandle, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError> }`.
    This is an inherent method. Task 7 adds `impl ClaimRepository for PgLeaseStore`, which
    delegates `claim` to it. A stub `complete` that returns an error is not allowed.
  - `pub const DEAD_REASON: &str = "max attempts reached without a commit";`
  - `names::{AGENT_HOST_CLAIMS_TOTAL = "agent_host_claims_total", AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL = "agent_host_inbox_events_dead_total"}`, in `ALL`.

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/claim_pg.rs. cfg() = ClaimConfig { batch_limit: 20, max_attempts: 5, thread_ttl: 86400 s }
the_first_claim_uses_the_take_token              // acquire (t1), claim -> Some(claim), claim.lease_token == 1, handle.token()==1
a_second_claim_rotates_the_token                 // claim on an empty inbox -> Ok(None) (sets claimed_before); insert one event; claim -> lease_token == 2; lease row token == 2
a_claim_on_an_open_claim_is_claim_open           // claim Some, claim again -> Err(ClaimOpen); attempts of the events unchanged
a_claim_after_expiry_is_lease_lost               // expire; claim -> Err(LeaseLost); no run row; counter operation="claim" == 1
no_claimable_event_is_ok_none_and_keeps_the_lease // empty inbox: Ok(None); heartbeat Ok
events_of_one_thread_are_claimed_together        // thread T live; 3 events on T, 2 with no thread; oldest is on T -> claim.events == the 3, thread == T, fresh_thread true (last_run_at null)
the_batch_limit_caps_the_claim                   // 5 events no thread, batch_limit 2 -> 2 events, the 2 oldest by received_at
no_thread_events_get_a_new_thread                // claim.thread is a new row with predecessor_id NULL; each event.thread_id == claim.thread
an_expired_thread_gets_a_successor               // thread T with last_run_at = now() - 2 days; event on T -> new thread N, N.predecessor_id == T, T.successor_id still NULL
a_closed_thread_gets_a_successor                 // T.closed = true -> same as above
a_successor_chain_resolves_to_its_end            // T1.successor = T2 (live); event on T1 -> claim.thread == T2
a_cycle_is_treated_as_no_thread                  // T1.successor = T2, T2.successor = T1; event on T1 -> claimed with the no-thread group, new thread without predecessor
not_before_in_the_future_is_not_claimed          // not_before = now() + 1 h -> Ok(None)
exhausted_events_move_to_dead                    // attempts = 5 (max 5) -> Ok(None); status 'dead', last_error == DEAD_REASON; dead counter == 1
a_claim_marks_events_and_writes_run_event_rows   // attempts 1, claimed_run_id == run, status still 'pending'; run row 'running' with the token; one run_event row per event
a_failed_commit_poisons_the_handle
    // test-only: CREATE CONSTRAINT TRIGGER refuse_run AFTER INSERT ON run DEFERRABLE INITIALLY DEFERRED
    //   FOR EACH ROW EXECUTE FUNCTION <raises>; claim -> Err(ClaimError::Store(_)); handle.is_poisoned();
    // next claim -> Err(Poisoned) with no SQL; claims counter == 0; then DROP TRIGGER
```

  `acquire` cannot run while the lease is live, so this test needs no expire.

  Negative controls:
  - `a_claim_on_an_open_claim_is_claim_open`: remove the `handle.check_claim()?` call at the start
    of `claim_events`. The test must fail.
  - `a_claim_after_expiry_is_lease_lost`: remove `AND expires_at > clock_timestamp()` from the
    step 2 lock query (and from the rotation, for the rotation case). The test must fail.
  - `events_of_one_thread_are_claimed_together` (AG5): group by the event's raw `thread_id`, not the
    resolved thread, in a chain case. `a_successor_chain_resolves_to_its_end` must fail.

- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test claim_pg`. Expected: compile errors.
- [ ] **Step 3: Implement `claim_events`.** `check_claim()` first, then one transaction with the
  steps of §8.4. Use the handle token `T` (after rotation, the returned token). The SQL that the
  spec does not write out:

```sql
-- the "free" predicate of §8.4 step 4, reused by step 3 and step 4 ($1 inbox, $2 token T)
(e.claimed_run_id IS NULL OR NOT EXISTS (
   SELECT 1 FROM run r
   WHERE r.id = e.claimed_run_id AND r.status = 'running' AND r.lease_token = $2))

-- step 3 ($3 max_attempts)
UPDATE inbox_event e SET status = 'dead', last_error = $4
WHERE e.inbox_id = $1 AND e.status = 'pending' AND e.not_before <= now()
  AND e.attempts >= $3 AND <free> RETURNING e.id

-- step 4: id, thread_id, received_at of claimable events, ORDER BY received_at, id

-- step 5 ($2 = uuid[] of the distinct non-null thread_ids)
WITH RECURSIVE chain (start_id, id, successor_id) AS (
  SELECT t.id, t.id, t.successor_id FROM thread t WHERE t.inbox_id = $1 AND t.id = ANY($2)
  UNION ALL
  SELECT c.start_id, t.id, t.successor_id
  FROM chain c JOIN thread t ON t.id = c.successor_id AND t.inbox_id = $1
) CYCLE id SET is_cycle USING path
SELECT start_id, id AS end_id, is_cycle FROM chain WHERE successor_id IS NULL OR is_cycle
```

    A start with an `is_cycle` row resolves to no thread; log it at `warn` once per start. Step 6
    groups in Rust: the key of an event is its resolved thread (`Option<Uuid>`). Take the key of
    the oldest event, then the events with that key, in `received_at` order, up to `batch_limit`.
    Liveness of the resolved thread: `SELECT closed, last_run_at IS NULL OR last_run_at >= now() -
    ($n::bigint * interval '1 microsecond') AS recent, last_run_at IS NULL AS never_run FROM thread
    WHERE id = $1`. A new thread gets `mint_uuid()`. Step 8 is one `UPDATE … WHERE id = ANY($ids)
    RETURNING` with every column `row_to_event` reads.
  - After `COMMIT` is `Ok`: `handle.record_claim(T, run)` (run is `None` when nothing was claimed),
    then the claims and dead counters. `COMMIT` error: `handle.poison()`, return `Store`.
- [ ] **Step 4: Run and see them pass.** Same command, then clippy and fmt. Run `/bin/bash ci/affected-graph/run.sh` (the `dependsOn` change). Expected: PASS, no FAIL row.
- [ ] **Step 5: Commit.** `feat(rs): add the per-thread claim with token rotation and dead events (SMA-787)`,
  body names §8.4, §8.4.1, AG5 and the poisoned handle, then the co-author line.

---

### Task 7: Complete and the `Queue` mid-run policy

**Files:**
- Create: `src/adapters/persistence/lease_store/complete.rs`, `tests/complete_pg.rs`
- Modify: `lease_store/mod.rs`, `nextest.toml`, `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: Task 6 `claim_events`.
- Produces: `impl ClaimRepository for PgLeaseStore` (`claim` delegates to `claim_events`;
  `complete` is new).

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/complete_pg.rs
complete_marks_only_the_claimed_events_done      // claim 2 events; complete -> Ok(ids) == the 2; status 'done'; run 'completed', finished_at set; handle.open_claim() None
complete_sets_last_run_at_and_the_successor      // expired thread T, claim -> N with predecessor T; complete; N.last_run_at set; T.successor_id == N
complete_does_not_overwrite_a_successor          // T.successor_id already S; complete of a run on N (predecessor T) -> T.successor_id still S
a_stale_complete_is_lease_lost_and_changes_no_row
    // claim (token 1); expire; another handle takes (token 2); complete(old handle, old claim) -> Err(LeaseLost);
    // the events stay 'pending'; run stays 'running'; counter operation="complete" == 1
an_old_claim_on_a_new_handle_is_lease_lost       // worker A claim c1, expire, worker B acquire and claim c2; complete(B handle, c1) -> LeaseLost; no row changes
queue_keeps_a_mid_run_event_pending              // G6: claim on thread T (1 event); insert a 2nd event on T; complete -> returns only the 1st; the 2nd is 'pending';
                                                 // next claim (rotation, lease_token+1) takes the 2nd, same thread T, fresh_thread false
a_crash_retry_reports_a_fresh_thread             // claim on a new thread; expire; B acquires and claims -> same thread, same events, attempts 2, fresh_thread true
```

  Negative controls:
  - `queue_keeps_a_mid_run_event_pending`: remove `id = ANY($claimed) AND` from the step 3
    `UPDATE`. The test must fail.
  - `a_stale_complete_is_lease_lost_and_changes_no_row`: remove `AND token = $2` from the step 1
    lease lock. The test must fail. (The old run still has token 1, so step 2 alone cannot see a
    lease that moved to token 2. Step 1 is the only guard for this case.)
  - `an_old_claim_on_a_new_handle_is_lease_lost`: two guards catch it, the Rust pre-check and the
    step 1 token check. Remove each one alone: the test stays green. Remove both: it must fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test complete_pg`. Expected: compile errors.
- [ ] **Step 3: Implement `complete`.** A poisoned handle, or `claim.run != handle.open_claim()`, or
  `claim.lease_token != handle.token()`: `LeaseLost`, no SQL. Then one transaction with the four
  steps of §8.5 (the lease row `FOR UPDATE` with `token = $t AND expires_at > clock_timestamp()`).
  Thread step: `UPDATE thread SET last_run_at = now() WHERE id = $1 AND inbox_id = $2 RETURNING
  predecessor_id`, then `UPDATE thread SET successor_id = $1 WHERE id = $pred AND inbox_id = $2 AND
  successor_id IS NULL`. After `COMMIT` is `Ok`: `handle.record_complete()`. A `COMMIT` error:
  `handle.poison()`, `Store`.
- [ ] **Step 4: Run and see them pass.** Same command, plus `--test claim_pg`, then clippy and fmt. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the minimal complete and the queue mid-run behaviour (SMA-787)`,
  body names §8.5, G6 and §11, then the co-author line.

---

### Task 8: The `LeaseKeeper`

**Files:**
- Create: `src/application/lease_keeper.rs`, `tests/keeper_pg.rs`
- Modify: `src/application/mod.rs`, `rs/Cargo.toml` (`tokio-util = { version = "0.7", default-features = false }`
  in `[workspace.dependencies]`, with a comment that Helikon 0.6.12 re-exports
  `tokio_util::sync::CancellationToken`), the service `Cargo.toml`, `rs/Cargo.lock`, `nextest.toml`,
  `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: Task 2 ports; Tasks 5 to 7 `PgLeaseStore`.
- Produces:
  - `KeeperTiming { pub lease_ttl: Duration, pub heartbeat_interval: Duration }` with
    `KeeperTiming::new(ttl, interval) -> Result<Self, KeeperTimingError>` (requires
    `ttl >= 3 * interval` and `interval > 0`).
  - `LeaseKeeper`:
    `pub fn start(handle: LeaseHandle, leases: Arc<dyn LeaseRepository>, claims: Arc<dyn ClaimRepository>, timing: KeeperTiming) -> LeaseKeeper`;
    `pub fn cancellation(&self) -> CancellationToken`;
    `pub async fn claim(&self, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError>`;
    `pub async fn complete(&self, claim: Claim) -> Result<Vec<EventId>, CompleteError>`;
    `pub async fn release(self) -> Result<(), StoreError>`.
    `impl Drop for LeaseKeeper`: abort the heartbeat task. The keeper is the guard of §8.6.
  - The handle lives in `Arc<tokio::sync::Mutex<LeaseHandle>>`. `claim`, `complete`, `release`
    and each heartbeat hold the mutex for the whole repository call (§8.2).

- [ ] **Step 1: Write the failing tests.**

```rust
// lease_keeper.rs unit tests: a FakeStore implementing both ports, #[tokio::test(start_paused = true)]
the_keeper_token_is_the_helikon_token
    // fn same(t: tokio_util::sync::CancellationToken) -> paigasus_helikon::core::CancellationToken { t }  (compiles only if the types are one)
a_lease_lost_heartbeat_cancels                    // fake heartbeat -> LeaseLost; advance 1 interval; cancellation().is_cancelled()
store_errors_cancel_at_the_local_deadline_not_before
    // ttl 30 s, interval 10 s, margin 10 s: heartbeats return Store; at 19.9 s not cancelled; at 20.1 s cancelled
a_slow_heartbeat_counts_against_the_deadline     // fake heartbeat sleeps 25 s -> cancelled at 20 s (the keeper's timeout) with the "deadline" counter == 1
a_heartbeat_never_runs_during_a_claim
    // fake claim sets an AtomicBool for 15 s (paused time) then clears it; fake heartbeat asserts the flag is false
a_heartbeat_sends_the_token_after_a_rotation      // fake claim rotates 1 -> 2; the next heartbeat sees handle.token() == 2
dropping_the_keeper_stops_the_heartbeats          // count heartbeats; drop; advance 100 s; count unchanged
a_poisoned_handle_stops_the_heartbeats_and_cancels // fake claim poisons the handle; next tick: no heartbeat call, cancelled
```

```rust
// tests/keeper_pg.rs (real Postgres, real PgLeaseStore)
a_keeper_survives_two_rotations
    // timing ttl 3 s, interval 1 s; acquire; LeaseKeeper::start; claim, complete, claim, complete (events inserted between);
    // sleep 4 s real time between the two cycles; cancellation() is never cancelled; final token == 2 (take 1, one rotation)
```

  Negative controls:
  - `a_heartbeat_never_runs_during_a_claim`: in the heartbeat task, read the handle with a copy of
    the token taken before the lock (no lock held across the call). The test must fail.
  - `store_errors_cancel_at_the_local_deadline_not_before`: move the deadline update to run on
    every heartbeat result, not only on success. The test must fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -E 'test(keeper)'`. Expected: compile errors.
- [ ] **Step 3: Implement.** The heartbeat loop uses absolute instants:
  `deadline = sent + lease_ttl - heartbeat_interval` after each success (initially from `start`),
  `next = sent + heartbeat_interval`, and `tokio::select!` over `sleep_until(deadline)` and
  `sleep_until(next)`. Each heartbeat call is wrapped in `tokio::time::timeout(heartbeat_interval)`.
  A timeout or `Store` does not move the deadline. `LeaseLost` from `claim` or `complete` also
  cancels. Increment `AGENT_HOST_LEASE_LOST_TOTAL{operation="deadline"}` when the deadline fires.
- [ ] **Step 4: Run and see them pass.** Same command, then clippy, fmt, and `/bin/bash ci/affected-graph/run.sh`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the lease keeper with a monotonic deadline (SMA-787)`,
  body names §8.2, §8.6 and the tokio-util choice, then the co-author line.

---

### Task 9: The sequential property test

**Files:**
- Create: `tests/lease_property_pg.rs`, `tests/lease_property_pg.proptest-regressions`
- Modify: the service `Cargo.toml` (dev `proptest = { workspace = true }`), `rs/Cargo.lock`,
  `rs/.config/nextest.toml`, `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: `PgLeaseStore` (Tasks 5 to 7), `PgInboxRepository` (Task 4).

- [ ] **Step 1: Write the test.** One `#[test] fn lease_and_claim_keep_their_invariants()`. It
  builds a multi-thread runtime, starts one Postgres with `migrated_postgres()` (skip on `None`),
  and runs a `proptest::test_runner::TestRunner` with `Config { cases: 64,
  failure_persistence: Some(Box::new(FileFailurePersistence::Direct("tests/lease_property_pg.proptest-regressions"))), .. }`.
  Each case uses a new inbox. One pinned connection (`connect_pinned`) per worker; operations run
  one after another (`rt.block_on`).

```rust
enum Op { Acquire(usize), Heartbeat(usize), Claim(usize), Complete { worker: usize, old: bool },
          Release(usize), Insert { thread: ThreadChoice }, Expire }
enum ThreadChoice { None, ExistingOfRun, Foreign }
// strategy: workers 2..=4, ops 1..=40, cfg ClaimConfig { batch_limit: 3, max_attempts: 3, thread_ttl: 86400 s }
```

  Invariants after every step (read with SQL, not from the model):
  - at most one worker holds a handle whose token equals `inbox_lease.token` and the lease is live
    (`expires_at > clock_timestamp()`);
  - at most one `run` row with `status = 'running'` and `lease_token` = the current live token;
  - each token that an `Acquire` or a rotating `Claim` allocates is strictly larger than every
    earlier token; the first `Claim` after an `Acquire` returns the same token as that `Acquire`
    (no rotation, Task 5 R6);
  - the `done` id sets of the successful `Complete` calls are pairwise disjoint;
  - every event with a `claimed_run_id` has `thread_id = run.thread_id` (AG5);
  - every `attempts <= max_attempts`;
  - a `Complete` with a stale token or an old claim returns `LeaseLost`, and a row snapshot
    (`inbox_event`, `run`, `thread`) before and after is equal;
  - a `Claim` on a handle with an open claim returns `ClaimOpen`.

  After the sequence: drain. One fresh worker loops `acquire` (expire first if needed), `claim`,
  `complete` until `claim` returns `Ok(None)`. Then every inserted event is `done` or `dead` (AG2).

  Coverage: `AtomicUsize` counters over all cases for "stale complete", "take after expire" and
  "rotation". After `run` returns, assert each is at least 1.

  Create `tests/lease_property_pg.proptest-regressions` with only the SPDX line and the line
  `# proptest failure seeds for lease_property_pg. Commit every seed it writes.`

  `nextest.toml`: an override `package(=paigasus-agent-host) and binary(lease_property_pg)` with
  `retries = 0`, placed BEFORE the agent-host `docker-containers` override (the first match sets
  `retries`), and the binary name in the `docker-containers` filter. Add it to
  `tests/docker_preflight.rs`.

- [ ] **Step 2: Run it.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test lease_property_pg`. Expected: PASS. Measure the wall time and write it in the commit body. It must fit in the `heavy-integration` mutex; if it takes more than 120 s, lower the op count, not the case count, and say so.
- [ ] **Step 3: Prove it bites.** Remove the token check from the `complete` step 1 query and the
  `claim.lease_token != handle.token()` pre-check. The test must fail with a shrunk sequence.
  Restore both by edit. Delete any seed file lines this run wrote, so the committed file has only
  the header.
- [ ] **Step 4: Commit.** `test(rs): add the sequential lease and claim property test (SMA-787)`,
  body names §10.1, the case count, the wall time and `retries = 0`, then the co-author line.

---

### Task 10: The stress test and the crash test

**Files:**
- Create: `tests/lease_stress_pg.rs`, `tests/lease_crash_pg.rs`
- Modify: `rs/.config/nextest.toml`, `tests/docker_preflight.rs`

**Interfaces:**
- Consumes: `LeaseKeeper` (Task 8), `PgLeaseStore`, `PgInboxRepository`.

- [ ] **Step 1: Write the stress test** `concurrent_workers_never_complete_one_event_twice`:
  8 tasks, each with `connect_pinned` and its own `LeaseKeeper` (ttl 3 s, interval 1 s), for 150
  loop rounds: `acquire`, `claim`, sometimes `expire` from a separate connection (a seeded LCG,
  seed printed on failure), `complete`, `release`. A writer task inserts 200 events (a third on one
  thread, a third with no thread, a third on a foreign thread). Each task records
  `(run, token, done ids)` of each successful complete. Then drain (as Task 9). Assert: no event id
  in two successful completes; every recorded run row is `completed` with `lease_token` = the
  recorded token; the recorded tokens are unique; every event is `done` or `dead`.
- [ ] **Step 2: Write the crash test** (two tests, `aborted` and `panicked`, one helper):
  - worker A: `acquire` with holder `"a"`, `LeaseKeeper::start` (ttl 3 s, interval 1 s), `claim`
    inside a spawned task; the task sends the `Claim` and the token on a channel, then waits on a
    pending future (case 1) or panics (case 2). The test aborts the task in case 1.
  - negative control: right after A's claim, worker B's `acquire` returns `None`.
  - B polls `acquire` every 200 ms, at most 15 s, until `Some`. B claims: the same event ids,
    `attempts == 2`, the same thread, `fresh_thread == true`. B completes; every event is `done`.
  - late complete: `LeaseHandle::new(inbox, a_token, "a")` with `record_claim(a_token, Some(a_run))`,
    then `complete(&mut h, a_claim)` returns `Err(CompleteError::LeaseLost)`; a row snapshot before
    and after is equal.
- [ ] **Step 3: Register both binaries** in `nextest.toml` (`docker-containers`) and in
  `docker_preflight.rs`.
- [ ] **Step 4: Run them.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test lease_stress_pg --test lease_crash_pg`. Expected: PASS.
- [ ] **Step 5: Prove the crash test bites.** Remove the `impl Drop for LeaseKeeper` abort. Case 1
  must fail (B never takes the lease within 15 s). Restore by edit.
- [ ] **Step 6: Commit.** `test(rs): add the concurrent stress and crash tests of the inbox lease (SMA-787)`,
  body names §10.2, §10.3 and G5, then the co-author line.

---

### Task 11: IAM: `WriteInboxEvent` and `agent_event_writer`

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs` (enum near `:57`, `ALL` near
  `:138`, wire form near `:199`, `is_write` near `:279`, the exhaustive test match near `:371`, the
  count at `:393-396`), `authz/schema.rs:26-30`, `authz/roles.rs` (`:3`, `:75`, `:81-92`, `:104`,
  `:228-236`, the role list, `template_source` `:365-375`, the case table),
  `rs/crates/services/paigasus-iam/src/application/bootstrap.rs:3` (doc text)

**Interfaces:**
- Produces: `Action::WriteInboxEvent`, wire `"WriteInboxEvent"`, `is_write() == true`.
  Role key `agent_event_writer` (template id the same), `scope_kinds: vec![NodeKind::Organization]`,
  description `"Write events into the agent inboxes of an organization."`, actions
  `&[Action::WriteInboxEvent]`. The agent host (Task 13) sends the wire string.

- [ ] **Step 1: Write the failing tests.**

```rust
// action.rs
write_inbox_event_is_a_write_action               // Action::parse("WriteInboxEvent") == Some(WriteInboxEvent); is_write()
// the existing count test: Action::ALL.len() == 55, the message adds "+ 1 SMA-787 WriteInboxEvent"
// roles.rs case table (grant numbers: the next free numbers)
"agent_event_writer allows WriteInboxEvent on its granted org (SMA-787)"            -> Allow (resource uni.org_o prn)
"agent_event_writer denies WriteInboxEvent on a different org (SMA-787)"            -> Deny
"agent_event_writer denies InvokeModel on its org (SMA-787)"                        -> Deny
"org_admin does not hold WriteInboxEvent on its own org (SMA-787 §6.1)"             -> Deny
"gateway_user denies WriteInboxEvent on its org (SMA-787)"                          -> Deny
"forbid-archived-writes denies WriteInboxEvent on an archived project even for agent_event_writer" -> Deny
    // the archived node of the InvokeModel case at roles.rs:691
the_write_inbox_event_action_is_in_the_generated_forbid_source
    // forbid_archived_writes_source().contains(r#"Pgs::Iam::Action::"WriteInboxEvent""#)
```

  Negative control for the `org_admin` case: add `Action::WriteInboxEvent` to `ORG_ADMIN_ACTIONS`.
  The case must fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-iam-core`. Expected: compile errors, then the starter hash test fails.
- [ ] **Step 3: Implement.** Add the action everywhere `InvokeModel` appears in `action.rs` and in
  the schema `action` list. Add the role, its `template_source` arm, and `"agent_event_writer"` as
  the last entry of `STARTER_POLICY_IDS`. Set `STARTER_POLICY_REVISION` to 8 with a doc paragraph
  "`8`: SMA-787 added `WriteInboxEvent` (a non-restore write, so it joins the generated forbid
  list) and the `agent_event_writer` role template." Replace `EXPECTED_STARTER_CONTENT_HASH` with
  the value the hash test prints. Change "nine system roles" to "ten system roles" in `roles.rs:3`,
  `roles.rs:236` and `bootstrap.rs:3`.
- [ ] **Step 4: Run and see them pass.** Run: `cd rs && cargo nextest run --locked -p paigasus-iam-core && cargo nextest run --locked -p paigasus-iam --test authz_bootstrap && cd .. && moon run paigasus-console-core-ts:test`. Expected: PASS (the console test checks only a subset).
- [ ] **Step 5: Commit.** `feat(rs): add the WriteInboxEvent action and the agent_event_writer role (SMA-787)`,
  body names §6.1, revision 8 and the `org_admin` exclusion, then the co-author line.

---

### Task 12: The `store-unavailable` error reason

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (shared banner, after `:389`), the
  generated `common/v1` files of Rust, Python and TypeScript, `rs/crates/libs/paigasus-proto/src/error.rs`
  (`EXPECTED_REASONS` "Shared" block, the count at `:273`), `ts/packages/paigasus-proto/src/error.test.ts:57-60`,
  `ts/packages/paigasus-sdk/src/errors/presentation.ts` (comment `:12`, a new entry),
  `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`

**Interfaces:**
- Produces: `ERROR_REASON_STORE_UNAVAILABLE = 910`, wire `"store-unavailable"`; Rust
  `ErrorReason::StoreUnavailable`.

- [ ] **Step 1: Edit the registry.** Comment, in the form of the other shared values: `// In the
  shared range (900-999). Emitted by the agent host (SMA-787).` and `// "store-unavailable" — the
  service cannot reach its database. Retryable (503).` Then the value line.
- [ ] **Step 2: Update the three cardinality guards** from 92 to 93 and the presentation comment
  to "93 keys rather than 92". Add `"store-unavailable"` to `EXPECTED_REASONS` and
  `[ErrorReason.STORE_UNAVAILABLE]: 'from-transport'` with a one-line SMA-787 comment.
- [ ] **Step 3: Generate.** Run: `cd contracts && buf format -w && cd .. && moon run contracts:fmt contracts:lint contracts:breaking && moon run contracts:generate --force`.
  Expected: PASS; `git status` shows only the three `common/v1` generated files changed. If
  `error_details_pb.ts` is deleted (a BSR rate limit), restore it with `git checkout -- <path>`
  and run generate again later.
- [ ] **Step 4: Run the registry tests.** Run: `cd rs && cargo nextest run --locked -p paigasus-proto && cd .. && moon run paigasus-proto-ts:test paigasus-sdk-ts:test paigasus-sdk-ts:typecheck paigasus-proto-py:test ts:fmt repo:error-code-single-site`. Expected: PASS.
- [ ] **Step 5: Commit.** The type is `feat(contracts)` (a new value in the published registry):
  `feat(contracts): add the store-unavailable error reason (SMA-787)`, body names §5.2 step 9 and
  the shared range, then the co-author line.

---

### Task 13: The agent-host IAM adapter, authentication, authorisation and config

**Files:**
- Create: `src/adapters/iam/{mod.rs,client.rs}`, `src/adapters/http/auth.rs`, `tests/support/fake_iam.rs`
- Modify: `src/adapters/mod.rs`, `src/adapters/http/error.rs`, `src/config.rs`, the service
  `Cargo.toml` (`tonic` with `tls-ring` and `tls-native-roots`, `tonic-types`, `paigasus-proto`
  moves from dev to normal), `rs/Cargo.lock`, `tests/support/mod.rs`, `ci/error-registry/check.py`
  (the guard name of the `http/error.rs` row)

**Interfaces:**
- Consumes: Task 2 `Inbox`, `MidRunPolicy`, `ClaimConfig`; Task 11 the action wire string.
- Produces:
  - `adapters::iam::client`: `IamError { Connect(String), Rpc(tonic::Status) }`;
    `CallerCredential { ApiKey(String), Bearer(String) }` (`Debug` prints no secret);
    `#[async_trait] pub trait Iam: Send + Sync { introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError>; introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError>; is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError>; }`;
    `IamClient::connect(cfg: &IamClientConfig) -> Result<IamClient, IamError>`. Copy
    `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs` without the DPoP parts
    (`DpopContext`, `CallerCredential::Dpop`, the `dpop` metadata). Add
    `Endpoint::timeout(Duration::from_millis(cfg.call_timeout_ms))`.
  - `adapters::http::auth`:
    `pub const WRITE_INBOX_EVENT_ACTION: &str = "WriteInboxEvent";`
    `pub enum CallerKind { ApiKey { scope_prn: String }, Oidc { member_orgs: Vec<Uuid> } }`;
    `pub struct Caller { pub principal: PrincipalId, pub credential: CallerCredential, pub kind: CallerKind }`;
    `pub fn bearer_token(headers: &HeaderMap) -> Result<String, ApiError>` (copy the gateway
    `credentials` parse; `DPoP …` and every other scheme give `InvalidToken`);
    `pub async fn authenticate(iam: &dyn Iam, token: &str) -> Result<Caller, ApiError>`;
    `pub async fn authenticate_discovery(iam: &dyn Iam, token: &str) -> Result<(), ApiError>`
    (also accepts `identity-not-provisioned`);
    `pub async fn authorize_write(iam: &dyn Iam, caller: &Caller, inbox: &Inbox) -> Result<(), ApiError>`.
    `ApiError` lives in `http/error.rs`. This task creates it with only the variants it uses:
    `InvalidToken` (401 `invalid-token`), `Forbidden` (403 `forbidden`), `IamUnavailable` (503
    `iam-unavailable`), `Internal` (500 `internal`), with `fn parts(&self) -> (StatusCode,
    &'static str, &'static str)` and `impl IntoResponse`. Rename the test
    `the_not_found_code_is_in_the_registry` to `every_agent_host_http_code_is_in_the_registry`. It
    checks `NOT_FOUND` and every variant with `ErrorReason::from_wire_reason`. Change the guard name
    of the `http/error.rs` row in `ci/error-registry/check.py` to match. Task 14 adds variants to
    the same test.
  - `config.rs`: `WorkerConfig { enabled: bool }`, `LeaseConfig { ttl_secs: u64, heartbeat_interval_secs: u64 }`,
    `ClaimSettings { batch_limit: u32, max_attempts: u32, thread_ttl_secs: u64 }` with
    `to_claim_config(&self) -> ClaimConfig`, `IngressConfig { max_body_bytes: usize }`,
    `IamClientConfig { grpc_addr: String, tls: IamTlsConfig, call_timeout_ms: u64 }` (serde default
    2000), `IamTlsConfig` copied as is from `paigasus-gateway/src/config.rs:89-105` with its doc,
    `MidRunConfig { policy: MidRunPolicy }`. `Config` gains `worker`, `lease`, `claim`, `ingress`,
    `iam: Option<IamClientConfig>`, `mid_run`. Defaults and ranges: §8.7 exactly. New
    `ConfigError` variants, one per range, plus `LeaseTtlBelowThreeHeartbeats { ttl, interval }`,
    `IamLoopbackInsecureNotLoopback`, `MidRunPolicyNotSupported(MidRunPolicy)` with the message
    `mid_run.policy = "<fold|interrupt>" is not supported: it needs the Helikon turn-boundary change (ADR (h)); use "queue"`.
    Copy `is_loopback_host` and its tests from `paigasus-gateway/src/config.rs:527-553`.
  - `tests/support/fake_iam.rs`: `FakeIam` implementing `Iam`, with keys, users, an allow set of
    `(principal_prn, resource_prn)`, an outage switch, and `calls(&self) -> Vec<&'static str>`.

- [ ] **Step 1: Write the failing tests.**

```rust
// auth.rs unit tests (a FakeIam in the test module; ORG = u(0x500), INBOX principal = u(0x10))
// Review Focus 5:
bearer_token_reads_a_case_insensitive_scheme_and_trims  // "bearer  key " -> Ok("key"); "BEARER key" -> Ok
dpop_and_other_schemes_are_invalid_token                // "DPoP x", "Basic x", "Bearer " -> Err(InvalidToken)
// §6.2 table, exact call lists:
an_api_key_owner_costs_one_call                         // calls == ["introspect_api_key"]
an_api_key_non_owner_costs_two_calls                    // ["introspect_api_key","is_authorized_self"]; allowed -> Ok
an_oidc_owner_costs_two_calls                           // ["introspect_api_key","introspect_token"]
an_oidc_non_owner_costs_three_calls                     // + "is_authorized_self"
the_self_query_uses_the_callers_own_credential_and_the_org // recorded (credential, principal, action, resource) == (Bearer(token), caller prn, "WriteInboxEvent", "prn:pgs:iam:::organization/<ORG>")
// §6.3 step 1:
a_key_scoped_to_another_org_is_forbidden                // scope prn:pgs:iam:::organization/<u(0x600)> -> Forbidden, no is_authorized_self call
a_key_scoped_to_a_team_or_project_is_forbidden          // prn:pgs:iam::<ORG>:team/<..> and :project/<..> -> Forbidden (also for the owner)
a_root_key_passes_the_scope_rule                        // prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000, non-owner, allowed -> Ok
an_oidc_owner_without_membership_is_forbidden           // memberships name only u(0x600) -> Forbidden; 2 calls
the_owner_check_compares_uuids                          // inbox principal u(0x10); introspected prn with the UUID in upper case -> owner path (1 call)
an_inactive_key_falls_through_to_the_token_leg          // status "revoked" -> introspect_token runs
an_iam_outage_on_both_legs_is_iam_unavailable           // Unavailable on both -> IamUnavailable (preserve_outage)
discovery_accepts_identity_not_provisioned               // PermissionDenied + ErrorInfo reason identity-not-provisioned (IAM domain) -> Ok
discovery_refuses_principal_inactive                    // -> InvalidToken
// config.rs
the_new_defaults_match_the_spec                         // worker.enabled true, ttl 30, interval 10, batch 20, attempts 5, thread_ttl 86400, max_body 262144, policy Queue, iam None
each_range_is_enforced_at_both_ends                     // table over every §8.7 range: min-1 Err, min Ok, max Ok, max+1 Err
the_ttl_must_be_three_heartbeats                        // ttl 29, interval 10 -> Err(LeaseTtlBelowThreeHeartbeats); 30 Ok
fold_and_interrupt_are_refused_at_boot                  // Err(MidRunPolicyNotSupported(Fold)); message contains "turn-boundary"
iam_call_timeout_defaults_to_2000                       // [iam] grpc_addr + tls only -> call_timeout_ms 2000
loopback_insecure_needs_a_loopback_host                 // http://10.0.0.1:9090 -> Err; http://127.0.0.1:9090 -> Ok
// client.rs: copy the gateway request-builder tests without the DPoP ones
```

  Negative controls:
  - `a_key_scoped_to_a_team_or_project_is_forbidden`: remove the key-scope check. The test must fail.
  - `the_owner_check_compares_uuids`: compare the PRN strings instead. The test must fail.
  - `an_oidc_owner_without_membership_is_forbidden`: remove the membership check. The test must fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -E 'test(auth) | test(config) | test(client)'`. Expected: compile errors.
- [ ] **Step 3: Implement.** `authenticate` follows `bearer_iam_auth` of the gateway
  (`auth.rs:196-240`): the key leg, then the token leg, `preserve_outage`, and the same error
  mapping (`introspect_error`, `authz_error` minus DPoP and quota arms). `member_orgs` uses a copy
  of the gateway `org_of` (`domain/mod.rs:107-118`) over the membership `node_prn`s. The principal
  comes from `PrincipalId::parse`; a parse failure is `Internal` with an `error` log.
  `authorize_write` is §6.3 steps 1 to 3 in that order. No cache.
- [ ] **Step 4: Run and see them pass.** Same command, then clippy and fmt, and `cd .. && moon run repo:deny repo:machete repo:error-code-single-site`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the agent-host IAM adapter and the ingress authorisation (SMA-787)`,
  body names §6.2, §6.3, §8.7 and §11, then the co-author line.

---

### Task 14: The HTTP route, the extractors and `GET /v1/service-info`

**Files:**
- Create: `src/adapters/http/{path.rs,bytes.rs,ingress.rs,service_info.rs}`, `tests/ingress_http.rs`,
  `tests/ingress_pg.rs`, `tests/support/fake_inbox.rs`
- Modify: `src/adapters/http/{mod.rs,error.rs}`, `src/main.rs` (`boot`), the service `Cargo.toml`
  (`paigasus-cloudevents`, `paigasus-service-info`), `moon.yml` (`dependsOn` and `upstreams`),
  `ci/http-extractor/check.py` (only if the gate asks), `rs/Cargo.lock`, `nextest.toml`,
  `tests/docker_preflight.rs`, `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: Tasks 1, 4, 12, 13.
- Produces:
  - `error::ApiError` gains `UnsupportedContentType` (415 `unsupported-content-type`),
    `RequestTooLarge` (413 `request-too-large`), `BadRequestBody` (400 `invalid-request-body`),
    `InvalidRequestSchema` (400 `invalid-request-schema`), `InvalidUuid` (400 `invalid-uuid`),
    `StoreUnavailable` (503 `store-unavailable`). Extend
    `every_agent_host_http_code_is_in_the_registry` (Task 13) with each new variant.
  - `path::{PathField, InboxIdField, UuidPath<F>}` (copy of IAM `path.rs` for one segment; marker
    `InboxIdField => "inbox"`; a bad segment is `ApiError::InvalidUuid`).
  - `bytes::EnvelopeBytes` (copy of the gateway `bytes.rs`; 413 → `RequestTooLarge`, 400 →
    `BadRequestBody`).
  - `ingress::{precheck, post_event}` and the routes; `AppState` becomes
    `AppState { db: DatabaseConnection, inboxes: Arc<dyn InboxRepository>, ingress: Option<Arc<Ingress>> }`
    with `Ingress { iam: Arc<dyn Iam>, max_body_bytes: usize }`. `app_router` mounts
    `POST /v1/inboxes/{inbox}/events` and `GET /v1/service-info` only when `ingress` is `Some`.
  - `service_info::SERVICE = "agents"`; the body is `ServiceInfoDto` of
    `paigasus_service_info::descriptor(SERVICE, env!("CARGO_PKG_VERSION"), &[])`.
  - `main.rs` `boot`: build `PgInboxRepository`, and when `config.iam` is set, `IamClient::connect`
    and `Ingress`. Log at `info` when the ingress is off. `worker.enabled` changes nothing in
    SMA-787: log `worker.enabled is read, and SMA-787 starts no worker` once at `info`.

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/ingress_http.rs: app_router(AppState{ db: DatabaseConnection::default(), inboxes: FakeInbox, ingress: Some(FakeIam, max 262144) }) + oneshot
// One row per §5.2 step:
a_wrong_content_type_is_415_before_auth          // application/json, no credential -> 415 unsupported-content-type; iam.calls() empty
the_content_type_ignores_case_and_parameters     // Review Focus 1: "Application/CloudEvents+JSON; charset=UTF-8" -> 202; "application/cloudevents+jsonx" -> 415
a_large_content_length_is_413_before_auth        // Content-Length 262145, no credential -> 413 request-too-large; iam.calls() empty
a_chunked_body_over_the_limit_is_413_after_auth  // no Content-Length, 300 KiB stream -> 413; iam.calls() not empty
no_credential_is_401_invalid_token
an_iam_outage_is_503_iam_unavailable
an_unknown_inbox_and_a_refused_caller_get_the_same_403 // two responses: status 403, bodies byte-equal; unknown inbox made no is_authorized_self call
a_malformed_inbox_segment_is_400_invalid_uuid
invalid_json_is_400_invalid_request_body
an_invalid_cloudevent_is_400_invalid_request_schema   // specversion "0.3"
an_agents_type_is_400_invalid_request_schema          // "type": "agents.outbox.message"
the_host_sets_source_and_actorprn                     // caller sends source "x" and actorprn "y"; stored envelope source == "urn:paigasus:agents:http:prn:pgs:iam:::principal/<uuid>", actorprn == that PRN
a_foreign_threadid_is_dropped_and_the_event_accepted  // threadid of another inbox -> 202; NewInboxEvent.thread == Some(that id) (the repository drops it; Task 4 tests the drop); envelope keeps the raw threadid
a_non_uuid_or_non_string_threadid_is_dropped          // "threadid": "x" and "threadid": 7 -> 202, NewInboxEvent.thread None
a_duplicate_is_202_with_the_stored_id                 // FakeInbox returns Duplicate{id: E1} -> body {"id": "<E1>"}
a_store_error_is_503_store_unavailable                // FakeInbox lookup Err(Store) and insert Err(Store) -> 503 store-unavailable
service_info_needs_a_credential                       // no credential -> 401
service_info_accepts_an_unprovisioned_identity        // -> 200 {"service":"agents","version":<crate version>,"capabilities":[]}
without_iam_the_ingress_routes_are_404                // ingress None -> POST /v1/inboxes/<uuid>/events is 404 not-found
// tests/ingress_pg.rs (real Postgres, real PgInboxRepository, FakeIam)
a_posted_event_is_stored_with_the_host_fields         // 202; row source/ce_id/ce_type/envelope as sent plus host fields; status pending
a_client_retry_gets_the_same_body                     // post twice -> both 202 with the same {"id"}; one row
a_foreign_threadid_is_stored_as_null                  // threadid = a thread row of another inbox -> 202; thread_id IS NULL; envelope threadid is the raw value
```

  Register `ingress_pg` in `nextest.toml` and `docker_preflight.rs`.

  Negative controls:
  - `a_large_content_length_is_413_before_auth`: put `precheck` inside the auth layer. The test
    must fail (401 or IAM calls).
  - `the_host_sets_source_and_actorprn`: keep the caller `source`. The test must fail.
  - `an_unknown_inbox_and_a_refused_caller_get_the_same_403`: give the unknown inbox `not-found`.
    The test must fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test ingress_http --test ingress_pg`. Expected: compile errors.
- [ ] **Step 3: Implement.**
  - Layer order on the events route, outer to inner: `precheck` (content type, then
    `Content-Length`), the authentication middleware (puts `Caller` in the extensions), then the
    handler under `DefaultBodyLimit::max(max_body_bytes)`. The handler: `UuidPath<InboxIdField>`,
    `Extension<Caller>`, `EnvelopeBytes`; then lookup, `authorize_write`, `paigasus_cloudevents::parse`
    (`Json` → `BadRequestBody`, `Schema` → `InvalidRequestSchema`), the `agents.` check, the host
    fields (§5.2 step 7), a new `EventId` from `mint_uuid()`, then `ingest(…, Channel::Http)`.
  - Host fields: `set_source("urn:paigasus:agents:http:" + principal canonical PRN)`,
    `set_extension("actorprn", String(<same PRN>))`. `thread` = `threadid()` parsed with
    `ThreadId::parse`; the raw extension stays in the envelope. The envelope is `event.to_json()`.
  - The service-info route has its own middleware that calls `authenticate_discovery`. It has no
    `precheck` (a `GET` has no body) and no authorisation (§5.3).
  - Add `paigasus-cloudevents-rs` and `paigasus-service-info-rs` to `dependsOn` and their
    `src/**/*` and `Cargo.toml` lines to `upstreams`.
- [ ] **Step 4: Run and see them pass.** Same command, then clippy and fmt, then `cd .. && moon run repo:error-code-single-site repo:http-extractor-envelope` and `/bin/bash ci/affected-graph/run.sh`. If `repo:http-extractor-envelope` reds on `bytes.rs`, add an `ALLOW` row with the reason of the gateway `bytes.rs` row; else add none. Expected: PASS, no FAIL row.
- [ ] **Step 5: Commit.** `feat(rs): add the CloudEvents ingress route and the service-info route (SMA-787)`,
  body names §5, §6 and the registry rows, then the co-author line.

---

### Task 15: The handler latency test

**Files:**
- Create: `tests/ingress_latency_pg.rs`
- Modify: `tests/support/fake_iam.rs` (a tonic server wrapper), the service `Cargo.toml` (dev
  `tokio-stream` with `net`), `rs/Cargo.lock`, `nextest.toml`, `tests/docker_preflight.rs`,
  `tests/support/mod.rs` (`spawn_service_with(url, extra_env: &[(&str, String)])`)

**Interfaces:**
- Consumes: the built binary; `FakeIam`.
- Produces: `fake_iam::serve_grpc(iam: Arc<FakeIam>) -> (SocketAddr, JoinHandle<()>)` that serves
  `AuthnServiceServer` and `AuthorizationServiceServer` on `127.0.0.1:0`. RPCs other than
  `Introspect`, `IntrospectApiKey` and `IsAuthorized` return `Status::unimplemented`.

- [ ] **Step 1: Write the test** `handler_p99_is_under_50_ms_with_the_worker_off`:
  Postgres via `start_raw_postgres`; `serve_grpc` with one active API key whose principal owns the
  inbox (one IAM call per request); spawn the binary with `AGENT_HOST_IAM__GRPC_ADDR=http://<addr>`,
  `AGENT_HOST_IAM__TLS__MODE=loopback_insecure`, `AGENT_HOST_WORKER__ENABLED=false`; wait for
  `/readyz`; insert the inbox by SQL; 20 warm-up requests; then 1000 POSTs with unique `id`s at a
  concurrency of 16 (a `tokio::sync::Semaphore`), each timed on the client. Assert every status is
  202, and p99 < 50 ms. Print p50, p99 and max. Install the `ring` provider before the reqwest
  client (SMA-809).
- [ ] **Step 2: Register it** in `nextest.toml` (`docker-containers`) and `docker_preflight.rs`.
  The agent-host `test` task already runs on the `heavy-integration` mutex.
- [ ] **Step 3: Run it three times.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host --test ingress_latency_pg --no-capture`. Expected: PASS each time. Write the three p99 values in the commit body. If one run fails, do not raise the limit: report it to the coordinator.
- [ ] **Step 4: Commit.** `test(rs): add the ingress handler latency test (SMA-787)`, body names
  §10.4 and the measured p99 values, then the co-author line.

---

### Task 16: The NATS identity `agent-host-consumer`

**Files:**
- Modify: `ops/nats/subjects.env`, `ops/nats/provision.sh`, `ops/nats/check-subjects.sh`,
  `ops/nats/test/accounts.conf.tmpl`, `ops/nats/permissions.md`, `ops/nats/README.md`,
  `rs/crates/services/paigasus-iam/tests/nats_permissions.rs`

**Interfaces:**
- Produces (in `subjects.env`, read by Task 17's pin test):
  `AGENT_HOST_DURABLE_NAME="agent-host-inbox"`, `AGENT_HOST_INBOX_PREFIX="_INBOX_AGH"`,
  `AGENT_HOST_CONSUMER_PUB=( '$JS.API.CONSUMER.MSG.NEXT.'"$STREAM_NAME"'.'"$AGENT_HOST_DURABLE_NAME"
  '$JS.API.CONSUMER.INFO.'"$STREAM_NAME"'.'"$AGENT_HOST_DURABLE_NAME"
  '$JS.ACK.'"$STREAM_NAME"'.'"$AGENT_HOST_DURABLE_NAME"'.>' )`,
  `AGENT_HOST_CONSUMER_SUB=( "$AGENT_HOST_INBOX_PREFIX.>" )`,
  `AGENT_HOST_FILTER_SUBJECTS=( "iam.organization.>" )`, with a comment block in the style of
  `gateway-consumer` (§9.2, §9.5).

- [ ] **Step 1: Write the failing tests** in `nats_permissions.rs` (mint the identity and replace
  `{{AGENT_HOST_CONSUMER_NKEY}}` in `start_fixture`):

```rust
the_agent_host_consumer_pulls_acks_and_reads_info_of_its_durable
    // provision the durable agent-host-inbox (pull, explicit ack, deliver new, max-deliver -1, ack wait 30 s, filter iam.organization.>)
    // publish iam.organization.created; the identity fetches it, reads CONSUMER.INFO, acks; no permission violation
the_agent_host_consumer_cannot_create_a_consumer_or_read_the_gateway_durable
    // CONSUMER.CREATE in any form, MSG.NEXT on gateway-cache-invalidator -> permission violation
the_agent_host_consumer_cannot_subscribe_to_or_forge_events
    // sub iam.> and pub iam.organization.created -> permission violation
neither_consumer_identity_can_subscribe_to_the_others_inbox
    // _INBOX_GW.> by agent-host-consumer and _INBOX_AGH.> by gateway-consumer -> violation
```

- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --profile iam-nats`. Expected: FAIL (an unreplaced placeholder or a missing grant).
- [ ] **Step 3: Implement.**
  - `accounts.conf.tmpl`: a user block for `{{AGENT_HOST_CONSUMER_NKEY}}` in the `PAIGASUS_IAM`
    account with exactly the two arrays. `check-subjects.sh`: a `check_identity agent-host-consumer
    '{{AGENT_HOST_CONSUMER_NKEY}}' AGENT_HOST_CONSUMER_PUB AGENT_HOST_CONSUMER_SUB '<next nkey>'`
    row in template order, and change the terminator of the row before it.
  - `provision.sh`: `add_user agent-host-consumer AGENT_HOST_CONSUMER_PUB AGENT_HOST_CONSUMER_SUB`,
    and after the gateway durable, one `--filter` per `AGENT_HOST_FILTER_SUBJECTS` entry and
    `consumer add "$STREAM_NAME" "$AGENT_HOST_DURABLE_NAME" --pull "${filter_args[@]}" --ack explicit
    --deliver new --max-deliver=-1 --wait 30s --defaults`. A comment states why each flag (§9.2).
    The final `echo` names `agent-host-consumer.creds`.
  - `permissions.md` and `README.md`: a row and a paragraph for the identity, in the style of
    `gateway-consumer`.
- [ ] **Step 4: Run and see them pass.** Run: `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` (bash 4.3+), then the Step 2 command (needs Docker). Expected: PASS.
- [ ] **Step 5: Commit.** `feat(repo): add the agent-host-consumer NATS identity and durable (SMA-787)`,
  body names §9.2 and §9.5, then the co-author line. (Use scope `repo`: the change is in `ops/` and
  in an IAM test.)

---

### Task 17: The NATS source: config, channels and tenancy

**Files:**
- Create: `src/adapters/nats/{mod.rs,channels.rs,tenancy.rs}`
- Modify: `src/config.rs`, `src/adapters/mod.rs`

**Interfaces:**
- Consumes: Task 4 `ingest`, Task 1 `parse`.
- Produces:
  - `config::NatsConfig { url: RedactedUrl, creds_file: Option<String>, ca_file: Option<String>, stream: String, durable: String, channels: Vec<ChannelConfig> }`,
    `ChannelConfig { subject: String, inbox: Uuid, source: String }`; `Config.nats: Option<NatsConfig>`.
    `ConfigError` gains `NatsChannelsOverlap { a: String, b: String }`,
    `NatsSourcePrefix(String)` (must start with `urn:paigasus:agents:nats:`),
    `NatsDuplicateSource(String)`, `NatsNoChannel`.
  - `channels::{filters_overlap(a: &str, b: &str) -> bool, subject_matches(filter: &str, subject: &str) -> bool}`.
  - `tenancy::names_org(subject_attr: Option<&str>, org: Uuid) -> bool`.
  - `pub const INBOX_PREFIX: &str = "_INBOX_AGH";` `pub const DURABLE_NAME: &str = "agent-host-inbox";`

- [ ] **Step 1: Write the failing tests.**

```rust
// channels.rs
overlap_table   // ("iam.organization.>", "iam.organization.created") true; ("iam.*.created", "iam.organization.>") true;
                // ("iam.organization.>", "iam.principal.>") false; ("a.b", "a.b.c") false; ("a.>", "a") false; (">", "x") true; ("a.*", "a.b") true
match_table     // subject_matches("iam.organization.>", "iam.organization.created") true; ("iam.organization.>", "iam.organization") false; ("a.*.c", "a.b.c") true
// tenancy.rs (ORG = u(0x500))
the_org_prn_of_the_inbox_org_passes            // "prn:pgs:iam:::organization/<ORG>" -> true; upper-case UUID -> true
a_prn_with_the_org_segment_passes              // "prn:pgs:iam::<ORG>:team/<x>" -> true; "prn:pgs:agents::<ORG>:inbox/<x>" -> true
another_org_fails                              // organization/<u(0x600)> -> false
no_subject_or_no_org_fails_closed              // None, "", "prn:pgs:iam:::principal/<x>", "not a prn" -> false
// config.rs
two_overlapping_channels_are_refused
a_source_without_the_nats_prefix_is_refused
two_channels_with_one_source_are_refused
the_nats_section_is_optional                   // no [nats] -> nats None, validate Ok
// mod.rs: pin to ops/nats/subjects.env
the_rust_constants_match_subjects_env
    // include_str!("../../../../../../../ops/nats/subjects.env") contains AGENT_HOST_INBOX_PREFIX="<INBOX_PREFIX>" and AGENT_HOST_DURABLE_NAME="<DURABLE_NAME>"
```

  Negative control for `another_org_fails` (AG6): return `true` when the PRN parses. The test must
  fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -E 'test(nats) | test(config)'`. Expected: compile errors.
- [ ] **Step 3: Implement.** Overlap: split both filters on `.`; walk the tokens in pairs; a `>` on
  either side means overlap; `*` or equal tokens continue; else no overlap. At the end, overlap
  only when both have no tokens left. `subject_matches` is the same walk with a literal subject.
  `names_org` parses with `Prn::parse` and is true when the PRN is `iam` `organization` with
  `resource_id() == org`, or when `org() == Some(org)`. No NATS dependency in this task.
- [ ] **Step 4: Run and see them pass.** Same command, then clippy and fmt. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the NATS channel config and the org tenancy filter (SMA-787)`,
  body names §9.1, §9.4 and D6 = T1, then the co-author line.

---

### Task 18: The NATS consumer and the boot checks

**Files:**
- Create: `src/adapters/nats/consumer.rs`, `tests/nats_source.rs`
- Modify: `src/adapters/nats/mod.rs`, `src/main.rs` (`boot`, the server set, describe lines),
  `names.rs`, the service `Cargo.toml` (`async-nats`, `paigasus-nats-creds`; dev
  `testcontainers-modules` gets the feature `nats` next to `postgres`), `moon.yml` (`dependsOn` and `upstreams` for
  `paigasus-nats-creds-rs`), `rs/Cargo.lock`, `nextest.toml`, `tests/docker_preflight.rs`,
  `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: Task 17, Task 4 `ingest`, Task 2 `InboxRepository`.
- Produces:
  - `BoundChannel { filter: String, inbox: Inbox, source: String }`.
  - `pub async fn bind_channels(cfg: &NatsConfig, inboxes: &dyn InboxRepository) -> Result<Vec<BoundChannel>, NatsBootError>`
    (an unknown inbox is `NatsBootError::UnknownInbox(Uuid)`).
  - `pub fn check_filters(configured: &[String], filter_subject: &str, filter_subjects: &[String]) -> Result<(), NatsBootError>`
    (set equality of the configured subjects and the union of both server fields; Review Focus 3).
  - `pub enum Disposition { Ack, Nak(Duration), Term }`;
    `pub async fn handle_message(channels: &[BoundChannel], inboxes: &dyn InboxRepository, subject: &str, payload: &[u8], delivered: u64) -> Disposition`.
  - `pub async fn connect_and_check(cfg: &NatsConfig, channels: &[BoundChannel]) -> Result<NatsSource, NatsBootError>`
    (connect with the gateway pattern of `policy_events.rs:95-111`, `custom_inbox_prefix(INBOX_PREFIX)`;
    read `CONSUMER.INFO` with a 30 s bound; `check_filters`); `NatsSource::run(self, shutdown: watch::Receiver<()>) -> anyhow::Result<()>`.
  - `names::{AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL = "agent_host_nats_messages_terminated_total",
    AGENT_HOST_NATS_MESSAGES_TENANCY_SKIPPED_TOTAL = "agent_host_nats_messages_tenancy_skipped_total",
    AGENT_HOST_NATS_MESSAGES_UNMAPPED_TOTAL = "agent_host_nats_messages_unmapped_total"}`, in `ALL`.

- [ ] **Step 1: Write the failing tests.**

```rust
// consumer.rs unit tests (an in-memory InboxRepository; FIX_ORG = include_bytes! of organization_created_trailing_zeros.json)
an_org_event_for_the_inbox_org_is_inserted_and_acked   // inbox org u(0x500) -> Ack; stored source == channel source; envelope has no actorprn and no threadid
a_duplicate_is_acked                                   // second call -> Ack; one row
an_event_of_another_org_is_acked_and_skipped           // inbox org u(0x600) -> Ack; no row; tenancy counter == 1
a_bad_body_is_terminated                               // b"not json" -> Term; terminated counter == 1
an_unmapped_subject_is_nacked_with_the_maximum_delay   // subject "iam.role.granted" -> Nak(60 s); unmapped counter == 1
a_store_error_backs_off_and_doubles                    // delivered 1 -> Nak(1 s); 2 -> 2 s; 3 -> 4 s; 8 -> 60 s
check_filters_reads_both_server_fields                 // Review Focus 3: configured ["iam.organization.>"], filter_subject "iam.organization.>", filter_subjects [] -> Ok;
                                                       // filter_subjects ["iam.role.>"] -> Err; configured two, server one -> Err
// tests/nats_source.rs (real NATS via testcontainers nats module with JetStream, real Postgres;
// the test creates IAM_EVENTS and the durable itself, as provision.sh does; bytes from IAM's encoder: the Task 1 fixtures)
ack_on_insert                                          // publish FIX_ORG on iam.organization.created -> one row; consumer num_ack_pending == 0 and num_pending == 0 within 10 s
ack_on_duplicate                                       // publish the same bytes twice without Nats-Msg-Id -> one row; both acked
ack_and_skip_on_a_tenancy_refusal                      // inbox of another org -> no row; acked
term_on_a_bad_body                                     // publish b"{" -> acked by term: num_ack_pending 0, num_redelivered 0 after 3 s
redelivery_after_a_database_error                      // ALTER TABLE inbox_event RENAME TO inbox_event_off; publish; wait until num_redelivered >= 1;
                                                       // rename back; one row within 10 s
boot_fails_on_a_filter_mismatch                        // durable filter iam.role.> -> connect_and_check Err(FilterMismatch)
boot_fails_on_an_unknown_inbox                         // bind_channels with an inbox UUID that has no row -> Err(UnknownInbox)
```

  Register `nats_source` in `nextest.toml` and `docker_preflight.rs`.

  Negative controls:
  - `an_event_of_another_org_is_acked_and_skipped` (AG6): skip the `names_org` call. The test must
    fail.
  - `an_org_event_for_the_inbox_org_is_inserted_and_acked` (AG3): keep the incoming `source`. The
    test must fail.
- [ ] **Step 2: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -E 'test(consumer) | binary(nats_source)'`. Expected: compile errors.
- [ ] **Step 3: Implement.**
  - `handle_message`: find the one channel whose filter matches the subject (none: `Nak(60 s)`,
    counter, `error` log). Parse (error: `Term`, counter, `warn` log with the stream sequence, which
    the caller passes in the log span). `set_source(channel.source)`, `remove_extension("actorprn")`,
    `remove_extension("threadid")`. `names_org(subject(), channel.inbox.org())` false: `Ack`,
    counter, `debug` log. Then `ingest(…, Channel::Nats)` with `thread: None`: `Ok` → `Ack`;
    `Err` → `Nak(min(60, 2^(delivered-1)) s)`.
  - `NatsSource::run`: `consumer.messages()` of the pull consumer; per message call
    `handle_message` with `msg.info()?.delivered`, then `ack`, `ack_with(AckKind::Nak(Some(d)))` or
    `ack_with(AckKind::Term)`. Stop on the shutdown watch.
  - `main.rs` `boot`: after the migrations, when `config.nats` is set: `bind_channels`, then
    `connect_and_check`; an error fails the boot (it is a config error, §9.1). Spawn `run` into the
    server `JoinSet` with the shutdown watch. Add the three `describe_counter!` lines.
- [ ] **Step 4: Run and see them pass.** Same command, then clippy, fmt, `cd .. && moon run repo:deny repo:machete` and `/bin/bash ci/affected-graph/run.sh`. Expected: PASS, no FAIL row.
- [ ] **Step 5: Commit.** `feat(rs): add the NATS JetStream source of the inbox (SMA-787)`, body
  names §9.2, §9.3 and the ack rules, then the co-author line.

---

### Task 19: Documentation and the full gate graph

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/{README.md,CHANGELOG.md,agent-host.toml.example}`

- [ ] **Step 1: Update the README.** Add: the two routes with their status codes (§5.2) and the
  `store-unavailable` code; the authentication and authorisation rules (§6.2, §6.3) and the role
  `agent_event_writer`; the new tables and the two states of `inbox_event.thread_id` (§7.1); the
  NATS source, the boot order and the first-deployment steps (§9.1); the config table rows for
  every new key of §8.7 and §9.1 with the environment variable, the default and the range; the
  sentence "SMA-787 starts no worker. `worker.enabled` is read and validated only."; the sentence
  "The ingress routes are mounted only when `[iam]` is set."
- [ ] **Step 2: Update `agent-host.toml.example`.** Every new key, commented, with its default and
  range, in the form of the existing lines. The `[nats]` example is the §9.1 block.
- [ ] **Step 3: Update `CHANGELOG.md`** under `## [Unreleased]` with `### Added` lines: the
  CloudEvents ingress, the inbox tables, the lease and claim, the NATS source, `GET
  /v1/service-info`.
- [ ] **Step 4: Run the full gate graph.** Export the PATH line and `PROTO_REPORTER=text`. Copy the
  `moon ci …` command from the root `CLAUDE.md`, from between the `ci-targets` markers. Do not
  retype it. Run it from the repository root. Expected: every task passes, except the gates that
  need the other bash.
- [ ] **Step 5: Re-run the gates that need another bash, directly**, and read these results
  instead of the `moon ci` verdict for them:
  - `/bin/bash ci/affected-graph/run.sh` (system bash 3.2).
  - `/opt/homebrew/bin/bash ci/ruff/run.sh`, `/opt/homebrew/bin/bash ci/next-public/run.sh`,
    `/opt/homebrew/bin/bash ci/publish-metadata/run.sh` (bash 4+).
  - `/opt/homebrew/bin/bash ops/nats/check-subjects.sh`, then
    `( cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )`.
  - `/opt/homebrew/bin/bash ci/actionlint/run.sh`. Read its pipe preflight line first. If it
    reports a pipe below 8192 bytes, run the gate in a Linux container (`docker run ubuntu:24.04`)
    and say so in the report.
  Expected: each passes. A `mapfile`, `declare -A` or `local -n` failure means the wrong bash ran
  the gate; it is not a finding.
- [ ] **Step 6: Commit.** `docs(rs): document the event inbox of the agent host (SMA-787)`, body
  lists the gate results and the bash used for each re-run, then the co-author line.
