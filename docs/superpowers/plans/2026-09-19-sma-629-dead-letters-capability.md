# SMA-629 dead-letters capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register the `iam.deadletters` capability key, make IAM always report it, and add a Root-only `/iam/dead-letters` screen to the IAM console that lists, replays and discards single dead letters, visible only when IAM reports the key.

**Architecture:** One appended enum value in the proto registry, regenerated into the Rust, TS and Python bindings. IAM's `Capabilities::enabled()` pushes the new variant unconditionally. `@paigasus/console-core` gets an `outbox` client, one `IAM_ACTIONS` entry, fake-IAM routing and stateful dev-world handlers. The IAM console gets a nav entry (mayI + capability), a gated page whose every non-throwing view renders inside a client `DeadLettersFrame` that owns the only result region and one action runner (the SMA-636 gateway frame design), two Server Actions, and e2e rows R17–R19.

**Tech Stack:** protobuf + buf, Rust (prost, tonic, axum, nextest), TypeScript (Next.js 16 App Router, React 19, zod 4, vitest 5, Playwright 1.63, Testing Library), Python (betterproto2, pytest), Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md` (revision 2, approved 2026-09-19). Read it. Every `§` and `D` reference below is to that document.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-629-dead-letters`, branch `feature/sma-629-dead-letters`. Check the branch before the first commit.
- Before any Moon, buf, uv, pnpm or `cargo nextest` command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text`.
- If the sandbox refuses a compound shell command, write it to a script in `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/60d1d45d-b338-40eb-b51d-ba6e2f04ed45/scratchpad/` and run it with `/bin/bash <script>`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (TS, Rust) or `# SPDX-License-Identifier: Apache-2.0` (Python). A `'use client'` or `'use server'` directive comes after the header comment block, as in the existing files.
- Conventional commits with a workspace scope from the commitlint allowlist (`rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`, `repo`, `claude`, `workspace`). This plan uses `feat(contracts)`, `feat(rs)`, `feat(ts)`, `test(ts)` and `docs(repo)`, exactly as each task's commit step shows. Header at most 100 characters, body lines at most 100 characters. No body line may start with `#NNN` or have the shape `token: value` (it fails `footer-leading-blank`). Each commit step passes the trailer as its own `-m` paragraph.
- Every commit message ends with a blank line and then `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Add new commits. Never `git commit --amend`. Never `--no-verify`. Never bare `git stash`.
- The key is `iam.deadletters`. The enum value is `CAPABILITY_IAM_DEADLETTERS = 5`. The generated names are `Capability::IamDeadletters` (Rust), `Capability.IAM_DEADLETTERS` (TS and Python). The URL is `/iam/dead-letters`. The two Server Actions revalidate `revalidatePath('/dead-letters', 'page')` (basePath-relative, like `TENANCY_PATH`).
- After any `.proto` edit, run `buf format -w` on the file. `contracts:fmt` fails on an unformatted `.proto`, and the failure is easy to miss in a `moon ci` run.
- If `moon run contracts:generate` fails (for example a BSR rate limit), it deletes `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts`. Restore that file with `git checkout -- <path>` and run `moon run contracts:generate --force` again. Never commit that deletion.
- The IAM integration suites (`grpc_service_info`, `http_service_info`) need Docker. A filtered run needs `PAIGASUS_REQUIRE_DOCKER=1`, or a missing daemon skips the suite in silence. `paigasus-console-core-ts:test-e2e` and `gateway-console-ts:test-e2e` also need Docker, and a `console-core` or `iam-console` edit selects them.
- The React 19 frame rule (§ 6.4): a replay or discard result lives ONLY in `DeadLettersFrame`'s result region. No row control holds a result. The runner calls the Server Action directly inside a transition, with `null` as the previous state, never through `useActionState`. It catches a rejected promise and never rethrows.
- Server Action rules (`tests/unit/actions-structure.test.ts`): the first statement is `'use server'`; every export is an async function that calls `iamClientsForAction()` first; no `iamClients`, no `mayI`, no `redirect`, `permanentRedirect`, `forbidden`, `unauthorized` or `notFound`; a `'use server'` file exports only async functions.
- App code (`app/`, `lib/`) uses EXTENSIONLESS relative imports (Turbopack does not map `./x.js` to `./x.ts`).
- `lib/*.ts`, `load.ts` and `commands.ts` open with `import 'server-only'`. A client component, `app/_components/error-copy.ts` and `dead-letter-table.tsx` do not. A client component imports console-core names with `import type` only.
- `page.tsx` exports only its default export (Next refuses other page exports at build time).
- Playwright specs never call `locator.waitFor()` (`tests/unit/hydration.test.ts` scans for it). Use `waitForHydration(page)` after every `page.goto` that precedes a click, and `expect(...)` for every other wait.
- New Rust code emits no error code, so `repo:error-code-single-site` needs no `MANIFEST` edit. No new file name is a Windows reserved device name, and no new directory is named `build/`. The new route directory `app/(console)/dead-letters/` is covered by the existing `app/**/*` inputs and by `ts/moon.yml`'s `apps/*/app/**/*` source group; no Moon edit is needed.
- Do not write the name of Moon's CI report JSON file into any new file: `repo:actionlint` check 12 requires a marker on every file that names it.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `contracts/proto/paigasus/common/v1/service_info.proto` | Append `CAPABILITY_IAM_DEADLETTERS = 5`. Modify. | 1 |
| `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` | Regenerated. | 1 |
| `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts` | Regenerated. | 1 |
| `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py` | Regenerated. | 1 |
| `rs/crates/libs/paigasus-proto/src/capability.rs` | `ALL`, the spelling test, the "not registered" probe. Modify. | 1 |
| `ts/packages/paigasus-proto/src/capability.test.ts` | Spelling pin and member count. Modify. | 1 |
| `py/packages/paigasus-proto/tests/test_service_info_smoke.py` | Fifth name. Modify. | 1 |
| `ts/packages/paigasus-discovery/src/types.ts` | `CapabilityKey` gets `'iam.deadletters'`. Modify. | 1 |
| `ts/packages/paigasus-discovery/tests/vocabulary.test.ts` | `declared` gets the key. Modify. | 1 |
| `rs/crates/services/paigasus-iam/src/service_info.rs` | Always emit the key; unit tests. Modify. | 2 |
| `rs/crates/services/paigasus-iam/tests/grpc_service_info.rs` | Expected sets. Modify. | 2 |
| `rs/crates/services/paigasus-iam/tests/http_service_info.rs` | Expected sets; rename the all-off test. Modify. | 2 |
| `ts/packages/paigasus-console-core/src/iam-clients.ts` | `IamClients.outbox`. Modify. | 3 |
| `ts/packages/paigasus-console-core/src/authorize.ts` | `IAM_ACTIONS` gets `ListOutboxDeadLetters`. Modify. | 3 |
| `ts/packages/paigasus-console-core/testing/fake-iam.ts` | Route `OutboxService`; header says seven services. Modify. | 3 |
| `ts/packages/paigasus-console-core/tests/integration/outbox-client.test.ts` | The client reaches `OutboxService`. Create. | 3 |
| `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts` | Pin the one new name. Modify. | 3 |
| `ts/apps/iam-console/tests/unit/action-session.test.ts` | Exact `IamClients` key list. Modify. | 3 |
| `ts/apps/iam-console/tests/e2e/support/world.ts` | `ALL_ACTIONS` (Task 3); outbox defaults, ids, descriptor (Task 11). Modify. | 3, 11 |
| `ts/packages/paigasus-console-core/testing/dev-world.ts` | Descriptor key; stateful outbox handlers. Modify. | 4 |
| `ts/packages/paigasus-console-core/tests/unit/dev-world.test.ts` | Descriptor, `REQUIRED`, handler behaviour. Modify. | 4 |
| `ts/apps/iam-console/lib/time.ts` | `timestampIso`, moved from the audit loader. Create. | 5 |
| `ts/apps/iam-console/app/(console)/audit/load.ts` | Use `timestampIso`. Modify. | 5 |
| `ts/apps/iam-console/lib/paging.ts` | `parseEventType`, `MAX_EVENT_TYPE_LENGTH`, `listHref`. Modify. | 5 |
| `ts/apps/iam-console/lib/form.ts` | `refreshesAfterDeadLetterAction`. Modify. | 5 |
| `ts/apps/iam-console/tests/unit/lib-time.test.ts` | Create. | 5 |
| `ts/apps/iam-console/tests/unit/paging.test.ts`, `tests/unit/form.test.ts` | Modify. | 5 |
| `ts/apps/iam-console/lib/nav.ts` | The Dead letters entry. Modify. | 6 |
| `ts/apps/iam-console/app/(console)/layout.tsx` | Second `may(...)` in parallel. Modify. | 6 |
| `ts/apps/iam-console/tests/unit/nav.test.ts` | Full matrix. Replace. | 6 |
| `ts/apps/iam-console/app/(console)/dead-letters/load.ts` | `deadLettersGate`, `loadDeadLettersPage`, `DeadLetterRow`. Create. | 7 |
| `ts/apps/iam-console/tests/unit/dead-letters-gate.test.ts` | Create. | 7 |
| `ts/apps/iam-console/tests/integration/dead-letters-page.test.ts` | Loader against the fake IAM. Create. | 7 |
| `ts/apps/iam-console/tests/integration/support.ts` | `clientsFor` gets `outbox`. Modify. | 7 |
| `ts/apps/iam-console/app/_components/error-copy.ts` | `DEAD_LETTER_GONE`. Modify. | 8 |
| `ts/apps/iam-console/app/_components/form-error.tsx` | Optional `message` prop. Modify. | 8 |
| `ts/apps/iam-console/app/(console)/dead-letters/commands.ts` | `deadLetterForm`, `replayDeadLetter`, `discardDeadLetter`. Create. | 8 |
| `ts/apps/iam-console/app/(console)/dead-letters/actions.ts` | `replayDeadLetterAction`, `discardDeadLetterAction`. Create. | 8 |
| `ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts` | Create. | 8 |
| `ts/apps/iam-console/tests/unit/actions-structure.test.ts`, `actions-revalidate.test.ts`, `error-copy.test.ts`, `error-views.test.tsx` | Modify. | 8 |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx` | Client frame, runner, result region, row controls. Create. | 9 |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx` | The table, row details, `<pre>` payload. Create. | 9 |
| `ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx` | jsdom frame test. Create. | 9 |
| `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` | The page. Create. | 10 |
| `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts` | Branch-by-branch element-tree test. Create. | 10 |
| `ts/apps/iam-console/tests/e2e/dead-letters.spec.ts` | R17–R19. Create. | 11 |
| `ts/apps/iam-console/tests/e2e/token-leak.spec.ts` | R11 covers the new page and a replay. Modify. | 11 |
| `ts/apps/iam-console/tests/unit/e2e-rows.test.ts` | 16 → 19 rows. Modify. | 11 |
| `docs/ops/RUNBOOK-observability.md` | One remediation bullet. Modify. | 12 |

A plan decision beyond the spec's file list: `dead-letter-table.tsx` holds the table markup so that the jsdom frame test renders the REAL row, details and `<pre>` code, and so proves the payload escaping (§ 7.2) against production markup rather than a test copy.

---

### Task 1: The registry value, the regenerated bindings, and every spelling pin

Regenerating the bindings reds three suites at once: the Rust probe `adding_a_capability_forces_updating_these_tests`, the TS member count, and `@paigasus/discovery`'s `the CapabilityKey union matches the registry exactly` (because `CAPABILITY_KEYS` derives from the generated enum). So the discovery vocabulary edit lives in this task, not in a later one, and the commit leaves every suite green.

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/service_info.proto` (after `CAPABILITY_GATEWAY_CHAT_STREAM = 4;`, `:142`)
- Regenerate: the three generated files named in the File Structure table
- Modify: `rs/crates/libs/paigasus-proto/src/capability.rs` (`:69`, `:80-85`, `:156-161`)
- Modify: `ts/packages/paigasus-proto/src/capability.test.ts` (`:7-12`, `:24-29`)
- Modify: `py/packages/paigasus-proto/tests/test_service_info_smoke.py` (`:17-22`)
- Modify: `ts/packages/paigasus-discovery/src/types.ts` (`:68`)
- Modify: `ts/packages/paigasus-discovery/tests/vocabulary.test.ts` (`:46`)

**Interfaces:**
- Produces: proto `CAPABILITY_IAM_DEADLETTERS = 5`; Rust `Capability::IamDeadletters`; TS `Capability.IAM_DEADLETTERS`; Python `Capability.IAM_DEADLETTERS`; TS `type CapabilityKey = 'iam.authz.cedar' | 'iam.apikeys' | 'iam.audit' | 'gateway.chat.stream' | 'iam.deadletters'`.

- [ ] **Step 1: Write the failing tests**

In `rs/crates/libs/paigasus-proto/src/capability.rs`, replace the `ALL` constant (`:69`):

```rust
    const ALL: [Capability; 5] = [Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamAudit, Capability::GatewayChatStream, Capability::IamDeadletters];
```

In `the_registry_spells_the_adr_keys_exactly`, add after the `gateway.chat.stream` line:

```rust
        // SMA-629 D3. The round-trip test alone would also pass for "iam.dead.letters"; this pins
        // the single-segment spelling.
        assert_eq!(Capability::IamDeadletters.as_wire_key().unwrap(), "iam.deadletters");
```

Replace `adding_a_capability_forces_updating_these_tests`:

```rust
    #[test]
    fn adding_a_capability_forces_updating_these_tests() {
        // ALL covers discriminants 1..=5. Registering a sixth value fails here,
        // which is the signal to extend ALL and the literals test above.
        assert!(Capability::try_from(6).is_err());
    }
```

In `ts/packages/paigasus-proto/src/capability.test.ts`, add to `spells the ADR-0020 keys exactly` after the gateway line:

```ts
    // SMA-629 D3: one segment, not "iam.dead.letters".
    expect(capabilityWireKey(Capability.IAM_DEADLETTERS)).toBe('iam.deadletters');
```

and replace the count test:

```ts
  it('covers exactly the registered capabilities', () => {
    // Guards against a sixth capability being registered without a
    // corresponding assertion above. Six members: UNSPECIFIED plus five keys.
    const members = Object.values(Capability).filter((v) => typeof v === 'number');
    expect(members).toHaveLength(6);
  });
```

In `py/packages/paigasus-proto/tests/test_service_info_smoke.py`, append to `test_capability_registry_keeps_the_proto_names` (symmetry only; § 4.3):

```python
    assert names[Capability.IAM_DEADLETTERS.value] == "CAPABILITY_IAM_DEADLETTERS"
```

In `ts/packages/paigasus-discovery/tests/vocabulary.test.ts`, replace the `declared` line:

```ts
    const declared: CapabilityKey[] = ['iam.authz.cedar', 'iam.apikeys', 'iam.audit', 'gateway.chat.stream', 'iam.deadletters'];
```

- [ ] **Step 2: Run the tests and see them fail**

From `rs/`:
```bash
cargo nextest run -p paigasus-proto capability
```
Expected: a compile error, `no variant or associated item named IamDeadletters found for enum Capability`.

From the worktree root:
```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/capability.test.ts
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/vocabulary.test.ts
```
Expected: `spells the ADR-0020 keys exactly` fails (`expected undefined to be 'iam.deadletters'`), `covers exactly the registered capabilities` fails (`expected length 6, received 5`), and `the CapabilityKey union matches the registry exactly` fails (the declared list has one entry more than `CAPABILITY_KEYS`).

From `py/`:
```bash
uv run --locked pytest packages/paigasus-proto/tests/test_service_info_smoke.py -q
```
Expected: FAIL with `AttributeError` on `IAM_DEADLETTERS`.

- [ ] **Step 3: Append the enum value, format, and regenerate**

In `contracts/proto/paigasus/common/v1/service_info.proto`, after `CAPABILITY_GATEWAY_CHAT_STREAM = 4;` and before the closing `}` of `enum Capability`, add a blank line and:

```proto
  // "iam.deadletters" — the Root-only dead-letter queue (OutboxService) is served. IAM always
  // reports it: OutboxService has no config switch (a break-glass surface).
  CAPABILITY_IAM_DEADLETTERS = 5;
```

From `contracts/`:
```bash
buf format -w proto/paigasus/common/v1/service_info.proto
```

From the worktree root:
```bash
moon run contracts:fmt contracts:lint contracts:breaking
moon run contracts:generate --force
git status --short
```
Expected: `contracts:fmt`, `contracts:lint` and `contracts:breaking` pass (an appended enum value is not a breaking change). `git status --short` shows the `.proto`, the five test and type files of Step 1 and Step 3, and exactly the three generated files from the File Structure table. If `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` shows as deleted, restore it with `git checkout -- ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` and run `moon run contracts:generate --force` again. If any OTHER generated file changed, stop and inspect it before you continue.

Confirm the generated names:
```bash
grep -n "IamDeadletters" rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs
grep -n "IAM_DEADLETTERS" ts/packages/paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts
grep -n "IAM_DEADLETTERS" py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py
```
Expected: `IamDeadletters = 5`, `IAM_DEADLETTERS = 5` and `IAM_DEADLETTERS = 5`. If a name differs, use the generated name everywhere in this plan instead.

In `ts/packages/paigasus-discovery/src/types.ts`, replace line `:68`:

```ts
export type CapabilityKey = 'iam.authz.cedar' | 'iam.apikeys' | 'iam.audit' | 'gateway.chat.stream' | 'iam.deadletters';
```

- [ ] **Step 4: Run the tests and see them pass**

From `rs/`: `cargo nextest run -p paigasus-proto`
From the worktree root:
```bash
pnpm -C ts/packages/paigasus-proto exec vitest run
pnpm -C ts/packages/paigasus-discovery exec vitest run
moon run paigasus-discovery-ts:typecheck paigasus-proto-ts:typecheck paigasus-iam-rs:build
```
From `py/`: `uv run --locked pytest packages/paigasus-proto/tests/test_service_info_smoke.py -q`
Expected: all PASS. `paigasus-iam-rs:build` proves that IAM still compiles against the regenerated enum.

- [ ] **Step 5: Commit**

```bash
git add contracts/proto/paigasus/common/v1/service_info.proto rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs ts/packages/paigasus-proto/src/generated/paigasus/common/v1/service_info_pb.ts py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py rs/crates/libs/paigasus-proto/src/capability.rs ts/packages/paigasus-proto/src/capability.test.ts py/packages/paigasus-proto/tests/test_service_info_smoke.py ts/packages/paigasus-discovery/src/types.ts ts/packages/paigasus-discovery/tests/vocabulary.test.ts
git commit -m "feat(contracts): register the iam.deadletters capability key (SMA-629)" -m "Append CAPABILITY_IAM_DEADLETTERS = 5 to the Capability registry and regenerate the
Rust, TS and Python bindings. Pin the one-segment spelling in the Rust and TS tests, and
add the key to the discovery CapabilityKey union that the vocabulary test holds to the enum." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: IAM always reports `iam.deadletters`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/service_info.rs` (module doc `:9-11`; `enabled()` `:45-58`; tests `:91-125`)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_service_info.rs` (`:108`, `:177`)
- Modify: `rs/crates/services/paigasus-iam/tests/http_service_info.rs` (`:49-52`, `:74-77`, `:119`, `:156`, `:157-179`)

**Interfaces:**
- Consumes: `Capability::IamDeadletters` (Task 1).
- Produces: `Capabilities::enabled()` always contains `Capability::IamDeadletters`. `Capabilities` keeps exactly its three boolean fields (no fourth field, § 4.2).

- [ ] **Step 1: Write the failing unit tests**

In the `tests` module of `src/service_info.rs`, replace the four tests from `all_enabled_advertises_every_iam_capability` to `every_combination_advertises_exactly_its_enabled_keys` with:

```rust
    #[test]
    fn all_enabled_advertises_every_iam_capability() {
        assert_eq!(
            caps(true, true, true),
            HashSet::from([Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamAudit, Capability::IamDeadletters])
        );
    }

    /// SMA-629 D2: OutboxService has no config switch, so every flag off still advertises it.
    #[test]
    fn all_flags_off_advertises_only_iam_deadletters() {
        assert_eq!(caps(false, false, false), HashSet::from([Capability::IamDeadletters]));
    }

    /// AC 3's central assertion. Asserting only "the key is absent" would pass against an
    /// implementation returning an empty list unconditionally, so every case ALSO asserts the
    /// siblings survive.
    #[test]
    fn disabling_one_flag_removes_exactly_its_key() {
        assert_eq!(caps(false, true, true), HashSet::from([Capability::IamApikeys, Capability::IamAudit, Capability::IamDeadletters]));
        assert_eq!(caps(true, false, true), HashSet::from([Capability::IamAuthzCedar, Capability::IamAudit, Capability::IamDeadletters]));
        assert_eq!(caps(true, true, false), HashSet::from([Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamDeadletters]));
    }

    /// R3: the real risk surface is combinations, not single flags. All 8 are cheap here
    /// because this is a pure function. The dead-letters key is in EVERY combination.
    #[test]
    fn every_combination_advertises_exactly_its_enabled_keys() {
        for authz in [false, true] {
            for apikeys in [false, true] {
                for audit in [false, true] {
                    let got = caps(authz, apikeys, audit);
                    assert_eq!(got.contains(&Capability::IamAuthzCedar), authz);
                    assert_eq!(got.contains(&Capability::IamApikeys), apikeys);
                    assert_eq!(got.contains(&Capability::IamAudit), audit);
                    assert!(got.contains(&Capability::IamDeadletters), "iam.deadletters must be advertised for ({authz}, {apikeys}, {audit})");
                }
            }
        }
    }
```

- [ ] **Step 2: Run the unit tests and see them fail**

From `rs/`: `cargo nextest run -p paigasus-iam --lib service_info`
Expected: 4 FAIL. `all_flags_off_advertises_only_iam_deadletters` reports `left: {}` against `right: {IamDeadletters}`.

- [ ] **Step 3: Implement**

Replace `enabled()` and its doc comment:

```rust
    /// The registered capabilities this build currently has enabled. Pure — the unit under
    /// test for AC 3.
    ///
    /// `iam.deadletters` is UNCONDITIONAL (SMA-629 D2). `OutboxService` is registered on gRPC and
    /// HTTP with no config switch, because a break-glass surface must not be disable-able
    /// (`adapters/grpc/dead_letters.rs:13-18`). The key therefore means "this build serves
    /// `OutboxService`". It has no field on `Capabilities`: a field that is always `true` would be
    /// a false degree of freedom.
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
        caps.push(Capability::IamDeadletters);
        caps
    }
```

Replace the module-doc paragraph that starts "`enabled()` is a pure function of three booleans" with:

```rust
//! `enabled()` is a pure function of three booleans, plus one unconditional key
//! (`iam.deadletters`, SMA-629). That is what makes AC 3's central assertion ("flip the flag, the
//! key disappears, the siblings remain") an ordinary unit test with no `AppState`, no Postgres and
//! no Docker.
```

- [ ] **Step 4: Run the unit tests and see them pass**

From `rs/`: `cargo nextest run -p paigasus-iam --lib service_info`
Expected: PASS, including the unchanged `every_advertised_string_is_a_registered_capability_key`.

- [ ] **Step 5: Update the integration tests**

`tests/grpc_service_info.rs:108`:

```rust
    assert_eq!(
        caps,
        HashSet::from(["iam.authz.cedar".to_string(), "iam.apikeys".to_string(), "iam.audit".to_string(), "iam.deadletters".to_string()])
    );
```

In the audit-off block of `the_grpc_and_http_transports_describe_the_same_build`, replace the "siblings must survive" assertion (`:177`):

```rust
        assert!(
            grpc_caps.contains("iam.authz.cedar") && grpc_caps.contains("iam.apikeys") && grpc_caps.contains("iam.deadletters"),
            "siblings must survive: {grpc_caps:?}"
        );
```

`tests/http_service_info.rs`, in `the_descriptor_requires_a_bearer_and_reports_every_enabled_capability`:

```rust
    assert_eq!(
        capability_set(&body),
        std::collections::HashSet::from(["iam.authz.cedar".to_string(), "iam.apikeys".to_string(), "iam.audit".to_string(), "iam.deadletters".to_string()])
    );
```

In `disabling_audit_query_removes_both_the_route_and_the_key`, add after the two sibling asserts:

```rust
    assert!(caps.contains("iam.deadletters"), "siblings must survive: {body}");
```

In `disabling_authz_admin_removes_policy_role_grant_and_retirement_routes`, replace the final assert:

```rust
    assert!(caps.contains("iam.apikeys") && caps.contains("iam.audit") && caps.contains("iam.deadletters"), "siblings must survive: {body}");
```

In `disabling_apikey_management_removes_management_but_keeps_introspection`, replace the final assert:

```rust
    assert!(caps.contains("iam.authz.cedar") && caps.contains("iam.audit") && caps.contains("iam.deadletters"), "siblings must survive: {body}");
```

Replace the last test of the file (doc comment included) with:

```rust
/// Every capability flag off (SMA-629 D2). IAM can no longer emit an empty list, because
/// `iam.deadletters` is unconditional. The test keeps its second purpose, R3's multi-flag
/// combination: conditional router merging must not panic at registration with every flag off.
/// The empty-array rule itself (SMA-499 § 2.7's MUST-emit-defaults) stays proven by
/// `rs/crates/services/paigasus-gateway/tests/service_info.rs:281-295` and by
/// `rs/crates/libs/paigasus-service-info/src/lib.rs:107`.
#[tokio::test]
async fn all_capability_flags_off_serves_only_iam_deadletters() {
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
    assert_eq!(body["capabilities"], serde_json::json!(["iam.deadletters"]), "only the unconditional key remains: {body}");
}
```

- [ ] **Step 6: Run the integration suites (Docker needed)**

From `rs/`:
```bash
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_service_info --test http_service_info
```
Expected: PASS. Without Docker this panics instead of skipping, which is the intent.

- [ ] **Step 7: Format, lint, commit**

From `rs/`:
```bash
cargo fmt --check
cargo clippy --locked -p paigasus-iam -p paigasus-proto --all-targets -- -D warnings
```
Expected: no fmt diff, no clippy warning. On a fmt diff, run `cargo fmt` and check again.

```bash
git add rs/crates/services/paigasus-iam/src/service_info.rs rs/crates/services/paigasus-iam/tests/grpc_service_info.rs rs/crates/services/paigasus-iam/tests/http_service_info.rs
git commit -m "feat(rs): IAM always reports the iam.deadletters capability (SMA-629)" -m "OutboxService has no config switch, so Capabilities::enabled() pushes the key with no
condition and Capabilities keeps its three config-backed booleans. The all-flags-off HTTP
test now asserts the one-key list and keeps its router-merge purpose." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: `@paigasus/console-core` — the `outbox` client, the action name, the fake IAM

Adding a name to `IAM_ACTIONS` reds the IAM console's `tests/unit/world-actions.test.ts`, which holds the e2e world's `ALL_ACTIONS` to the same SET. Adding `outbox` to `IamClients` reds `tests/unit/action-session.test.ts:110`, which pins the exact key list. Both edits are therefore in this task, not in the e2e task.

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/iam-clients.ts` (`:13`, `:16-24`, `:46-58`)
- Modify: `ts/packages/paigasus-console-core/src/authorize.ts` (`:26-59`)
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts` (`:3-4`, `:51`, `:57-66`)
- Create: `ts/packages/paigasus-console-core/tests/integration/outbox-client.test.ts`
- Modify: `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts` (after `:60`)
- Modify: `ts/apps/iam-console/tests/unit/action-session.test.ts` (`:103-111`)
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts` (`ALL_ACTIONS`, `:47-69`)

**Interfaces:**
- Consumes: `OutboxService` from `@paigasus/sdk/iam` (already exported, `ts/packages/paigasus-sdk/src/iam.ts:16`) and from `@paigasus/proto/iam`.
- Produces:
  - `IamClients['outbox']: Client<typeof OutboxService>`, with methods `listDeadLetters`, `replayDeadLetter`, `bulkReplayDeadLetters`, `discardDeadLetter`.
  - `IamAction` includes `'ListOutboxDeadLetters'`.
  - `FakeIamMethod` includes `'outbox.listDeadLetters' | 'outbox.replayDeadLetter' | 'outbox.bulkReplayDeadLetters' | 'outbox.discardDeadLetter'`.

- [ ] **Step 1: Write the failing tests**

Create `ts/packages/paigasus-console-core/tests/integration/outbox-client.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-629 spec § 5.3. IamClients carries an outbox client, and the fake IAM routes OutboxService
// and records each call with its bearer and its correlation id.
//
// An unrouted service also answers Unimplemented, so "rejects" alone proves nothing about routing.
// The proof is the call log: the fake records a call only in its own dispatch.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createIamClients } from '../../src/iam-clients';
import { startFakeIam, type FakeIam } from '../../testing/index';

const ID = '0190a1f0-0000-7000-8000-0000000000d1';
const CID = '0198f2c1-8888-7000-8000-000000000629';
const METHODS = ['listDeadLetters', 'replayDeadLetter', 'bulkReplayDeadLetters', 'discardDeadLetter'] as const;

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

describe('the outbox client', () => {
  it('reaches the fake OutboxService with the bearer and the correlation id', async () => {
    iam.setHandlers({
      'outbox.listDeadLetters': (req) => ({ entries: [{ id: ID, eventType: req.eventType }], nextCursor: '' }),
    });
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-outbox', correlationId: CID });

    const listed = await clients.outbox.listDeadLetters({ eventType: 'iam.team.created', cursor: '', limit: 50 });

    expect(listed.entries.map((entry) => entry.id)).toEqual([ID]);
    const [call] = iam.callsTo('outbox.listDeadLetters');
    expect(call?.token).toBe('tok-outbox');
    expect(call?.correlationId).toBe(CID);
    expect(call?.request).toMatchObject({ eventType: 'iam.team.created', limit: 50 });
  });

  it('routes all four RPCs, and an unscripted one answers Unimplemented', async () => {
    iam.setHandlers({});
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-outbox' });
    const before = Object.fromEntries(METHODS.map((method) => [method, iam.callsTo(`outbox.${method}`).length]));

    const results = await Promise.allSettled([
      clients.outbox.listDeadLetters({}),
      clients.outbox.replayDeadLetter({ id: ID }),
      clients.outbox.bulkReplayDeadLetters({ maxRows: 1n }),
      clients.outbox.discardDeadLetter({ id: ID }),
    ]);

    for (const result of results) {
      expect(result.status).toBe('rejected');
      if (result.status === 'rejected') expect((result.reason as ConnectError).code).toBe(Code.Unimplemented);
    }
    for (const method of METHODS) {
      expect(iam.callsTo(`outbox.${method}`).length - (before[method] ?? 0)).toBe(1);
    }
  });
});
```

In `tests/unit/action-names.test.ts`, add inside the `describe` after the SMA-636 test:

```ts
  // SMA-629 spec § 5.3. The IAM console's layout asks ListOutboxDeadLetters at Root to show the Dead
  // letters entry. Replay and discard have no mayI caller: the page is Root-only, so a user who can
  // list can act, and IAM decides every action anyway.
  it('holds ListOutboxDeadLetters, and neither ReplayOutboxDeadLetter nor DiscardOutboxDeadLetter', () => {
    expect([...IAM_ACTIONS]).toContain('ListOutboxDeadLetters');
    expect([...IAM_ACTIONS]).not.toContain('ReplayOutboxDeadLetter');
    expect([...IAM_ACTIONS]).not.toContain('DiscardOutboxDeadLetter');
  });
```

In `ts/apps/iam-console/tests/unit/action-session.test.ts`, replace the test `returns the six clients when the session resolves`:

```ts
  it('returns the seven clients when the session resolves', async () => {
    setRequestHeaders({});
    signedIn();

    const result = await iamClientsForAction();

    expect(result.ok).toBe(true);
    // SMA-636 added serviceAccounts to IamClients, and SMA-629 added outbox.
    if (result.ok) expect(Object.keys(result.value).sort()).toEqual(['audit', 'authn', 'authz', 'outbox', 'serviceAccounts', 'serviceInfo', 'tenancy']);
  });
```

- [ ] **Step 2: Run the tests and see them fail**

```bash
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/integration/outbox-client.test.ts tests/unit/action-names.test.ts
pnpm -C ts/apps/iam-console exec vitest run tests/unit/action-session.test.ts
```
Expected: `outbox-client.test.ts` fails with `TypeError: Cannot read properties of undefined (reading 'listDeadLetters')`; the action-names case fails (`expected [...] to include 'ListOutboxDeadLetters'`); action-session fails (the key list lacks `outbox`).

- [ ] **Step 3: Implement**

`src/iam-clients.ts`, the import line:

```ts
import { AuditService, AuthnService, AuthorizationService, createIamClient, OutboxService, ServiceAccountService, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
```

Add to `IamClients` after `serviceAccounts`:

```ts
  /** The Root-only dead-letter queue (SMA-629): the IAM zone's /iam/dead-letters page. */
  outbox: Client<typeof OutboxService>;
```

Change the doc comment `/** The six clients for one bearer token, …` to `/** The seven clients for one bearer token, over one base URL. Pure: tests call it with a fake IAM. */`, and add to the returned object after `serviceAccounts`:

```ts
    outbox: withCorrelation(createIamClient(OutboxService, transport, auth), id),
```

`src/authorize.ts`: add after `'ListAuditLog',` in `IAM_ACTIONS`:

```ts
  // SMA-629: the Dead letters nav entry asks this at Root. Replay and discard are deliberately
  // ABSENT: the page is Root-only, so no button needs its own question.
  'ListOutboxDeadLetters',
```

`testing/fake-iam.ts`:
- Header line `:3-4`: replace "for the six services the console calls (SMA-636 added ServiceAccountService)" with "for the seven services the console calls (SMA-636 added ServiceAccountService, SMA-629 OutboxService)".
- Import line `:51`:

```ts
import { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService } from '@paigasus/proto/iam';
```

- Add to `SERVICES` after `serviceAccounts: ServiceAccountService,`:

```ts
  // SMA-629. No default answers, like `audit`: an unscripted dead-letter RPC answers Unimplemented.
  outbox: OutboxService,
```

`DEFAULT_DESCRIPTOR` does NOT change (§ 5.3): a missing key is the default in every test, so AC 2 needs no special setup.

`ts/apps/iam-console/tests/e2e/support/world.ts`, `ALL_ACTIONS`: add after `'ListAuditLog',`:

```ts
  // SMA-629: the Dead letters nav entry.
  'ListOutboxDeadLetters',
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/packages/paigasus-console-core exec vitest run
pnpm -C ts/apps/iam-console exec vitest run tests/unit/action-session.test.ts tests/unit/world-actions.test.ts
moon run paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck
```
Expected: all PASS. `world-actions.test.ts` passes only because `ALL_ACTIONS` changed with `IAM_ACTIONS`.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-console-core/src/iam-clients.ts ts/packages/paigasus-console-core/src/authorize.ts ts/packages/paigasus-console-core/testing/fake-iam.ts ts/packages/paigasus-console-core/tests/integration/outbox-client.test.ts ts/packages/paigasus-console-core/tests/unit/action-names.test.ts ts/apps/iam-console/tests/unit/action-session.test.ts ts/apps/iam-console/tests/e2e/support/world.ts
git commit -m "feat(ts): an outbox client and the ListOutboxDeadLetters action name (SMA-629)" -m "IamClients gets outbox, the fake IAM routes OutboxService with no default answers, and
IAM_ACTIONS gets the one name the IAM console layout asks. The e2e world's ALL_ACTIONS and
the exact IamClients key list move with them." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The dev world models a current IAM

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/dev-world.ts` (header `:1-15`; `DEV_IAM_DESCRIPTOR` `:43-44`; new constants after `:46`; `devWorld()` `:69-199`)
- Modify: `ts/packages/paigasus-console-core/tests/unit/dev-world.test.ts` (`REQUIRED` `:11-37`, `:79-85`, new cases)

**Interfaces:**
- Consumes: `FakeIamHandlers` keys `outbox.*` (Task 3).
- Produces: `DEV_IAM_DESCRIPTOR.capabilities` is `['iam.authz.cedar', 'iam.audit', 'iam.deadletters']`; `devWorld()` scripts `outbox.listDeadLetters`, `outbox.replayDeadLetter`, `outbox.discardDeadLetter` over three seeded entries.

- [ ] **Step 1: Write the failing tests**

In `tests/unit/dev-world.test.ts`, append to `REQUIRED` after `'audit.listAuditEntries',`:

```ts
  'outbox.listDeadLetters',
  'outbox.replayDeadLetter',
  'outbox.discardDeadLetter',
```

Replace the test `pins the two descriptors the consoles switch on`:

```ts
  it('pins the two descriptors the consoles switch on', () => {
    // myScopes() lists role grants only when discovery reports iam.authz.cedar
    // (src/scopes.ts:116-118), the audit page needs iam.audit, and the dead-letters page needs
    // iam.deadletters. The dev world is a CURRENT IAM, which always reports that key (SMA-629).
    expect(DEV_IAM_DESCRIPTOR.capabilities).toEqual(['iam.authz.cedar', 'iam.audit', 'iam.deadletters']);
    expect(DEV_GATEWAY_DESCRIPTOR.capabilities).toEqual(['gateway.chat.stream']);
  });
```

Add at the end of the `describe('devWorld', …)` block:

```ts
  // SMA-629 spec § 5.3: three parked entries with RFC 4122 ids, newest first, as IAM orders them.
  const RFC_4122 = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  type Listed = { entries: { id: string; eventType: string }[]; nextCursor: string };
  const list = (handlers: ReturnType<typeof devWorld>, eventType = ''): Listed => handlers['outbox.listDeadLetters']?.({ eventType } as never, {} as never) as Listed;

  it('lists three dead letters with RFC 4122 ids, in descending id order', () => {
    const listed = list(devWorld());
    expect(listed.entries).toHaveLength(3);
    for (const entry of listed.entries) expect(entry.id).toMatch(RFC_4122);
    const ids = listed.entries.map((entry) => entry.id);
    expect(ids).toEqual([...ids].sort().reverse());
    expect(listed.nextCursor).toBe('');
  });

  it('filters by the exact event type', () => {
    const handlers = devWorld();
    const [first] = list(handlers).entries;
    expect(first).toBeDefined();
    const filtered = list(handlers, first?.eventType);
    expect(filtered.entries.every((entry) => entry.eventType === first?.eventType)).toBe(true);
    expect(filtered.entries.length).toBeGreaterThan(0);
  });

  it.each(['outbox.replayDeadLetter', 'outbox.discardDeadLetter'] as const)('%s removes the entry, and a second call answers NotFound', (method) => {
    const handlers = devWorld();
    const id = list(handlers).entries[0]?.id;
    expect(id).toBeDefined();
    const answer = handlers[method]?.({ id } as never, {} as never) as { entry: { id: string } };
    expect(answer.entry.id).toBe(id);
    expect(list(handlers).entries.map((entry) => entry.id)).not.toContain(id);
    let caught: unknown;
    try {
      void handlers[method]?.({ id } as never, {} as never);
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(ConnectError);
    expect((caught as ConnectError).code).toBe(Code.NotFound);
  });
```

- [ ] **Step 2: Run the test and see it fail**

```bash
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/dev-world.test.ts
```
Expected: FAIL. `scripts every method the consoles reach` misses `outbox.listDeadLetters`; the descriptor test gets two keys; the list test throws `Cannot read properties of undefined (reading 'entries')`.

- [ ] **Step 3: Implement**

In the header comment of `testing/dev-world.ts`, change the first sentence to: "The IAM the DEV STACK talks to (SMA-641): one organization, one team, one project, one audit entry, three dead letters (SMA-629), every action allowed. It models a CURRENT IAM, which always reports `iam.deadletters`."

Replace `DEV_IAM_DESCRIPTOR` and its comment:

```ts
/**
 * iam.authz.cedar gates myScopes() (src/scopes.ts:116-118); iam.audit gates the audit page;
 * iam.deadletters gates the dead-letters page. A current IAM always reports the last one (SMA-629 D2).
 */
export const DEV_IAM_DESCRIPTOR: ServiceDescriptorBody = { service: 'iam', version: '0.0.0-dev', capabilities: ['iam.authz.cedar', 'iam.audit', 'iam.deadletters'] };
```

Add after `DEV_GATEWAY_DESCRIPTOR`:

```ts
/** A parked outbox event, as IAM's DeadLetterEntry carries it. `''` means "none" (iam.proto:619-624). */
type DeadLetterFixture = {
  id: string;
  occurredAt: { seconds: bigint; nanos: number };
  eventType: string;
  schemaVersion: number;
  aggregatePrn: string;
  actorPrn: string;
  payload: string;
  correlationId: string;
  attempts: number;
  parkedAt: { seconds: bigint; nanos: number };
  lastError: string;
};

/** Three parked events with RFC 4122 (UUIDv7-shaped) ids, like the ids IAM mints. */
function seededDeadLetters(): Map<string, DeadLetterFixture> {
  const entries: DeadLetterFixture[] = [
    {
      id: '0190a1f0-0000-7000-8000-00000000d201',
      occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
      eventType: 'iam.organization.created',
      schemaVersion: 1,
      aggregatePrn: ORG_PRN,
      actorPrn: PRINCIPAL_PRN,
      payload: '{"slug":"dev","name":"Dev Organization"}',
      correlationId: 'corr-dev-dead-letter-1',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_300n, nanos: 0 },
      lastError: 'nats: no responders available for request',
    },
    {
      id: '0190a1f0-0000-7000-8000-00000000d202',
      occurredAt: { seconds: 1_788_000_600n, nanos: 0 },
      eventType: 'iam.team.created',
      schemaVersion: 1,
      aggregatePrn: TEAM_PRN,
      actorPrn: PRINCIPAL_PRN,
      payload: '{"slug":"platform","name":"Platform Team"}',
      correlationId: 'corr-dev-dead-letter-2',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_900n, nanos: 0 },
      lastError: 'nats: timeout',
    },
    {
      id: '0190a1f0-0000-7000-8000-00000000d203',
      occurredAt: { seconds: 1_788_001_200n, nanos: 0 },
      eventType: 'iam.project.created',
      schemaVersion: 2,
      aggregatePrn: PROJECT_PRN,
      actorPrn: '',
      payload: '{"slug":"gateway","name":"Inference Gateway"}',
      correlationId: '',
      attempts: 5,
      parkedAt: { seconds: 1_788_001_500n, nanos: 0 },
      lastError: '',
    },
  ];
  return new Map(entries.map((entry) => [entry.id, entry]));
}
```

Inside `devWorld()`, after `const projects = …`:

```ts
  // SMA-629: replay and discard remove the entry, as IAM does; an unknown id answers NotFound.
  const deadLetters = seededDeadLetters();

  function takeDeadLetter(id: string): DeadLetterFixture {
    const entry = deadLetters.get(id);
    if (entry === undefined) throw notFound();
    deadLetters.delete(id);
    return entry;
  }
```

Add to the returned map after `'audit.listAuditEntries': …`:

```ts
    // IAM orders by id DESCENDING (tests/dead_letters_pg.rs:432) and matches event_type exactly.
    'outbox.listDeadLetters': (req: { eventType: string }) => ({
      entries: [...deadLetters.values()].filter((entry) => req.eventType === '' || entry.eventType === req.eventType).sort((a, b) => (a.id < b.id ? 1 : -1)),
      nextCursor: '',
    }),
    'outbox.replayDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(req.id) }),
    'outbox.discardDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(req.id) }),
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/dev-world.test.ts
moon run paigasus-console-core-ts:typecheck
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-console-core/testing/dev-world.ts ts/packages/paigasus-console-core/tests/unit/dev-world.test.ts
git commit -m "feat(ts): the dev world reports iam.deadletters and serves three dead letters (SMA-629)" -m "The dev stack models a current IAM, so its descriptor carries the key, and it scripts
stateful list, replay and discard handlers over three seeded entries with RFC 4122 ids." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: IAM console `lib/` helpers — time, query parsing, hrefs, the revalidate rule

**Files:**
- Create: `ts/apps/iam-console/lib/time.ts`
- Modify: `ts/apps/iam-console/app/(console)/audit/load.ts` (`:9`, `:44-53`, `:60`)
- Modify: `ts/apps/iam-console/lib/paging.ts` (header `:3-5`; add after `parseCursor`, `:86`)
- Modify: `ts/apps/iam-console/lib/form.ts` (add after `refreshesAfterLifecycleAction`, `:82`)
- Create: `ts/apps/iam-console/tests/unit/lib-time.test.ts`
- Modify: `ts/apps/iam-console/tests/unit/paging.test.ts`, `ts/apps/iam-console/tests/unit/form.test.ts`

**Interfaces:**
- Produces:
  - `lib/time.ts`: `type ProtoTimestamp = { readonly seconds: bigint; readonly nanos: number }`; `function timestampIso(value: ProtoTimestamp | undefined): string | null`.
  - `lib/paging.ts`: `const MAX_EVENT_TYPE_LENGTH = 200`; `type ParsedEventType = { readonly ok: true; readonly value: string } | { readonly ok: false; readonly error: PaigasusError }`; `function parseEventType(raw: string | readonly string[] | undefined): ParsedEventType`; `function listHref(path: string, query: Readonly<Record<string, string | number | null>>): string`.
  - `lib/form.ts`: `function refreshesAfterDeadLetterAction(result: ActionResult): boolean`.

The spec (§ 6.3) says "the audit page's `occurredAtIso`, moved to `lib/time.ts`". The plan names the moved function `timestampIso`, because the dead-letters page uses it for `parkedAt` too. The body is unchanged.

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/iam-console/tests/unit/lib-time.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// timestampIso (SMA-629 spec § 6.3): ONE copy of the IAM timestamp conversion for the audit and the
// dead-letters pages. The audit loader's own boundary cases stay in tests/integration/audit-page.test.ts.
import { describe, expect, it } from 'vitest';
import { timestampIso } from '../../lib/time';

describe('timestampIso', () => {
  it('reads no timestamp as null', () => {
    expect(timestampIso(undefined)).toBeNull();
  });

  it('adds the nanos as whole milliseconds', () => {
    expect(timestampIso({ seconds: 1_788_000_000n, nanos: 250_999_999 })).toBe(new Date(1_788_000_000_250).toISOString());
  });

  it('keeps the exact Date ceiling and reads one second past it as null, not a thrown RangeError', () => {
    expect(timestampIso({ seconds: 8_640_000_000_000n, nanos: 0 })).toBe(new Date(8_640_000_000_000_000).toISOString());
    expect(timestampIso({ seconds: 8_640_000_000_001n, nanos: 0 })).toBeNull();
    expect(timestampIso({ seconds: 1_000_000_000_000_000n, nanos: 0 })).toBeNull();
  });
});
```

In `tests/unit/paging.test.ts`, change the import line to:

```ts
import { MAX_CURSOR_LENGTH, MAX_EVENT_TYPE_LENGTH, PAGE_SIZE, listHref, nextOffset, pageHref, parseCursor, parseEventType, parseOffset } from '../../lib/paging';
```

and append:

```ts
// SMA-629 spec § 6.3. The dead-letters filter: IAM matches event_type exactly, and '' is no filter.
describe('parseEventType', () => {
  it('reads the first value, trimmed, and no value as no filter', () => {
    expect(parseEventType('iam.team.created')).toEqual({ ok: true, value: 'iam.team.created' });
    expect(parseEventType(['iam.team.created', 'iam.project.created'])).toEqual({ ok: true, value: 'iam.team.created' });
    expect(parseEventType('  iam.team.created  ')).toEqual({ ok: true, value: 'iam.team.created' });
    expect(parseEventType(undefined)).toEqual({ ok: true, value: '' });
    expect(parseEventType('')).toEqual({ ok: true, value: '' });
    expect(parseEventType('   ')).toEqual({ ok: true, value: '' });
  });

  it('accepts 200 characters after the trim and refuses 201 as invalid input that never reached IAM', () => {
    expect(MAX_EVENT_TYPE_LENGTH).toBe(200);
    expect(parseEventType(` ${'e'.repeat(200)} `)).toEqual({ ok: true, value: 'e'.repeat(200) });

    const parsed = parseEventType('e'.repeat(201));

    expect(parsed.ok).toBe(false);
    if (parsed.ok) throw new Error('expected an invalid event type');
    expect(parsed.error.presentation).toBe('invalid-input');
    expect(parsed.error.correlationId).toBeNull();
    expect(parsed.error.reason).toBeNull();
  });
});

describe('listHref', () => {
  it('leaves out every empty value, so the first unfiltered page has no query', () => {
    expect(listHref('/iam/dead-letters', {})).toBe('/iam/dead-letters');
    expect(listHref('/iam/dead-letters', { eventType: '', cursor: null })).toBe('/iam/dead-letters');
    expect(listHref('/iam/dead-letters', { eventType: 'iam.team.created', cursor: '' })).toBe('/iam/dead-letters?eventType=iam.team.created');
  });

  it('encodes the values and keeps their order', () => {
    expect(listHref('/iam/dead-letters', { eventType: 'a b', cursor: 'c&d=e' })).toBe('/iam/dead-letters?eventType=a+b&cursor=c%26d%3De');
  });
});
```

In `tests/unit/form.test.ts`, change the import from `../../lib/form` to:

```ts
import { currentField, refreshesAfterDeadLetterAction, refreshesAfterLifecycleAction, renameChange, renameForm, slugField } from '../../lib/form';
```

and append:

```ts
// SMA-629 spec § 6.4. A dead-letter action refreshes on success and on not-found (the row is stale),
// and NOT on forbidden: the refreshed list read would be forbidden too, and forbidden() would replace
// the page and lose the inline error.
describe('refreshesAfterDeadLetterAction', () => {
  const refused = (presentation: Presentation) => ({ ok: false as const, error: { ...invalidFormInput(), presentation } });

  it('refreshes on a success', () => {
    expect(refreshesAfterDeadLetterAction({ ok: true })).toBe(true);
  });

  it.each<[Presentation, boolean]>([
    ['not-found', true],
    ['forbidden', false],
    ['degraded', false],
    ['conflict', false],
    ['invalid-input', false],
    ['relogin', false],
    ['rate-limited', false],
    ['disabled', false],
    ['generic', false],
  ])('a refusal with presentation %s refreshes: %s', (presentation, expected) => {
    expect(refreshesAfterDeadLetterAction(refused(presentation))).toBe(expected);
  });
});
```

- [ ] **Step 2: Run the tests and see them fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/lib-time.test.ts tests/unit/paging.test.ts tests/unit/form.test.ts
```
Expected: FAIL. `lib-time.test.ts` cannot resolve `../../lib/time`; `parseEventType is not a function`; `refreshesAfterDeadLetterAction is not a function`.

- [ ] **Step 3: Implement**

Create `ts/apps/iam-console/lib/time.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// IAM timestamps as ISO strings (SMA-629 spec § 6.3). ONE copy, moved from the audit page's loader:
// the audit page and the dead-letters page both use it.
import 'server-only';

/** A protobuf Timestamp as protobuf-es gives it. */
export type ProtoTimestamp = { readonly seconds: bigint; readonly nanos: number };

/**
 * `seconds` is a protobuf int64 and can hold a value outside the ECMAScript Date range
 * (±8,640,000,000,000 ms from the epoch). `Date#toISOString()` throws on an out-of-range Date, so
 * validate first and return null rather than crash the page on a malformed IAM timestamp.
 */
export function timestampIso(value: ProtoTimestamp | undefined): string | null {
  if (value === undefined) return null;
  const date = new Date(Number(value.seconds) * 1000 + Math.floor(value.nanos / 1_000_000));
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}
```

In `app/(console)/audit/load.ts`: delete the `occurredAtIso` function and its doc comment (`:44-53`), add the import after the `PAGE_SIZE` import:

```ts
import { timestampIso } from '../../../lib/time';
```

and change the mapping line to `occurredAt: timestampIso(entry.occurredAt),`.

In `lib/paging.ts`, change the header's first sentence to: "Offset paging for the tenancy lists, cursor paging for the audit and dead-letters lists, and the dead-letters filter (spec § 5.2; SMA-629 spec § 6.3)." Then append after `parseCursor`:

```ts
/**
 * The longest event-type filter the console forwards, in characters. It is the console's bound for a
 * short identifier (lib/form.ts's `slugField`). IAM matches `event_type` exactly, so the bound only
 * limits an attacker-controlled query string.
 */
export const MAX_EVENT_TYPE_LENGTH = 200;

/** What `parseEventType` answers: the filter to send to IAM ('' for none), or the error the page renders. */
export type ParsedEventType = { readonly ok: true; readonly value: string } | { readonly ok: false; readonly error: PaigasusError };

/** A filter longer than the console forwards. Like `cursorTooLong`, it never reached IAM. */
function eventTypeTooLong(): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'The event type filter is too long.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}

/**
 * The dead-letters filter (SMA-629 spec § 6.3): the first value, trimmed. `''` is no filter. A value
 * past MAX_EVENT_TYPE_LENGTH is REPORTED as invalid, never cut or dropped, for the same reason as
 * `parseCursor`: a quiet change would show the user another list than the one they asked for.
 */
export function parseEventType(raw: string | readonly string[] | undefined): ParsedEventType {
  const value = (first(raw) ?? '').trim();
  return value.length > MAX_EVENT_TYPE_LENGTH ? { ok: false, error: eventTypeTooLong() } : { ok: true, value };
}

/**
 * `path` plus a query of the given entries, in their order (SMA-629 spec § 6.3). An entry of 0, ''
 * or null is a default and is left out, so the first unfiltered page has no query at all. A copy of
 * the gateway console's `linkHref` (ts/apps/gateway-console/lib/paging.ts:40); nothing gates a
 * divergence.
 */
export function listHref(path: string, query: Readonly<Record<string, string | number | null>>): string {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === null || value === 0 || value === '') continue;
    params.set(name, String(value));
  }
  const text = params.toString();
  return text === '' ? path : `${path}?${text}`;
}
```

In `lib/form.ts`, append:

```ts
/**
 * Whether a replay or discard action refreshes /iam/dead-letters (SMA-629 spec § 6.4). On a success,
 * and ALSO on `not-found`: the entry is no longer parked, so its row is stale and must go. NOT on
 * `forbidden`, unlike `refreshesAfterLifecycleAction`: a forbidden replay means that the caller is
 * not Root, so the list read is forbidden too, the refreshed page would call forbidden() and replace
 * the whole page, and the inline error would be lost. Other refusals change nothing on the page.
 */
export function refreshesAfterDeadLetterAction(result: ActionResult): boolean {
  return result.ok || result.error.presentation === 'not-found';
}
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/lib-time.test.ts tests/unit/paging.test.ts tests/unit/form.test.ts tests/integration/audit-page.test.ts
moon run iam-console-ts:typecheck
```
Expected: PASS. `audit-page.test.ts` proves the move changed nothing for the audit page.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/iam-console/lib/time.ts ts/apps/iam-console/lib/paging.ts ts/apps/iam-console/lib/form.ts "ts/apps/iam-console/app/(console)/audit/load.ts" ts/apps/iam-console/tests/unit/lib-time.test.ts ts/apps/iam-console/tests/unit/paging.test.ts ts/apps/iam-console/tests/unit/form.test.ts
git commit -m "feat(ts): the query, href, time and revalidate helpers for the dead-letters page (SMA-629)" -m "Move the audit loader's timestamp conversion to lib/time.ts, add parseEventType and
listHref next to parseCursor, and add refreshesAfterDeadLetterAction, which refreshes on
ok and not-found and never on forbidden." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: The Dead letters nav entry

**Files:**
- Modify: `ts/apps/iam-console/lib/nav.ts` (`:20-35`)
- Modify: `ts/apps/iam-console/app/(console)/layout.tsx` (`:3-4`, `:31-32`)
- Replace: `ts/apps/iam-console/tests/unit/nav.test.ts`

**Interfaces:**
- Consumes: `CapabilityKey` `'iam.deadletters'` (Task 1); `IamAction` `'ListOutboxDeadLetters'` (Task 3).
- Produces: `buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap; auditAllowed: boolean; deadLettersAllowed: boolean }): NavEntry[]`. Entry order: Organizations, Audit, Dead letters, Gateway.

- [ ] **Step 1: Write the failing test**

Replace `ts/apps/iam-console/tests/unit/nav.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// buildNavEntries (spec § 5.4, § 6.6, § 9.2; SMA-629 spec § 6.1): every IAM state, with and without
// `iam.audit` and `iam.deadletters`, with each mayI question allowed and denied; the gateway absent,
// degraded and available; and every entry's state went through navStateOf().
import { describe, expect, it, vi } from 'vitest';
import type { NavEntry } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

const navStateOf = vi.hoisted(() => vi.fn());
vi.mock('@paigasus/app-shell', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@paigasus/app-shell')>();
  navStateOf.mockImplementation(actual.navStateOf);
  return { ...actual, navStateOf };
});

const { buildNavEntries } = await import('../../lib/nav');

const ZONES = { iam: '/iam', gateway: '/gateway' };
const iamUp = (capabilities: string[]): ServiceState => ({ state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1', capabilities }, capabilities });
const iamDown = (capabilities: string[]): ServiceState => ({ state: 'degraded', service: 'iam', reason: 'timeout', descriptor: { service: 'iam', version: '1', capabilities }, capabilities });
const IAM_DOWN = iamDown(['iam.audit']);
const IAM_ABSENT: ServiceState = { state: 'absent', service: 'iam' };
const GATEWAY_ABSENT: ServiceState = { state: 'absent', service: 'gateway' };
const GATEWAY_DOWN: ServiceState = { state: 'degraded', service: 'gateway', reason: 'network', descriptor: null, capabilities: [] };
const GATEWAY_UP: ServiceState = { state: 'available', service: 'gateway', descriptor: { service: 'gateway', version: '1', capabilities: [] }, capabilities: [] };
const DEGRADED: NavEntry['state'] = { state: 'degraded', service: 'iam', reason: 'timeout' };

const byLabel = (entries: ReturnType<typeof buildNavEntries>) => Object.fromEntries(entries.map((entry) => [entry.label, entry]));

describe('buildNavEntries', () => {
  it('lists Organizations, Audit, Dead letters and Gateway, in that order, with full-path hrefs when everything is available', () => {
    const entries = buildNavEntries({ iam: iamUp(['iam.audit', 'iam.deadletters']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true });
    expect(entries.map((e) => [e.label, e.zone, e.href, e.state.state])).toEqual([
      ['Organizations', 'iam', '/iam/orgs', 'available'],
      ['Audit', 'iam', '/iam/audit', 'available'],
      ['Dead letters', 'iam', '/iam/dead-letters', 'available'],
      ['Gateway', 'gateway', '/gateway/', 'available'],
    ]);
  });

  it('makes Audit absent when IAM does not report iam.audit', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: false }));
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
    expect(entries['Organizations']?.state).toEqual({ state: 'available' });
  });

  it('omits Audit when mayI says no', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: false, deadLettersAllowed: false }));
    expect(entries['Audit']).toBeUndefined();
  });

  it('disables the IAM entries with a reason when IAM is degraded', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_DOWN, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true }));
    expect(entries['Organizations']?.state).toEqual(DEGRADED);
    expect(entries['Audit']?.state).toEqual(DEGRADED);
    expect(entries['Dead letters']?.state).toEqual(DEGRADED);
  });

  it('makes the IAM entries absent when IAM is absent', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_ABSENT, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true }));
    expect(entries['Organizations']?.state).toEqual({ state: 'absent' });
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
    expect(entries['Dead letters']?.state).toEqual({ state: 'absent' });
  });

  it('gives the Gateway entry the gateway’s own state: absent, or degraded with a reason', () => {
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_ABSENT, zones: ZONES, auditAllowed: false, deadLettersAllowed: false }))['Gateway']?.state).toEqual({ state: 'absent' });
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_DOWN, zones: ZONES, auditAllowed: false, deadLettersAllowed: false }))['Gateway']?.state).toEqual({
      state: 'degraded',
      service: 'gateway',
      reason: 'network',
    });
  });

  it('leaves out the Gateway entry when gateway is not a zone', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: { iam: '/iam' }, auditAllowed: false, deadLettersAllowed: false }));
    expect(entries['Gateway']).toBeUndefined();
  });

  it('takes EVERY entry’s state from navStateOf()', () => {
    navStateOf.mockClear();
    const entries = buildNavEntries({ iam: iamUp(['iam.audit', 'iam.deadletters']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true });
    expect(navStateOf).toHaveBeenCalledTimes(entries.length);
    expect(navStateOf.mock.results.map((r) => r.value as unknown)).toEqual(entries.map((e) => e.state));
  });
});

// SMA-629 spec § 6.1, § 7.2, AC 1 and AC 2. The matrix: the mayI answer × the key present or absent ×
// IAM up, degraded or absent. `undefined` means no entry at all.
describe('the Dead letters entry', () => {
  it.each<[string, ServiceState, boolean, NavEntry['state'] | undefined]>([
    ['allowed, IAM up with the key', iamUp(['iam.deadletters']), true, { state: 'available' }],
    ['allowed, IAM up without the key (an older IAM, AC 2)', iamUp(['iam.audit']), true, { state: 'absent' }],
    ['allowed, IAM degraded with the key', iamDown(['iam.deadletters']), true, DEGRADED],
    ['allowed, IAM degraded without the key (the § 8 exception)', iamDown([]), true, DEGRADED],
    ['allowed, IAM absent', IAM_ABSENT, true, { state: 'absent' }],
    ['denied, IAM up with the key', iamUp(['iam.deadletters']), false, undefined],
    ['denied, IAM up without the key', iamUp([]), false, undefined],
    ['denied, IAM degraded', iamDown(['iam.deadletters']), false, undefined],
    ['denied, IAM absent', IAM_ABSENT, false, undefined],
  ])('%s', (_label, iam, allowed, expected) => {
    const entry = byLabel(buildNavEntries({ iam, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: allowed }))['Dead letters'];
    expect(entry?.state).toEqual(expected);
    if (expected !== undefined) expect(entry?.href).toBe('/iam/dead-letters');
  });
});
```

- [ ] **Step 2: Run the test and see it fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/nav.test.ts
```
Expected: FAIL. The order test lacks the `Dead letters` row, and every `allowed` matrix row gets `undefined`.

- [ ] **Step 3: Implement**

In `lib/nav.ts`, replace `buildNavEntries`:

```ts
export function buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap; auditAllowed: boolean; deadLettersAllowed: boolean }): NavEntry[] {
  const entries: NavEntry[] = [{ zone: 'iam', href: `${IAM_BASE_PATH}/orgs`, label: 'Organizations', state: navStateOf(input.iam) }];
  // ListAuditEntries is Root-only (rs/crates/services/paigasus-iam/src/application/audit.rs:36-38),
  // so the layout asks mayI('ListAuditLog', ROOT_PRN) and omits the entry on a clear "no". The
  // capability decides the rest: absent without `iam.audit`, disabled when IAM is degraded.
  if (input.auditAllowed) {
    entries.push({ zone: 'iam', href: `${IAM_BASE_PATH}/audit`, label: 'Audit', state: navStateOf(input.iam, 'iam.audit') });
  }
  // SMA-629 § 6.1. Every OutboxService RPC is Root-only, so the layout asks
  // mayI('ListOutboxDeadLetters', ROOT_PRN). mayI() fails open, so during an IAM authz outage a
  // non-Root user can see this entry and IAM refuses the page with a 403, as for Audit. The
  // capability decides the rest: absent without `iam.deadletters` (an older IAM, AC 2), disabled
  // with a reason whenever IAM is degraded, whatever the key (§ 8).
  if (input.deadLettersAllowed) {
    entries.push({ zone: 'iam', href: `${IAM_BASE_PATH}/dead-letters`, label: 'Dead letters', state: navStateOf(input.iam, 'iam.deadletters') });
  }
  // A cross-zone entry. PrimaryNav drops it when `gateway` is not a zone (its rule 1), and
  // navStateOf answers `absent` when `gateway` is not a configured service.
  const gatewayBase = input.zones['gateway'];
  if (gatewayBase !== undefined) {
    entries.push({ zone: 'gateway', href: `${gatewayBase}/`, label: 'Gateway', state: navStateOf(input.gateway) });
  }
  return entries;
}
```

In `app/(console)/layout.tsx`, replace the header lines `:3-4`:

```tsx
// Every page that needs a session (spec § 5.4). It resolves the session FIRST — requireSession()
// redirects to login when there is none — then builds the shell from five request-scoped reads, and
// asks mayI() two questions in parallel: ListAuditLog and ListOutboxDeadLetters, both at Root. Each
// question is one IsAuthorized call, so SMA-629 added one IAM call to every console render.
```

and replace the `const nav = …` line (`:32`):

```tsx
  const [auditAllowed, deadLettersAllowed] = await Promise.all([may('ListAuditLog', ROOT_PRN), may('ListOutboxDeadLetters', ROOT_PRN)]);
  const nav = buildNavEntries({ iam, gateway, zones, auditAllowed, deadLettersAllowed });
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/nav.test.ts
moon run iam-console-ts:typecheck
```
Expected: PASS. The typecheck proves the layout passes the new required field.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/iam-console/lib/nav.ts "ts/apps/iam-console/app/(console)/layout.tsx" ts/apps/iam-console/tests/unit/nav.test.ts
git commit -m "feat(ts): a Dead letters nav entry gated by mayI and iam.deadletters (SMA-629)" -m "The layout asks ListOutboxDeadLetters at Root in parallel with ListAuditLog, and
buildNavEntries puts the entry after Audit with its state from navStateOf." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The gate and the loader

**Files:**
- Create: `ts/apps/iam-console/app/(console)/dead-letters/load.ts`
- Create: `ts/apps/iam-console/tests/unit/dead-letters-gate.test.ts`
- Create: `ts/apps/iam-console/tests/integration/dead-letters-page.test.ts`
- Modify: `ts/apps/iam-console/tests/integration/support.ts` (`:5`, `:23-31`)

**Interfaces:**
- Consumes: `capabilityOutcome` (`@paigasus/discovery/client`); `callIam`, `IamClients`, `IamResult` (`@paigasus/console-core`); `PAGE_SIZE` (`lib/paging.ts`); `timestampIso` (Task 5).
- Produces (all exported from `load.ts`):
  - `type DeadLettersGate = 'not-found' | 'degraded' | 'available'`
  - `function deadLettersGate(state: ServiceState): DeadLettersGate`
  - `type DeadLetterRow = { readonly id: string; readonly eventType: string; readonly aggregatePrn: string; readonly payload: string; readonly schemaVersion: number; readonly attempts: number; readonly parkedAt: string | null; readonly occurredAt: string | null; readonly actorPrn: string | null; readonly correlationId: string | null; readonly lastError: string | null }`
  - `type DeadLettersPageData = IamResult<{ readonly rows: readonly DeadLetterRow[]; readonly cursor: string; readonly nextCursor: string | null }>`
  - `function loadDeadLettersPage(deps: { readonly outbox: Pick<IamClients['outbox'], 'listDeadLetters'> }, params: { readonly cursor: string; readonly eventType: string }): Promise<DeadLettersPageData>`
  - `support.ts`'s `clientsFor(iam)` also returns `outbox`.

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/iam-console/tests/unit/dead-letters-gate.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// deadLettersGate (SMA-629 spec § 6.2, § 7.2, § 8): every branch, including a degraded IAM whose
// cached descriptor has no key, which is `degraded` and not a 404.
import type { ServiceState } from '@paigasus/discovery/types';
import { describe, expect, it } from 'vitest';
import { deadLettersGate } from '../../app/(console)/dead-letters/load';

const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1.0.0', capabilities });

describe('deadLettersGate', () => {
  it.each<[string, ServiceState, ReturnType<typeof deadLettersGate>]>([
    ['absent', { state: 'absent', service: 'iam' }, 'not-found'],
    ['available with iam.deadletters', { state: 'available', service: 'iam', descriptor: descriptor(['iam.deadletters']), capabilities: ['iam.deadletters'] }, 'available'],
    ['available without iam.deadletters (an older IAM, AC 2)', { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'not-found'],
    ['degraded with the key', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(['iam.deadletters']), capabilities: ['iam.deadletters'] }, 'degraded'],
    ['degraded with a cached descriptor that has no key (§ 8)', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'degraded'],
    ['degraded with no descriptor', { state: 'degraded', service: 'iam', reason: 'network', descriptor: null, capabilities: [] }, 'degraded'],
  ])('%s', (_label, state, gate) => {
    expect(deadLettersGate(state)).toBe(gate);
  });
});
```

In `tests/integration/support.ts`, change the import to:

```ts
import { AuditService, AuthorizationService, OutboxService, TenancyService, createIamClient } from '@paigasus/sdk/iam';
```

and add to the object `clientsFor` returns:

```ts
    outbox: createIamClient(OutboxService, options, auth),
```

Create `ts/apps/iam-console/tests/integration/dead-letters-page.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// ListDeadLetters (SMA-629 spec § 6.3, § 7.3) through the fake IAM: every field of the row, IAM's
// "empty means none" strings as null, the filter and the cursor on the wire, the last page, and an
// IAM error as the PaigasusError.
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { loadDeadLettersPage } from '../../app/(console)/dead-letters/load';
import { PAGE_SIZE } from '../../lib/paging';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ID = '0190a1f0-0000-7000-8000-0000000000d1';
const TEAM_PRN = `prn:pgs:iam::${IDS.orgA}:team/${IDS.teamA1}`;

const entry = {
  id: ID,
  occurredAt: { seconds: 1_788_000_000n, nanos: 250_000_000 },
  eventType: 'iam.team.created',
  schemaVersion: 2,
  aggregatePrn: TEAM_PRN,
  actorPrn: IDS.principalPrn,
  payload: '{"slug":"platform"}',
  correlationId: 'corr-dead-letter-1',
  attempts: 5,
  parkedAt: { seconds: 1_788_000_300n, nanos: 0 },
  lastError: 'nats: no responders available for request',
};

describe('loadDeadLettersPage', () => {
  it('maps every field, sends the filter and the cursor, and returns the next cursor', async () => {
    iam.setHandlers({ 'outbox.listDeadLetters': () => ({ entries: [entry], nextCursor: 'cursor-2' }) });
    const calls = callsSince(iam);

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: 'cursor-1', eventType: 'iam.team.created' });

    expect(data).toEqual({
      ok: true,
      value: {
        cursor: 'cursor-1',
        nextCursor: 'cursor-2',
        rows: [
          {
            id: ID,
            eventType: 'iam.team.created',
            aggregatePrn: TEAM_PRN,
            payload: '{"slug":"platform"}',
            schemaVersion: 2,
            attempts: 5,
            parkedAt: new Date(1_788_000_300_000).toISOString(),
            occurredAt: new Date(1_788_000_000_250).toISOString(),
            actorPrn: IDS.principalPrn,
            correlationId: 'corr-dead-letter-1',
            lastError: 'nats: no responders available for request',
          },
        ],
      },
    });
    expect(calls('outbox.listDeadLetters')[0]?.request).toMatchObject({ eventType: 'iam.team.created', cursor: 'cursor-1', limit: PAGE_SIZE });
  });

  it("maps IAM's empty strings and missing timestamps to null, and an empty next_cursor to the last page", async () => {
    iam.setHandlers({
      'outbox.listDeadLetters': () => ({ entries: [{ ...entry, actorPrn: '', correlationId: '', lastError: '', parkedAt: undefined, occurredAt: undefined }], nextCursor: '' }),
    });

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.nextCursor).toBeNull();
    expect(data.value.rows[0]).toMatchObject({ actorPrn: null, correlationId: null, lastError: null, parkedAt: null, occurredAt: null });
  });

  it('reads a parked time outside the Date range as null, not a thrown error', async () => {
    iam.setHandlers({ 'outbox.listDeadLetters': () => ({ entries: [{ ...entry, parkedAt: { seconds: 8_640_000_000_001n, nanos: 0 } }], nextCursor: '' }) });

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.rows[0]?.parkedAt).toBeNull();
  });

  it('returns an IAM denial as the PaigasusError with the correlation id', async () => {
    iam.setHandlers({
      'outbox.listDeadLetters': () => {
        throw denial({ correlationId: 'corr-dead-letters-403' });
      },
    });

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '' });

    if (data.ok) throw new Error('expected a denial');
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-dead-letters-403');
  });
});
```

- [ ] **Step 2: Run the tests and see them fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/dead-letters-gate.test.ts tests/integration/dead-letters-page.test.ts
```
Expected: FAIL, `Failed to resolve import "../../app/(console)/dead-letters/load"`.

- [ ] **Step 3: Implement**

Create `ts/apps/iam-console/app/(console)/dead-letters/load.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2, § 6.3). The screen exists only when IAM reports
// `iam.deadletters`. The gate is a pure function of the ServiceState, so a unit test covers every
// branch. The loader turns IAM's DeadLetterEntry into plain data for the page.
import 'server-only';
import { capabilityOutcome } from '@paigasus/discovery/client';
import type { ServiceState } from '@paigasus/discovery/types';
import { callIam, type IamClients, type IamResult } from '@paigasus/console-core';
import { PAGE_SIZE } from '../../../lib/paging';
import { timestampIso } from '../../../lib/time';

export type DeadLettersGate = 'not-found' | 'degraded' | 'available';

/**
 * The audit mapping, applied to `iam.deadletters`. Absent, or available without the key (an older
 * IAM, AC 2), is `hidden`, which is a 404. Degraded is NOT a 404, whatever the cached descriptor
 * says (§ 8): the feature may exist.
 */
const GATE = { hidden: 'not-found', shown: 'available', degraded: 'degraded' } as const;

export function deadLettersGate(state: ServiceState): DeadLettersGate {
  return GATE[capabilityOutcome(state, 'iam.deadletters')];
}

/** One dead letter as the page shows it. `null` is IAM's "none", shown as "—". */
export type DeadLetterRow = {
  readonly id: string;
  readonly eventType: string;
  readonly aggregatePrn: string;
  /** A JSON string. The page never parses it and never renders it as HTML (§ 6.3). */
  readonly payload: string;
  readonly schemaVersion: number;
  readonly attempts: number;
  /** ISO 8601, or null for a missing or out-of-range timestamp. */
  readonly parkedAt: string | null;
  readonly occurredAt: string | null;
  readonly actorPrn: string | null;
  readonly correlationId: string | null;
  readonly lastError: string | null;
};

export type DeadLettersPageData = IamResult<{ readonly rows: readonly DeadLetterRow[]; readonly cursor: string; readonly nextCursor: string | null }>;

/** The proto says "empty means none" for actor_prn, correlation_id and last_error (iam.proto:619,621,624). */
function noneIfEmpty(value: string): string | null {
  return value === '' ? null : value;
}

/**
 * IAM orders the list by id DESCENDING (tests/dead_letters_pg.rs:432), and IAM mints UUIDv7 ids, so
 * this is close to creation order, not park order. The page keeps that order and does not sort.
 */
export async function loadDeadLettersPage(deps: { readonly outbox: Pick<IamClients['outbox'], 'listDeadLetters'> }, params: { readonly cursor: string; readonly eventType: string }): Promise<DeadLettersPageData> {
  const result = await callIam(() => deps.outbox.listDeadLetters({ eventType: params.eventType, cursor: params.cursor, limit: PAGE_SIZE }));
  if (!result.ok) return result;
  const rows = result.value.entries.map(
    (entry): DeadLetterRow => ({
      id: entry.id,
      eventType: entry.eventType,
      aggregatePrn: entry.aggregatePrn,
      payload: entry.payload,
      schemaVersion: entry.schemaVersion,
      attempts: entry.attempts,
      parkedAt: timestampIso(entry.parkedAt),
      occurredAt: timestampIso(entry.occurredAt),
      actorPrn: noneIfEmpty(entry.actorPrn),
      correlationId: noneIfEmpty(entry.correlationId),
      lastError: noneIfEmpty(entry.lastError),
    }),
  );
  return { ok: true, value: { rows, cursor: params.cursor, nextCursor: result.value.nextCursor === '' ? null : result.value.nextCursor } };
}
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/dead-letters-gate.test.ts tests/integration/dead-letters-page.test.ts tests/integration/audit-page.test.ts
moon run iam-console-ts:typecheck
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add "ts/apps/iam-console/app/(console)/dead-letters/load.ts" ts/apps/iam-console/tests/unit/dead-letters-gate.test.ts ts/apps/iam-console/tests/integration/dead-letters-page.test.ts ts/apps/iam-console/tests/integration/support.ts
git commit -m "feat(ts): the dead-letters gate and loader (SMA-629)" -m "deadLettersGate applies the audit mapping to iam.deadletters. loadDeadLettersPage sends the
filter and the cursor, maps IAM's empty strings to null and keeps IAM's id order." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Error copy, commands and Server Actions

**Files:**
- Modify: `ts/apps/iam-console/app/_components/error-copy.ts` (append)
- Modify: `ts/apps/iam-console/app/_components/form-error.tsx` (`:1-4`, `:15-26`)
- Create: `ts/apps/iam-console/app/(console)/dead-letters/commands.ts`
- Create: `ts/apps/iam-console/app/(console)/dead-letters/actions.ts`
- Create: `ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts`
- Modify: `ts/apps/iam-console/tests/unit/actions-structure.test.ts` (`:25-30`)
- Modify: `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts` (`:17-51`, append)
- Modify: `ts/apps/iam-console/tests/unit/error-copy.test.ts` (append)
- Modify: `ts/apps/iam-console/tests/unit/error-views.test.tsx` (`FormError` block, `:93-109`)

**Interfaces:**
- Produces:
  - `error-copy.ts`: `const DEAD_LETTER_GONE: string`.
  - `form-error.tsx`: `FormError({ error, message }: { readonly error: PaigasusError | null; readonly message?: string | undefined })`.
  - `commands.ts`: `const deadLetterForm` (zod object `{ id }`); `type DeadLetterInput = { id: string }`; `replayDeadLetter(deps: { readonly outbox: Pick<IamClients['outbox'], 'replayDeadLetter'> }, input: DeadLetterInput): Promise<ActionResult>`; `discardDeadLetter(deps: { readonly outbox: Pick<IamClients['outbox'], 'discardDeadLetter'> }, input: DeadLetterInput): Promise<ActionResult>`.
  - `actions.ts`: `replayDeadLetterAction(_previous: ActionState, form: FormData): Promise<ActionState>`; `discardDeadLetterAction(_previous: ActionState, form: FormData): Promise<ActionState>`.

- [ ] **Step 1: Write the failing tests**

Append to the `describe` in `tests/unit/error-copy.test.ts` (and add `DEAD_LETTER_GONE` to its import from `../../app/_components/error-copy`):

```ts
  // SMA-629 spec § 6.5. The words of RUNBOOK-observability.md's 404 note: the entry is gone, maybe by
  // an earlier attempt of the same request. The global not-found copy does not change.
  it('has a dead-letter not-found sentence and leaves the global not-found copy alone', () => {
    expect(DEAD_LETTER_GONE).toBe('This entry is no longer in the dead-letter queue. It was already replayed or discarded, maybe by an earlier attempt of this request.');
    expect(FORM_REASON_COPY[ErrorReason.NOT_FOUND]).toBe('The item was not found. It may have been removed.');
  });
```

In `tests/unit/error-views.test.tsx`, add `import { DEAD_LETTER_GONE, PRESENTATION_COPY } from '../../app/_components/error-copy';` and add inside `describe('FormError', …)`:

```tsx
  // SMA-629 spec § 6.5: `message` replaces the copy only when it is set, and the id stays.
  it('shows the message prop instead of the copy, and keeps the correlation id', () => {
    const html = render(<FormError error={errorWith('not-found')} message={DEAD_LETTER_GONE} />);
    expect(html).toContain(DEAD_LETTER_GONE);
    expect(html).not.toContain(PRESENTATION_COPY['not-found'].body);
    expect(html).toContain(CID);
  });

  it('shows the copy when message is undefined', () => {
    const html = render(<FormError error={errorWith('not-found')} message={undefined} />);
    expect(html).toContain(PRESENTATION_COPY['not-found'].body);
  });
```

Create `ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The two dead-letter commands (SMA-629 spec § 6.4, § 7.3) against the fake IAM: each sends the id,
// and IAM's NotFound maps to `not-found`. A command takes no mayI and does no validation of its own:
// the action parses the form (tests/unit/actions-revalidate.test.ts holds the zero-call rule).
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { deadLetterForm, discardDeadLetter, replayDeadLetter } from '../../app/(console)/dead-letters/commands';
import { callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ID = '0190a1f0-0000-7000-8000-0000000000d1';

const COMMANDS = [
  ['replayDeadLetter', 'outbox.replayDeadLetter', replayDeadLetter],
  ['discardDeadLetter', 'outbox.discardDeadLetter', discardDeadLetter],
] as const;

describe.each(COMMANDS)('%s', (_name, method, command) => {
  it('sends the id and answers ok', async () => {
    iam.setHandlers({ 'outbox.replayDeadLetter': () => ({ entry: { id: ID } }), 'outbox.discardDeadLetter': () => ({ entry: { id: ID } }) });
    const calls = callsSince(iam);

    const result = await command({ outbox: clientsFor(iam).outbox }, { id: ID });

    expect(result).toEqual({ ok: true });
    expect(calls(method)).toHaveLength(1);
    expect(calls(method)[0]?.request).toMatchObject({ id: ID });
  });

  it('maps NotFound to not-found with the correlation id', async () => {
    const gone = (): never => {
      throw denial({ code: Code.NotFound, reason: 'not-found', correlationId: 'corr-dead-letter-404' });
    };
    iam.setHandlers({ 'outbox.replayDeadLetter': gone, 'outbox.discardDeadLetter': gone });

    const result = await command({ outbox: clientsFor(iam).outbox }, { id: ID });

    expect(result).toMatchObject({ ok: false, error: { presentation: 'not-found', correlationId: 'corr-dead-letter-404' } });
  });
});

describe('deadLetterForm', () => {
  it('trims and accepts an RFC 4122 id, and refuses anything else', () => {
    expect(deadLetterForm.safeParse({ id: ` ${ID} ` }).data).toEqual({ id: ID });
    for (const bad of ['', 'not-a-uuid', '0190a1f000007000800000000000d1', null]) {
      expect(deadLetterForm.safeParse({ id: bad }).success).toBe(false);
    }
  });
});
```

In `tests/unit/actions-structure.test.ts`, add to `EXPECTED`:

```ts
  '(console)/dead-letters/actions.ts': ['discardDeadLetterAction', 'replayDeadLetterAction'],
```

In `tests/unit/actions-revalidate.test.ts`:
- replace the `vi.hoisted` block (`:17-49`) so it also returns an `outbox` of two recording mocks:

```ts
const { tenancy, outbox, failNext } = vi.hoisted(() => {
  let failure: Code | null = null;
  const call = (): Promise<Record<string, never>> => {
    if (failure === null) return Promise.resolve({});
    const code = failure;
    failure = null;
    return Promise.reject(new ConnectError('nope', code));
  };
  return {
    failNext: (code: Code): void => {
      failure = code;
    },
    tenancy: {
      createOrganization: call,
      attachMembership: call,
      detachMembership: call,
      createTeam: call,
      createProject: call,
      renameOrganization: call,
      archiveOrganization: call,
      restoreOrganization: call,
      renameTeam: call,
      archiveTeam: call,
      restoreTeam: call,
      renameProject: call,
      archiveProject: call,
      restoreProject: call,
    },
    // SMA-629: recording mocks, so a test can count the calls an invalid id must NOT make.
    outbox: {
      replayDeadLetter: vi.fn<(request: { id: string }) => Promise<Record<string, never>>>(call),
      discardDeadLetter: vi.fn<(request: { id: string }) => Promise<Record<string, never>>>(call),
    },
  };
});
```

- replace the `vi.mock('../../lib/console', …)` line:

```ts
vi.mock('../../lib/console', () => ({ iamClientsForAction: () => Promise.resolve({ ok: true, value: { tenancy, outbox } }) }));
```

- add after the other `await import(...)` lines:

```ts
const { discardDeadLetterAction, replayDeadLetterAction } = await import('../../app/(console)/dead-letters/actions');
```

- append:

```ts
// SMA-629 spec § 6.4. The two dead-letter actions refresh the ONE page, on ok and on not-found (the
// row is stale), and never on forbidden: the refreshed list read would call forbidden() and replace
// the inline error with the 403 view.
const DEAD_LETTER_ID = '0190a1f0-0000-7000-8000-0000000000d1';
const PAGE_REFRESH = [{ path: '/dead-letters', type: 'page' }];
const DEAD_LETTER_ACTIONS = [
  ['replayDeadLetterAction', (id: string) => replayDeadLetterAction(null, form({ id })), outbox.replayDeadLetter],
  ['discardDeadLetterAction', (id: string) => discardDeadLetterAction(null, form({ id })), outbox.discardDeadLetter],
] as const;

describe('the two dead-letter actions', () => {
  it.each(DEAD_LETTER_ACTIONS)('%s sends the trimmed id and refreshes the page on a success', async (_name, run, rpc) => {
    const result = await run(` ${DEAD_LETTER_ID} `);

    expect(result).toEqual({ ok: true });
    expect(rpc.mock.calls.at(-1)?.[0]).toEqual({ id: DEAD_LETTER_ID });
    expect(revalidatedPaths).toEqual(PAGE_REFRESH);
  });

  it.each(DEAD_LETTER_ACTIONS)('%s refreshes the page when IAM answers not-found', async (_name, run) => {
    failNext(Code.NotFound);

    const result = await run(DEAD_LETTER_ID);

    expect(result).toMatchObject({ ok: false, error: { presentation: 'not-found' } });
    expect(revalidatedPaths).toEqual(PAGE_REFRESH);
  });

  it.each(DEAD_LETTER_ACTIONS)('%s refreshes nothing when IAM answers forbidden', async (_name, run) => {
    failNext(Code.PermissionDenied);

    const result = await run(DEAD_LETTER_ID);

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it.each(DEAD_LETTER_ACTIONS)('%s refreshes nothing when IAM is degraded', async (_name, run) => {
    failNext(Code.Unavailable);

    const result = await run(DEAD_LETTER_ID);

    expect(result).toMatchObject({ ok: false, error: { presentation: 'degraded' } });
    expect(revalidatedPaths).toEqual([]);
  });

  // § 7.3: this check belongs at the action level, because the command does not validate.
  it.each(DEAD_LETTER_ACTIONS)('%s refuses an id that is not a UUID and makes ZERO IAM calls', async (_name, run, rpc) => {
    const before = rpc.mock.calls.length;

    const result = await run('not-a-uuid');

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(rpc.mock.calls.length).toBe(before);
    expect(revalidatedPaths).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests and see them fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-copy.test.ts tests/unit/error-views.test.tsx tests/unit/actions-structure.test.ts tests/unit/actions-revalidate.test.ts tests/integration/dead-letter-commands.test.ts
```
Expected: FAIL. `DEAD_LETTER_GONE` is undefined; the `message` test finds the presentation copy; `actions-structure` lists a missing file; `actions-revalidate` and `dead-letter-commands` cannot resolve `../../app/(console)/dead-letters/actions` and `…/commands`.

- [ ] **Step 3: Implement**

Append to `app/_components/error-copy.ts`:

```ts
/**
 * A replay or discard answered `not-found` (SMA-629 spec § 6.5). The frame of /iam/dead-letters passes
 * it as FormError's `message` for that one answer; FORM_REASON_COPY[ErrorReason.NOT_FOUND] does not
 * change. The words follow RUNBOOK-observability.md's 404 note: a retry of a replay whose answer was
 * lost also lands here, so the copy does not blame another operator.
 */
export const DEAD_LETTER_GONE = 'This entry is no longer in the dead-letter queue. It was already replayed or discarded, maybe by an earlier attempt of this request.';
```

Replace `app/_components/form-error.tsx`'s header comment and component:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// A Server Action failed (spec § 6.1, third column). CLIENT component: a form renders it from its
// useActionState result, and a frame from its runner result. It shows the reason's copy when the
// form knows the reason, else the presentation's copy, and always the correlation id — never IAM's
// message. `message` replaces the copy when it is set (SMA-629 spec § 6.5), as in the gateway
// console's copy of this file.
'use client';

import type { ReactElement } from 'react';
import { usePathname } from 'next/navigation';
import { useZone } from '@paigasus/app-shell';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { formMessage } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export function FormError({ error, message }: { readonly error: PaigasusError | null; readonly message?: string | undefined }): ReactElement | null {
  const pathname = usePathname();
  const { basePath } = useZone();
  if (error === null) return null;
  return (
    <div role="alert" data-testid="form-error" data-presentation={error.presentation} className="text-destructive mt-2 text-sm">
      <p>{message ?? formMessage(error)}</p>
      {/* usePathname() has no basePath (SMA-510 spec F19), and returnTo must keep it. */}
      {error.presentation === 'relogin' ? <SignInAgain returnTo={`${basePath}${pathname}`} /> : <CorrelationReference id={error.correlationId} />}
    </div>
  );
}
```

Create `ts/apps/iam-console/app/(console)/dead-letters/commands.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands behind the dead-letters Server Actions (SMA-629 spec § 6.4). A command takes its IAM
// client as a port, so the tier-2 tests call it against the fake IAM with no session and no Next
// runtime. It takes NO mayI (SMA-511 § 6.3): the page is Root-only, and IAM decides.
import 'server-only';
import { z } from 'zod';
import { callIam, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';

/**
 * The id of a dead letter, after a trim. zod 4's `z.uuid()` is RFC-strict and refuses some ids that
 * IAM's `Uuid::parse_str` accepts. That is acceptable: IAM mints UUIDv7 ids, which are RFC 4122 ids.
 */
export const deadLetterForm = z.object({ id: z.string().trim().pipe(z.uuid()) });
export type DeadLetterInput = z.infer<typeof deadLetterForm>;

/** A replay of an id that is no longer parked answers `not-found`. */
export async function replayDeadLetter(deps: { readonly outbox: Pick<IamClients['outbox'], 'replayDeadLetter'> }, input: DeadLetterInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.outbox.replayDeadLetter({ id: input.id })));
}

/** A discard deletes the entry; IAM's audit log keeps a copy of the event. */
export async function discardDeadLetter(deps: { readonly outbox: Pick<IamClients['outbox'], 'discardDeadLetter'> }, input: DeadLetterInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.outbox.discardDeadLetter({ id: input.id })));
}
```

Create `ts/apps/iam-console/app/(console)/dead-letters/actions.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// The two dead-letter Server Actions (SMA-629 spec § 6.4). See ../orgs/actions.ts for the rules every
// action follows; tests/unit/actions-structure.test.ts holds them. Each gets its client through
// iamClientsForAction(), parses the form with zod, and never navigates. The frame of the page calls
// them directly, with `null` as the previous state. They refresh the page on ok and on not-found only
// (lib/form.ts's refreshesAfterDeadLetterAction says why forbidden does not refresh).
import { revalidatePath } from 'next/cache';
import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
import { iamClientsForAction } from '../../../lib/console';
import { refreshesAfterDeadLetterAction } from '../../../lib/form';
import { deadLetterForm, discardDeadLetter, replayDeadLetter } from './commands';

/** basePath-RELATIVE, like TENANCY_PATH: Next adds /iam. Not exported: a 'use server' file exports only async functions. */
const DEAD_LETTERS_PATH = '/dead-letters';

export async function replayDeadLetterAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = deadLetterForm.safeParse(formFields(form, ['id']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await replayDeadLetter({ outbox: clients.value.outbox }, parsed.data);
  if (refreshesAfterDeadLetterAction(result)) revalidatePath(DEAD_LETTERS_PATH, 'page');
  return result;
}

export async function discardDeadLetterAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = deadLetterForm.safeParse(formFields(form, ['id']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await discardDeadLetter({ outbox: clients.value.outbox }, parsed.data);
  if (refreshesAfterDeadLetterAction(result)) revalidatePath(DEAD_LETTERS_PATH, 'page');
  return result;
}
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-copy.test.ts tests/unit/error-views.test.tsx tests/unit/actions-structure.test.ts tests/unit/actions-revalidate.test.ts tests/integration/dead-letter-commands.test.ts
moon run iam-console-ts:typecheck
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/iam-console/app/_components/error-copy.ts ts/apps/iam-console/app/_components/form-error.tsx "ts/apps/iam-console/app/(console)/dead-letters/commands.ts" "ts/apps/iam-console/app/(console)/dead-letters/actions.ts" ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts ts/apps/iam-console/tests/unit/actions-structure.test.ts ts/apps/iam-console/tests/unit/actions-revalidate.test.ts ts/apps/iam-console/tests/unit/error-copy.test.ts ts/apps/iam-console/tests/unit/error-views.test.tsx
git commit -m "feat(ts): replay and discard Server Actions for dead letters (SMA-629)" -m "Two commands over the outbox client, two actions that parse an RFC 4122 id and refresh the
page on ok and not-found only, the DEAD_LETTER_GONE copy, and a message prop on FormError." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: The frame, the row controls and the table

**Files:**
- Create: `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx`
- Create: `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx`
- Create: `ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx`

**Interfaces:**
- Consumes: `FormAction`, `ActionResult` (type-only, `@paigasus/console-core`); `FormError` with `message` and `DEAD_LETTER_GONE` (Task 8); `DeadLetterRow` (type-only, Task 7).
- Produces:
  - `dead-letters-frame.tsx` (`'use client'`): `type DeadLetterControl = 'replay' | 'discard'`; `type DeadLetterActions = { readonly replay: FormAction; readonly discard: FormAction }`; `const UNREACHED_TEXT: string`; `function successText(control: DeadLetterControl, id: string): string`; `function discardConfirmation(id: string): string`; `function DeadLettersFrame(props: { readonly actions: DeadLetterActions; readonly children: ReactNode }): ReactElement`; `function DeadLetterRowControls(props: { readonly id: string }): ReactElement`.
  - `dead-letter-table.tsx` (no directive): `function DeadLetterTable(props: { readonly rows: readonly DeadLetterRow[] }): ReactElement`.
  - DOM contract used by Task 10 and Task 11: the result region `role="status"` `data-testid="dead-letters-result"`; each main row `data-testid="dead-letter-row"`; each row's controls `data-testid="dead-letter-controls-<id>"`; forms named `Replay event <id>` and `Confirm the discard of event <id>`; buttons `Replay`, `Discard`, `Confirm discard`, `Cancel`; `data-testid="dead-letter-payload"` and `data-testid="dead-letter-last-error"`.

Replay and discard need JavaScript: a row form hands its `FormData` to the runner in `onSubmit` with `preventDefault()`, as in the gateway frame. Before hydration a Replay submit is a plain GET of the same page with an `id` query, which only re-renders the list (no mutation). The first Discard state has no form at all.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The dead-letters frame (SMA-629 spec § 6.4, § 7.2). Each case renders the REAL frame and table with
// real transitions and real form submissions, then renders it AGAIN with the children that the
// revalidated page sends — the technique of manage-controls.test.tsx and the gateway console's
// service-account-section.test.tsx.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import type { PaigasusError, Presentation } from '@paigasus/sdk/errors/types';
import { DEAD_LETTER_GONE, PRESENTATION_COPY } from '../../app/_components/error-copy';
import { SectionError } from '../../app/_components/section-error';
import { DeadLetterTable } from '../../app/(console)/dead-letters/dead-letter-table';
import { DeadLettersFrame, discardConfirmation, UNREACHED_TEXT, type DeadLetterActions } from '../../app/(console)/dead-letters/dead-letters-frame';
import type { DeadLetterRow } from '../../app/(console)/dead-letters/load';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/dead-letters' }));
// SectionError's errorTail imports requestPath from the package root, which would load the whole
// server runtime into jsdom. Only a relogin error reads it, and no case here uses one.
vi.mock('@paigasus/console-core', () => ({ requestPath: () => Promise.resolve('/iam/dead-letters') }));

afterEach(() => {
  cleanup();
});

const A = '0190a1f0-0000-7000-8000-0000000000a1';
const B = '0190a1f0-0000-7000-8000-0000000000b1';

function row(id: string, overrides: Partial<DeadLetterRow> = {}): DeadLetterRow {
  return {
    id,
    eventType: 'iam.team.created',
    aggregatePrn: 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1',
    payload: '{"slug":"platform"}',
    schemaVersion: 1,
    attempts: 5,
    parkedAt: '2026-09-01T10:05:00.000Z',
    occurredAt: '2026-09-01T10:00:00.000Z',
    actorPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0',
    correlationId: 'corr-dead-letter-a',
    lastError: 'nats: timeout',
    ...overrides,
  };
}

const ROW_A = row(A);
const ROW_B = row(B, { actorPrn: null, correlationId: null, lastError: null });

function errorWith(presentation: Presentation): PaigasusError {
  return {
    presentation,
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'corr-unit-dead-letters',
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 5, codeName: 'NotFound' },
  };
}

type Sig = (previous: ActionState, form: FormData) => Promise<ActionState>;

function actions(overrides: Partial<Record<'replay' | 'discard', Sig>> = {}) {
  return {
    replay: vi.fn<Sig>(overrides.replay ?? (() => Promise.resolve({ ok: true }))),
    discard: vi.fn<Sig>(overrides.discard ?? (() => Promise.resolve({ ok: true }))),
  };
}

function frame(children: ReactNode, a: DeadLetterActions): ReactNode {
  return (
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <DeadLettersFrame actions={a}>{children}</DeadLettersFrame>
    </ZoneProvider>
  );
}

/** Renders the children of the revalidated page, with an awaited act() (see manage-controls.test.tsx). */
async function refresh(rerender: (ui: ReactNode) => void, ui: ReactNode): Promise<void> {
  await act(() => {
    rerender(ui);
    return Promise.resolve();
  });
}

const region = (): HTMLElement => screen.getByTestId('dead-letters-result');
const controlsOf = (id: string): HTMLElement => screen.getByTestId(`dead-letter-controls-${id}`);
const rowButtons = (): HTMLButtonElement[] => screen.getAllByRole<HTMLButtonElement>('button', { name: /^(Replay|Discard)$/ });

describe('one result region: a result survives every refresh (§ 6.4)', () => {
  it('keeps "Replayed event A." after the row goes, the list empties, and the table becomes an error', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, a));

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Replay' }));
    await within(region()).findByText(`Replayed event ${A}.`);
    expect(a.replay).toHaveBeenCalledTimes(1);
    expect(a.replay.mock.calls[0]?.[0]).toBeNull();
    expect(a.replay.mock.calls[0]?.[1].get('id')).toBe(A);

    await refresh(rerender, frame(<DeadLetterTable rows={[ROW_B]} />, a));
    expect(screen.queryByTestId(`dead-letter-controls-${A}`)).toBeNull();
    expect(within(region()).getByText(`Replayed event ${A}.`)).toBeDefined();

    await refresh(rerender, frame(<p>No dead letters</p>, a));
    expect(within(region()).getByText(`Replayed event ${A}.`)).toBeDefined();

    const sectionError = await SectionError({ error: errorWith('degraded') });
    await refresh(rerender, frame(sectionError, a));
    expect(screen.getByTestId('section-error')).toBeDefined();
    expect(within(region()).getByText(`Replayed event ${A}.`)).toBeDefined();
  });
});

describe('the runner', () => {
  it('shows a client-built error for a rejected action and does not throw (the frame stays mounted)', async () => {
    const user = userEvent.setup();
    render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, actions({ replay: () => Promise.reject(new TypeError('Failed to fetch')) })));

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Replay' }));

    expect(await within(region()).findByText(UNREACHED_TEXT)).toBeDefined();
    expect(within(region()).getByTestId('form-error').getAttribute('data-presentation')).toBe('generic');
    expect(controlsOf(B)).toBeDefined();
  });

  it('shows the NEWER submission’s result when an older one completes later', async () => {
    let finishFirst: (state: ActionState) => void = () => undefined;
    const a = actions({
      replay: (_previous, form) =>
        form.get('id') === A
          ? new Promise<ActionState>((resolve) => {
              finishFirst = resolve;
            })
          : Promise.resolve({ ok: false, error: errorWith('forbidden') }),
    });
    render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, a));

    // fireEvent.submit reaches onSubmit although the buttons are disabled while A runs.
    fireEvent.submit(screen.getByRole('form', { name: `Replay event ${A}` }));
    fireEvent.submit(screen.getByRole('form', { name: `Replay event ${B}` }));
    await within(region()).findByText(PRESENTATION_COPY.forbidden.body);
    await act(async () => {
      finishFirst({ ok: true });
      await Promise.resolve();
    });

    expect(within(region()).queryByText(`Replayed event ${A}.`)).toBeNull();
    expect(within(region()).getByTestId('form-error').getAttribute('data-presentation')).toBe('forbidden');
    // Only not-found gets the dead-letter sentence (§ 6.5).
    expect(within(region()).queryByText(DEAD_LETTER_GONE)).toBeNull();
  });

  it('disables every Replay and Discard button while an action runs', async () => {
    const user = userEvent.setup();
    let finish: (state: ActionState) => void = () => undefined;
    render(
      frame(
        <DeadLetterTable rows={[ROW_A, ROW_B]} />,
        actions({
          replay: () =>
            new Promise<ActionState>((resolve) => {
              finish = resolve;
            }),
        }),
      ),
    );

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Replay' }));

    await waitFor(() => {
      expect(rowButtons()).toHaveLength(4);
      for (const button of rowButtons()) expect(button.disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      for (const button of rowButtons()) expect(button.disabled).toBe(false);
    });
  });
});

describe('discard', () => {
  it('asks first with the exact text; Cancel returns, and Confirm discard submits the id', async () => {
    const user = userEvent.setup();
    const a = actions();
    render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, a));

    expect(discardConfirmation(A)).toBe(`Discard event ${A}? IAM deletes it from the dead-letter queue, and it is not published again. The audit log keeps a copy of the event.`);
    expect(within(controlsOf(A)).getByRole('button', { name: 'Discard' }).getAttribute('type')).toBe('button');

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Discard' }));
    expect(within(controlsOf(A)).getByText(discardConfirmation(A))).toBeDefined();
    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Cancel' }));
    expect(within(controlsOf(A)).queryByText(discardConfirmation(A))).toBeNull();
    expect(a.discard).not.toHaveBeenCalled();

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Discard' }));
    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Confirm discard' }));

    await within(region()).findByText(`Discarded event ${A}.`);
    expect(a.discard).toHaveBeenCalledTimes(1);
    expect(a.discard.mock.calls[0]?.[1].get('id')).toBe(A);
  });

  it('shows DEAD_LETTER_GONE and the correlation id for a not-found answer', async () => {
    const user = userEvent.setup();
    render(frame(<DeadLetterTable rows={[ROW_A]} />, actions({ discard: () => Promise.resolve({ ok: false, error: errorWith('not-found') }) })));

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Discard' }));
    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Confirm discard' }));

    expect(await within(region()).findByText(DEAD_LETTER_GONE)).toBeDefined();
    expect(within(region()).getByTestId('correlation-id').textContent).toBe('corr-unit-dead-letters');
  });
});

describe('the table', () => {
  it('renders a hostile payload and last error as text, never as HTML', () => {
    const hostile = '</pre><script>alert(1)</script>';
    const { container } = render(frame(<DeadLetterTable rows={[row(A, { payload: hostile, lastError: hostile })]} />, actions()));

    expect(container.querySelector('script')).toBeNull();
    expect(screen.getByTestId('dead-letter-payload').textContent).toBe(hostile);
    expect(screen.getByTestId('dead-letter-last-error').textContent).toBe(hostile);
  });

  it('shows "—" for IAM’s "none" and keeps IAM’s order', () => {
    render(frame(<DeadLetterTable rows={[ROW_B, ROW_A]} />, actions()));

    const rows = screen.getAllByTestId('dead-letter-row');
    expect(rows.map((element) => element.getAttribute('data-id'))).toEqual([B, A]);
    expect(within(rows[0] as HTMLElement).getAllByText('—').length).toBeGreaterThan(0);
    expect(screen.getByText('Newest events first.')).toBeDefined();
  });
});
```

- [ ] **Step 2: Run the test and see it fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/dead-letters-frame.test.tsx
```
Expected: FAIL, `Failed to resolve import "../../app/(console)/dead-letters/dead-letter-table"`.

- [ ] **Step 3: Implement the frame**

Create `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The frame of /iam/dead-letters (SMA-629 spec § 6.4). CLIENT component. The page renders it for
// EVERY view that does not throw — the table, the empty list, the degraded view and a SectionError —
// keyed by `${eventType}|${cursor}`, with the view as children. The design is the gateway console's
// ServiceAccountFrame (ts/apps/gateway-console/app/_components/service-account-frame.tsx:1-27).
//
// WHY A FRAME. React 19 resets a form before every action. A successful replay or discard removes the
// row, and the revalidated render can also replace the table with an error view. A control that held
// its own result would lose it when it unmounts. So this frame holds the ONE result region, and no
// row control holds a result.
//
// HOW AN ACTION RUNS. A row form hands its FormData to the runner, which calls the Server Action
// DIRECTLY in a transition, with `null` as the previous state (not through useActionState). It
// records the id from the FormData, so the result can name it. A generation counter makes the newest
// submission's result win. While any action runs, EVERY Replay and Discard button is disabled.
//
// A REJECTED ACTION (a network drop, a server fault). The runner catches it and shows a client-built
// error. It never rethrows: a rethrow reaches (console)/error.tsx, which unmounts this frame. The
// action may still have run on the server, so the text says the result is unknown; a later retry of
// the same id answers not-found, and DEAD_LETTER_GONE covers that (§ 9).
'use client';

import { createContext, use, useRef, useState, useTransition, type FormEvent, type ReactElement, type ReactNode } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import type { ActionResult, FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { DEAD_LETTER_GONE } from '../../_components/error-copy';
import { FormError } from '../../_components/form-error';

export type DeadLetterControl = 'replay' | 'discard';

export type DeadLetterActions = { readonly replay: FormAction; readonly discard: FormAction };

type FrameResult = null | { readonly kind: 'answer'; readonly control: DeadLetterControl; readonly id: string; readonly state: ActionResult } | { readonly kind: 'unreached'; readonly error: PaigasusError };

/** The action's promise rejected: no answer came back. The action can still have run on the server. */
export const UNREACHED_TEXT = 'No answer came back from the server, so the result is unknown. Reload the page to see the current queue.';

export function successText(control: DeadLetterControl, id: string): string {
  return control === 'replay' ? `Replayed event ${id}.` : `Discarded event ${id}.`;
}

/** § 6.4 (application/dead_letters.rs:215-229): a discard deletes the entry; the audit log keeps a copy. */
export function discardConfirmation(id: string): string {
  return `Discard event ${id}? IAM deletes it from the dead-letter queue, and it is not published again. The audit log keeps a copy of the event.`;
}

/** A client-built error for a rejected action. It never reached IAM, so it has no correlation id. */
function unreachedError(): PaigasusError {
  return {
    presentation: 'generic',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: UNREACHED_TEXT,
    correlationId: null,
    requestId: null,
    retryable: null,
    metadata: {},
    transport: { kind: 'transport', cause: 'network' },
  };
}

function ResultMessage({ result }: { readonly result: FrameResult }): ReactElement | null {
  if (result === null) return null;
  if (result.kind === 'unreached') return <FormError error={result.error} message={UNREACHED_TEXT} />;
  if (result.state.ok) return <p className="text-sm">{successText(result.control, result.id)}</p>;
  const error = result.state.error;
  // § 6.5: only a not-found answer of these two actions gets the dead-letter sentence.
  return <FormError error={error} message={error.presentation === 'not-found' ? DEAD_LETTER_GONE : undefined} />;
}

type Runner = {
  /** True while any action of the frame runs. Every Replay and Discard button is disabled then. */
  readonly busy: boolean;
  run(control: DeadLetterControl, form: FormData): void;
};

const RunnerContext = createContext<Runner | null>(null);

function useRunner(): Runner {
  const runner = use(RunnerContext);
  if (runner === null) throw new Error('DeadLetterRowControls must render inside DeadLettersFrame.');
  return runner;
}

export function DeadLettersFrame({ actions, children }: { readonly actions: DeadLetterActions; readonly children: ReactNode }): ReactElement {
  const [result, setResult] = useState<FrameResult>(null);
  const [busy, startWork] = useTransition();
  const generationRef = useRef(0);

  const runner: Runner = {
    busy,
    run(control, form) {
      generationRef.current += 1;
      const mine = generationRef.current;
      const raw = form.get('id');
      const id = typeof raw === 'string' ? raw.trim() : '';
      setResult(null);
      startWork(async () => {
        try {
          const state = await actions[control](null, form);
          if (state !== null && generationRef.current === mine) setResult({ kind: 'answer', control, id, state });
        } catch {
          if (generationRef.current === mine) setResult({ kind: 'unreached', error: unreachedError() });
        }
      });
    },
  };

  return (
    <RunnerContext value={runner}>
      <div className="flex flex-col gap-4">
        <div role="status" data-testid="dead-letters-result" className="flex flex-col gap-2">
          <ResultMessage result={result} />
        </div>
        {children}
      </div>
    </RunnerContext>
  );
}

/**
 * Replay (one step: it is the normal recovery path) and Discard (two steps, the ArchiveButton
 * pattern of app/_components/lifecycle-button.tsx:42-80). No result of their own: the frame shows it.
 */
export function DeadLetterRowControls({ id }: { readonly id: string }): ReactElement {
  const runner = useRunner();
  const [confirming, setConfirming] = useState(false);

  function submit(control: DeadLetterControl) {
    return (event: FormEvent<HTMLFormElement>): void => {
      event.preventDefault();
      runner.run(control, new FormData(event.currentTarget));
      if (control === 'discard') setConfirming(false);
    };
  }

  return (
    <div data-testid={`dead-letter-controls-${id}`} className="flex flex-col gap-2">
      <form aria-label={`Replay event ${id}`} onSubmit={submit('replay')}>
        <input type="hidden" name="id" value={id} />
        <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
          Replay
        </button>
      </form>
      {confirming ? (
        <form aria-label={`Confirm the discard of event ${id}`} className="flex flex-col gap-2" onSubmit={submit('discard')}>
          <p className="text-sm">{discardConfirmation(id)}</p>
          <input type="hidden" name="id" value={id} />
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
              Confirm discard
            </button>
            <button
              type="button"
              disabled={runner.busy}
              className={SECONDARY_BUTTON_CLASS}
              onClick={() => {
                setConfirming(false);
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          disabled={runner.busy}
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Discard
        </button>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Implement the table**

Create `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The dead-letter list (SMA-629 spec § 6.3). NO directive: the page, a server component, renders it
// inside DeadLettersFrame, and tests/unit/dead-letters-frame.test.tsx renders the same code in jsdom.
// It keeps IAM's order (id descending). The payload and the last error render as TEXT in a <pre>:
// React escapes them, and the page never parses the payload and never renders it as HTML. The payload
// has no size bound (only last_error has one, 1 KB), so each <pre> has a maximum height and scrolls.
import { Fragment, type ReactElement } from 'react';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { DeadLetterRowControls } from './dead-letters-frame';
import type { DeadLetterRow } from './load';

const NONE = '—';
const PRE_CLASS = 'bg-muted max-h-64 overflow-auto rounded-pgs p-2 text-xs break-all whitespace-pre-wrap';
const COLUMNS = 7;

function Code({ value }: { readonly value: string | null }): ReactElement {
  return value === null ? <>{NONE}</> : <code className="text-xs">{value}</code>;
}

export function DeadLetterTable({ rows }: { readonly rows: readonly DeadLetterRow[] }): ReactElement {
  return (
    <Table>
      <caption className="text-muted-foreground mb-2 caption-top text-left text-sm">Newest events first.</caption>
      <TableHeader>
        <TableRow>
          <TableHead>Event id</TableHead>
          <TableHead>Parked</TableHead>
          <TableHead>Event type</TableHead>
          <TableHead>Aggregate</TableHead>
          <TableHead>Attempts</TableHead>
          <TableHead>Correlation id</TableHead>
          <TableHead>Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <Fragment key={row.id}>
            <TableRow data-testid="dead-letter-row" data-id={row.id}>
              <TableCell>
                <Code value={row.id} />
              </TableCell>
              <TableCell>{row.parkedAt ?? NONE}</TableCell>
              <TableCell>{row.eventType}</TableCell>
              <TableCell>
                <Code value={row.aggregatePrn} />
              </TableCell>
              <TableCell>{row.attempts}</TableCell>
              <TableCell>
                <Code value={row.correlationId} />
              </TableCell>
              <TableCell>
                <DeadLetterRowControls id={row.id} />
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell colSpan={COLUMNS}>
                <details>
                  <summary className="cursor-pointer text-sm">{`Details of event ${row.id}`}</summary>
                  <dl className="mt-2 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
                    <dt>Occurred</dt>
                    <dd>{row.occurredAt ?? NONE}</dd>
                    <dt>Schema version</dt>
                    <dd>{row.schemaVersion}</dd>
                    <dt>Actor</dt>
                    <dd>
                      <Code value={row.actorPrn} />
                    </dd>
                  </dl>
                  <p className="mt-2 text-sm font-medium">Payload</p>
                  <pre data-testid="dead-letter-payload" className={PRE_CLASS}>
                    {row.payload}
                  </pre>
                  <p className="mt-2 text-sm font-medium">Last error</p>
                  <pre data-testid="dead-letter-last-error" className={PRE_CLASS}>
                    {row.lastError ?? NONE}
                  </pre>
                </details>
              </TableCell>
            </TableRow>
          </Fragment>
        ))}
      </TableBody>
    </Table>
  );
}
```

- [ ] **Step 5: Run the test and see it pass**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/dead-letters-frame.test.tsx
moon run iam-console-ts:typecheck
```
Expected: PASS. If `TableRow` or `TableCell` does not forward `data-*` or `colSpan` (both spread `ComponentPropsWithoutRef<'tr'>`/`<'td'>` today), the typecheck says so; do not change `@paigasus/ui`, stop and report.

- [ ] **Step 6: Commit**

```bash
git add "ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx" "ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx" ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx
git commit -m "feat(ts): the dead-letters frame, row controls and table (SMA-629)" -m "The frame owns the one result region and one runner for replay and discard, keeps a result
through every refresh, lets the newest submission win, and catches a rejected action. The
table shows each payload and last error as text in a scrolling pre." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 10: The page

**Files:**
- Create: `ts/apps/iam-console/app/(console)/dead-letters/page.tsx`
- Create: `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts`

**Interfaces:**
- Consumes: `discovery`, `iamClients`, `sessionToken` (`lib/console.ts`); `deadLettersGate`, `loadDeadLettersPage` (Task 7); `parseEventType`, `parseCursor`, `listHref`, `MAX_EVENT_TYPE_LENGTH` (Task 5); `replayDeadLetterAction`, `discardDeadLetterAction` (Task 8); `DeadLettersFrame`, `DeadLetterTable` (Task 9); `PageError`, `SectionError`, `PRESENTATION_COPY`.
- Produces: `export default async function DeadLettersPage({ searchParams }: { searchParams: Promise<Record<string, string | string[] | undefined>> }): Promise<ReactElement>`. The only export (Next refuses other page exports).

**Order of the page (§ 6.2, § 6.4).** Session token and discovery, then the gate. `not-found` → `notFound()`. Then the two query parsers run: they are pure and make no IAM call, and they run before the degraded branch only so that the frame key is the same for every view. `degraded` → the degraded view inside the frame, no IAM call, whatever the query. A refused query → `PageError` with `invalid-input`, before the clients are built. Then the loader. A list error with presentation `forbidden` or `not-found` → `PageError` (a fresh GET keeps its real 403 or 404). Every other list error → `SectionError` inside the frame.

The test walks the element tree that the page RETURNS, without rendering it: `PageError` and `SectionError` are async server components, which a synchronous static render cannot resolve. Helpers the page calls as plain functions (`filterForm`, `shell`) are inlined into that tree, so the walk sees the form and the frame.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters, branch by branch (SMA-629 spec § 6.2–§ 6.4). The page's own accessors are
// mocked at the lib/console boundary, and the test reads the element tree the page returns. It
// proves: the 404 gate, the degraded view inside the frame with no IAM call, a refused query that
// never becomes an IAM call, the 403/404 list errors as PageError, every other list error as a
// SectionError inside the frame, the frame key, the GET form with no `action`, and the paging hrefs.
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import { Code, ConnectError } from '@connectrpc/connect';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ServiceState } from '@paigasus/discovery/types';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

const mocks = vi.hoisted(() => ({
  state: { current: { state: 'absent', service: 'iam' } as unknown },
  listDeadLetters: vi.fn(),
  iamClients: vi.fn(),
}));

vi.mock('../../lib/console', () => ({
  sessionToken: () => Promise.resolve('tok-page'),
  discovery: () => ({ getServiceState: () => Promise.resolve(mocks.state.current) }),
  iamClients: mocks.iamClients,
  iamClientsForAction: vi.fn(),
}));

const { default: DeadLettersPage } = await import('../../app/(console)/dead-letters/page');
const { PageError } = await import('../../app/_components/page-error');
const { SectionError } = await import('../../app/_components/section-error');
const { DeadLettersFrame } = await import('../../app/(console)/dead-letters/dead-letters-frame');
const { DeadLetterTable } = await import('../../app/(console)/dead-letters/dead-letter-table');
const { PAGE_SIZE } = await import('../../lib/paging');

const ID = '0190a1f0-0000-7000-8000-0000000000d1';
const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1.0.0', capabilities });
const WITH_KEY: ServiceState = { state: 'available', service: 'iam', descriptor: descriptor(['iam.deadletters']), capabilities: ['iam.deadletters'] };
const WITHOUT_KEY: ServiceState = { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] };
const DEGRADED: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor([]), capabilities: [] };

const entry = {
  id: ID,
  occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
  eventType: 'orders',
  schemaVersion: 1,
  aggregatePrn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a',
  actorPrn: '',
  payload: '{}',
  correlationId: '',
  attempts: 3,
  parkedAt: { seconds: 1_788_000_060n, nanos: 0 },
  lastError: '',
};

/** Every element in the returned tree, depth first. Components are NOT rendered, only walked. */
function* walk(node: ReactNode): Generator<ReactElement> {
  if (Array.isArray(node)) {
    for (const child of node as ReactNode[]) yield* walk(child);
    return;
  }
  if (!isValidElement(node)) return;
  yield node;
  yield* walk((node.props as { children?: ReactNode }).children);
}

const all = (tree: ReactNode, type: unknown): ReactElement[] => [...walk(tree)].filter((element) => element.type === type);
const byTestId = (tree: ReactNode, id: string): ReactElement[] => [...walk(tree)].filter((element) => (element.props as Record<string, unknown>)['data-testid'] === id);

function visit(query: Record<string, string | string[] | undefined>): Promise<ReactElement> {
  return DeadLettersPage({ searchParams: Promise.resolve(query) });
}

beforeEach(() => {
  mocks.listDeadLetters.mockReset();
  mocks.iamClients.mockReset();
  mocks.iamClients.mockImplementation(() => Promise.resolve({ outbox: { listDeadLetters: mocks.listDeadLetters } }));
});

describe('the gate', () => {
  it.each<[string, ServiceState]>([
    ['absent', { state: 'absent', service: 'iam' }],
    ['available without iam.deadletters (AC 2)', WITHOUT_KEY],
  ])('answers 404 when IAM is %s, and never builds a client', async (_label, state) => {
    mocks.state.current = state;

    await expect(visit({})).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;404' });
    expect(mocks.iamClients).not.toHaveBeenCalled();
  });

  it('renders the degraded view INSIDE the frame, with no IAM call, whatever the query', async () => {
    mocks.state.current = DEGRADED;

    const tree = await visit({ eventType: 'e'.repeat(201) });

    const [frame] = all(tree, DeadLettersFrame);
    expect(frame).toBeDefined();
    expect(byTestId(frame, 'dead-letters-degraded')).toHaveLength(1);
    expect(mocks.iamClients).not.toHaveBeenCalled();
  });
});

describe('the query', () => {
  it.each([
    ['an event type of 201 characters', { eventType: 'e'.repeat(201) }],
    ['a cursor past the bound', { cursor: 'c'.repeat(1025) }],
  ])('refuses %s as a PageError before any client is built', async (_label, query) => {
    mocks.state.current = WITH_KEY;

    const tree = await visit(query);

    expect(tree.type).toBe(PageError);
    expect((tree.props as { error: PaigasusError }).error.presentation).toBe('invalid-input');
    expect(mocks.iamClients).not.toHaveBeenCalled();
  });
});

describe('the list', () => {
  beforeEach(() => {
    mocks.state.current = WITH_KEY;
  });

  it('sends the filter and the cursor, keys the frame by both, and renders the table, the GET form and the paging links', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [entry], nextCursor: 'c2' });

    const tree = await visit({ eventType: ' orders ', cursor: 'c1' });

    expect(mocks.listDeadLetters).toHaveBeenCalledWith({ eventType: 'orders', cursor: 'c1', limit: PAGE_SIZE });
    const [frame] = all(tree, DeadLettersFrame);
    expect(frame?.key).toBe('orders|c1');
    const [table] = all(tree, DeadLetterTable);
    expect((table?.props as { rows: { id: string }[] }).rows.map((row) => row.id)).toEqual([ID]);

    const [form] = all(tree, 'form');
    expect(form?.props).toMatchObject({ method: 'get' });
    // § 6.3: no `action`, so the form submits to the current URL and stays in the /iam zone.
    expect('action' in (form?.props as object)).toBe(false);

    const hrefs = [...walk(tree)].map((element) => (element.props as { href?: unknown }).href).filter((href) => typeof href === 'string');
    expect(hrefs).toEqual(['/iam/dead-letters?eventType=orders', '/iam/dead-letters?eventType=orders&cursor=c2']);
  });

  it('shows the empty state and no paging links on an empty first page, and keys the frame "|"', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [], nextCursor: '' });

    const tree = await visit({});

    expect(all(tree, DeadLettersFrame)[0]?.key).toBe('|');
    expect(all(tree, DeadLetterTable)).toHaveLength(0);
    expect([...walk(tree)].some((element) => (element.props as { title?: unknown }).title === 'No dead letters')).toBe(true);
    expect([...walk(tree)].filter((element) => typeof (element.props as { href?: unknown }).href === 'string')).toHaveLength(0);
  });

  it.each([
    ['forbidden', Code.PermissionDenied],
    ['not-found', Code.NotFound],
  ])('returns a %s list error as a PageError, so a fresh GET keeps its real status', async (presentation, code) => {
    mocks.listDeadLetters.mockRejectedValue(new ConnectError('nope', code));

    const tree = await visit({});

    expect(tree.type).toBe(PageError);
    expect((tree.props as { error: PaigasusError }).error.presentation).toBe(presentation);
  });

  it('renders every other list error as a SectionError INSIDE the frame', async () => {
    mocks.listDeadLetters.mockRejectedValue(new ConnectError('nope', Code.Unavailable));

    const tree = await visit({});

    const [frame] = all(tree, DeadLettersFrame);
    const [section] = all(frame, SectionError);
    expect((section?.props as { error: PaigasusError }).error.presentation).toBe('degraded');
  });
});
```

- [ ] **Step 2: Run the test and see it fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/dead-letters-page.test.ts
```
Expected: FAIL, `Failed to resolve import "../../app/(console)/dead-letters/page"`.

- [ ] **Step 3: Implement**

Create `ts/apps/iam-console/app/(console)/dead-letters/page.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2–§ 6.4, AC 1 and AC 2). The page asks discovery, not mayI()
// (SMA-511 § 6.3): the Dead letters nav entry is the affordance and mayI() hides it; a typed URL is a
// user action, and IAM answers it. Every OutboxService RPC is Root-only inside IAM.
//
// Every view that does not throw renders inside DeadLettersFrame, keyed by `${eventType}|${cursor}`,
// so a revalidated render after a replay or discard keeps the frame and its result, and a move to
// another page or filter starts an empty frame. The two query parsers are pure and run before the
// degraded branch ONLY to compute that key; a degraded IAM gets the degraded view whatever the query.
import type { ReactElement, ReactNode } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, ErrorState, Field, Input, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { SectionError } from '../../_components/section-error';
import { discovery, iamClients, sessionToken } from '../../../lib/console';
import { listHref, MAX_EVENT_TYPE_LENGTH, parseCursor, parseEventType } from '../../../lib/paging';
import { discardDeadLetterAction, replayDeadLetterAction } from './actions';
import { DeadLetterTable } from './dead-letter-table';
import { DeadLettersFrame } from './dead-letters-frame';
import { deadLettersGate, loadDeadLettersPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

/** The full path, as the ingress sees it (ZoneLink needs it). */
const PATH = '/iam/dead-letters';

/** The breadcrumbs, the heading and the frame around one view. A plain function, not a component. */
function shell(frameKey: string, body: ReactNode): ReactElement {
  return (
    <div className="flex flex-col gap-6 p-6">
      <Breadcrumbs items={[{ label: 'Dead letters' }]} />
      <h1 className="text-2xl font-semibold">Dead letters</h1>
      <DeadLettersFrame key={frameKey} actions={{ replay: replayDeadLetterAction, discard: discardDeadLetterAction }}>
        {body}
      </DeadLettersFrame>
    </div>
  );
}

/**
 * A plain GET form with NO `action` attribute: it submits to the current URL, /iam/dead-letters. A
 * basePath-relative action="/dead-letters" would leave the zone (§ 6.3). It needs no JavaScript, and a
 * new filter starts at the first page.
 */
function filterForm(eventType: string): ReactElement {
  return (
    <form method="get" aria-label="Filter dead letters" className="flex flex-wrap items-end gap-2">
      <Field label="Event type" htmlFor="dead-letters-event-type">
        <Input name="eventType" defaultValue={eventType} maxLength={MAX_EVENT_TYPE_LENGTH} autoComplete="off" />
      </Field>
      <button type="submit" className={SECONDARY_BUTTON_CLASS}>
        Filter
      </button>
    </form>
  );
}

export default async function DeadLettersPage({ searchParams }: Props): Promise<ReactElement> {
  const [query, token] = await Promise.all([searchParams, sessionToken()]);
  const gate = deadLettersGate(await discovery().getServiceState('iam', token));
  if (gate === 'not-found') notFound();

  const eventType = parseEventType(query.eventType);
  const cursor = parseCursor(query.cursor);
  const frameKey = `${eventType.ok ? eventType.value : ''}|${cursor.ok ? cursor.cursor : ''}`;

  if (gate === 'degraded') {
    return shell(
      frameKey,
      <div data-testid="dead-letters-degraded">
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>,
    );
  }

  // Both parsers run BEFORE the clients are built: a refused query never becomes an IAM call.
  if (!eventType.ok) return <PageError error={eventType.error} />;
  if (!cursor.ok) return <PageError error={cursor.error} />;

  const clients = await iamClients();
  const data = await loadDeadLettersPage({ outbox: clients.outbox }, { cursor: cursor.cursor, eventType: eventType.value });
  if (!data.ok) {
    // A fresh GET keeps its real 403 or 404. Every other list error stays inside the frame.
    if (data.error.presentation === 'forbidden' || data.error.presentation === 'not-found') return <PageError error={data.error} />;
    return shell(
      frameKey,
      <>
        {filterForm(eventType.value)}
        <SectionError error={data.error} />
      </>,
    );
  }

  const { rows, nextCursor } = data.value;
  return shell(
    frameKey,
    <>
      {filterForm(eventType.value)}
      {rows.length === 0 ? <EmptyState title="No dead letters" /> : <DeadLetterTable rows={rows} />}
      <nav aria-label="Dead-letter pages" className="flex gap-4 text-sm">
        {cursor.cursor === '' ? null : (
          <ZoneLink href={listHref(PATH, { eventType: eventType.value })} className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={listHref(PATH, { eventType: eventType.value, cursor: nextCursor })} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </>,
  );
}
```

- [ ] **Step 4: Run the tests and the build**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/dead-letters-page.test.ts
moon run iam-console-ts:typecheck iam-console-ts:build
```
Expected: the test PASSES; `next build` succeeds and its route list includes `/dead-letters`. A build error about an invalid page export means a named export was added to `page.tsx`; remove it.

- [ ] **Step 5: Commit**

```bash
git add "ts/apps/iam-console/app/(console)/dead-letters/page.tsx" ts/apps/iam-console/tests/unit/dead-letters-page.test.ts
git commit -m "feat(ts): the /iam/dead-letters page (SMA-629)" -m "The page gates on iam.deadletters, validates the filter and the cursor before any IAM call,
and renders every non-throwing view inside the frame keyed by filter and cursor. A 403 or
404 list error stays a page error." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 11: e2e rows R17–R19 and the R11 extension

**Files:**
- Modify: `ts/apps/iam-console/tests/unit/e2e-rows.test.ts` (`:1-14`)
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts` (header `:1-15`; new exports after `DEFAULT_DESCRIPTOR`, `:72`; `worldHandlers()` `:128-199`)
- Create: `ts/apps/iam-console/tests/e2e/dead-letters.spec.ts`
- Modify: `ts/apps/iam-console/tests/e2e/token-leak.spec.ts` (imports `:25-27`; route predicate `:134`; path list `:170`; after `:190`; guards after `:223`)

**Interfaces:**
- Consumes: the DOM contract of Task 9; the page of Task 10; `harness.useWorld`, `harness.iam.callsTo`, `signIn`, `waitForHydration`.
- Produces (from `world.ts`): `DEAD_LETTER_A_ID`, `DEAD_LETTER_B_ID`, `DEAD_LETTERS_DESCRIPTOR: Descriptor`; default handlers `outbox.listDeadLetters`, `outbox.replayDeadLetter`, `outbox.discardDeadLetter` (stateful per `worldHandlers()` call). `DEFAULT_DESCRIPTOR` keeps its two keys (§ 5.3), so AC 2 is the default.

- [ ] **Step 1: Write the failing row test**

In `tests/unit/e2e-rows.test.ts`, replace the header comment, `ROWS` and the `describe` title:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The SMA-511 spec (§ 9.4) has thirteen rows, R1–R13. The SMA-630 spec
// (docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md, § 9.3) adds three, R14–R16.
// The SMA-629 spec (docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md,
// § 7.4) adds three, R17–R19. Each row must have exactly one Playwright test whose title starts with
// its row id. A deleted or renamed scenario then fails this vitest suite, which runs in
// iam-console-ts:test on every PR that touches the app, even when the e2e task does not run.
```

```ts
const ROWS = Array.from({ length: 19 }, (_, index) => `R${String(index + 1)}`);

describe('the e2e tier covers every row of SMA-511 § 9.4, SMA-630 § 9.3 and SMA-629 § 7.4', () => {
```

- [ ] **Step 2: Run it and see it fail**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/e2e-rows.test.ts
```
Expected: FAIL for `R17`, `R18`, `R19` (`expected [] to have length 1`).

- [ ] **Step 3: Extend the e2e world**

In `tests/e2e/support/world.ts`, add to the header comment after the R6 paragraph:

```ts
// SMA-629: three outbox handlers are in the default set too, because every override key must exist
// there. They hold two seeded dead letters with RFC 4122 ids, per worldHandlers() call; replay and
// discard remove the entry, and an unknown id answers NotFound, as IAM does.
```

Add after `DEFAULT_DESCRIPTOR`:

```ts
export const DEAD_LETTER_A_ID = '0190a1f0-0000-7000-8000-0000000000d1';
export const DEAD_LETTER_B_ID = '0190a1f0-0000-7000-8000-0000000000d2';

/**
 * An IAM that serves OutboxService (SMA-629). The FULL Descriptor shape — service, version and
 * capabilities — because the type needs all three. DEFAULT_DESCRIPTOR keeps its two keys, so a
 * test that does not ask for this one meets an older IAM (AC 2).
 */
export const DEAD_LETTERS_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.audit', 'iam.deadletters'] };

type DeadLetterFixture = {
  id: string;
  occurredAt: { seconds: bigint; nanos: number };
  eventType: string;
  schemaVersion: number;
  aggregatePrn: string;
  actorPrn: string;
  payload: string;
  correlationId: string;
  attempts: number;
  parkedAt: { seconds: bigint; nanos: number };
  lastError: string;
};

function seededDeadLetters(): Map<string, DeadLetterFixture> {
  const entries: DeadLetterFixture[] = [
    {
      id: DEAD_LETTER_A_ID,
      occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
      eventType: 'iam.team.created',
      schemaVersion: 1,
      aggregatePrn: TEAM_PRN,
      actorPrn: PRINCIPAL_PRN,
      payload: '{"slug":"platform"}',
      correlationId: 'corr-dead-letter-a',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_300n, nanos: 0 },
      lastError: 'nats: no responders available for request',
    },
    {
      id: DEAD_LETTER_B_ID,
      occurredAt: { seconds: 1_788_000_600n, nanos: 0 },
      eventType: 'iam.project.created',
      schemaVersion: 1,
      aggregatePrn: PROJECT_PRN,
      actorPrn: '',
      payload: '{"slug":"gateway"}',
      correlationId: '',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_900n, nanos: 0 },
      lastError: '',
    },
  ];
  return new Map(entries.map((entry) => [entry.id, entry]));
}

function takeDeadLetter(deadLetters: Map<string, DeadLetterFixture>, id: string): DeadLetterFixture {
  const entry = deadLetters.get(id);
  if (entry === undefined) throw notFound();
  deadLetters.delete(id);
  return entry;
}
```

`seededDeadLetters` uses `TEAM_PRN`, `PROJECT_PRN` and `PRINCIPAL_PRN`, and `takeDeadLetter` uses `notFound`; place both functions after the `notFound` constant (`:83`), and keep the two id constants and the descriptor next to `DEFAULT_DESCRIPTOR`.

In `worldHandlers()`, after `const created = …`:

```ts
  const deadLetters = seededDeadLetters();
```

and add to the returned map after `'audit.listAuditEntries': …` and before `...options.overrides`:

```ts
    // SMA-629. IAM orders by id DESCENDING and matches event_type exactly.
    'outbox.listDeadLetters': (req: { eventType: string }) => ({
      entries: [...deadLetters.values()].filter((entry) => req.eventType === '' || entry.eventType === req.eventType).sort((a, b) => (a.id < b.id ? 1 : -1)),
      nextCursor: '',
    }),
    'outbox.replayDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(deadLetters, req.id) }),
    'outbox.discardDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(deadLetters, req.id) }),
```

- [ ] **Step 4: Write the three rows**

Create `ts/apps/iam-console/tests/e2e/dead-letters.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-629 AC 1 and AC 2 (spec § 7.4). PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each
// page load probes the fake's GET /v1/service-info again, and a test changes the IAM state between
// two loads. Discard needs JavaScript, so every page.goto before a click waits for hydration.
import { DEAD_LETTER_A_ID, DEAD_LETTER_B_ID, DEAD_LETTERS_DESCRIPTOR } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

test('R17: iam.deadletters reported and the user is Root, so the page lists, replays and discards (SMA-629 AC 1)', async ({ page, harness }) => {
  harness.useWorld({ descriptor: DEAD_LETTERS_DESCRIPTOR });
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });
  await expect(nav.getByRole('link', { name: 'Dead letters', exact: true })).toBeVisible();

  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  const rows = page.getByTestId('dead-letter-row');
  const region = page.getByTestId('dead-letters-result');
  await expect(rows).toHaveCount(2);

  const replaysBefore = harness.iam.callsTo('outbox.replayDeadLetter').length;
  await page.getByTestId(`dead-letter-controls-${DEAD_LETTER_A_ID}`).getByRole('button', { name: 'Replay' }).click();
  await expect(region).toContainText(`Replayed event ${DEAD_LETTER_A_ID}.`);
  await expect(rows).toHaveCount(1);
  const replays = harness.iam.callsTo('outbox.replayDeadLetter').slice(replaysBefore);
  expect(replays).toHaveLength(1);
  expect(replays[0]?.request).toMatchObject({ id: DEAD_LETTER_A_ID });

  const discardsBefore = harness.iam.callsTo('outbox.discardDeadLetter').length;
  const controlsB = page.getByTestId(`dead-letter-controls-${DEAD_LETTER_B_ID}`);
  await controlsB.getByRole('button', { name: 'Discard' }).click();
  await expect(controlsB.getByText(`Discard event ${DEAD_LETTER_B_ID}?`, { exact: false })).toBeVisible();
  expect(harness.iam.callsTo('outbox.discardDeadLetter').length).toBe(discardsBefore);
  await controlsB.getByRole('button', { name: 'Confirm discard' }).click();

  await expect(region).toContainText(`Discarded event ${DEAD_LETTER_B_ID}.`);
  await expect(rows).toHaveCount(0);
  await expect(page.getByText('No dead letters')).toBeVisible();
  // The frame outlived the swap from the table to the empty state, so the result is still there.
  await expect(region).toContainText(`Discarded event ${DEAD_LETTER_B_ID}.`);
  const discards = harness.iam.callsTo('outbox.discardDeadLetter').slice(discardsBefore);
  expect(discards).toHaveLength(1);
  expect(discards[0]?.request).toMatchObject({ id: DEAD_LETTER_B_ID });
});

test('R18: iam.deadletters not reported, so the entry is absent and /iam/dead-letters is a 404 (SMA-629 AC 2)', async ({ page, harness }) => {
  // The default world: DEFAULT_DESCRIPTOR has no iam.deadletters, an older IAM.
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });

  await expect(nav.getByRole('link', { name: 'Organizations', exact: true })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Dead letters', exact: true })).toHaveCount(0);
  const before = harness.iam.callsTo('outbox.listDeadLetters').length;
  const response = await page.goto(harness.url('/iam/dead-letters'));
  expect(response?.status()).toBe(404);
  expect(harness.iam.callsTo('outbox.listDeadLetters').length).toBe(before);
});

test('R19: IAM degraded, so the entry is disabled with a reason and /iam/dead-letters shows the degraded view (SMA-629 § 8)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({ descriptor: { status: 503 } });

  await page.goto(harness.url('/iam/orgs'));

  const nav = page.getByRole('navigation', { name: 'Primary' });
  const entry = nav.getByRole('link', { name: 'Dead letters', exact: true });
  await expect(entry).toHaveAttribute('aria-disabled', 'true');
  const reasonId = await entry.getAttribute('aria-describedby');
  expect(reasonId).toBeTruthy();
  await expect(page.locator(`[id="${String(reasonId)}"]`)).toHaveText(/\S/);

  const before = harness.iam.callsTo('outbox.listDeadLetters').length;
  const response = await page.goto(harness.url('/iam/dead-letters'));
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('dead-letters-degraded')).toBeVisible();
  // A degraded IAM never reaches ListDeadLetters.
  expect(harness.iam.callsTo('outbox.listDeadLetters').length).toBe(before);
});
```

- [ ] **Step 5: Extend R11**

In `tests/e2e/token-leak.spec.ts`:
- Change the world import to `import { DEAD_LETTER_A_ID, DEAD_LETTERS_DESCRIPTOR, ORG_ID, ORG_NAME, PROJECT_ID, TEAM_ID } from './support/world';`.
- Make the first statement of the test body `harness.useWorld({ descriptor: DEAD_LETTERS_DESCRIPTOR });` (before `const buffered = …`). The new page and its two Server Actions are a new Flight surface (§ 7.4).
- Change the route predicate (`:134`) so the dead-letters action POST is buffered too. It arrives chunked under load exactly like the create action, and an unbuffered body would red the residue assertion:

```ts
    (url) => url.origin === harness.origin && (url.pathname === '/iam/orgs' || url.pathname === '/iam/dead-letters' || url.searchParams.has('_rsc')),
```

- Add `'/iam/dead-letters'` to the path list (`:170`), after `'/iam/audit'`.
- After the `await page.waitForLoadState('networkidle');` that follows the create (`:190`), add:

```ts
  // SMA-629: one replay, so the dead-letters Server Action result is scanned too.
  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  await page.getByTestId(`dead-letter-controls-${DEAD_LETTER_A_ID}`).getByRole('button', { name: 'Replay' }).click();
  await expect(page.getByTestId('dead-letters-result')).toContainText(`Replayed event ${DEAD_LETTER_A_ID}.`);
  await page.waitForLoadState('networkidle');
```

- After the last vacuity guard (`:223`), add:

```ts
  // SMA-629: the dead-letters action body was buffered and scanned, not only the create action's.
  expect(seen.some((response) => response.action && response.method === 'POST' && new URL(response.url).pathname === '/iam/dead-letters' && response.buffered && scanned(response))).toBe(true);
```

- [ ] **Step 6: Run the unit guards, then the e2e tier**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/e2e-rows.test.ts tests/unit/hydration.test.ts tests/unit/world-actions.test.ts
moon run iam-console-ts:typecheck
moon run iam-console-ts:test-e2e
```
Expected: the three unit files PASS (`hydration.test.ts` proves no unbounded `.waitFor(` was added). `iam-console-ts:test-e2e` builds the app and runs every spec; R11 and R17–R19 PASS with every other row. If R11's residue assertion names a `/iam/dead-letters` URL, the route predicate edit is missing.

- [ ] **Step 7: Commit**

```bash
git add ts/apps/iam-console/tests/unit/e2e-rows.test.ts ts/apps/iam-console/tests/e2e/support/world.ts ts/apps/iam-console/tests/e2e/dead-letters.spec.ts ts/apps/iam-console/tests/e2e/token-leak.spec.ts
git commit -m "test(ts): e2e rows R17-R19 for the dead-letters screen, and R11 covers it (SMA-629)" -m "R17 lists, replays and discards as Root with the key, R18 proves the 404 and no IAM call
without the key, and R19 the disabled entry and the degraded view. R11 now scans the new
page and one replay action for a token." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 12: The runbook line, and the full verification

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` (the Remediation list, after the "Replay one row" bullet, `:442-446`)

- [ ] **Step 1: Add the runbook bullet**

After the bullet that starts `- **Replay one row:**`, add:

```markdown
- **In the IAM console:** a Root user can also list, replay and discard single entries at
  `/iam/dead-letters` (SMA-629). Each row shows the payload and `last_error`, and a discard asks for
  a confirmation first. The screen appears only when IAM reports the `iam.deadletters` capability,
  which every IAM build since SMA-629 does. Bulk replay stays API-only.
```

- [ ] **Step 2: Run the whole-tree TS gates**

```bash
moon run ts:fmt ts:lint
```
Expected: PASS. `ts:fmt` is its own whole-tree Prettier gate, separate from `ts:lint`. On a Prettier failure, run `pnpm -C ts exec prettier --write <the files it names>` and run `ts:fmt` again.

- [ ] **Step 3: Run the full CI target graph, as CI does**

This is the command from CLAUDE.md, between its ci-targets markers:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

Expected: every selected task passes. The proto edit selects `contracts:*`, every binding and the codegen-drift inputs, so this run is long. Docker must be up: the IAM integration suites, `paigasus-console-core-ts:test-e2e` and `gateway-console-ts:test-e2e` need it, and an `iam-console` edit selects the gateway e2e tier too.

Known local-only noise, from CLAUDE.md. Read these before you diagnose a red:
- No single local bash runs every gate. `repo:affected-smoke` needs `/bin/bash` 3.2 (use a bash-only shim directory, not `/bin` first on PATH). `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+. `repo:actionlint` has no working local bash; CI is its only verdict. An empty stdout with a one-line `declare: -A` or `mapfile` stderr is a bash-version artifact, not a finding. Re-run such a gate directly with the right bash (`<bash-binary> ci/<gate>/run.sh`) and read that result.
- A sub-3s `repo:affected-smoke` abort under a concurrent `moon ci` with a `proto-shim` line is the known EACCES flake; `moon run repo:affected-smoke --force` alone passes.
- `paigasus-auth`'s two-tab e2e (`roundtrip.spec.ts:130`) is flaky on `main` too (SMA-652).
- For any other unattributed red, follow CLAUDE.md's "Diagnosing an unattributed `moon ci` failure" procedure, and copy the evidence out of `.moon/cache` BEFORE you re-run anything.

- [ ] **Step 4: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md
git commit -m "docs(repo): name the /iam/dead-letters screen in the observability runbook (SMA-629)" -m "A Root user can list, replay and discard single dead letters in the IAM console. Bulk
replay stays API-only." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 5: Hand back the follow-ups (no code)**

Tell the controller that § 11 needs two actions outside this PR: a new Linear issue for bulk replay (`BulkReplayDeadLetters`, with a required `max_rows`) and the parked-time filter in the console, and a note on ADR-0020 in Notion that a capability key can mean "this build serves the surface", with `iam.deadletters` as the first such key.

---

## Spec coverage

| Spec section | Task |
|---|---|
| § 4.1 registry, `buf format`, regenerate | 1 |
| § 4.2 Rust `capability.rs` | 1 |
| § 4.2 IAM emission, unit and integration tests | 2 |
| § 4.3 Python smoke test | 1 |
| § 5.1 `@paigasus/proto` test | 1 |
| § 5.2 `@paigasus/discovery` vocabulary | 1 |
| § 5.3 `outbox` client, integration test, `IAM_ACTIONS`, fake IAM | 3 |
| § 5.3 dev world | 4 |
| § 6.1 nav and layout | 6 |
| § 6.2 gate | 7 |
| § 6.3 `parseEventType`, `listHref`, loader, `lib/time.ts` | 5, 7 |
| § 6.3 filter form, paging, columns, row details | 9, 10 |
| § 6.4 frame, runner, discard confirmation, commands, form schema, actions, revalidation | 5, 8, 9, 10 |
| § 6.5 `message` prop, `DEAD_LETTER_GONE` | 8, 9 |
| § 7.2 unit tests | 5, 6, 7, 8, 9, 10 |
| § 7.3 integration tests | 3, 7, 8 |
| § 7.4 e2e rows, world, R11 | 3 (`ALL_ACTIONS`), 11 |
| § 7.5 gates | 12 |
| § 8 compatibility (degraded without the key) | 6, 7, 11 (R19) |
| § 10 runbook | 12 |
| § 11 follow-ups | 12, Step 5 (hand-back only) |
