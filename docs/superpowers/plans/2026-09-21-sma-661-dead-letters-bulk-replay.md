# SMA-661 bulk replay and the parked-time filter on `/iam/dead-letters` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `/iam/dead-letters` a parked-time filter that reaches IAM and survives paging, and a
bulk-replay form, bound to that filter, whose confirmation names the exact row budget.

**Architecture:** Nothing in the backend changes. `ts/packages/paigasus-console-core` gains one
constant with a Rust drift test and one action name. `ts/apps/iam-console` gains a shared bound
parser in `lib/paging.ts`, a Timestamp helper in `lib/time.ts`, a bulk command and Server Action,
a second runner method in the frame, a new client form file, and the page wiring. The e2e world
becomes window-aware, cursor-aware and bulk-aware, and a new row R20 drives the whole flow.

**Tech Stack:** TypeScript, Next.js 16 App Router, React 19, zod 4.6.5, protobuf-es v2 message
init shapes, vitest 5 (node and jsdom), Playwright 1.63, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-21-sma-661-dead-letters-bulk-replay-design.md`
(revision 3; § 13 records six measured corrections this plan already implements). Its decisions
D1–D6 and its § 12 challenge log are settled. Do not re-open them.

## Global Constraints

- **Worktree.** Work only in
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk`
  (branch `feature/sma-661-dead-letters-bulk-replay`). Below, `$WT` means that path. Do not read
  or write the main checkout `/Users/smaschek/dev/paigasus/paigasus-core`. Do not run
  `git checkout`, `git switch`, `git stash` or any branch command.
- **PATH.** Start every shell command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`, so pnpm, node and moon resolve to the
  repo-pinned versions.
- **SPDX.** Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- **Extensionless relative imports.** Turbopack (Next 16.3.4) does not resolve a `.js` relative
  specifier to a `.ts` file (`ts/CLAUDE.md`, SMA-510). Every relative value import in `app/`,
  `lib/` and `ts/packages/*/src/` is extensionless. Test files may keep `.js`, but the existing
  test files here are extensionless, so stay extensionless.
- **Server-only boundaries.** `@paigasus/console-core`'s root entry imports `server-only`
  (`ts/packages/paigasus-console-core/src/index.ts:6`), and so does `lib/paging.ts`. A `'use client'`
  file may import them with `import type` only. A client file never imports a VALUE from them.
- **Copy.** Write every user-facing sentence EXACTLY as this plan gives it. The sentences that come
  from the spec are copied from it word for word. Do not paraphrase.
- **Commits.** Conventional commits with a workspace scope (`feat(ts): …`, `test(ts): …`,
  `docs(repo): …`). A body line is at most 100 characters. No body line starts with `#NNN`, and no
  body line looks like `token: value` (commitlint's `footer-leading-blank` rejects both). Every
  message ends, after a blank line, with
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- **Never** `git commit --no-verify`. **Never** `git commit --amend`: other commits can land between
  tasks. Add, don't amend. Commits are SSH-signed through 1Password; the error "failed to fill
  whole buffer" means 1Password is locked. Ask the coordinator; do not disable signing.
- **Formatting.** Prettier (`printWidth: 200`, single quotes, trailing commas) is its own
  whole-tree gate, `ts:fmt`, separate from lint and tsc. Run `pnpm exec prettier --write` on every
  file you touch before you commit.
- **vitest does not typecheck.** oxc strips types. Every task runs its package's `typecheck` too.
- **Mutation proofs.** A green test after red-first is not proof. Where a task says "Prove it bites",
  make the named edit, run the named test, see it FAIL, then restore the edit BY HAND to the exact
  original text. Never restore with `git checkout -- <file>`: that also deletes the uncommitted
  work under test. Each mutation below is chosen so that it still compiles.
- **React transitions.** `isPending` from `useTransition` can lag the transition's own `setState`
  by one or two macrotasks. A test that asserts a disabled button waits with `waitFor`, as
  `tests/unit/dead-letters-frame.test.tsx:189-196` does.
- **e2e.** After every `page.goto` and after every full-document navigation, call
  `waitForHydration(page)` before the first click. Playwright `waitFor*` calls have no default
  timeout here; prefer `expect(...)` and `expect.poll(...)`, which the expect timeout bounds.

### Measured commands (2026-09-21, in this worktree)

| What | Working directory | Command | Measured result |
|---|---|---|---|
| One console-core vitest file | `$WT/ts/packages/paigasus-console-core` | `pnpm exec vitest run tests/unit/action-names.test.ts` | 27 tests ran |
| Whole console-core suite | `$WT/ts/packages/paigasus-console-core` | `pnpm exec vitest run` | 23 files, 231 tests |
| One iam-console vitest file | `$WT/ts/apps/iam-console` | `pnpm exec vitest run tests/unit/paging.test.ts` | 13 tests ran |
| Whole iam-console suite | `$WT/ts/apps/iam-console` | `pnpm exec vitest run` | 50 files, 527 tests (needs the staged build below) |
| Typecheck | the package directory | `pnpm run typecheck` (runs `tsc -p tsconfig.json --noEmit`) | clean in both packages |
| Lint (whole tree, `ts:lint`) | `$WT/ts` | `pnpm exec eslint .` | clean, about 16 s |
| Prettier (whole tree, `ts:fmt`) | `$WT/ts` | `pnpm exec prettier --check .` | clean, about 2.5 s |
| Build and stage for e2e | `$WT/ts/apps/iam-console` | see "Build and stage" below | rc 0 |
| One e2e spec | `$WT/ts/apps/iam-console` | `pnpm exec playwright test tests/e2e/dead-letters.spec.ts` | R17–R19 passed |
| Whole e2e tier | `$WT/ts/apps/iam-console` | `pnpm exec playwright test` | — |
| Moon input check | `$WT` | see Task 1 Step 9 | listed `model.rs` and `action.rs` |

Do NOT use `pnpm --filter <pkg> test`: neither package has a `test` script, so it exits 0 and runs
nothing.

**Build and stage.** The e2e tier serves `.next/standalone`, and a test run needs a build of the
current code. These are the lines of `iam-console-ts:build`'s own `script:` (`moon.yml:445-464`),
without `contracts:generate`. `moon run iam-console-ts:build --force` also works, but it runs
`contracts:generate`, which calls the Buf Schema Registry, and a BSR rate limit can delete a
generated file. `public/` does not exist in this app, so there is no `public` copy.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
rm -rf .next/static && pnpm exec next build && rm -rf .next/standalone/apps/iam-console/.next/static && cp -R .next/static .next/standalone/apps/iam-console/.next/static && echo STAGED
```

Expected: the last line is `STAGED`.

---

## File structure

| File | Task | Responsibility |
|---|---|---|
| `ts/packages/paigasus-console-core/src/form.ts` | 1 | `MAX_BULK_REPLAY_ROWS` |
| `ts/packages/paigasus-console-core/src/index.ts` | 1 | re-export it |
| `ts/packages/paigasus-console-core/moon.yml` | 1 | `dead_letter.rs` as a `test` input |
| `ts/packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts` | 1 | **new**: the drift test |
| `ts/packages/paigasus-console-core/src/authorize.ts` | 2 | `ReplayOutboxDeadLetter` in `IAM_ACTIONS`, two comment blocks |
| `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts` | 2 | the name moves to the positive arm |
| `ts/apps/iam-console/tests/e2e/support/world.ts` | 2, 9 | `ALL_ACTIONS` (Task 2); the outbox handlers (Task 9) |
| `ts/apps/iam-console/lib/paging.ts` | 3 | the bound pattern, `canonicalParkedBound`, `parseParkedBound` |
| `ts/apps/iam-console/lib/time.ts` | 3 | `timestampFromIso` |
| `ts/apps/iam-console/tests/unit/paging.test.ts` | 3 | the parser cases (AC 4, unit half) |
| `ts/apps/iam-console/tests/unit/lib-time.test.ts` | 3 | `timestampFromIso` |
| `ts/apps/iam-console/app/(console)/dead-letters/load.ts` | 4 | `parkedWindow`, the window on the list request |
| `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` | 4, 6, 8 | parsers, D6 view, filter form, key, links (4); bulk action (6); `mayI`, bulk form, `canReplay` (8) |
| `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts` | 4, 8 | the page branches |
| `ts/apps/iam-console/tests/integration/dead-letters-page.test.ts` | 4 | the window on the wire |
| `ts/apps/iam-console/app/(console)/dead-letters/max-rows.ts` | 5 | **new**: the ONE `max_rows` pattern, client- and server-safe |
| `ts/apps/iam-console/app/(console)/dead-letters/commands.ts` | 5 | `parkedBoundField`, `bulkReplayForm`, the result types, `bulkReplayDeadLetters` |
| `ts/apps/iam-console/app/(console)/dead-letters/actions.ts` | 5 | `bulkReplayDeadLettersAction` |
| `ts/apps/iam-console/tests/unit/actions-structure.test.ts` | 5 | `EXPECTED` |
| `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts` | 5 | AC 1 at the action level, and the refresh rule |
| `ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts` | 5 | the command on the wire, and the form schema |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx` | 6 | `runBulk`, `bulk-answer`, `useRunner` export, `canReplay` on the row |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx` | 6 | `canReplay` passed through, the caption |
| `ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx` | 6 | the bulk arm and the row flag |
| `ts/apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx` | 7 | **new**: the form and its pure texts |
| `ts/apps/iam-console/tests/unit/bulk-replay-form.test.tsx` | 7 | **new** (AC 2) |
| `ts/apps/iam-console/tests/e2e/dead-letters.spec.ts` | 9 | R20 (AC 3, AC 4) |
| `ts/apps/iam-console/tests/unit/e2e-rows.test.ts` | 9 | 19 → 20 |
| `docs/ops/RUNBOOK-observability.md` | 10 | the "API-only" sentence |

Why the order: Tasks 1–2 change `console-core`, which every later `iam-console` task imports.
Task 2 folds in `world.ts`'s `ALL_ACTIONS`, because `world-actions.test.ts` reds the moment
`IAM_ACTIONS` gains a name; no task ends red. Task 3's parser is pure and feeds Task 4's page and
Task 5's schema. Task 6 needs Task 5's types. Task 7 needs Task 6's `useRunner`. Task 8 needs
Task 7's form. Task 9 needs everything, because it drives the built app.

---

### Task 1: The bulk-replay ceiling and its drift test (console-core)

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/form.ts:18-19`
- Modify: `ts/packages/paigasus-console-core/src/index.ts:13`
- Modify: `ts/packages/paigasus-console-core/moon.yml:69` (after the `action.rs` input)
- Create: `ts/packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `export const MAX_BULK_REPLAY_ROWS = 10_000;` from `@paigasus/console-core`
  (server-only root). Task 5 and Task 8 import it.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// MAX_BULK_REPLAY_ROWS against IAM's clamp (SMA-661 spec § 7.2, D3). IAM clamps `max_rows` to
// BulkReplayRequest::MAX_BULK_REPLAY without a signal (iam.proto:704-710), so a console ceiling
// above it would let the confirmation name a number that IAM never replays. This test reads the
// Rust constant as TEXT. moon.yml lists dead_letter.rs as an input of this package's `test` task,
// so a Rust edit selects this test (the iam-console `test` task has no /rs/** input, § 7.2).
//
// NOT VACUOUS. The type's doc comment in the same file names `MAX_BULK_REPLAY` and `(10_000)` in
// prose (dead_letter.rs:60-62), so a loose pattern reads 10_000 from the comment with the constant
// DELETED. The pattern is line-anchored on the `pub const` declaration, exactly one match is
// required, and `_` is stripped before Number(): Number('10_000') is NaN.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { MAX_BULK_REPLAY_ROWS } from '../../src/form';

// tests/unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five `../`, the same
// depth as tests/unit/action-names.test.ts.
const REPO_ROOT = new URL('../../../../../', import.meta.url);
const DEAD_LETTER_RS = 'rs/crates/libs/paigasus-iam-core/src/dead_letter.rs';
const DECLARATION = /^\s*pub const MAX_BULK_REPLAY: u64 = ([0-9_]+);/gm;

describe('MAX_BULK_REPLAY_ROWS against the Rust clamp', () => {
  const matches = [...readFileSync(fileURLToPath(new URL(DEAD_LETTER_RS, REPO_ROOT)), 'utf8').matchAll(DECLARATION)];

  it('finds exactly one declaration of the constant', () => {
    expect(matches).toHaveLength(1);
  });

  it('equals the Rust value', () => {
    const value = Number((matches[0]?.[1] ?? '').replaceAll('_', ''));
    expect(Number.isInteger(value)).toBe(true);
    expect(value).toBeGreaterThan(0);
    expect(MAX_BULK_REPLAY_ROWS).toBe(value);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/packages/paigasus-console-core
pnpm exec vitest run tests/unit/bulk-replay-ceiling.test.ts
```

Expected: FAIL. `equals the Rust value` fails with `expected undefined to be 10000`. (`finds
exactly one declaration` already passes.)

- [ ] **Step 3: Add the constant**

In `ts/packages/paigasus-console-core/src/form.ts`, directly after line 19
(`export const NAME_MAX_CODE_POINTS = 256;`), add:

```ts

/**
 * The largest bulk-replay budget the console sends (SMA-661 spec § 6.5, D3). IAM clamps `max_rows`
 * to BulkReplayRequest::MAX_BULK_REPLAY (paigasus-iam-core dead_letter.rs) and says nothing, so a
 * larger budget would make the confirmation name a number that IAM never replays. The console
 * refuses it instead. tests/unit/bulk-replay-ceiling.test.ts holds this value to the Rust constant.
 */
export const MAX_BULK_REPLAY_ROWS = 10_000;
```

In `ts/packages/paigasus-console-core/src/index.ts`, replace line 13:

```ts
export { NAME_MAX_CODE_POINTS, formFields, invalidFormInput, nameField, prnField, toActionResult, type ActionResult, type FormAction } from './form';
```

with:

```ts
export { MAX_BULK_REPLAY_ROWS, NAME_MAX_CODE_POINTS, formFields, invalidFormInput, nameField, prnField, toActionResult, type ActionResult, type FormAction } from './form';
```

- [ ] **Step 4: Run the test to verify it passes**

Same command as Step 2. Expected: 2 passed.

- [ ] **Step 5: Prove it bites — delete the Rust constant (TEMPORARY Rust edit, never commit it)**

In `$WT/rs/crates/libs/paigasus-iam-core/src/dead_letter.rs`, change line 77

```rust
    pub const MAX_BULK_REPLAY: u64 = 10_000;
```

to

```rust
    // pub const MAX_BULK_REPLAY: u64 = 10_000;
```

Run the Step 2 command. Expected: FAIL, both cases. `finds exactly one declaration` reports
`expected [] to have a length of 1 but got +0`. This proves the doc comment's prose at lines
60-62 does not satisfy the pattern.

Restore line 77 by hand to exactly `    pub const MAX_BULK_REPLAY: u64 = 10_000;` (four spaces).

- [ ] **Step 6: Prove it bites — change the Rust value**

Change `10_000` on line 77 to `10_001`. Run the Step 2 command. Expected: FAIL in
`equals the Rust value` (`expected 10000 to be 10001`). Restore `10_000` by hand.

- [ ] **Step 7: Prove it bites — drop the `_` strip**

In the test, change `.replaceAll('_', '')` to `.replaceAll('#', '')`. Run the Step 2 command.
Expected: FAIL in `equals the Rust value` (`expected false to be true`, because
`Number('10_000')` is `NaN`). Restore `.replaceAll('_', '')` by hand.

- [ ] **Step 8: Confirm the Rust file is back to its committed state**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk diff --stat -- rs/
```

Expected: NO output. If there is output, the Rust restore is wrong. Fix it by hand; do not commit
any Rust change.

- [ ] **Step 9: Add the Moon input and confirm it resolves**

In `ts/packages/paigasus-console-core/moon.yml`, after the line
`      - '/rs/crates/libs/paigasus-iam-core/src/authz/action.rs'` (the last line of the `test`
task's `inputs`), add:

```yaml
      # SMA-661 spec § 7.2. tests/unit/bulk-replay-ceiling.test.ts reads the bulk-replay clamp as TEXT.
      # Without this input, a changed Rust clamp selects no paigasus-console-core-ts task, and Moon
      # serves a cached PASS on exactly the PR that breaks the ceiling. The iam-console `test` task
      # cannot carry it instead: it has no /rs/** input, and adding one would rebuild the console on
      # every dead_letter.rs edit, because that task depends on `~:build`.
      - '/rs/crates/libs/paigasus-iam-core/src/dead_letter.rs'
```

Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
moon task paigasus-console-core-ts:test --json 2>/dev/null | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{console.log(Object.keys(JSON.parse(s).inputFiles).filter(f=>f.startsWith("rs/")))})'
```

Expected: an array with THREE entries, `model.rs`, `action.rs` and
`rs/crates/libs/paigasus-iam-core/src/dead_letter.rs`.

- [ ] **Step 10: Typecheck, format, and run the package suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write packages/paigasus-console-core/src/form.ts packages/paigasus-console-core/src/index.ts packages/paigasus-console-core/moon.yml packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts
cd packages/paigasus-console-core && pnpm run typecheck && pnpm exec vitest run
```

Expected: tsc prints nothing after its command line; vitest reports 24 files passed.

- [ ] **Step 11: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add ts/packages/paigasus-console-core/src/form.ts ts/packages/paigasus-console-core/src/index.ts ts/packages/paigasus-console-core/moon.yml ts/packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts
git status --short -- rs/
git commit -m "feat(ts): pin the bulk-replay ceiling to the IAM clamp (SMA-661)" -m "Add MAX_BULK_REPLAY_ROWS to @paigasus/console-core and a drift test that reads the Rust
constant as text. The test task now lists dead_letter.rs as an input, so a Rust edit
selects it." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

Expected: `git status --short -- rs/` prints nothing before the commit.

---

### Task 2: Ask `mayI` about `ReplayOutboxDeadLetter` (console-core and the e2e world's list)

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/authorize.ts:37-40, 59-62`
- Modify: `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts:62-69`
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts:68-69`

**Interfaces:**
- Consumes: nothing.
- Produces: `'ReplayOutboxDeadLetter'` is a member of `IAM_ACTIONS`, so `IamAction` accepts it and
  `may('ReplayOutboxDeadLetter', ROOT_PRN)` compiles (Task 8). The e2e world allows it by default
  (Task 9's R17 and R20 click Replay and bulk replay).

- [ ] **Step 1: Write the failing test**

In `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts`, replace lines 62-69 (the
comment and the last `it`) with:

```ts
  // SMA-629 spec § 5.3 and SMA-661 spec § 8. The IAM console's layout asks ListOutboxDeadLetters at
  // Root to show the Dead letters entry, and the dead-letters page asks ReplayOutboxDeadLetter at
  // Root to show the bulk-replay form and the row Replay buttons (bulk replay has no Cedar action of
  // its own). Discard has no mayI caller: it is a separate Cedar action, and IAM decides each
  // discard anyway.
  it('holds ListOutboxDeadLetters and ReplayOutboxDeadLetter, and not DiscardOutboxDeadLetter', () => {
    expect([...IAM_ACTIONS]).toContain('ListOutboxDeadLetters');
    expect([...IAM_ACTIONS]).toContain('ReplayOutboxDeadLetter');
    expect([...IAM_ACTIONS]).not.toContain('DiscardOutboxDeadLetter');
  });
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/packages/paigasus-console-core
pnpm exec vitest run tests/unit/action-names.test.ts
```

Expected: FAIL in `holds ListOutboxDeadLetters and ReplayOutboxDeadLetter…`
(`expected [ …(22) ] to include 'ReplayOutboxDeadLetter'`).

- [ ] **Step 3: Add the name and rewrite both comment blocks**

In `ts/packages/paigasus-console-core/src/authorize.ts`, replace lines 37-40:

```ts
 * SMA-629 added ListOutboxDeadLetters, the Dead letters nav entry's question. ReplayOutboxDeadLetter
 * and DiscardOutboxDeadLetter are deliberately ABSENT: they are separate Cedar actions, but the
 * console asks no mayI() question about either one — the buttons show for every user who reaches
 * the page, and IAM decides each action anyway (the UI does not pre-judge, SMA-511 spec § 6.3).
```

with:

```ts
 * SMA-629 added ListOutboxDeadLetters, the Dead letters nav entry's question. SMA-661 added
 * ReplayOutboxDeadLetter: the dead-letters page asks it at Root to show the bulk-replay form and
 * every row's Replay button (SMA-661 spec D4). Bulk replay has no Cedar action of its own; IAM checks
 * ReplayOutboxDeadLetter for it. The answer is cosmetic and fails open, like every answer here.
 * DiscardOutboxDeadLetter stays deliberately ABSENT: it is a separate Cedar action, the console asks
 * no mayI() question about it, and IAM decides each discard anyway (the UI does not pre-judge,
 * SMA-511 spec § 6.3).
```

Replace lines 59-62:

```ts
  // SMA-629: the Dead letters nav entry asks this at Root. Replay and discard are deliberately
  // ABSENT: they are separate Cedar actions, but the console asks no mayI() question about
  // either one — every button shows, and IAM decides each action anyway.
  'ListOutboxDeadLetters',
```

with:

```ts
  // SMA-629: the Dead letters nav entry asks this at Root.
  'ListOutboxDeadLetters',
  // SMA-661: the dead-letters page asks this at Root, for the bulk form and the row Replay buttons.
  // DiscardOutboxDeadLetter is deliberately ABSENT: the console asks no question about discard.
  'ReplayOutboxDeadLetter',
```

- [ ] **Step 4: Run the test to verify it passes**

Same command as Step 2. Expected: 28 passed (27 before; the `%s is a wire name` row for
`ReplayOutboxDeadLetter` is new and passes, because `action.rs:152` has that arm).

- [ ] **Step 5: See the iam-console guard go red, then fix the world**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/world-actions.test.ts
```

Expected: FAIL (`holds the same set as IAM_ACTIONS`). This is the red that spec § 7.1 predicts.

In `ts/apps/iam-console/tests/e2e/support/world.ts`, replace lines 68-69:

```ts
  // SMA-629: the Dead letters nav entry.
  'ListOutboxDeadLetters',
```

with:

```ts
  // SMA-629: the Dead letters nav entry.
  'ListOutboxDeadLetters',
  // SMA-661: the replay affordance of the dead-letters page. Without it the default world DENIES
  // replay, the page hides every Replay button, and R17 fails at its Replay click.
  'ReplayOutboxDeadLetter',
```

Run the same command. Expected: 1 passed.

- [ ] **Step 6: Typecheck and format**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write packages/paigasus-console-core/src/authorize.ts packages/paigasus-console-core/tests/unit/action-names.test.ts apps/iam-console/tests/e2e/support/world.ts
(cd packages/paigasus-console-core && pnpm run typecheck) && (cd apps/iam-console && pnpm run typecheck)
```

Expected: both tsc runs print nothing after their command line.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add ts/packages/paigasus-console-core/src/authorize.ts ts/packages/paigasus-console-core/tests/unit/action-names.test.ts ts/apps/iam-console/tests/e2e/support/world.ts
git commit -m "feat(ts): ask mayI about ReplayOutboxDeadLetter (SMA-661)" -m "IAM_ACTIONS gains ReplayOutboxDeadLetter, and the e2e world allows it by default, so
world-actions.test.ts stays green. Discard stays out of the list." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: The parked-bound parser and the Timestamp helper

**Files:**
- Modify: `ts/apps/iam-console/lib/paging.ts:1-6` (header) and after `:127` (after `parseEventType`)
- Modify: `ts/apps/iam-console/lib/time.ts` (append)
- Test: `ts/apps/iam-console/tests/unit/paging.test.ts`, `ts/apps/iam-console/tests/unit/lib-time.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces (all server-only):
  - `export const MAX_PARKED_BOUND_LENGTH = 40;`
  - `export const PARKED_BOUND_PATTERN: RegExp;`
  - `export function canonicalParkedBound(value: string): string | null;` — `value` is already
    trimmed and non-empty; the answer is `new Date(ms).toISOString()` or `null`.
  - `export function standardInstant(value: string): string | null;` — the value rewritten in
    ECMAScript's standard Date Time String Format, the only string `Date.parse` ever reads (spec § 13
    item 2); exported for its unit test.
  - `export type ParkedBoundField = 'parkedFrom' | 'parkedTo';`
  - `export type ParsedParkedBound = { readonly ok: true; readonly raw: string; readonly iso: string } | { readonly ok: false; readonly raw: string; readonly error: PaigasusError };`
  - `export function parseParkedBound(raw: string | readonly string[] | undefined, field: ParkedBoundField): ParsedParkedBound;`
  - `lib/time.ts`: `export function timestampFromIso(iso: string): ProtoTimestamp;`

**Choice (the spec leaves it open):** the spec says the GET parser and the POST field share
`PARKED_BOUND_PATTERN`. This plan shares the WHOLE check instead, `canonicalParkedBound`, so the two
cannot drift in any of the three checks.

**Deviation (the real runtime contradicts the spec):** spec § 4.1 says `Date.parse` refuses
`2026-02-30T00:00:00Z`. MEASURED on Node 24.18.1: `Date.parse('2026-02-30T00:00:00Z')` is
`1772409600000`, which is 2026-03-02. So the spec's two checks ACCEPT a day that does not exist and
silently filter on another day. `canonicalParkedBound` adds a third check, `isCalendarDay`, which
round-trips the date through `Date.UTC`. A test pins the measured V8 behaviour, so the reason for
the check stays visible.

- [ ] **Step 1: Write the failing tests**

In `ts/apps/iam-console/tests/unit/paging.test.ts`, replace line 3 with:

```ts
import {
  MAX_CURSOR_LENGTH,
  MAX_EVENT_TYPE_LENGTH,
  MAX_PARKED_BOUND_LENGTH,
  PAGE_SIZE,
  PARKED_BOUND_PATTERN,
  canonicalParkedBound,
  listHref,
  nextOffset,
  pageHref,
  parseCursor,
  parseEventType,
  parseOffset,
  parseParkedBound,
  standardInstant,
} from '../../lib/paging';
```

Insert this block after the `parseEventType` describe (after line 94, before `describe('listHref'`):

```ts
// SMA-661 spec § 4.1. A parked-time bound: an ISO instant WITH a zone, in four more spellings (D6).
// '' is no filter. A refused bound echoes the typed value, so the filter form can keep it (D6).
describe('parseParkedBound', () => {
  const CANONICAL = '2026-09-19T00:00:00.000Z';

  it('reads no value, an empty value and blanks as no filter', () => {
    for (const raw of [undefined, '', '   ']) expect(parseParkedBound(raw, 'parkedFrom')).toEqual({ ok: true, raw: '', iso: '' });
  });

  it.each([
    ['the canonical Z form', '2026-09-19T00:00:00Z', CANONICAL],
    ['a lower-case z', '2026-09-19T00:00:00z', CANONICAL],
    ['a space instead of T', '2026-09-19 00:00:00Z', CANONICAL],
    ['no seconds', '2026-09-19T00:00Z', CANONICAL],
    ['fractional seconds, cut to milliseconds', '2026-09-19T00:00:00.123456789Z', '2026-09-19T00:00:00.123Z'],
    ['a +02:00 offset', '2026-09-19T02:00:00+02:00', CANONICAL],
    ['the longest legal value, 35 characters', '2026-09-19T02:00:00.123456789+02:00', '2026-09-19T00:00:00.123Z'],
  ])('accepts %s and normalises it', (_label, raw, iso) => {
    expect(parseParkedBound(raw, 'parkedFrom')).toEqual({ ok: true, raw, iso });
  });

  it('reads the first value, trims it, and echoes the TRIMMED value as raw', () => {
    expect(parseParkedBound(['2026-09-19T00:00:00Z', 'not a time'], 'parkedTo')).toEqual({ ok: true, raw: '2026-09-19T00:00:00Z', iso: CANONICAL });
    expect(parseParkedBound('  2026-09-19T00:00:00Z  ', 'parkedTo')).toEqual({ ok: true, raw: '2026-09-19T00:00:00Z', iso: CANONICAL });
  });

  it.each([
    ['a basic-format offset', '2026-09-19T00:00:00+0200'],
    ['no zone', '2026-09-19T00:00:00'],
    ['a date only', '2026-09-19'],
    ['a day that does not exist', '2026-02-30T00:00:00Z'],
    ['a minute that does not exist', '2026-09-19T00:60:00Z'],
    ['words', 'not a time'],
    ['41 characters', '9'.repeat(41)],
  ])('refuses %s, echoes it back, and never reached IAM', (_label, raw) => {
    const parsed = parseParkedBound(raw, 'parkedFrom');

    expect(parsed.ok).toBe(false);
    if (parsed.ok) throw new Error('expected a refused bound');
    expect(parsed.raw).toBe(raw);
    expect(parsed.error.presentation).toBe('invalid-input');
    expect(parsed.error.correlationId).toBeNull();
    expect(parsed.error.reason).toBeNull();
  });

  it('names the field in the sentence', () => {
    const from = parseParkedBound('2026-09-19', 'parkedFrom');
    const to = parseParkedBound('2026-09-19', 'parkedTo');

    if (from.ok || to.ok) throw new Error('expected two refused bounds');
    expect(from.error.message).toBe('The "parked from" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.');
    expect(to.error.message).toBe('The "parked to" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.');
  });

  // THE REASON FOR THE CALENDAR CHECK, measured on Node 24 (V8). The pattern accepts 2026-02-30, and
  // so does Date.parse: it reads the value as 2026-03-02. The spec expected Date.parse to refuse it.
  it('does not trust Date.parse with a day that does not exist', () => {
    expect(PARKED_BOUND_PATTERN.test('2026-02-30T00:00:00Z')).toBe(true);
    expect(new Date('2026-02-30T00:00:00Z').toISOString()).toBe('2026-03-02T00:00:00.000Z');
    expect(canonicalParkedBound('2026-02-30T00:00:00Z')).toBeNull();
  });

  // Date.parse sees only the standard format. V8 happens to accept every D6 spelling raw as well, so
  // canonicalParkedBound's output cannot show which string it parsed; this case pins the rewrite.
  it.each([
    ['2026-09-19T00:00:00Z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19 00:00:00Z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19T00:00Z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19T00:00:00z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19T00:00:00.5+02:00', '2026-09-19T00:00:00.500+02:00'],
    ['2026-09-19T00:00:00.123456789Z', '2026-09-19T00:00:00.123Z'],
  ])('rewrites %s as the standard %s before Date.parse reads it', (value, standard) => {
    expect(standardInstant(value)).toBe(standard);
  });

  it('rewrites nothing the pattern refuses', () => {
    expect(standardInstant('2026-09-19T00:00:00')).toBeNull();
    expect(standardInstant('2026-09-19T00:00:00+0200')).toBeNull();
  });

  it('bounds the length at 40, above the 35 characters of the longest legal value', () => {
    expect(MAX_PARKED_BOUND_LENGTH).toBe(40);
    expect('2026-09-19T02:00:00.123456789+02:00'.length).toBe(35);
  });
});
```

Inside `describe('listHref', …)`, after the `encodes the values and keeps their order` case, add:

```ts

  it('carries the canonical parked bounds and leaves an empty one out (SMA-661 § 4.5)', () => {
    expect(listHref('/iam/dead-letters', { eventType: '', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '' })).toBe('/iam/dead-letters?parkedFrom=2026-09-19T00%3A00%3A00.000Z');
    expect(listHref('/iam/dead-letters', { eventType: 'a', parkedFrom: '', parkedTo: '2026-09-20T00:00:00.000Z', cursor: 'c2' })).toBe(
      '/iam/dead-letters?eventType=a&parkedTo=2026-09-20T00%3A00%3A00.000Z&cursor=c2',
    );
  });
```

In `ts/apps/iam-console/tests/unit/lib-time.test.ts`, replace line 6 with
`import { timestampFromIso, timestampIso } from '../../lib/time';` and append:

```ts

// SMA-661 spec § 4.4. The request side: a canonical instant as a protobuf Timestamp.
describe('timestampFromIso', () => {
  it('is the inverse of timestampIso for a canonical instant', () => {
    expect(timestampFromIso('2026-09-19T00:00:00.250Z')).toEqual({ seconds: 1_789_776_000n, nanos: 250_000_000 });
    expect(timestampIso(timestampFromIso('2026-09-19T00:00:00.250Z'))).toBe('2026-09-19T00:00:00.250Z');
  });

  it('floors the seconds before the epoch, so the nanos stay non-negative', () => {
    expect(timestampFromIso('1969-12-31T23:59:59.500Z')).toEqual({ seconds: -1n, nanos: 500_000_000 });
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/paging.test.ts tests/unit/lib-time.test.ts
```

Expected: FAIL. `parseParkedBound is not a function` and `timestampFromIso is not a function`.
The new `listHref` case already passes (the helper is generic); that is expected.

- [ ] **Step 3: Implement the parser**

In `ts/apps/iam-console/lib/paging.ts`, replace lines 3-4 of the header:

```ts
// Offset paging for the tenancy lists, cursor paging for the audit and dead-letters lists, and the
// dead-letters filter (spec § 5.2; SMA-629 spec § 6.3). IAM's tenancy list responses carry no
```

with:

```ts
// Offset paging for the tenancy lists, cursor paging for the audit and dead-letters lists, and the
// dead-letters filters (spec § 5.2; SMA-629 spec § 6.3; SMA-661 spec § 4.1). IAM's tenancy list responses carry no
```

After `parseEventType` (after line 127, before the `listHref` doc comment), insert:

```ts

/**
 * The longest parked-time bound the console reads, in characters (SMA-661 spec § 4.1). The longest
 * value PARKED_BOUND_PATTERN accepts, `2026-09-19T00:00:00.123456789+02:00`, has 35, so the bound
 * refuses nothing legal; it only keeps an attacker-controlled query string away from the pattern.
 * The filter input carries it as `maxLength`, which bounds TYPING only: the parser never cuts a value.
 */
export const MAX_PARKED_BOUND_LENGTH = 40;

/**
 * An ISO 8601 instant WITH a zone (SMA-661 D2, D6). It also accepts a space instead of `T`, no
 * seconds, a lower-case `z`, and up to nine fractional digits. A value with no zone is refused: the
 * console would have to guess the zone, and the guess would differ from what the operator saw. The
 * basic-format offset `+0200` is refused too, because the ECMAScript Date Time String Format does
 * not define it.
 */
export const PARKED_BOUND_PATTERN = /^(\d{4}-\d{2}-\d{2})[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,9}))?)?(Z|z|[+-]\d{2}:\d{2})$/;

/**
 * The bound rewritten in ECMAScript's own Date Time String Format, `YYYY-MM-DDTHH:mm:ss.sssZ` or
 * `…±HH:mm`, built from the pattern's parts; null when the pattern refuses the value. `Date.parse`
 * reads only THIS string, never the operator's spelling. A space separator, a lower-case `z`, omitted
 * seconds and more than three fraction digits all lie outside that format, and `Date.parse` accepts
 * them only through implementation-specific behaviour — the same reason the basic offset `+0200` is
 * refused. Digits past the millisecond are dropped: the canonical instant has millisecond precision.
 */
export function standardInstant(value: string): string | null {
  const match = PARKED_BOUND_PATTERN.exec(value);
  if (match === null) return null;
  const [, date = '', hour = '', minute = '', second = '00', fraction = '', zone = ''] = match;
  return `${date}T${hour}:${minute}:${second}.${fraction.padEnd(3, '0').slice(0, 3)}${zone.toUpperCase()}`;
}

const PARKED_DATE = /^(\d{4})-(\d{2})-(\d{2})/;

/**
 * Whether the date part names a real calendar day. MEASURED on Node 24 (V8): `Date.parse` ACCEPTS
 * `2026-02-30T00:00:00Z` and reads it as 2026-03-02, so a finite `Date.parse` does not prove that the
 * day exists. Without this check, a typo would silently filter on another day.
 */
function isCalendarDay(value: string): boolean {
  const match = PARKED_DATE.exec(value);
  if (match === null) return false;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(Date.UTC(year, month - 1, day));
  return date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
}

/**
 * The canonical instant of a trimmed, non-empty bound, or null when the console refuses it. ONE
 * function for the GET filter (`parseParkedBound`) and the bulk form's POST field (commands.ts's
 * `parkedBoundField`), so the two cannot accept different values. Each check refuses something the
 * others let through: the pattern refuses a value with no zone, which `Date.parse` reads as LOCAL
 * time; `isCalendarDay` refuses a day that does not exist; `Date.parse` refuses a time that does not
 * exist (`00:60`).
 */
export function canonicalParkedBound(value: string): string | null {
  if (value.length > MAX_PARKED_BOUND_LENGTH || !isCalendarDay(value)) return null;
  const standard = standardInstant(value);
  if (standard === null) return null;
  const ms = Date.parse(standard);
  return Number.isFinite(ms) ? new Date(ms).toISOString() : null;
}

/** Which parked-time input a bound came from. It only selects the error sentence. */
export type ParkedBoundField = 'parkedFrom' | 'parkedTo';

/**
 * What `parseParkedBound` answers. `raw` is the trimmed value the operator typed, so a refused bound
 * keeps it in the input (D6). `iso` is the canonical instant: the paging links, the bulk form and the
 * confirmation use it, so a link is stable. An empty filter is `{ ok: true, raw: '', iso: '' }`.
 */
export type ParsedParkedBound = { readonly ok: true; readonly raw: string; readonly iso: string } | { readonly ok: false; readonly raw: string; readonly error: PaigasusError };

const PARKED_BOUND_LABEL: Readonly<Record<ParkedBoundField, string>> = { parkedFrom: 'parked from', parkedTo: 'parked to' };

/** A bound the console refuses. Like `cursorTooLong`, it never reached IAM. */
function parkedBoundInvalid(field: ParkedBoundField): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: `The "${PARKED_BOUND_LABEL[field]}" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.`,
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}

/**
 * A parked-time filter (SMA-661 spec § 4.1): the first value, trimmed. `''` is no filter. Any other
 * value must pass `canonicalParkedBound`. A refused value is REPORTED, never dropped, for the same
 * reason as `parseCursor`: a quiet change would show the operator another list than the one they
 * asked for.
 */
export function parseParkedBound(raw: string | readonly string[] | undefined, field: ParkedBoundField): ParsedParkedBound {
  const value = (first(raw) ?? '').trim();
  if (value === '') return { ok: true, raw: '', iso: '' };
  const iso = canonicalParkedBound(value);
  return iso === null ? { ok: false, raw: value, error: parkedBoundInvalid(field) } : { ok: true, raw: value, iso };
}
```

Append to `ts/apps/iam-console/lib/time.ts`:

```ts

/**
 * The inverse of `timestampIso`, for a request (SMA-661 spec § 4.4). `iso` is a value that
 * lib/paging.ts's `canonicalParkedBound` already accepted, so `Date.parse` is finite. The seconds are
 * floored, so an instant before the epoch keeps non-negative nanos, as the Timestamp type requires.
 * The spec named protobuf-es's `timestampFromDate`; this app does not depend on `@bufbuild/protobuf`,
 * and a plain `{ seconds, nanos }` is a valid message init for a Timestamp field.
 */
export function timestampFromIso(iso: string): ProtoTimestamp {
  const ms = Date.parse(iso);
  const seconds = Math.floor(ms / 1000);
  return { seconds: BigInt(seconds), nanos: (ms - seconds * 1000) * 1_000_000 };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Same command as Step 2. Expected: all pass.

- [ ] **Step 5: Prove the calendar check bites**

In `lib/paging.ts`, in `canonicalParkedBound`, change `|| !isCalendarDay(value)) return null;` to
`|| (!isCalendarDay(value) && false)) return null;` (it still compiles and still reads the helper).
Run the Step 2 command. Expected: FAIL in `refuses a day that does not exist…` and in
`does not trust Date.parse…`. Restore the exact original text `|| !isCalendarDay(value)) return null;`.

- [ ] **Step 6: Prove the zone requirement bites**

In `PARKED_BOUND_PATTERN`, change the ending `(Z|z|[+-]\d{2}:\d{2})$/` to
`(Z|z|[+-]\d{2}:?\d{2})?$/`. Run the Step 2 command. Expected: FAIL in the `no zone`, `a date
only` and `a basic-format offset` rows. Restore the exact ending `(Z|z|[+-]\d{2}:\d{2})$/`.

- [ ] **Step 7: Typecheck, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write apps/iam-console/lib/paging.ts apps/iam-console/lib/time.ts apps/iam-console/tests/unit/paging.test.ts apps/iam-console/tests/unit/lib-time.test.ts
cd apps/iam-console && pnpm run typecheck && pnpm exec vitest run tests/unit/paging.test.ts tests/unit/lib-time.test.ts
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add ts/apps/iam-console/lib/paging.ts ts/apps/iam-console/lib/time.ts ts/apps/iam-console/tests/unit/paging.test.ts ts/apps/iam-console/tests/unit/lib-time.test.ts
git commit -m "feat(ts): parse the parked-time filter bounds (SMA-661)" -m "parseParkedBound accepts an ISO instant with a zone in five spellings and normalises it.
A calendar check refuses a day that does not exist, because V8's Date.parse accepts
2026-02-30. timestampFromIso turns a canonical instant into a request Timestamp." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The parked-time filter on the page and the loader (AC 3, unit and integration)

**Files:**
- Modify: `ts/apps/iam-console/app/(console)/dead-letters/load.ts`
- Modify (whole file): `ts/apps/iam-console/app/(console)/dead-letters/page.tsx`
- Test: `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts`
- Test: `ts/apps/iam-console/tests/integration/dead-letters-page.test.ts`

**Interfaces:**
- Consumes: Task 3's `parseParkedBound`, `ParsedParkedBound`, `ParkedBoundField`,
  `MAX_PARKED_BOUND_LENGTH`, `timestampFromIso`, `ProtoTimestamp`.
- Produces:
  - `load.ts`: `export type ParkedWindow = { readonly parkedFrom: string; readonly parkedTo: string };`
  - `load.ts`: `export function parkedWindow(bounds: ParkedWindow): { parkedFrom?: ProtoTimestamp; parkedTo?: ProtoTimestamp };` — Task 5's command uses it.
  - `loadDeadLettersPage(deps, params: { readonly cursor: string; readonly eventType: string } & ParkedWindow)` — the two new fields are REQUIRED.
  - The frame key is `${eventType}|${parkedFromIso}|${parkedToIso}|${cursor}`.
  - The filter inputs are named `eventType`, `parkedFrom`, `parkedTo`, with `Field` ids
    `dead-letters-event-type`, `dead-letters-parked-from`, `dead-letters-parked-to` and labels
    `Event type`, `Parked from`, `Parked to` (R20 finds them by label).

- [ ] **Step 1: Write the failing unit tests**

In `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts`:

(a) Replace the header comment (lines 3-7) with:

```ts
// /iam/dead-letters, branch by branch (SMA-629 spec § 6.2–§ 6.4; SMA-661 spec § 4.2–§ 4.5). The
// page's own accessors are mocked at the lib/console boundary, and the test reads the element tree
// the page returns. It proves: the 404 gate, the degraded view inside the frame with no IAM call, a
// refused query that never becomes an IAM call, a refused parked bound that re-renders the filter
// form (D6), the 403/404 list errors as PageError, every other list error as a SectionError inside
// the frame, the frame key, the GET form with no `action`, and the paging hrefs.
```

(b) After line 12 (`import type { PaigasusError } …`), add:

```ts
import { Field, Input } from '@paigasus/ui';
```

(c) In `the list` describe, change `expect(frame?.key).toBe('orders|c1');` to
`expect(frame?.key).toBe('orders|||c1');`, and change
`expect(all(tree, DeadLettersFrame)[0]?.key).toBe('|');` to
`expect(all(tree, DeadLettersFrame)[0]?.key).toBe('|||');`. Rename the second test's title from
`… and keys the frame "|"` to `… and keys the frame "|||"`.

(d) Append at the end of the file:

```ts

describe('the parked-time filter (SMA-661 § 4.2–§ 4.5)', () => {
  beforeEach(() => {
    mocks.state.current = WITH_KEY;
  });

  it('sends the CANONICAL bounds, keys the frame by them, keeps them in both paging links, and echoes the typed values', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [entry], nextCursor: 'c2' });

    const tree = await visit({ eventType: 'orders', parkedFrom: '2026-09-19 02:00+02:00', parkedTo: '2026-09-20T00:00:00z', cursor: 'c1' });

    expect(mocks.listDeadLetters).toHaveBeenCalledWith({
      eventType: 'orders',
      cursor: 'c1',
      limit: PAGE_SIZE,
      parkedFrom: { seconds: 1_789_776_000n, nanos: 0 },
      parkedTo: { seconds: 1_789_862_400n, nanos: 0 },
    });
    expect(all(tree, DeadLettersFrame)[0]?.key).toBe('orders|2026-09-19T00:00:00.000Z|2026-09-20T00:00:00.000Z|c1');

    const bounds = 'parkedFrom=2026-09-19T00%3A00%3A00.000Z&parkedTo=2026-09-20T00%3A00%3A00.000Z';
    const hrefs = [...walk(tree)].map((element) => (element.props as { href?: unknown }).href).filter((href) => typeof href === 'string');
    expect(hrefs).toEqual([`/iam/dead-letters?eventType=orders&${bounds}`, `/iam/dead-letters?eventType=orders&${bounds}&cursor=c2`]);

    // D6: the inputs show what the operator typed. Following a link converges them on the canonical form.
    const inputs = all(tree, Input).map((element) => element.props as { name: string; defaultValue: string });
    expect(inputs.find((input) => input.name === 'parkedFrom')?.defaultValue).toBe('2026-09-19 02:00+02:00');
    expect(inputs.find((input) => input.name === 'parkedTo')?.defaultValue).toBe('2026-09-20T00:00:00z');
  });

  it('gives each parked input the example, the typing bound and the inclusive-bounds help (§ 4.3)', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [], nextCursor: '' });

    const tree = await visit({});

    for (const name of ['parkedFrom', 'parkedTo']) {
      const input = all(tree, Input).find((element) => (element.props as { name: string }).name === name);
      expect(input?.props).toMatchObject({ placeholder: '2026-09-19T00:00:00Z', maxLength: 40, autoComplete: 'off', defaultValue: '' });
    }
    const fields = all(tree, Field).map((element) => element.props as { htmlFor: string; label: string; description?: string });
    expect(fields.map((field) => [field.htmlFor, field.label])).toEqual([
      ['dead-letters-event-type', 'Event type'],
      ['dead-letters-parked-from', 'Parked from'],
      ['dead-letters-parked-to', 'Parked to'],
    ]);
    for (const field of fields.slice(1)) expect(field.description).toBe('Both bounds are included. Use a zone, for example 2026-09-19T00:00:00Z or +02:00.');
  });

  it.each<[string, Record<string, string>, string, string]>([
    ['parkedFrom', { parkedFrom: '2026-09-19T00:00:00' }, 'dead-letters-parked-from', 'parked from'],
    ['parkedTo', { parkedTo: '2026-02-30T00:00:00Z' }, 'dead-letters-parked-to', 'parked to'],
  ])('D6: a refused %s re-renders the filter form with the typed value and one field error, and makes no IAM call', async (name, query, fieldId, words) => {
    const tree = await visit({ eventType: 'orders', ...query });

    expect(mocks.iamClients).not.toHaveBeenCalled();
    expect(tree.type).not.toBe(PageError);
    expect(all(tree, DeadLettersFrame)).toHaveLength(1);
    expect(all(tree, DeadLetterTable)).toHaveLength(0);
    expect(all(tree, SectionError)).toHaveLength(0);

    const inputs = all(tree, Input).map((element) => element.props as { name: string; defaultValue: string });
    expect(inputs.find((input) => input.name === 'eventType')?.defaultValue).toBe('orders');
    expect(inputs.find((input) => input.name === name)?.defaultValue).toBe(query[name]);

    const fields = all(tree, Field).map((element) => element.props as { htmlFor: string; error?: string });
    expect(fields.find((field) => field.htmlFor === fieldId)?.error).toBe(`The "${words}" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.`);
    expect(fields.filter((field) => field.error !== undefined)).toHaveLength(1);
  });
});
```

- [ ] **Step 2: Write the failing integration test**

In `ts/apps/iam-console/tests/integration/dead-letters-page.test.ts`, the four existing calls pass
`{ cursor: …, eventType: … }`. Add `parkedFrom: '', parkedTo: ''` to each one (lines 46, 78, 88,
101), for example line 46 becomes:

```ts
    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: 'cursor-1', eventType: 'iam.team.created', parkedFrom: '', parkedTo: '' });
```

and lines 78, 88 and 101 each become:

```ts
    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '', parkedFrom: '', parkedTo: '' });
```

Replace the header comment's first line `// ListDeadLetters (SMA-629 spec § 6.3, § 7.3) through the fake IAM: every field of the row, IAM's`
with `// ListDeadLetters (SMA-629 spec § 6.3, § 7.3; SMA-661 spec § 7.3) through the fake IAM: every field of the row, IAM's`.

Inside `describe('loadDeadLettersPage', …)`, after the first `it`, add:

```ts

  it('sends the canonical parked bounds as Timestamps, and leaves an empty bound out (SMA-661 § 4.4)', async () => {
    iam.setHandlers({ 'outbox.listDeadLetters': () => ({ entries: [], nextCursor: '' }) });
    const calls = callsSince(iam);

    await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '', parkedFrom: '2026-09-19T00:00:00.250Z', parkedTo: '' });

    const request = calls('outbox.listDeadLetters')[0]?.request;
    expect(request).toMatchObject({ parkedFrom: { seconds: 1_789_776_000n, nanos: 250_000_000 } });
    expect((request as { parkedTo?: unknown }).parkedTo).toBeUndefined();
  });
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/dead-letters-page.test.ts tests/integration/dead-letters-page.test.ts
```

Expected: FAIL. The key assertions report `orders|c1` for `orders|||c1`; the new cases fail; the
integration window case reports `parkedFrom` undefined.

- [ ] **Step 4: Implement the loader**

In `ts/apps/iam-console/app/(console)/dead-letters/load.ts`, replace line 11
(`import { timestampIso } from '../../../lib/time';`) with:

```ts
import { timestampFromIso, timestampIso, type ProtoTimestamp } from '../../../lib/time';
```

Replace the header's line 5 `// branch. The loader turns IAM's DeadLetterEntry into plain data for the page.` with:

```ts
// branch. The loader turns IAM's DeadLetterEntry into plain data for the page. SMA-661 added the
// parked-time window; parkedWindow() is shared with the bulk-replay command.
```

Replace the whole `loadDeadLettersPage` function (lines 50-74, the doc comment included) with:

```ts
/** The list filter's parked-time window: CANONICAL ISO instants, '' for no bound (SMA-661 spec § 4.4). */
export type ParkedWindow = { readonly parkedFrom: string; readonly parkedTo: string };

/**
 * The window as request fields, for the list and the bulk replay alike. An empty bound is LEFT OUT,
 * and IAM reads an absent timestamp as no filter (iam.proto:662-665). The page and the bulk form's
 * schema refused every value that lib/paging.ts's canonicalParkedBound does not accept, so the
 * conversion cannot fail.
 */
export function parkedWindow(bounds: ParkedWindow): { parkedFrom?: ProtoTimestamp; parkedTo?: ProtoTimestamp } {
  return {
    ...(bounds.parkedFrom === '' ? {} : { parkedFrom: timestampFromIso(bounds.parkedFrom) }),
    ...(bounds.parkedTo === '' ? {} : { parkedTo: timestampFromIso(bounds.parkedTo) }),
  };
}

/**
 * IAM orders the list by id DESCENDING (tests/dead_letters_pg.rs:432), and IAM mints UUIDv7 ids, so
 * this is close to creation order, not park order. The page keeps that order and does not sort.
 */
export async function loadDeadLettersPage(
  deps: { readonly outbox: Pick<IamClients['outbox'], 'listDeadLetters'> },
  params: { readonly cursor: string; readonly eventType: string } & ParkedWindow,
): Promise<DeadLettersPageData> {
  const result = await callIam(() => deps.outbox.listDeadLetters({ eventType: params.eventType, cursor: params.cursor, limit: PAGE_SIZE, ...parkedWindow(params) }));
  if (!result.ok) return result;
  const rows = result.value.entries.map((entry): DeadLetterRow => ({
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
  }));
  return { ok: true, value: { rows, cursor: params.cursor, nextCursor: result.value.nextCursor === '' ? null : result.value.nextCursor } };
}
```

- [ ] **Step 5: Implement the page**

Replace the whole of `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2–§ 6.4, AC 1 and AC 2). The page asks discovery, not mayI()
// (SMA-511 § 6.3): the Dead letters nav entry is the affordance and mayI() hides it; a typed URL is a
// user action, and IAM answers it. Every OutboxService RPC is Root-only inside IAM.
//
// Every view that does not throw renders inside DeadLettersFrame, keyed by
// `${eventType}|${parkedFrom}|${parkedTo}|${cursor}` with the CANONICAL bounds (SMA-661 spec § 4.2),
// so a revalidated render after a replay or discard keeps the frame and its result, two spellings of
// one window share a frame, and a move to another page or filter starts an empty frame. The four
// query parsers are pure and run before the degraded branch ONLY to compute that key; a degraded IAM
// gets the degraded view whatever the query.
import type { ReactElement, ReactNode } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, ErrorState, Field, Input, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { SectionError } from '../../_components/section-error';
import { discovery, iamClients, sessionToken } from '../../../lib/console';
import { listHref, MAX_EVENT_TYPE_LENGTH, MAX_PARKED_BOUND_LENGTH, parseCursor, parseEventType, parseParkedBound, type ParkedBoundField, type ParsedParkedBound } from '../../../lib/paging';
import { discardDeadLetterAction, replayDeadLetterAction } from './actions';
import { DeadLetterTable } from './dead-letter-table';
import { DeadLettersFrame } from './dead-letters-frame';
import { deadLettersGate, loadDeadLettersPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

/** The full path, as the ingress sees it (ZoneLink needs it). */
const PATH = '/iam/dead-letters';

/** SMA-661 spec § 4.3. Both bounds are inclusive in IAM's query (`>=` and `<=`), and the input cannot show that. */
const PARKED_PLACEHOLDER = '2026-09-19T00:00:00Z';
const PARKED_DESCRIPTION = 'Both bounds are included. Use a zone, for example 2026-09-19T00:00:00Z or +02:00.';

/** The three filter values: the event type, and each parked bound as parsed (typed value, canonical value or error). */
type Filter = { readonly eventType: string; readonly parkedFrom: ParsedParkedBound; readonly parkedTo: ParsedParkedBound };

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
 * One parked-time input (SMA-661 spec § 4.3). It shows the TYPED value, so a refused bound keeps
 * what the operator wrote (D6). A refused bound's sentence goes to Field's `error`, which wires
 * aria-describedby and aria-invalid. `maxLength` bounds typing only; the parser never cuts a value.
 */
function parkedField(label: string, id: string, name: ParkedBoundField, bound: ParsedParkedBound): ReactElement {
  const input = <Input name={name} defaultValue={bound.raw} placeholder={PARKED_PLACEHOLDER} maxLength={MAX_PARKED_BOUND_LENGTH} autoComplete="off" />;
  return bound.ok ? (
    <Field label={label} htmlFor={id} description={PARKED_DESCRIPTION}>
      {input}
    </Field>
  ) : (
    <Field label={label} htmlFor={id} description={PARKED_DESCRIPTION} error={bound.error.message}>
      {input}
    </Field>
  );
}

/**
 * A plain GET form with NO `action` attribute: it submits to the current URL, /iam/dead-letters. A
 * basePath-relative action="/dead-letters" would leave the zone (§ 6.3). It needs no JavaScript, and a
 * new filter starts at the first page.
 */
function filterForm(filter: Filter): ReactElement {
  return (
    <form method="get" aria-label="Filter dead letters" className="flex flex-wrap items-end gap-2">
      <Field label="Event type" htmlFor="dead-letters-event-type">
        <Input name="eventType" defaultValue={filter.eventType} maxLength={MAX_EVENT_TYPE_LENGTH} autoComplete="off" />
      </Field>
      {parkedField('Parked from', 'dead-letters-parked-from', 'parkedFrom', filter.parkedFrom)}
      {parkedField('Parked to', 'dead-letters-parked-to', 'parkedTo', filter.parkedTo)}
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
  const parkedFrom = parseParkedBound(query.parkedFrom, 'parkedFrom');
  const parkedTo = parseParkedBound(query.parkedTo, 'parkedTo');
  const cursor = parseCursor(query.cursor);
  const frameKey = `${eventType.ok ? eventType.value : ''}|${parkedFrom.ok ? parkedFrom.iso : ''}|${parkedTo.ok ? parkedTo.iso : ''}|${cursor.ok ? cursor.cursor : ''}`;

  if (gate === 'degraded') {
    return shell(
      frameKey,
      <div data-testid="dead-letters-degraded">
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>,
    );
  }

  // Every parser runs BEFORE the clients are built: a refused query never becomes an IAM call.
  if (!eventType.ok) return <PageError error={eventType.error} />;
  if (!cursor.ok) return <PageError error={cursor.error} />;
  const filter: Filter = { eventType: eventType.value, parkedFrom, parkedTo };
  // SMA-661 D6: a refused bound is a typing mistake, not a hand-edited cursor. The filter form comes
  // back with every typed value and the field's error. There is no table and no IAM call.
  if (!parkedFrom.ok || !parkedTo.ok) return shell(frameKey, filterForm(filter));

  const clients = await iamClients();
  const data = await loadDeadLettersPage({ outbox: clients.outbox }, { cursor: cursor.cursor, eventType: eventType.value, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso });
  if (!data.ok) {
    // A fresh GET keeps its real 403 or 404. Every other list error stays inside the frame.
    if (data.error.presentation === 'forbidden' || data.error.presentation === 'not-found') return <PageError error={data.error} />;
    return shell(
      frameKey,
      <>
        {filterForm(filter)}
        <SectionError error={data.error} />
      </>,
    );
  }

  const { rows, nextCursor } = data.value;
  // The paging links carry the CANONICAL filter (SMA-661 spec § 4.5). listHref leaves out an empty value.
  const kept = { eventType: eventType.value, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso };
  return shell(
    frameKey,
    <>
      {filterForm(filter)}
      {rows.length === 0 ? <EmptyState title="No dead letters" /> : <DeadLetterTable rows={rows} />}
      <nav aria-label="Dead-letter pages" className="flex gap-4 text-sm">
        {cursor.cursor === '' ? null : (
          <ZoneLink href={listHref(PATH, kept)} className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={listHref(PATH, { ...kept, cursor: nextCursor })} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </>,
  );
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Same command as Step 3. Expected: all pass.

- [ ] **Step 7: Prove the window reaches IAM**

In `load.ts`, change `limit: PAGE_SIZE, ...parkedWindow(params) }` to `limit: PAGE_SIZE }`. Run the
Step 3 command. Expected: FAIL in `sends the CANONICAL bounds…` and in the integration
`sends the canonical parked bounds…`. Restore the exact text `limit: PAGE_SIZE, ...parkedWindow(params) }`.

- [ ] **Step 8: Prove the paging links carry the bounds**

In `page.tsx`, change `listHref(PATH, { ...kept, cursor: nextCursor })` to
`listHref(PATH, { eventType: eventType.value, cursor: nextCursor })`. Run the Step 3 command.
Expected: FAIL in `sends the CANONICAL bounds…` (the second href). Restore
`listHref(PATH, { ...kept, cursor: nextCursor })`.

- [ ] **Step 9: Prove D6 keeps the typed value**

In `parkedField`, change `defaultValue={bound.raw}` to `defaultValue={bound.ok ? bound.iso : ''}`.
Run the Step 3 command. Expected: FAIL in both `D6: a refused …` rows and in the echo assertions of
`sends the CANONICAL bounds…`. Restore `defaultValue={bound.raw}`.

- [ ] **Step 10: Typecheck, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write 'apps/iam-console/app/(console)/dead-letters/load.ts' 'apps/iam-console/app/(console)/dead-letters/page.tsx' apps/iam-console/tests/unit/dead-letters-page.test.ts apps/iam-console/tests/integration/dead-letters-page.test.ts
cd apps/iam-console && pnpm run typecheck && pnpm exec vitest run tests/unit/dead-letters-page.test.ts tests/integration/dead-letters-page.test.ts
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add 'ts/apps/iam-console/app/(console)/dead-letters/load.ts' 'ts/apps/iam-console/app/(console)/dead-letters/page.tsx' ts/apps/iam-console/tests/unit/dead-letters-page.test.ts ts/apps/iam-console/tests/integration/dead-letters-page.test.ts
git commit -m "feat(ts): send the parked-time filter from /iam/dead-letters (SMA-661)" -m "The page parses both bounds, sends the canonical instants to IAM, keys the frame and the
paging links by them, and answers a refused bound with the filter form and a field error." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The bulk-replay command and Server Action (AC 1)

**Files:**
- Create: `ts/apps/iam-console/app/(console)/dead-letters/max-rows.ts`
- Modify (whole file): `ts/apps/iam-console/app/(console)/dead-letters/commands.ts`
- Modify (whole file): `ts/apps/iam-console/app/(console)/dead-letters/actions.ts`
- Test: `ts/apps/iam-console/tests/unit/actions-structure.test.ts:26`
- Test: `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts`
- Test: `ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts`

**Interfaces:**
- Consumes: Task 1's `MAX_BULK_REPLAY_ROWS`; Task 3's `canonicalParkedBound`,
  `MAX_EVENT_TYPE_LENGTH`; Task 4's `parkedWindow`.
- Produces:
  - `max-rows.ts`: `export const MAX_ROWS_PATTERN = /^\d{1,5}$/;` — NO directive and NO
    `server-only`, so the client form (Task 7) and the server schema share one value.
  - `commands.ts`: `parkedBoundField`, `bulkReplayForm`, `type BulkReplayInput = { eventType: string; parkedFrom: string; parkedTo: string; maxRows: number }`,
    `type BulkReplayResult = { readonly ok: true; readonly replayed: number } | { readonly ok: false; readonly error: PaigasusError }`,
    `type BulkReplayState = BulkReplayResult | null`,
    `type BulkReplayAction = (previous: BulkReplayState, form: FormData) => Promise<BulkReplayState>`,
    `bulkReplayDeadLetters(deps: { readonly outbox: Pick<IamClients['outbox'], 'bulkReplayDeadLetters'> }, input: BulkReplayInput): Promise<BulkReplayResult>`.
  - `actions.ts`: `bulkReplayDeadLettersAction(_previous: BulkReplayState, form: FormData): Promise<BulkReplayState>`,
    reading the fields `eventType`, `parkedFrom`, `parkedTo`, `maxRows`.

**Choice:** the spec requires the confirmation gate and the schema to use "the same" digits rule
but puts the schema in server-only `commands.ts`. A `'use client'` file cannot import a value from
a server-only module, and a server module that imports a value from a `'use client'` file gets a
client reference, not the value. So the pattern lives in a new directive-free module, `max-rows.ts`.

**Choice:** spec § 7.1 does not list `actions-revalidate.test.ts`, but AC 1 ("never reaches IAM")
is a property of the ACTION (the command does not validate), and that file is where the SMA-629
zero-call rule for the two row actions already lives (`:230-238`). AC 1's proof goes there.

- [ ] **Step 1: Write the failing tests**

(a) `ts/apps/iam-console/tests/unit/actions-structure.test.ts`, line 26, becomes:

```ts
  '(console)/dead-letters/actions.ts': ['bulkReplayDeadLettersAction', 'discardDeadLetterAction', 'replayDeadLetterAction'],
```

(b) `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts`. In the `vi.hoisted` block, replace
the `outbox` object (lines 46-50) with:

```ts
    // SMA-629: recording mocks, so a test can count the calls an invalid id must NOT make. SMA-661
    // added bulk replay, which answers a count.
    outbox: {
      replayDeadLetter: vi.fn<(request: { id: string }) => Promise<Record<string, never>>>(call),
      discardDeadLetter: vi.fn<(request: { id: string }) => Promise<Record<string, never>>>(call),
      bulkReplayDeadLetters: vi.fn<(request: { eventType: string; maxRows: bigint }) => Promise<{ replayed: bigint }>>(() => call().then(() => ({ replayed: 3n }))),
    },
```

Replace line 62 with:

```ts
const { bulkReplayDeadLettersAction, discardDeadLetterAction, replayDeadLetterAction } = await import('../../app/(console)/dead-letters/actions');
```

Append at the end of the file:

```ts

// SMA-661 spec § 6.5, § 6.6 and AC 1. The action parses the form, so a bulk replay with no valid
// max_rows never reaches IAM. On a success it refreshes the one page; on a failure it refreshes
// nothing (bulk replay never answers not-found).
const bulkForm = (fields: Record<string, string>): FormData => form({ eventType: '', parkedFrom: '', parkedTo: '', ...fields });

describe('the bulk-replay action', () => {
  it('sends the budget as a bigint and the canonical bound, answers the count, and refreshes the page', async () => {
    const result = await bulkReplayDeadLettersAction(null, bulkForm({ maxRows: '500', parkedFrom: '2026-09-19 00:00Z' }));

    expect(result).toEqual({ ok: true, replayed: 3 });
    expect(outbox.bulkReplayDeadLetters.mock.calls.at(-1)?.[0]).toMatchObject({ eventType: '', maxRows: 500n, parkedFrom: { seconds: 1_789_776_000n, nanos: 0 } });
    expect(revalidatedPaths).toEqual(PAGE_REFRESH);
  });

  it.each<[string, Record<string, string>]>([
    ['no maxRows field at all', {}],
    ['an empty maxRows', { maxRows: '' }],
    ['zero', { maxRows: '0' }],
    ['one past the ceiling', { maxRows: '10001' }],
    ['a hexadecimal budget', { maxRows: '0x10' }],
    ['a fractional budget', { maxRows: '7.5' }],
    ['a parked bound with no zone', { maxRows: '5', parkedFrom: '2026-09-19T00:00:00' }],
  ])('refuses %s as invalid input and makes ZERO IAM calls (AC 1)', async (_label, fields) => {
    const before = outbox.bulkReplayDeadLetters.mock.calls.length;

    const result = await bulkReplayDeadLettersAction(null, bulkForm(fields));

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(outbox.bulkReplayDeadLetters.mock.calls.length).toBe(before);
    expect(revalidatedPaths).toEqual([]);
  });

  it('refreshes nothing when IAM answers forbidden', async () => {
    failNext(Code.PermissionDenied);

    const result = await bulkReplayDeadLettersAction(null, bulkForm({ maxRows: '5' }));

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual([]);
  });
});
```

(c) `ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts`. Replace line 10 with:

```ts
import { bulkReplayDeadLetters, bulkReplayForm, deadLetterForm, discardDeadLetter, replayDeadLetter } from '../../app/(console)/dead-letters/commands';
```

Replace the first header line `// The two dead-letter commands (SMA-629 spec § 6.4, § 7.3) against the fake IAM: each sends the id,`
with `// The dead-letter commands (SMA-629 spec § 6.4, § 7.3; SMA-661 spec § 7.3) against the fake IAM: each sends the id,`.
Append:

```ts

// SMA-661 spec § 6.5. The schema that stands between a form and IAM: it trims, normalises the
// bounds with the GET parser's own check, and refuses every max_rows IAM would clamp or misread.
describe('bulkReplayForm', () => {
  it('trims, normalises a bound, reads an absent bound as no filter, and reads the budget as a number', () => {
    expect(bulkReplayForm.safeParse({ eventType: ' iam.team.created ', parkedFrom: '2026-09-19 00:00Z', parkedTo: null, maxRows: ' 500 ' }).data).toEqual({
      eventType: 'iam.team.created',
      parkedFrom: '2026-09-19T00:00:00.000Z',
      parkedTo: '',
      maxRows: 500,
    });
    expect(bulkReplayForm.safeParse({ eventType: '', parkedFrom: '', parkedTo: '', maxRows: '10000' }).data?.maxRows).toBe(10_000);
  });

  it.each(['', '0', '10001', '100000', '7.5', '0x10', '0b1010', '+7', '7.', '1e3', null])('refuses the budget %j', (maxRows) => {
    expect(bulkReplayForm.safeParse({ eventType: '', parkedFrom: '', parkedTo: '', maxRows }).success).toBe(false);
  });

  it('refuses a bound with no zone, and a missing event type', () => {
    expect(bulkReplayForm.safeParse({ eventType: '', parkedFrom: '2026-09-19T00:00:00', parkedTo: '', maxRows: '5' }).success).toBe(false);
    expect(bulkReplayForm.safeParse({ eventType: null, parkedFrom: '', parkedTo: '', maxRows: '5' }).success).toBe(false);
  });
});

describe('bulkReplayDeadLetters', () => {
  it('sends the scope and the budget, leaves an empty bound out, and answers the count as a number', async () => {
    iam.setHandlers({ 'outbox.bulkReplayDeadLetters': () => ({ replayed: 8n }) });
    const calls = callsSince(iam);

    const result = await bulkReplayDeadLetters({ outbox: clientsFor(iam).outbox }, { eventType: 'iam.team.created', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '', maxRows: 500 });

    expect(result).toEqual({ ok: true, replayed: 8 });
    const request = calls('outbox.bulkReplayDeadLetters')[0]?.request;
    expect(request).toMatchObject({ eventType: 'iam.team.created', parkedFrom: { seconds: 1_789_776_000n, nanos: 0 }, maxRows: 500n });
    expect((request as { parkedTo?: unknown }).parkedTo).toBeUndefined();
  });

  it('sends both bounds when both are set', async () => {
    iam.setHandlers({ 'outbox.bulkReplayDeadLetters': () => ({ replayed: 0n }) });
    const calls = callsSince(iam);

    const result = await bulkReplayDeadLetters({ outbox: clientsFor(iam).outbox }, { eventType: '', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '2026-09-20T00:00:00.000Z', maxRows: 1 });

    expect(result).toEqual({ ok: true, replayed: 0 });
    expect(calls('outbox.bulkReplayDeadLetters')[0]?.request).toMatchObject({
      eventType: '',
      parkedFrom: { seconds: 1_789_776_000n, nanos: 0 },
      parkedTo: { seconds: 1_789_862_400n, nanos: 0 },
      maxRows: 1n,
    });
  });

  it('maps a denial to forbidden with the correlation id', async () => {
    iam.setHandlers({
      'outbox.bulkReplayDeadLetters': () => {
        throw denial({ correlationId: 'corr-bulk-replay-403' });
      },
    });

    const result = await bulkReplayDeadLetters({ outbox: clientsFor(iam).outbox }, { eventType: '', parkedFrom: '', parkedTo: '', maxRows: 5 });

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden', correlationId: 'corr-bulk-replay-403' } });
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/actions-structure.test.ts tests/unit/actions-revalidate.test.ts tests/integration/dead-letter-commands.test.ts
```

Expected: FAIL. `bulkReplayDeadLettersAction is not a function`, `bulkReplayForm` undefined, and
`finds exactly the expected actions.ts files and exports` differs.

- [ ] **Step 3: Create the shared pattern**

Create `ts/apps/iam-console/app/(console)/dead-letters/max-rows.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The max_rows grammar of the bulk-replay form (SMA-661 spec § 6.4, § 6.5). NO directive and NO
// server-only import, on purpose: the client form gates its confirmation on this pattern, and
// commands.ts's zod schema parses the posted value with it. One value, so the number the
// confirmation names and the number IAM receives cannot disagree.

/** One to five ASCII digits. Not `z.coerce.number()`: that reads '0x10' as 16 and '7.' as 7 (§ 6.5). */
export const MAX_ROWS_PATTERN = /^\d{1,5}$/;
```

- [ ] **Step 4: Write the command**

Replace the whole of `ts/apps/iam-console/app/(console)/dead-letters/commands.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands behind the dead-letters Server Actions (SMA-629 spec § 6.4; SMA-661 spec § 5, § 6.5,
// § 6.6). A command takes its IAM client as a port, so the tier-2 tests call it against the fake IAM
// with no session and no Next runtime. It takes NO mayI (SMA-511 § 6.3): the page is Root-only, and
// IAM decides.
import 'server-only';
import { z } from 'zod';
import { callIam, MAX_BULK_REPLAY_ROWS, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { canonicalParkedBound, MAX_EVENT_TYPE_LENGTH } from '../../../lib/paging';
import { parkedWindow } from './load';
import { MAX_ROWS_PATTERN } from './max-rows';

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

/**
 * One parked-time bound of the bulk form, a hidden field (SMA-661 spec § 6.5). `''`, `null` and
 * `undefined` all mean "no filter": `formFields` answers `null` for a field that a hand-built POST
 * left out, and an omitted optional filter is not a missing required field. Any other value must pass
 * lib/paging.ts's `canonicalParkedBound`, the SAME check as the GET filter, and comes out canonical.
 */
export const parkedBoundField = z.preprocess(
  (value) => (value === null || value === undefined ? '' : value),
  z
    .string()
    .trim()
    .transform((value, ctx) => {
      if (value === '') return '';
      const iso = canonicalParkedBound(value);
      if (iso === null) {
        ctx.addIssue({ code: 'custom', message: 'not a valid parked time' });
        return z.NEVER;
      }
      return iso;
    }),
);

/**
 * The bulk-replay form (SMA-661 spec § 6.5). `maxRows` is REQUIRED, and it is IAM's only guard on
 * blast radius: an empty or absent value fails the digits pattern, so the action never builds a
 * request (AC 1). The pattern is max-rows.ts's, the one the form's first button is gated on, so the
 * number IAM receives is the number the confirmation named. A value above MAX_BULK_REPLAY_ROWS is
 * refused, because IAM would clamp it without a word (D3).
 */
export const bulkReplayForm = z.object({
  eventType: z.string().trim().max(MAX_EVENT_TYPE_LENGTH),
  parkedFrom: parkedBoundField,
  parkedTo: parkedBoundField,
  maxRows: z.string().trim().regex(MAX_ROWS_PATTERN).transform(Number).pipe(z.number().int().min(1).max(MAX_BULK_REPLAY_ROWS)),
});
export type BulkReplayInput = z.infer<typeof bulkReplayForm>;

/**
 * What a bulk replay answers (SMA-661 spec § 5). ActionResult carries no payload, so the count needs
 * its own type. A value of it is still assignable to ActionResult, so refreshesAfterDeadLetterAction
 * takes it unchanged. `replayed` is a number: IAM's uint64 is at most 10000 (the clamp), so the
 * conversion is exact and no bigint crosses the Server Action boundary.
 */
export type BulkReplayResult = { readonly ok: true; readonly replayed: number } | { readonly ok: false; readonly error: PaigasusError };
export type BulkReplayState = BulkReplayResult | null;
export type BulkReplayAction = (previous: BulkReplayState, form: FormData) => Promise<BulkReplayState>;

/** Bulk replay is NOT atomic (iam.proto:696-699): a failed call can still have replayed some rows. */
export async function bulkReplayDeadLetters(deps: { readonly outbox: Pick<IamClients['outbox'], 'bulkReplayDeadLetters'> }, input: BulkReplayInput): Promise<BulkReplayResult> {
  const result = await callIam(() => deps.outbox.bulkReplayDeadLetters({ eventType: input.eventType, maxRows: BigInt(input.maxRows), ...parkedWindow(input) }));
  return result.ok ? { ok: true, replayed: Number(result.value.replayed) } : { ok: false, error: result.error };
}
```

- [ ] **Step 5: Write the action**

Replace the whole of `ts/apps/iam-console/app/(console)/dead-letters/actions.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// The three dead-letter Server Actions (SMA-629 spec § 6.4; SMA-661 spec § 6.6). See
// ../orgs/actions.ts for the rules every action follows; tests/unit/actions-structure.test.ts holds
// them. Each gets its client through iamClientsForAction(), parses the form with zod, and never
// navigates. The frame of the page calls them directly, with `null` as the previous state. They
// refresh the page on ok and on not-found only (lib/form.ts's refreshesAfterDeadLetterAction says why
// forbidden does not refresh).
import { revalidatePath } from 'next/cache';
import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
import { iamClientsForAction } from '../../../lib/console';
import { refreshesAfterDeadLetterAction } from '../../../lib/form';
import { bulkReplayDeadLetters, bulkReplayForm, deadLetterForm, discardDeadLetter, replayDeadLetter, type BulkReplayState } from './commands';

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

/**
 * Bulk replay (SMA-661 spec § 6.6). Its state carries the replayed count, so it is not an
 * ActionState. A form with no valid `maxRows` never reaches IAM (AC 1). Bulk replay never answers
 * not-found, so in practice it refreshes the page on ok only, and a zero count refreshes too.
 */
export async function bulkReplayDeadLettersAction(_previous: BulkReplayState, form: FormData): Promise<BulkReplayState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = bulkReplayForm.safeParse(formFields(form, ['eventType', 'parkedFrom', 'parkedTo', 'maxRows']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await bulkReplayDeadLetters({ outbox: clients.value.outbox }, parsed.data);
  if (refreshesAfterDeadLetterAction(result)) revalidatePath(DEAD_LETTERS_PATH, 'page');
  return result;
}
```

(The only header change: "two" becomes "three", and the SMA-661 reference is added. The two
existing actions are unchanged.)

- [ ] **Step 6: Run the tests to verify they pass**

Same command as Step 2. Expected: all pass.

- [ ] **Step 7: Prove AC 1 bites**

In `commands.ts`, replace the `maxRows:` entry of `bulkReplayForm`

```ts
  maxRows: z.string().trim().regex(MAX_ROWS_PATTERN).transform(Number).pipe(z.number().int().min(1).max(MAX_BULK_REPLAY_ROWS)),
```

with the line below. It compiles: the output is still a number, and both imports stay read, so
`noUnusedLocals` does not fire. `MAX_ROWS_PATTERN.test('x')` is always false, so the schema now
accepts any string or null as `Number(value)`.

```ts
  maxRows: z.string().nullable().transform((value) => (MAX_ROWS_PATTERN.test('x') ? MAX_BULK_REPLAY_ROWS : Number(value))),
```

Run the Step 2 command. Expected: FAIL in the `refuses … and makes ZERO IAM calls (AC 1)` rows
(the mock is called with `maxRows: 0n` or `16n`) and in the `bulkReplayForm` refusal rows. Restore
the original `maxRows:` line exactly.

- [ ] **Step 8: Prove the shared bound check bites on the POST path**

In `parkedBoundField`, change `const iso = canonicalParkedBound(value);` to
`const iso = canonicalParkedBound(value) ?? value;`. Run the Step 2 command. Expected: FAIL in
`a parked bound with no zone` and in `refuses a bound with no zone…`. Restore
`const iso = canonicalParkedBound(value);`.

- [ ] **Step 9: Typecheck, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write 'apps/iam-console/app/(console)/dead-letters/max-rows.ts' 'apps/iam-console/app/(console)/dead-letters/commands.ts' 'apps/iam-console/app/(console)/dead-letters/actions.ts' apps/iam-console/tests/unit/actions-structure.test.ts apps/iam-console/tests/unit/actions-revalidate.test.ts apps/iam-console/tests/integration/dead-letter-commands.test.ts
cd apps/iam-console && pnpm run typecheck && pnpm exec vitest run tests/unit/actions-structure.test.ts tests/unit/actions-revalidate.test.ts tests/integration/dead-letter-commands.test.ts
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add 'ts/apps/iam-console/app/(console)/dead-letters/max-rows.ts' 'ts/apps/iam-console/app/(console)/dead-letters/commands.ts' 'ts/apps/iam-console/app/(console)/dead-letters/actions.ts' ts/apps/iam-console/tests/unit/actions-structure.test.ts ts/apps/iam-console/tests/unit/actions-revalidate.test.ts ts/apps/iam-console/tests/integration/dead-letter-commands.test.ts
git commit -m "feat(ts): add the bulk-replay Server Action (SMA-661)" -m "bulkReplayDeadLettersAction parses the scope and a required max_rows of 1 to 10000 and
calls BulkReplayDeadLetters. A form without a valid max_rows never reaches IAM (AC 1)." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: The frame runs a bulk replay, and a row can hide Replay

**Files:**
- Modify (whole file): `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx`
- Modify: `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx:21-24, 53`
- Modify: `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` (the `shell` actions and one import)
- Test: `ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx`

**Interfaces:**
- Consumes: Task 5's `BulkReplayAction`, `BulkReplayResult` (type-only imports), and
  `bulkReplayDeadLettersAction`.
- Produces:
  - `export type DeadLetterActions = { readonly replay: FormAction; readonly discard: FormAction; readonly bulkReplay: BulkReplayAction };` (the new field is required).
  - `export type Runner = { readonly busy: boolean; run(control: DeadLetterControl, form: FormData): void; runBulk(form: FormData): void };`
  - `export function useRunner(): Runner;` — Task 7 uses it.
  - `export function bulkReplayedText(replayed: number): string;`
  - `DeadLetterRowControls({ id, canReplay = true }: { readonly id: string; readonly canReplay?: boolean })`
  - `DeadLetterTable({ rows, canReplay = true }: { readonly rows: readonly DeadLetterRow[]; readonly canReplay?: boolean })` — Task 8 passes `canReplay`.
  - The caption text is `Newest events first. With a parked-time filter set, an event with no parked time is not listed.`

- [ ] **Step 1: Write the failing tests**

In `ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx`:

(a) Replace lines 10 and 18 with:

```ts
import type { ReactElement, ReactNode } from 'react';
```

```ts
import { bulkReplayedText, DeadLettersFrame, discardConfirmation, UNREACHED_TEXT, useRunner, type DeadLetterActions } from '../../app/(console)/dead-letters/dead-letters-frame';
import type { BulkReplayAction, BulkReplayState } from '../../app/(console)/dead-letters/commands';
```

(b) Replace the `actions` helper (lines 74-79) with:

```ts
function actions(overrides: Partial<Record<'replay' | 'discard', Sig>> & { readonly bulkReplay?: BulkReplayAction } = {}) {
  return {
    replay: vi.fn<Sig>(overrides.replay ?? (() => Promise.resolve({ ok: true }))),
    discard: vi.fn<Sig>(overrides.discard ?? (() => Promise.resolve({ ok: true }))),
    bulkReplay: vi.fn<BulkReplayAction>(overrides.bulkReplay ?? (() => Promise.resolve({ ok: true, replayed: 2 }))),
  };
}

/** A stand-in for the bulk form: it hands the runner a FormData with NO id, as the real form does. */
function BulkProbe(): ReactElement {
  const runner = useRunner();
  return (
    <button
      type="button"
      onClick={() => {
        const form = new FormData();
        form.append('maxRows', '5');
        runner.runBulk(form);
      }}
    >
      Run bulk
    </button>
  );
}
```

(c) In `describe('the table', …)`, change
`expect(screen.getByText('Newest events first.')).toBeDefined();` to
`expect(screen.getByText('Newest events first. With a parked-time filter set, an event with no parked time is not listed.')).toBeDefined();`
and add this case to the same describe:

```ts

  it('hides Replay and keeps Discard when canReplay is false (SMA-661 D4)', () => {
    render(frame(<DeadLetterTable canReplay={false} rows={[ROW_A]} />, actions()));

    expect(within(controlsOf(A)).queryByRole('button', { name: 'Replay' })).toBeNull();
    expect(screen.queryByRole('form', { name: `Replay event ${A}` })).toBeNull();
    expect(within(controlsOf(A)).getByRole('button', { name: 'Discard' })).toBeDefined();
  });
```

(d) Append at the end of the file:

```tsx

describe('bulk replay through the runner (SMA-661 spec § 6.1, § 6.7)', () => {
  it('words the count: plural, singular, and zero as a real answer', () => {
    expect(bulkReplayedText(8)).toBe('Replayed 8 events.');
    expect(bulkReplayedText(1)).toBe('Replayed 1 event.');
    expect(bulkReplayedText(0)).toBe('Replayed 0 events. No parked event matched the scope.');
  });

  it('shows the count, reads no id, and keeps the result across a refresh', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(
      frame(
        <>
          <BulkProbe />
          <DeadLetterTable rows={[ROW_A]} />
        </>,
        a,
      ),
    );

    await user.click(screen.getByRole('button', { name: 'Run bulk' }));
    await within(region()).findByText('Replayed 2 events.');
    expect(a.bulkReplay).toHaveBeenCalledTimes(1);
    expect(a.bulkReplay.mock.calls[0]?.[0]).toBeNull();
    expect(a.bulkReplay.mock.calls[0]?.[1].get('id')).toBeNull();
    expect(a.bulkReplay.mock.calls[0]?.[1].get('maxRows')).toBe('5');
    expect(a.replay).not.toHaveBeenCalled();

    await refresh(
      rerender,
      frame(
        <>
          <BulkProbe />
          <p>No dead letters</p>
        </>,
        a,
      ),
    );
    expect(within(region()).getByText('Replayed 2 events.')).toBeDefined();
  });

  it('shows a failure as FormError, and never the dead-letter sentence', async () => {
    const user = userEvent.setup();
    render(frame(<BulkProbe />, actions({ bulkReplay: () => Promise.resolve({ ok: false, error: errorWith('not-found') }) })));

    await user.click(screen.getByRole('button', { name: 'Run bulk' }));

    expect((await within(region()).findByTestId('form-error')).getAttribute('data-presentation')).toBe('not-found');
    expect(within(region()).queryByText(DEAD_LETTER_GONE)).toBeNull();
  });

  it('shows the unreached text for a rejected bulk action', async () => {
    const user = userEvent.setup();
    render(frame(<BulkProbe />, actions({ bulkReplay: () => Promise.reject(new TypeError('Failed to fetch')) })));

    await user.click(screen.getByRole('button', { name: 'Run bulk' }));

    expect(await within(region()).findByText(UNREACHED_TEXT)).toBeDefined();
  });

  it('lets a NEWER row submission win over an older bulk submission', async () => {
    let finishBulk: (state: BulkReplayState) => void = () => undefined;
    const a = actions({
      bulkReplay: () =>
        new Promise<BulkReplayState>((resolve) => {
          finishBulk = resolve;
        }),
      replay: () => Promise.resolve({ ok: false, error: errorWith('forbidden') }),
    });
    render(
      frame(
        <>
          <BulkProbe />
          <DeadLetterTable rows={[ROW_A]} />
        </>,
        a,
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Run bulk' }));
    fireEvent.submit(screen.getByRole('form', { name: `Replay event ${A}` }));
    await within(region()).findByText(PRESENTATION_COPY.forbidden.body);
    await act(async () => {
      finishBulk({ ok: true, replayed: 9 });
      await Promise.resolve();
    });

    expect(within(region()).queryByText('Replayed 9 events.')).toBeNull();
    expect(within(region()).getByTestId('form-error').getAttribute('data-presentation')).toBe('forbidden');
  });

  it('disables every row button while a bulk replay runs', async () => {
    let finish: (state: BulkReplayState) => void = () => undefined;
    render(
      frame(
        <>
          <BulkProbe />
          <DeadLetterTable rows={[ROW_A, ROW_B]} />
        </>,
        actions({
          bulkReplay: () =>
            new Promise<BulkReplayState>((resolve) => {
              finish = resolve;
            }),
        }),
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Run bulk' }));

    // isPending can lag the transition's own setState, so wait for the busy state.
    await waitFor(() => {
      expect(rowButtons()).toHaveLength(4);
      for (const button of rowButtons()) expect(button.disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true, replayed: 1 });
      await Promise.resolve();
    });
    await waitFor(() => {
      for (const button of rowButtons()) expect(button.disabled).toBe(false);
    });
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/dead-letters-frame.test.tsx
```

Expected: FAIL. `useRunner is not a function` breaks every `BulkProbe` case; the caption and
`canReplay` cases fail.

- [ ] **Step 3: Rewrite the frame**

Replace the whole of `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The frame of /iam/dead-letters (SMA-629 spec § 6.4). CLIENT component. The page renders it for
// EVERY view that does not throw — the table, the empty list, the degraded view and a SectionError —
// keyed by `${eventType}|${parkedFrom}|${parkedTo}|${cursor}`, with the view as children. The design
// is the gateway console's ServiceAccountFrame (ts/apps/gateway-console/app/_components/service-account-frame.tsx:1-27).
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
// BULK REPLAY (SMA-661 spec § 6.1). The bulk form (bulk-replay-form.tsx) hands its FormData to the
// runner's SECOND method, runBulk. It shares the generation counter and the busy lock with `run`, and
// it reads no id. Its answer carries the replayed count, so it has its own result arm.
// DeadLetterControl stays the two row controls, so successText can never answer "Discarded" for a
// bulk submission.
//
// A REJECTED ACTION (a network drop, a server fault). The runner catches it and shows a client-built
// error. It never rethrows: a rethrow reaches (console)/error.tsx, which unmounts this frame. The
// action may still have run on the server, so the text says the result is unknown and asks for a
// reload. A later retry of the same id answers not-found only while the row is not parked: if the
// new publish failed, the relay parks the row again and a retry replays it again (§ 9). For a bulk
// replay the same text is correct, and more important: a rejected call can have replayed an unknown
// number of events (SMA-661 fact 5).
'use client';

import { createContext, use, useRef, useState, useTransition, type FormEvent, type ReactElement, type ReactNode } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import type { ActionResult, FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { DEAD_LETTER_GONE } from '../../_components/error-copy';
import { FormError } from '../../_components/form-error';
import type { BulkReplayAction, BulkReplayResult } from './commands';

export type DeadLetterControl = 'replay' | 'discard';

export type DeadLetterActions = { readonly replay: FormAction; readonly discard: FormAction; readonly bulkReplay: BulkReplayAction };

type FrameResult =
  | null
  | { readonly kind: 'answer'; readonly control: DeadLetterControl; readonly id: string; readonly state: ActionResult }
  | { readonly kind: 'bulk-answer'; readonly state: BulkReplayResult }
  | { readonly kind: 'unreached'; readonly error: PaigasusError };

/** The action's promise rejected: no answer came back. The action can still have run on the server. */
export const UNREACHED_TEXT = 'No answer came back from the server, so the result is unknown. Reload the page to see the current queue.';

export function successText(control: DeadLetterControl, id: string): string {
  return control === 'replay' ? `Replayed event ${id}.` : `Discarded event ${id}.`;
}

/** SMA-661 spec § 6.7. Zero is a real and useful answer, not a failure. */
export function bulkReplayedText(replayed: number): string {
  if (replayed === 0) return 'Replayed 0 events. No parked event matched the scope.';
  return replayed === 1 ? 'Replayed 1 event.' : `Replayed ${String(replayed)} events.`;
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
  // SMA-661 § 6.7: bulk replay cannot answer not-found, so DEAD_LETTER_GONE stays on the two row actions.
  if (result.kind === 'bulk-answer') return result.state.ok ? <p className="text-sm">{bulkReplayedText(result.state.replayed)}</p> : <FormError error={result.state.error} />;
  if (result.state.ok) return <p className="text-sm">{successText(result.control, result.id)}</p>;
  const error = result.state.error;
  // § 6.5: only a not-found answer of these two actions gets the dead-letter sentence.
  return <FormError error={error} message={error.presentation === 'not-found' ? DEAD_LETTER_GONE : undefined} />;
}

export type Runner = {
  /** True while any action of the frame runs. Every Replay, Discard and bulk-replay button is disabled then. */
  readonly busy: boolean;
  run(control: DeadLetterControl, form: FormData): void;
  /** The bulk form's submission (SMA-661 § 6.1). It reads no id: the scope and the budget are in the FormData. */
  runBulk(form: FormData): void;
};

const RunnerContext = createContext<Runner | null>(null);

/** Exported for bulk-replay-form.tsx (SMA-661 § 6.1). */
export function useRunner(): Runner {
  const runner = use(RunnerContext);
  if (runner === null) throw new Error('A dead-letter control must render inside DeadLettersFrame.');
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
    runBulk(form) {
      generationRef.current += 1;
      const mine = generationRef.current;
      setResult(null);
      startWork(async () => {
        try {
          const state = await actions.bulkReplay(null, form);
          if (state !== null && generationRef.current === mine) setResult({ kind: 'bulk-answer', state });
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
 * Replay renders only when `canReplay` (SMA-661 D4): the page asks mayI() about ReplayOutboxDeadLetter.
 * It defaults to true, so a caller that does not ask shows Replay. Discard is a separate permission,
 * and the console asks nothing about it.
 */
export function DeadLetterRowControls({ id, canReplay = true }: { readonly id: string; readonly canReplay?: boolean }): ReactElement {
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
      {canReplay ? (
        <form aria-label={`Replay event ${id}`} onSubmit={submit('replay')}>
          <input type="hidden" name="id" value={id} />
          <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
            Replay
          </button>
        </form>
      ) : null}
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

- [ ] **Step 4: Pass `canReplay` through the table, and change the caption**

In `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx`, replace lines 21-24:

```tsx
export function DeadLetterTable({ rows }: { readonly rows: readonly DeadLetterRow[] }): ReactElement {
  return (
    <Table>
      <caption className="text-muted-foreground mb-2 caption-top text-left text-sm">Newest events first.</caption>
```

with:

```tsx
/**
 * `canReplay` (SMA-661 D4) reaches every row's controls; it defaults to true. The caption names the
 * NULL `parked_at` blind spot (SMA-661 § 4.6): with a time bound set, IAM never lists such a row.
 */
export function DeadLetterTable({ rows, canReplay = true }: { readonly rows: readonly DeadLetterRow[]; readonly canReplay?: boolean }): ReactElement {
  return (
    <Table>
      <caption className="text-muted-foreground mb-2 caption-top text-left text-sm">Newest events first. With a parked-time filter set, an event with no parked time is not listed.</caption>
```

and replace line 53 `<DeadLetterRowControls id={row.id} />` with
`<DeadLetterRowControls canReplay={canReplay} id={row.id} />`.

- [ ] **Step 5: Give the page's frame the bulk action**

In `ts/apps/iam-console/app/(console)/dead-letters/page.tsx`, replace
`import { discardDeadLetterAction, replayDeadLetterAction } from './actions';` with
`import { bulkReplayDeadLettersAction, discardDeadLetterAction, replayDeadLetterAction } from './actions';`
and, in `shell`, replace
`<DeadLettersFrame key={frameKey} actions={{ replay: replayDeadLetterAction, discard: discardDeadLetterAction }}>` with
`<DeadLettersFrame key={frameKey} actions={{ replay: replayDeadLetterAction, discard: discardDeadLetterAction, bulkReplay: bulkReplayDeadLettersAction }}>`.

- [ ] **Step 6: Run the test to verify it passes**

Same command as Step 2. Expected: all pass.

- [ ] **Step 7: Prove the row flag bites**

In `DeadLetterRowControls`, change `{canReplay ? (` to `{canReplay || true ? (`. Run the Step 2
command. Expected: FAIL in `hides Replay and keeps Discard…`. Restore `{canReplay ? (`.

- [ ] **Step 8: Prove the bulk generation check bites**

In `runBulk`, change `if (state !== null && generationRef.current === mine) setResult({ kind: 'bulk-answer', state });`
to `if (state !== null && (generationRef.current === mine || true)) setResult({ kind: 'bulk-answer', state });`.
Run the Step 2 command. Expected: FAIL in `lets a NEWER row submission win…`. Restore the exact
original line.

- [ ] **Step 9: Typecheck, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write 'apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx' 'apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx' 'apps/iam-console/app/(console)/dead-letters/page.tsx' apps/iam-console/tests/unit/dead-letters-frame.test.tsx
cd apps/iam-console && pnpm run typecheck && pnpm exec vitest run tests/unit/dead-letters-frame.test.tsx tests/unit/dead-letters-page.test.ts
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add 'ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx' 'ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx' 'ts/apps/iam-console/app/(console)/dead-letters/page.tsx' ts/apps/iam-console/tests/unit/dead-letters-frame.test.tsx
git commit -m "feat(ts): run a bulk replay through the dead-letters frame (SMA-661)" -m "The runner gains runBulk and a bulk-answer result arm with the replayed count. A row
renders Replay only when canReplay is true, and the table caption names the NULL
parked-time blind spot." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The bulk-replay form (AC 2)

**Files:**
- Create: `ts/apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx`
- Create: `ts/apps/iam-console/tests/unit/bulk-replay-form.test.tsx`

**Interfaces:**
- Consumes: Task 5's `MAX_ROWS_PATTERN` (`./max-rows`); Task 6's `useRunner`.
- Produces:
  - `export type BulkReplayScope = { readonly eventType: string; readonly parkedFrom: string; readonly parkedTo: string };` — CANONICAL values, `''` = no filter.
  - `export function BulkReplayForm(props: { readonly scope: BulkReplayScope; readonly ceiling: number }): ReactElement;`
  - `export function bulkReplayScopeText(scope: BulkReplayScope): string;`
  - `export function bulkReplayConfirmation(input: BulkReplayScope & { readonly maxRows: number }): string;`
  - `export function maxRowsOf(raw: string, ceiling: number): number | null;`
  - DOM: a `<section>` headed `<h2>Bulk replay</h2>`, a `<form aria-label="Bulk replay">`, a `Max rows`
    input named `maxRows`, three hidden inputs `eventType`, `parkedFrom`, `parkedTo`, and the buttons
    `Bulk replay…` (with the single character `…`), `Confirm bulk replay`, `Cancel`.

**Choices (the spec leaves them open):**
- The ceiling comes in as a prop, `ceiling`. The console-core root is server-only
  (`src/index.ts:6`), so this client file cannot import `MAX_BULK_REPLAY_ROWS`; the page passes it.
- The "A page holds 50 events" sentence is a literal, because `lib/paging.ts` (which holds
  `PAGE_SIZE`) is server-only too. A unit test holds the literal to `PAGE_SIZE`.
- The `Max rows` input is `readOnly` in step 2, so the number under the confirmation cannot change.
  It is NOT `disabled`: a disabled control is left out of the FormData, and the action would then
  refuse the form.
- A form with one text input and no submit button submits on Enter (implicit submission). `submit`
  therefore does nothing in step 1. Without this guard, Enter would skip the confirmation.
- The step closes after a submission, like Discard's.
- Wording the spec does not fix: an empty event type reads `any event type`; the scope line of
  § 6.3 reads `parked <from> → <to>` with `(no start)` / `(no end)`, or `any parked time`; a
  one-bound confirmation reads `parked from <from>, the bound included, with no end` or
  `parked up to <to>, the bound included, with no start`. "Both bounds included" is kept for the
  two-bound case only, because it is false with one bound.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/iam-console/tests/unit/bulk-replay-form.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The bulk-replay form (SMA-661 spec § 6.2–§ 6.4, AC 2). The pure texts first, then the REAL form
// inside the REAL frame: the confirmation cannot open without a valid budget, one form submits the
// budget and the three hidden scope fields, a step-1 submission (Enter) runs nothing, and every
// button waits while the frame is busy.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { BulkReplayForm, bulkReplayConfirmation, bulkReplayScopeText, maxRowsOf, type BulkReplayScope } from '../../app/(console)/dead-letters/bulk-replay-form';
import type { BulkReplayAction } from '../../app/(console)/dead-letters/commands';
import { DeadLetterTable } from '../../app/(console)/dead-letters/dead-letter-table';
import { DeadLettersFrame, type DeadLetterActions } from '../../app/(console)/dead-letters/dead-letters-frame';
import type { DeadLetterRow } from '../../app/(console)/dead-letters/load';
import { PAGE_SIZE } from '../../lib/paging';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/dead-letters' }));
// The frame's FormError chain must not load the whole server runtime into jsdom (see dead-letters-frame.test.tsx).
vi.mock('@paigasus/console-core', () => ({ requestPath: () => Promise.resolve('/iam/dead-letters') }));

afterEach(() => {
  cleanup();
});

const CEILING = 10_000;
const FROM = '2026-09-19T00:00:00.000Z';
const TO = '2026-09-20T00:00:00.000Z';
const FULL: BulkReplayScope = { eventType: 'iam.team.created', parkedFrom: FROM, parkedTo: TO };
const EMPTY: BulkReplayScope = { eventType: '', parkedFrom: '', parkedTo: '' };

const BEYOND = 'The scope can hold events this page does not show. A page holds 50 events, and the budget can be larger.';
const RISKS =
  'A replay can publish an event a second time when the broker deduplication window has passed. A cancelled or timed-out request can leave an unknown number of events already replayed; running it again is safe, because a replayed event is no longer parked.';
const NO_PARKED_TIME = 'An event with no parked time is not in this scope.';

describe('bulkReplayConfirmation (§ 6.4, AC 2)', () => {
  it('names the budget, the full scope with both bounds included, the page, the risks and the blind spot', () => {
    expect(bulkReplayConfirmation({ ...FULL, maxRows: 500 })).toBe(
      `Replay up to 500 parked events. Scope: event type iam.team.created, parked from ${FROM} to ${TO}, both bounds included. ${BEYOND} ${RISKS} ${NO_PARKED_TIME}`,
    );
  });

  it('names an absent bound as absent', () => {
    expect(bulkReplayConfirmation({ eventType: '', parkedFrom: FROM, parkedTo: '', maxRows: 5 })).toBe(
      `Replay up to 5 parked events. Scope: any event type, parked from ${FROM}, the bound included, with no end. ${BEYOND} ${RISKS} ${NO_PARKED_TIME}`,
    );
    expect(bulkReplayConfirmation({ eventType: '', parkedFrom: '', parkedTo: TO, maxRows: 5 })).toBe(
      `Replay up to 5 parked events. Scope: any event type, parked up to ${TO}, the bound included, with no start. ${BEYOND} ${RISKS} ${NO_PARKED_TIME}`,
    );
  });

  it('names the empty scope in words, with no blind-spot sentence (D5)', () => {
    expect(bulkReplayConfirmation({ ...EMPTY, maxRows: 10_000 })).toBe(
      `Replay up to 10000 parked events. The scope is EVERY parked event: no event type and no parked-time window. ${BEYOND} ${RISKS}`,
    );
  });

  it('adds no blind-spot sentence for an event type with no window', () => {
    expect(bulkReplayConfirmation({ eventType: 'iam.team.created', parkedFrom: '', parkedTo: '', maxRows: 2 })).toBe(
      `Replay up to 2 parked events. Scope: event type iam.team.created, any parked time. ${BEYOND} ${RISKS}`,
    );
  });

  it('uses the singular for a budget of one', () => {
    expect(bulkReplayConfirmation({ ...FULL, maxRows: 1 }).startsWith('Replay up to 1 parked event. ')).toBe(true);
  });

  it('states the page size that lib/paging.ts really uses', () => {
    expect(bulkReplayConfirmation({ ...EMPTY, maxRows: 1 })).toContain(`A page holds ${String(PAGE_SIZE)} events`);
  });
});

describe('bulkReplayScopeText (§ 6.3)', () => {
  it.each<[BulkReplayScope, string]>([
    [FULL, `Scope: event type iam.team.created, parked ${FROM} → ${TO}`],
    [{ eventType: 'iam.team.created', parkedFrom: FROM, parkedTo: '' }, `Scope: event type iam.team.created, parked ${FROM} → (no end)`],
    [{ eventType: '', parkedFrom: '', parkedTo: TO }, `Scope: any event type, parked (no start) → ${TO}`],
    [EMPTY, 'Scope: any event type, any parked time'],
  ])('%j reads %s', (scope, text) => {
    expect(bulkReplayScopeText(scope)).toBe(text);
  });
});

describe('maxRowsOf (§ 6.4)', () => {
  it('reads one to five digits from 1 to the ceiling', () => {
    expect(maxRowsOf('1', CEILING)).toBe(1);
    expect(maxRowsOf(' 500 ', CEILING)).toBe(500);
    expect(maxRowsOf('10000', CEILING)).toBe(10_000);
  });

  it.each(['', '0', '10001', '99999', '100000', '7.5', '0x10', '+7', '7.', '1e3', '-1'])('refuses %j', (raw) => {
    expect(maxRowsOf(raw, CEILING)).toBeNull();
  });
});

const ROW_A: DeadLetterRow = {
  id: '0190a1f0-0000-7000-8000-0000000000a1',
  eventType: 'iam.team.created',
  aggregatePrn: 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1',
  payload: '{}',
  schemaVersion: 1,
  attempts: 5,
  parkedAt: '2026-09-19T00:05:00.000Z',
  occurredAt: '2026-09-19T00:00:00.000Z',
  actorPrn: null,
  correlationId: null,
  lastError: null,
};

type Sig = (previous: ActionState, form: FormData) => Promise<ActionState>;

function actions(overrides: Partial<Record<'replay' | 'discard', Sig>> & { readonly bulkReplay?: BulkReplayAction } = {}) {
  return {
    replay: vi.fn<Sig>(overrides.replay ?? (() => Promise.resolve({ ok: true }))),
    discard: vi.fn<Sig>(overrides.discard ?? (() => Promise.resolve({ ok: true }))),
    bulkReplay: vi.fn<BulkReplayAction>(overrides.bulkReplay ?? (() => Promise.resolve({ ok: true, replayed: 2 }))),
  };
}

function page(a: DeadLetterActions, scope: BulkReplayScope = FULL): ReactNode {
  return (
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <DeadLettersFrame actions={a}>
        <BulkReplayForm scope={scope} ceiling={CEILING} />
        <DeadLetterTable rows={[ROW_A]} />
      </DeadLettersFrame>
    </ZoneProvider>
  );
}

const region = (): HTMLElement => screen.getByTestId('dead-letters-result');
const bulk = (): HTMLElement => screen.getByRole('form', { name: 'Bulk replay' });
const firstButton = (): HTMLButtonElement => within(bulk()).getByRole<HTMLButtonElement>('button', { name: 'Bulk replay…' });
const maxRowsInput = (): HTMLInputElement => within(bulk()).getByLabelText<HTMLInputElement>('Max rows');

describe('the form (§ 6.3, § 6.4)', () => {
  it('shows the scope as text and the range before anything is typed', () => {
    render(page(actions()));

    expect(screen.getByRole('heading', { name: 'Bulk replay' })).toBeDefined();
    expect(screen.getByText(bulkReplayScopeText(FULL))).toBeDefined();
    expect(screen.getByText('Enter a whole number from 1 to 10000.')).toBeDefined();
    expect(firstButton().disabled).toBe(true);
  });

  it.each(['0', '10001', '7.5', '0x10'])('keeps the first button disabled for %j, so no confirmation opens without a valid number', async (value) => {
    const user = userEvent.setup();
    render(page(actions()));

    await user.type(maxRowsInput(), value);

    expect(firstButton().disabled).toBe(true);
  });

  it('asks first; Cancel returns; Confirm submits the budget and the three scope fields from ONE form', async () => {
    const user = userEvent.setup();
    const a = actions();
    render(page(a));

    await user.type(maxRowsInput(), '500');
    await user.click(firstButton());
    const text = bulkReplayConfirmation({ ...FULL, maxRows: 500 });
    expect(within(bulk()).getByText(text)).toBeDefined();
    expect(maxRowsInput().readOnly).toBe(true);
    await user.click(within(bulk()).getByRole('button', { name: 'Cancel' }));
    expect(within(bulk()).queryByText(text)).toBeNull();
    expect(a.bulkReplay).not.toHaveBeenCalled();

    await user.click(firstButton());
    await user.click(within(bulk()).getByRole('button', { name: 'Confirm bulk replay' }));

    await within(region()).findByText('Replayed 2 events.');
    expect(a.bulkReplay).toHaveBeenCalledTimes(1);
    const form = a.bulkReplay.mock.calls[0]?.[1];
    expect(form?.get('maxRows')).toBe('500');
    expect(form?.get('eventType')).toBe(FULL.eventType);
    expect(form?.get('parkedFrom')).toBe(FROM);
    expect(form?.get('parkedTo')).toBe(TO);
    expect(within(bulk()).queryByRole('button', { name: 'Confirm bulk replay' })).toBeNull();
  });

  it('runs nothing when the form is submitted in step 1, as Enter in the input does', () => {
    const a = actions();
    render(page(a));

    fireEvent.change(maxRowsInput(), { target: { value: '500' } });
    fireEvent.submit(bulk());

    expect(a.bulkReplay).not.toHaveBeenCalled();
  });

  it('disables the buttons of both steps while the frame is busy', async () => {
    const user = userEvent.setup();
    let finish: (state: ActionState) => void = () => undefined;
    render(
      page(
        actions({
          replay: () =>
            new Promise<ActionState>((resolve) => {
              finish = resolve;
            }),
        }),
      ),
    );

    await user.type(maxRowsInput(), '500');
    await user.click(firstButton());
    const confirm = within(bulk()).getByRole<HTMLButtonElement>('button', { name: 'Confirm bulk replay' });
    const cancel = within(bulk()).getByRole<HTMLButtonElement>('button', { name: 'Cancel' });

    // A row replay makes the frame busy. isPending can lag the transition's setState, so wait for it.
    await user.click(screen.getByRole('button', { name: 'Replay' }));
    await waitFor(() => {
      expect(confirm.disabled).toBe(true);
      expect(cancel.disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(cancel.disabled).toBe(false);
    });

    // Step 1 too: with a valid budget, the first button still waits while the frame is busy.
    await user.click(cancel);
    await user.click(screen.getByRole('button', { name: 'Replay' }));
    await waitFor(() => {
      expect(firstButton().disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(firstButton().disabled).toBe(false);
    });
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/bulk-replay-form.test.tsx
```

Expected: FAIL: the import `../../app/(console)/dead-letters/bulk-replay-form` does not resolve.

- [ ] **Step 3: Write the form**

Create `ts/apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The bulk-replay form of /iam/dead-letters (SMA-661 spec § 6.2–§ 6.4). CLIENT component. It sits
// under the filter form, and it is BOUND to the filter (D1): it shows the scope as read-only text and
// posts it as three hidden fields, so the operator cannot type a second, unseen scope. The operator
// types only the row budget. The binding is a user-interface convention, not a server-enforced
// property (R7): a Root caller can post any scope.
//
// ONE <form> wraps both steps, unlike ArchiveButton (which renders no form in step 1): the budget
// and the hidden scope must be inside the element that submits. The budget is CONTROLLED state,
// because the confirmation names it (AC 2), and the first button stays disabled until it is a whole
// number from 1 to the ceiling. The form hands its FormData to the frame's runner, as every row
// control does; it never uses useActionState, because the frame owns the only result region.
'use client';

import { useState, type FormEvent, type ReactElement } from 'react';
import { Field, Input, PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { useRunner } from './dead-letters-frame';
import { MAX_ROWS_PATTERN } from './max-rows';

/** The scope of a bulk replay: the list filter as CANONICAL values. '' is no filter. */
export type BulkReplayScope = { readonly eventType: string; readonly parkedFrom: string; readonly parkedTo: string };

/** § 6.4 point 3. `50` is lib/paging.ts's PAGE_SIZE, which a client module cannot import; a unit test holds the two equal. */
const BEYOND_THE_PAGE = 'The scope can hold events this page does not show. A page holds 50 events, and the budget can be larger.';

/** § 6.4 point 4: the two risks (R1, R2). */
const BULK_RISKS =
  'A replay can publish an event a second time when the broker deduplication window has passed. A cancelled or timed-out request can leave an unknown number of events already replayed; running it again is safe, because a replayed event is no longer parked.';

/** § 6.4 point 5 (fact 7): a row with no parked_at never satisfies a time bound. */
const NO_PARKED_TIME = 'An event with no parked time is not in this scope.';

/** § 6.4 point 2 and D5: an empty scope is allowed, and it is named in words. */
const EVERY_PARKED_EVENT = 'The scope is EVERY parked event: no event type and no parked-time window.';

function eventWords(eventType: string): string {
  return eventType === '' ? 'any event type' : `event type ${eventType}`;
}

/** The window in words. Both bounds are inclusive (fact 8); an absent bound is named as absent. */
function windowWords(scope: BulkReplayScope): string {
  if (scope.parkedFrom !== '' && scope.parkedTo !== '') return `parked from ${scope.parkedFrom} to ${scope.parkedTo}, both bounds included`;
  if (scope.parkedFrom !== '') return `parked from ${scope.parkedFrom}, the bound included, with no end`;
  if (scope.parkedTo !== '') return `parked up to ${scope.parkedTo}, the bound included, with no start`;
  return 'any parked time';
}

/** § 6.3: the read-only scope line above the budget. */
export function bulkReplayScopeText(scope: BulkReplayScope): string {
  if (scope.parkedFrom === '' && scope.parkedTo === '') return `Scope: ${eventWords(scope.eventType)}, any parked time`;
  return `Scope: ${eventWords(scope.eventType)}, parked ${scope.parkedFrom === '' ? '(no start)' : scope.parkedFrom} → ${scope.parkedTo === '' ? '(no end)' : scope.parkedTo}`;
}

/**
 * § 6.4: the confirmation. It names the budget EXACTLY (AC 2): the form refused every value IAM would
 * clamp. Then the scope in words, that the scope reaches past the page, the two risks, and the NULL
 * blind spot when either bound is set.
 */
export function bulkReplayConfirmation(input: BulkReplayScope & { readonly maxRows: number }): string {
  const windowed = input.parkedFrom !== '' || input.parkedTo !== '';
  const budget = `Replay up to ${String(input.maxRows)} parked ${input.maxRows === 1 ? 'event' : 'events'}.`;
  const scope = input.eventType === '' && !windowed ? EVERY_PARKED_EVENT : `Scope: ${eventWords(input.eventType)}, ${windowWords(input)}.`;
  return [budget, scope, BEYOND_THE_PAGE, BULK_RISKS, ...(windowed ? [NO_PARKED_TIME] : [])].join(' ');
}

/** The budget the operator typed, or null while the first button must stay disabled (§ 6.4). */
export function maxRowsOf(raw: string, ceiling: number): number | null {
  const value = raw.trim();
  if (!MAX_ROWS_PATTERN.test(value)) return null;
  const rows = Number(value);
  return rows >= 1 && rows <= ceiling ? rows : null;
}

/** `ceiling` is MAX_BULK_REPLAY_ROWS: the page passes it, because @paigasus/console-core is server-only. */
export function BulkReplayForm({ scope, ceiling }: { readonly scope: BulkReplayScope; readonly ceiling: number }): ReactElement {
  const runner = useRunner();
  const [maxRows, setMaxRows] = useState('');
  const [confirming, setConfirming] = useState(false);
  const rows = maxRowsOf(maxRows, ceiling);

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    // Enter in the budget input submits a form that has one text input and no submit button, even in
    // step 1. Only the confirm button of step 2 may run the action.
    if (!confirming) return;
    runner.runBulk(new FormData(event.currentTarget));
    setConfirming(false);
  }

  return (
    <section aria-labelledby="dead-letters-bulk-heading" className="rounded-pgs flex flex-col gap-2 border p-4">
      <h2 id="dead-letters-bulk-heading" className="text-lg font-semibold">
        Bulk replay
      </h2>
      <p className="text-sm">{bulkReplayScopeText(scope)}</p>
      <form aria-label="Bulk replay" className="flex flex-col gap-2" onSubmit={submit}>
        <input type="hidden" name="eventType" value={scope.eventType} />
        <input type="hidden" name="parkedFrom" value={scope.parkedFrom} />
        <input type="hidden" name="parkedTo" value={scope.parkedTo} />
        <Field label="Max rows" htmlFor="dead-letters-max-rows" description={`Enter a whole number from 1 to ${String(ceiling)}.`}>
          <Input
            name="maxRows"
            value={maxRows}
            inputMode="numeric"
            autoComplete="off"
            readOnly={confirming}
            onChange={(event) => {
              setMaxRows(event.target.value);
            }}
          />
        </Field>
        {confirming && rows !== null ? (
          <div className="flex flex-col gap-2">
            <p className="text-sm">{bulkReplayConfirmation({ ...scope, maxRows: rows })}</p>
            <div className="flex flex-wrap gap-2">
              <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
                Confirm bulk replay
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
          </div>
        ) : (
          <div>
            <button
              type="button"
              disabled={runner.busy || rows === null}
              className={SECONDARY_BUTTON_CLASS}
              onClick={() => {
                setConfirming(true);
              }}
            >
              Bulk replay…
            </button>
          </div>
        )}
      </form>
    </section>
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

Same command as Step 2. Expected: all pass.

- [ ] **Step 5: Prove the step-1 guard bites**

In `submit`, change `if (!confirming) return;` to `if (!confirming && false) return;`. Run the
Step 2 command. Expected: FAIL in `runs nothing when the form is submitted in step 1…`. Restore
`if (!confirming) return;`.

- [ ] **Step 6: Prove the budget gate bites (AC 2)**

Change `disabled={runner.busy || rows === null}` to `disabled={runner.busy}`. Run the Step 2
command. Expected: FAIL in the four `keeps the first button disabled for …` rows and in
`shows the scope as text…`. Restore `disabled={runner.busy || rows === null}`.

- [ ] **Step 7: Typecheck, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write 'apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx' apps/iam-console/tests/unit/bulk-replay-form.test.tsx
cd apps/iam-console && pnpm run typecheck && pnpm exec vitest run tests/unit/bulk-replay-form.test.tsx
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add 'ts/apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx' ts/apps/iam-console/tests/unit/bulk-replay-form.test.tsx
git commit -m "feat(ts): add the bulk-replay form (SMA-661)" -m "One form holds the budget and the hidden filter scope. The confirmation opens only for a
budget from 1 to the ceiling, and it names that budget, the scope, the page limit, the
two risks and the NULL parked-time blind spot (AC 2)." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Page wiring — the replay question, the bulk form, and `canReplay`

**Files:**
- Modify (whole file): `ts/apps/iam-console/app/(console)/dead-letters/page.tsx`
- Test: `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts`

**Interfaces:**
- Consumes: Task 1's `MAX_BULK_REPLAY_ROWS`; `ROOT_PRN`; `mayI` from `lib/console`
  (`() => Promise<MayI>`, `lib/console.ts:19`); Task 6's `DeadLetterTable` `canReplay` prop;
  Task 7's `BulkReplayForm`.
- Produces: the final page. `may('ReplayOutboxDeadLetter', ROOT_PRN)` runs in parallel with the
  list read and never for a 404, a degraded IAM or a refused query.

- [ ] **Step 1: Write the failing tests**

In `ts/apps/iam-console/tests/unit/dead-letters-page.test.ts`:

(a) Replace the header comment (from Task 4) with:

```ts
// /iam/dead-letters, branch by branch (SMA-629 spec § 6.2–§ 6.4; SMA-661 spec § 4.2–§ 4.5, § 6.2,
// § 8). The page's own accessors are mocked at the lib/console boundary, and the test reads the
// element tree the page returns. It proves: the 404 gate, the degraded view inside the frame with no
// IAM call, a refused query that never becomes an IAM call, a refused parked bound that re-renders
// the filter form (D6), the 403/404 list errors as PageError, every other list error as a
// SectionError inside the frame, the frame key, the GET form with no `action`, the paging hrefs, and
// the replay question: asked at Root only when the list is read, hiding the bulk form and the row
// Replay buttons on no (D4).
```

(b) After the `import { Field, Input } from '@paigasus/ui';` line, add:

```ts
import { MAX_BULK_REPLAY_ROWS, ROOT_PRN } from '@paigasus/console-core';
```

(c) Replace the `mocks` block and the `vi.mock` call (lines 14-25) with:

```ts
type May = (action: string, resourcePrn: string) => Promise<boolean>;

const mocks = vi.hoisted(() => ({
  state: { current: { state: 'absent', service: 'iam' } },
  listDeadLetters: vi.fn(),
  iamClients: vi.fn(),
  may: vi.fn<May>(),
  mayI: vi.fn<() => Promise<May>>(),
}));

vi.mock('../../lib/console', () => ({
  sessionToken: () => Promise.resolve('tok-page'),
  discovery: () => ({ getServiceState: () => Promise.resolve(mocks.state.current) }),
  iamClients: mocks.iamClients,
  iamClientsForAction: vi.fn(),
  mayI: mocks.mayI,
}));
```

(d) After `const { DeadLetterTable } = await import(…);`, add:

```ts
const { BulkReplayForm } = await import('../../app/(console)/dead-letters/bulk-replay-form');
```

(e) Replace the top-level `beforeEach` with:

```ts
beforeEach(() => {
  mocks.listDeadLetters.mockReset();
  mocks.iamClients.mockReset();
  mocks.iamClients.mockImplementation(() => Promise.resolve({ outbox: { listDeadLetters: mocks.listDeadLetters } }));
  mocks.may.mockReset();
  mocks.may.mockResolvedValue(true);
  mocks.mayI.mockReset();
  mocks.mayI.mockImplementation(() => Promise.resolve(mocks.may));
});
```

(f) Add `expect(mocks.mayI).not.toHaveBeenCalled();` as the last line of: the 404 gate test, the
degraded test, the `refuses %s as a PageError…` test, and the `D6: a refused %s…` test.

(g) Append at the end of the file:

```ts

describe('the replay affordance (SMA-661 D4, § 6.2, § 8)', () => {
  beforeEach(() => {
    mocks.state.current = WITH_KEY;
  });

  it('asks ReplayOutboxDeadLetter at Root and, on yes, renders the bulk form between the filter and the table', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [entry], nextCursor: '' });

    const tree = await visit({ eventType: 'orders', parkedFrom: '2026-09-19T00:00:00Z' });

    expect(mocks.may).toHaveBeenCalledWith('ReplayOutboxDeadLetter', ROOT_PRN);
    const bulk = all(tree, BulkReplayForm);
    expect(bulk).toHaveLength(1);
    expect(bulk[0]?.props).toEqual({ scope: { eventType: 'orders', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '' }, ceiling: MAX_BULK_REPLAY_ROWS });
    expect((all(tree, DeadLetterTable)[0]?.props as { canReplay: boolean }).canReplay).toBe(true);

    const order = [...walk(tree)].map((element) => element.type);
    expect(order.indexOf('form')).toBeLessThan(order.indexOf(BulkReplayForm));
    expect(order.indexOf(BulkReplayForm)).toBeLessThan(order.indexOf(DeadLetterTable));
  });

  it('on no, hides the bulk form and passes canReplay=false to the table, whose rows keep Discard', async () => {
    mocks.may.mockResolvedValue(false);
    mocks.listDeadLetters.mockResolvedValue({ entries: [entry], nextCursor: '' });

    const tree = await visit({});

    expect(all(tree, BulkReplayForm)).toHaveLength(0);
    expect((all(tree, DeadLetterTable)[0]?.props as { canReplay: boolean }).canReplay).toBe(false);
  });

  it('renders the bulk form over an empty list', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [], nextCursor: '' });

    const tree = await visit({});

    expect(all(tree, BulkReplayForm)).toHaveLength(1);
  });

  it('renders no bulk form beside a SectionError', async () => {
    mocks.listDeadLetters.mockRejectedValue(new ConnectError('nope', Code.Unavailable));

    const tree = await visit({});

    expect(all(tree, SectionError)).toHaveLength(1);
    expect(all(tree, BulkReplayForm)).toHaveLength(0);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/dead-letters-page.test.ts
```

Expected: FAIL in the four new cases (no `may` call, no `BulkReplayForm`, `canReplay` undefined).

- [ ] **Step 3: Write the final page**

Replace the whole of `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2–§ 6.4, AC 1 and AC 2; SMA-661). The page asks discovery for
// the gate: the Dead letters nav entry is the affordance for the SCREEN, and mayI() hides it; a typed
// URL is a user action, and IAM answers it (SMA-511 § 6.3). Since SMA-661 (D4) the page also asks
// mayI() ONE question, ReplayOutboxDeadLetter at Root, for the replay affordance: the bulk-replay form
// and every row's Replay button. That question is cosmetic and FAILS OPEN (a failed query shows the
// controls), so IAM still decides every action, and every OutboxService RPC is Root-only inside IAM.
// It costs one more IsAuthorized call per render of this page, as SMA-629's layout recorded for its
// own two questions. Discard asks nothing: it is a separate Cedar action. A hidden screen (the 404), a
// degraded IAM and a refused query ask no question at all.
//
// Every view that does not throw renders inside DeadLettersFrame, keyed by
// `${eventType}|${parkedFrom}|${parkedTo}|${cursor}` with the CANONICAL bounds (SMA-661 spec § 4.2),
// so a revalidated render after an action keeps the frame and its result, two spellings of one window
// share a frame, and a move to another page or filter starts an empty frame. The four query parsers
// are pure and run before the degraded branch ONLY to compute that key; a degraded IAM gets the
// degraded view whatever the query.
import type { ReactElement, ReactNode } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { MAX_BULK_REPLAY_ROWS, ROOT_PRN } from '@paigasus/console-core';
import { EmptyState, ErrorState, Field, Input, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { SectionError } from '../../_components/section-error';
import { discovery, iamClients, mayI, sessionToken } from '../../../lib/console';
import { listHref, MAX_EVENT_TYPE_LENGTH, MAX_PARKED_BOUND_LENGTH, parseCursor, parseEventType, parseParkedBound, type ParkedBoundField, type ParsedParkedBound } from '../../../lib/paging';
import { bulkReplayDeadLettersAction, discardDeadLetterAction, replayDeadLetterAction } from './actions';
import { BulkReplayForm } from './bulk-replay-form';
import { DeadLetterTable } from './dead-letter-table';
import { DeadLettersFrame } from './dead-letters-frame';
import { deadLettersGate, loadDeadLettersPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

/** The full path, as the ingress sees it (ZoneLink needs it). */
const PATH = '/iam/dead-letters';

/** SMA-661 spec § 4.3. Both bounds are inclusive in IAM's query (`>=` and `<=`), and the input cannot show that. */
const PARKED_PLACEHOLDER = '2026-09-19T00:00:00Z';
const PARKED_DESCRIPTION = 'Both bounds are included. Use a zone, for example 2026-09-19T00:00:00Z or +02:00.';

/** The three filter values: the event type, and each parked bound as parsed (typed value, canonical value or error). */
type Filter = { readonly eventType: string; readonly parkedFrom: ParsedParkedBound; readonly parkedTo: ParsedParkedBound };

/** The breadcrumbs, the heading and the frame around one view. A plain function, not a component. */
function shell(frameKey: string, body: ReactNode): ReactElement {
  return (
    <div className="flex flex-col gap-6 p-6">
      <Breadcrumbs items={[{ label: 'Dead letters' }]} />
      <h1 className="text-2xl font-semibold">Dead letters</h1>
      <DeadLettersFrame key={frameKey} actions={{ replay: replayDeadLetterAction, discard: discardDeadLetterAction, bulkReplay: bulkReplayDeadLettersAction }}>
        {body}
      </DeadLettersFrame>
    </div>
  );
}

/**
 * One parked-time input (SMA-661 spec § 4.3). It shows the TYPED value, so a refused bound keeps
 * what the operator wrote (D6). A refused bound's sentence goes to Field's `error`, which wires
 * aria-describedby and aria-invalid. `maxLength` bounds typing only; the parser never cuts a value.
 */
function parkedField(label: string, id: string, name: ParkedBoundField, bound: ParsedParkedBound): ReactElement {
  const input = <Input name={name} defaultValue={bound.raw} placeholder={PARKED_PLACEHOLDER} maxLength={MAX_PARKED_BOUND_LENGTH} autoComplete="off" />;
  return bound.ok ? (
    <Field label={label} htmlFor={id} description={PARKED_DESCRIPTION}>
      {input}
    </Field>
  ) : (
    <Field label={label} htmlFor={id} description={PARKED_DESCRIPTION} error={bound.error.message}>
      {input}
    </Field>
  );
}

/**
 * A plain GET form with NO `action` attribute: it submits to the current URL, /iam/dead-letters. A
 * basePath-relative action="/dead-letters" would leave the zone (§ 6.3). It needs no JavaScript, and a
 * new filter starts at the first page.
 */
function filterForm(filter: Filter): ReactElement {
  return (
    <form method="get" aria-label="Filter dead letters" className="flex flex-wrap items-end gap-2">
      <Field label="Event type" htmlFor="dead-letters-event-type">
        <Input name="eventType" defaultValue={filter.eventType} maxLength={MAX_EVENT_TYPE_LENGTH} autoComplete="off" />
      </Field>
      {parkedField('Parked from', 'dead-letters-parked-from', 'parkedFrom', filter.parkedFrom)}
      {parkedField('Parked to', 'dead-letters-parked-to', 'parkedTo', filter.parkedTo)}
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
  const parkedFrom = parseParkedBound(query.parkedFrom, 'parkedFrom');
  const parkedTo = parseParkedBound(query.parkedTo, 'parkedTo');
  const cursor = parseCursor(query.cursor);
  const frameKey = `${eventType.ok ? eventType.value : ''}|${parkedFrom.ok ? parkedFrom.iso : ''}|${parkedTo.ok ? parkedTo.iso : ''}|${cursor.ok ? cursor.cursor : ''}`;

  if (gate === 'degraded') {
    return shell(
      frameKey,
      <div data-testid="dead-letters-degraded">
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>,
    );
  }

  // Every parser runs BEFORE the clients are built: a refused query never becomes an IAM call.
  if (!eventType.ok) return <PageError error={eventType.error} />;
  if (!cursor.ok) return <PageError error={cursor.error} />;
  const filter: Filter = { eventType: eventType.value, parkedFrom, parkedTo };
  // SMA-661 D6: a refused bound is a typing mistake, not a hand-edited cursor. The filter form comes
  // back with every typed value and the field's error. There is no table, no bulk form and no IAM call.
  if (!parkedFrom.ok || !parkedTo.ok) return shell(frameKey, filterForm(filter));

  // SMA-661 § 8: the replay question never runs serially with the list read.
  const [may, clients] = await Promise.all([mayI(), iamClients()]);
  const [canReplay, data] = await Promise.all([
    may('ReplayOutboxDeadLetter', ROOT_PRN),
    loadDeadLettersPage({ outbox: clients.outbox }, { cursor: cursor.cursor, eventType: eventType.value, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso }),
  ]);
  if (!data.ok) {
    // A fresh GET keeps its real 403 or 404. Every other list error stays inside the frame. There is
    // no bulk form here: the table is the evidence for the scope (D1), and there is no table.
    if (data.error.presentation === 'forbidden' || data.error.presentation === 'not-found') return <PageError error={data.error} />;
    return shell(
      frameKey,
      <>
        {filterForm(filter)}
        <SectionError error={data.error} />
      </>,
    );
  }

  const { rows, nextCursor } = data.value;
  // The paging links carry the CANONICAL filter (SMA-661 spec § 4.5). listHref leaves out an empty value.
  const kept = { eventType: eventType.value, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso };
  return shell(
    frameKey,
    <>
      {filterForm(filter)}
      {/* SMA-661 § 6.2: under the filter that defines it, and also over an empty list. */}
      {canReplay ? <BulkReplayForm scope={kept} ceiling={MAX_BULK_REPLAY_ROWS} /> : null}
      {rows.length === 0 ? <EmptyState title="No dead letters" /> : <DeadLetterTable canReplay={canReplay} rows={rows} />}
      <nav aria-label="Dead-letter pages" className="flex gap-4 text-sm">
        {cursor.cursor === '' ? null : (
          <ZoneLink href={listHref(PATH, kept)} className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={listHref(PATH, { ...kept, cursor: nextCursor })} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </>,
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

Same command as Step 2. Expected: all pass.

- [ ] **Step 5: Prove the D4 hide bites**

Change `may('ReplayOutboxDeadLetter', ROOT_PRN),` to
`may('ReplayOutboxDeadLetter', ROOT_PRN).then(() => true),` (it compiles, and `may` stays used).
Run the Step 2 command. Expected: FAIL in `on no, hides the bulk form…`. Restore
`may('ReplayOutboxDeadLetter', ROOT_PRN),`.

- [ ] **Step 6: Typecheck, run the whole unit tier, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write 'apps/iam-console/app/(console)/dead-letters/page.tsx' apps/iam-console/tests/unit/dead-letters-page.test.ts
cd apps/iam-console && pnpm run typecheck && pnpm exec vitest run tests/unit tests/integration
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add 'ts/apps/iam-console/app/(console)/dead-letters/page.tsx' ts/apps/iam-console/tests/unit/dead-letters-page.test.ts
git commit -m "feat(ts): show the bulk-replay form and gate Replay on mayI (SMA-661)" -m "The page asks ReplayOutboxDeadLetter at Root in parallel with the list read. A yes shows
the bulk form under the filter; a no hides it and every row Replay button, and Discard
stays. The question fails open, so IAM still decides." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

Expected before the commit: every unit and integration file passes. The standalone-runtime tests
under `tests/` that need a staged build may fail if no build exists; Task 10 runs the whole suite
after a build.

---

### Task 9: The e2e world and row R20 (AC 3 and AC 4, e2e half)

**Files:**
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts` (header `:13-15`, fixtures `:81-155`, handlers `:201, 273-279`)
- Modify: `ts/apps/iam-console/tests/e2e/dead-letters.spec.ts`
- Modify: `ts/apps/iam-console/tests/unit/e2e-rows.test.ts:3-8, 15, 17`

**Interfaces:**
- Consumes: the whole app from Tasks 1–8 (through a build).
- Produces (e2e only):
  - `export type DeadLetterFixture`
  - `export const DEAD_LETTER_C_ID = '0190a1f0-0000-7000-8000-0000000000d3';`, `export const DEAD_LETTER_C: DeadLetterFixture`
  - `export function seededDeadLetters(extra?: readonly DeadLetterFixture[]): Map<string, DeadLetterFixture>;`
  - `export function deadLetterHandlers(deadLetters: Map<string, DeadLetterFixture>, pageSize?: number): FakeIamHandlers;`

**Deviations (the real code contradicts the spec):**
- Spec fact 15 says `startFakeIam`'s `options.overrides` merges into the world. In the code,
  `startFakeIam` takes only `{ handlers }` (`ts/packages/paigasus-console-core/testing/fake-iam.ts:221`);
  `overrides` is `WorldOptions.overrides` of `worldHandlers` (`world.ts:99, 280`), which the
  harness's `useWorld` passes on (`harness.ts:187-189`). The seeded map lives inside
  `worldHandlers`' closure, so an override cannot ADD an entry to it. This plan therefore makes the
  outbox handlers a factory, `deadLetterHandlers(map, pageSize)`, and R20 passes a whole new outbox
  handler set through `overrides`. R17–R19 keep the two-entry world.
- Spec § 7.4 point 3 wants page 1 to answer a next cursor "when the filtered set is larger than the
  page". The console always asks for `limit: 50`, so three entries never fill a page. The factory
  takes a `pageSize` (R20 uses 1), and the fake pages by keyset: the cursor is the last id of the
  previous page, and the next page holds the smaller ids.

**R20's data.** A (`…d1`, parked 2026-08-29T10:45:00Z) and B (`…d2`, 10:55) are the seeded pair.
C (`…d3`, parked 2026-08-17T20:53:20Z) is new. The window starts at 2026-08-29T10:40:00Z, typed as
`2026-08-29 10:40z` (a space, no seconds, a lower-case `z`). So the window holds B and A, and not C.

- [ ] **Step 1: Write the failing row-count test**

In `ts/apps/iam-console/tests/unit/e2e-rows.test.ts`, replace lines 5-8:

```ts
// The SMA-629 spec (docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md,
// § 7.4) adds three, R17–R19. Each row must have exactly one Playwright test whose title starts with
// its row id. A deleted or renamed scenario then fails this vitest suite, which runs in
// iam-console-ts:test on every PR that touches the app, even when the e2e task does not run.
```

with:

```ts
// The SMA-629 spec (docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md,
// § 7.4) adds three, R17–R19. The SMA-661 spec
// (docs/superpowers/specs/2026-09-21-sma-661-dead-letters-bulk-replay-design.md, § 7.5) adds one,
// R20. Each row must have exactly one Playwright test whose title starts with its row id. A deleted
// or renamed scenario then fails this vitest suite, which runs in iam-console-ts:test on every PR
// that touches the app, even when the e2e task does not run.
```

Change line 15's `length: 19` to `length: 20`, and change line 17's title to
`'the e2e tier covers every row of SMA-511 § 9.4, SMA-630 § 9.3, SMA-629 § 7.4 and SMA-661 § 7.5'`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec vitest run tests/unit/e2e-rows.test.ts
```

Expected: FAIL in `R20 has exactly one test` (0 found).

- [ ] **Step 2: Make the world window-aware, cursor-aware and bulk-aware**

In `ts/apps/iam-console/tests/e2e/support/world.ts`:

(a) Replace header lines 13-15:

```ts
// SMA-629: three outbox handlers are in the default set too, because every override key must exist
// there. They hold two seeded dead letters with RFC 4122 ids, per worldHandlers() call; replay and
// discard remove the entry, and an unknown id answers NotFound, as IAM does.
```

with:

```ts
// SMA-629: the outbox handlers are in the default set too, because every override key must exist
// there. They hold two seeded dead letters with RFC 4122 ids, per worldHandlers() call; replay and
// discard remove the entry, and an unknown id answers NotFound, as IAM does. SMA-661 made them a
// factory, deadLetterHandlers(), so R20 can script a THIRD entry and a page size of one through
// `overrides` without changing the two-entry world that R17 counts. They match the parked-time
// window as IAM does (both bounds inclusive), page with a keyset cursor, and serve bulk replay.
```

(b) Change `type DeadLetterFixture = {` (line 104) to `export type DeadLetterFixture = {`.

(c) Replace `function seededDeadLetters(): Map<string, DeadLetterFixture> {` and its entries array
opening (lines 118-119) with:

```ts
/** The two dead letters every world starts with, plus `extra` (SMA-661: R20's third entry). */
export function seededDeadLetters(extra: readonly DeadLetterFixture[] = []): Map<string, DeadLetterFixture> {
  const entries: DeadLetterFixture[] = [
```

and replace the closing of that array, `  ];` followed by
`  return new Map(entries.map((entry) => [entry.id, entry]));` (lines 146-147), with:

```ts
    ...extra,
  ];
  return new Map(entries.map((entry) => [entry.id, entry]));
```

(d) After `takeDeadLetter` (after line 155), insert:

```ts

export const DEAD_LETTER_C_ID = '0190a1f0-0000-7000-8000-0000000000d3';

/**
 * R20's third entry (SMA-661 spec § 7.4). It is NOT in seededDeadLetters()'s default: R17 counts two
 * rows. It was parked on 2026-08-17, before R20's window starts, so the window leaves it out of the
 * list and out of the bulk replay. Its id is the highest, so it heads an unfiltered list.
 */
export const DEAD_LETTER_C: DeadLetterFixture = {
  id: DEAD_LETTER_C_ID,
  occurredAt: { seconds: 1_786_999_700n, nanos: 0 },
  eventType: 'iam.organization.created',
  schemaVersion: 1,
  aggregatePrn: ORG_PRN,
  actorPrn: PRINCIPAL_PRN,
  payload: '{"slug":"acme"}',
  correlationId: 'corr-dead-letter-c',
  attempts: 5,
  parkedAt: { seconds: 1_787_000_000n, nanos: 0 },
  lastError: 'nats: timeout',
};

/** A protobuf Timestamp, as a fixture and a decoded request both carry it. */
type Instant = { readonly seconds: bigint; readonly nanos: number };

const nanosOf = (instant: Instant): bigint => instant.seconds * 1_000_000_000n + BigInt(instant.nanos);

/** The list and bulk-replay filter. An absent bound is no filter (iam.proto:662-665). */
type DeadLetterScope = { readonly eventType: string; readonly parkedFrom?: Instant | undefined; readonly parkedTo?: Instant | undefined };

/** IAM's match: event_type exactly, `parked_at >= from` and `parked_at <= to` (pg_dead_letters.rs:100,104). */
function inScope(entry: DeadLetterFixture, scope: DeadLetterScope): boolean {
  if (scope.eventType !== '' && entry.eventType !== scope.eventType) return false;
  if (scope.parkedFrom !== undefined && nanosOf(entry.parkedAt) < nanosOf(scope.parkedFrom)) return false;
  return scope.parkedTo === undefined || nanosOf(entry.parkedAt) <= nanosOf(scope.parkedTo);
}

/**
 * The four outbox handlers over one map of dead letters (SMA-661 spec § 7.4). `pageSize` bounds a
 * list page below the console's limit of 50, so a test can page through a few entries; the default
 * leaves the request's limit alone. The cursor is keyset, like IAM's: the last id of the previous
 * page, and the next page holds the ids below it.
 */
export function deadLetterHandlers(deadLetters: Map<string, DeadLetterFixture>, pageSize = Number.POSITIVE_INFINITY): FakeIamHandlers {
  // IAM orders by id DESCENDING.
  const matching = (scope: DeadLetterScope): DeadLetterFixture[] => [...deadLetters.values()].filter((entry) => inScope(entry, scope)).sort((a, b) => (a.id < b.id ? 1 : -1));
  return {
    'outbox.listDeadLetters': (req) => {
      const rest = matching(req).filter((entry) => req.cursor === '' || entry.id < req.cursor);
      const entries = rest.slice(0, Math.min(req.limit, pageSize));
      const last = entries.at(-1);
      return { entries, nextCursor: rest.length > entries.length && last !== undefined ? last.id : '' };
    },
    'outbox.replayDeadLetter': (req) => ({ entry: takeDeadLetter(deadLetters, req.id) }),
    'outbox.discardDeadLetter': (req) => ({ entry: takeDeadLetter(deadLetters, req.id) }),
    // At most max_rows of the matching entries, newest first. There is no 10000 clamp here: the console
    // refuses a larger budget before it calls.
    'outbox.bulkReplayDeadLetters': (req) => {
      const replayed = matching(req).slice(0, Number(req.maxRows));
      for (const entry of replayed) deadLetters.delete(entry.id);
      return { replayed: BigInt(replayed.length) };
    },
  };
}
```

(e) In `worldHandlers`, delete the line `  const deadLetters = seededDeadLetters();` (line 201), and
replace the three outbox handlers (lines 273-279, from `// SMA-629. IAM orders by id DESCENDING…`
to the `'outbox.discardDeadLetter'` line) with:

```ts
    // SMA-629 and SMA-661: the four outbox handlers over this world's own two seeded entries.
    ...deadLetterHandlers(seededDeadLetters()),
```

- [ ] **Step 3: Write R20**

In `ts/apps/iam-console/tests/e2e/dead-letters.spec.ts`, replace the header and the imports
(lines 1-8) with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-629 AC 1 and AC 2 (spec § 7.4), and SMA-661 AC 3 and AC 4 (spec § 7.5, row R20).
// PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each page load probes the fake's GET
// /v1/service-info again, and a test changes the IAM state between two loads. Discard and bulk
// replay need JavaScript, so every page.goto before a click waits for hydration.
import { DEAD_LETTER_A_ID, DEAD_LETTER_B_ID, DEAD_LETTER_C, DEAD_LETTER_C_ID, DEAD_LETTERS_DESCRIPTOR, deadLetterHandlers, seededDeadLetters } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

/** R20's window start, typed in a D6 spelling: a space, no seconds, a lower-case z (SMA-661 spec § 4.1). */
const WINDOW_FROM_TYPED = '2026-08-29 10:40z';
const WINDOW_FROM_CANONICAL = '2026-08-29T10:40:00.000Z';
/** 2026-08-29T10:40:00Z. A (10:45) and B (10:55) are inside the window; C (2026-08-17) is not. */
const WINDOW_FROM_SECONDS = 1_788_000_000n;
```

Append at the end of the file:

```ts

test('R20: a parked-time filter reaches IAM and survives paging, and one bulk replay runs against the fake IAM (SMA-661 AC 3, AC 4)', async ({ page, harness }) => {
  // Three entries and one row per page, through `overrides`, so R17's two-entry world stays as it is.
  harness.useWorld({ descriptor: DEAD_LETTERS_DESCRIPTOR, overrides: deadLetterHandlers(seededDeadLetters([DEAD_LETTER_C]), 1) });
  await signIn(page, harness);

  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  const rows = page.getByTestId('dead-letter-row');
  const region = page.getByTestId('dead-letters-result');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_C_ID);

  // AC 3, part 1: the typed spelling reaches IAM as the canonical instant, and C leaves the list.
  await page.getByLabel('Parked from').fill(WINDOW_FROM_TYPED);
  await page.getByRole('button', { name: 'Filter' }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('parkedFrom')).toBe(WINDOW_FROM_TYPED);
  await waitForHydration(page);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_B_ID);
  await expect(rows).toHaveCount(1);
  const firstPage = harness.iam.callsTo('outbox.listDeadLetters').at(-1)?.request;
  expect(firstPage).toMatchObject({ cursor: '', parkedFrom: { seconds: WINDOW_FROM_SECONDS, nanos: 0 } });
  expect((firstPage as { parkedTo?: unknown }).parkedTo).toBeUndefined();

  // AC 3, part 2: the Next link carries the CANONICAL bound, and the input converges on it.
  await page.getByRole('navigation', { name: 'Dead-letter pages' }).getByRole('link', { name: 'Next' }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('cursor')).toBe(DEAD_LETTER_B_ID);
  // A client navigation keeps the attribute, so this returns at once; a full load waits here.
  await waitForHydration(page);
  expect(new URL(page.url()).searchParams.get('parkedFrom')).toBe(WINDOW_FROM_CANONICAL);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_A_ID);
  await expect(page.getByLabel('Parked from')).toHaveValue(WINDOW_FROM_CANONICAL);
  expect(harness.iam.callsTo('outbox.listDeadLetters').at(-1)?.request).toMatchObject({ cursor: DEAD_LETTER_B_ID, parkedFrom: { seconds: WINDOW_FROM_SECONDS, nanos: 0 } });

  // AC 4: one bulk replay of the filtered scope. The confirmation names the budget before any call.
  const bulkBefore = harness.iam.callsTo('outbox.bulkReplayDeadLetters').length;
  const bulk = page.getByRole('form', { name: 'Bulk replay' });
  await bulk.getByLabel('Max rows').fill('5');
  await bulk.getByRole('button', { name: 'Bulk replay…' }).click();
  await expect(bulk.getByText('Replay up to 5 parked events.', { exact: false })).toBeVisible();
  expect(harness.iam.callsTo('outbox.bulkReplayDeadLetters').length).toBe(bulkBefore);
  await bulk.getByRole('button', { name: 'Confirm bulk replay' }).click();

  // A and B are in the window and C is not, so a budget of 5 replays 2.
  await expect(region).toContainText('Replayed 2 events.');
  const bulkCalls = harness.iam.callsTo('outbox.bulkReplayDeadLetters').slice(bulkBefore);
  expect(bulkCalls).toHaveLength(1);
  expect(bulkCalls[0]?.request).toMatchObject({ eventType: '', parkedFrom: { seconds: WINDOW_FROM_SECONDS, nanos: 0 }, maxRows: 5n });
  expect((bulkCalls[0]?.request as { parkedTo?: unknown }).parkedTo).toBeUndefined();

  // C was outside the window, so it is still parked.
  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_C_ID);
});
```

- [ ] **Step 4: Run the vitest guards**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm run typecheck && pnpm exec vitest run tests/unit/e2e-rows.test.ts tests/unit/world-actions.test.ts tests/unit/e2e-read-only.test.ts
```

Expected: tsc prints nothing after its command line; all three files pass.

- [ ] **Step 5: Build, stage, and run the dead-letters e2e spec**

Run the "Build and stage" block from Global Constraints (expect `STAGED`), then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec playwright test tests/e2e/dead-letters.spec.ts
```

Expected: 4 passed (R17, R18, R19, R20). If R17 fails at its Replay click, `ALL_ACTIONS` lacks
`ReplayOutboxDeadLetter` (Task 2); do not change R17.

- [ ] **Step 6: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec prettier --write apps/iam-console/tests/e2e/support/world.ts apps/iam-console/tests/e2e/dead-letters.spec.ts apps/iam-console/tests/unit/e2e-rows.test.ts
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add ts/apps/iam-console/tests/e2e/support/world.ts ts/apps/iam-console/tests/e2e/dead-letters.spec.ts ts/apps/iam-console/tests/unit/e2e-rows.test.ts
git commit -m "test(ts): add e2e row R20 for the parked filter and bulk replay (SMA-661)" -m "The e2e world matches the parked-time window, pages with a keyset cursor and serves bulk
replay. R20 filters in a D6 spelling, follows the Next link, and runs one bulk replay
against the fake IAM (AC 3, AC 4)." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 10: The runbook line and the full verification

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md:421`

**Interfaces:**
- Consumes: everything.
- Produces: a verified branch.

- [ ] **Step 1: Correct the runbook**

In `docs/ops/RUNBOOK-observability.md`, in the bullet `- **In the IAM console:** …`, replace the
sentence `Bulk replay stays API-only.` with:

```markdown
Since SMA-661 the screen also filters by parked time (both bounds included) and runs a bulk
  replay of the filtered scope. The operator enters a row budget from 1 to 10000, and the
  confirmation names that budget and the scope. A filtered list and a filtered bulk replay both
  leave out a row with no `parked_at`, as described under Confirm above.
```

(Keep the bullet's two-space continuation indent.)

- [ ] **Step 2: Build and stage the current code**

Run the "Build and stage" block from Global Constraints. Expected: `STAGED`.

- [ ] **Step 3: Run both packages' full vitest suites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/packages/paigasus-console-core && pnpm exec vitest run
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console && pnpm exec vitest run
```

Expected: console-core 24 files passed; iam-console 51 files passed (50 before, plus
`bulk-replay-form.test.tsx`), 0 failed.

- [ ] **Step 4: Typecheck both packages**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/packages/paigasus-console-core && pnpm run typecheck
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console && pnpm run typecheck
```

Expected: no output after each `$ tsc -p tsconfig.json --noEmit` line.

- [ ] **Step 5: Run the two whole-tree gates, lint and Prettier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts
pnpm exec eslint .
pnpm exec prettier --check .
```

Expected: eslint prints nothing; Prettier prints `All matched files use Prettier code style!`. If
Prettier reports a file, run `pnpm exec prettier --write <that file>`, re-run the check, and
include the file in Step 7's commit.

- [ ] **Step 6: Run the whole e2e tier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk/ts/apps/iam-console
pnpm exec playwright test
```

Expected: every test passes, R1–R20 included.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-661-dead-letters-bulk
git add docs/ops/RUNBOOK-observability.md
git commit -m "docs(repo): the IAM console runs a filtered bulk replay (SMA-661)" -m "The runbook no longer says that bulk replay is API-only. It names the parked-time filter,
the 1 to 10000 budget and the NULL parked_at blind spot." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
git status --short
```

Expected: `git status --short` prints nothing (the `.next` build output is git-ignored).

- [ ] **Step 8: Report the repo-level gates that this plan did not run**

This plan runs the package suites, both typechecks, `ts:lint`, `ts:fmt` and the iam-console e2e
tier directly. It does not run the full `moon ci` graph from the root `CLAUDE.md`
(`ci-targets` block), which also runs `contracts:generate` against the BSR and the `repo:*`
gates. Tell the coordinator so, and let the coordinator decide whether to run it before the push.
