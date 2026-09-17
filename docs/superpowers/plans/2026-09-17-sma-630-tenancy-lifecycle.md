# SMA-630 Rename, Archive and Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the nine `TenancyService` lifecycle mutations (rename, archive and restore of an organization, a team and a project) to `ts/apps/iam-console`, and show the archived state of each node.

**Architecture:** Each detail route folder gets three zod schemas and three commands (`commands.ts`) and three Server Action shells (`actions.ts`). A server component `ManageSection` renders a controlled rename form and one lifecycle control, which `mayI()` and the node's own status select. The loaders map `status` and `effectiveStatus` to a `NodeLifecycle`, which a header badge and a Status column show. `@paigasus/sdk` gets one guard-free entry, `@paigasus/sdk/iam/types`, for `NodeStatus`. `@paigasus/console-core` gets a runtime `IAM_ACTIONS` array that a test holds to the Rust action catalog.

**Tech Stack:** Next 16.3.4 (App Router, Server Actions), React 19.2, TypeScript 6, zod 4.5, Connect-ES v2, vitest 5 (node and jsdom environments), React Testing Library 16, Playwright 1.63, Moon 2.5.3, pnpm 11.

**Spec:** `docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md` (draft 2). The parent spec is `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` ("511 § n"). Read the spec before each task.

## Global Constraints

- Work ONLY in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-630-tenancy-lifecycle`. Before the first command, run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-630-tenancy-lifecycle branch --show-current`. It must print `feature/sma-630-tenancy-lifecycle`. If it does not, STOP and report. Never `cd` to the parent repository.
- Run every command from the worktree root. Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run foreground commands only. Do not start a background job and end your turn.
- **Add commits, do not amend.** Never `git commit --amend`, never `--no-verify`. If the `commit-msg` hook reports `commitlint not found`, run `pnpm -C ts install` and commit again.
- Commit subject: conventional commit, scope `ts` (or `docs` for a spec edit), lower-case first word, issue key at the END, for example `feat(ts): add the rename commands (SMA-630)`. A subject that starts with `SMA-630` fails commitlint. No body line starts with `#NNN` or `token: value`. End every message with a blank line and `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`. No file has a Windows-reserved base name (`con`, `prn`, `aux`, `nul`, `com1`…, `lpt1`…).
- Relative VALUE imports in `app/**`, `lib/**` and `ts/packages/*/src/**` are EXTENSIONLESS (Turbopack). Test files in `ts/packages/paigasus-sdk/tests/` keep the `.js` suffix, as the existing tests do.
- The app never imports `@paigasus/proto`. It imports `NodeStatus` from `@paigasus/sdk/iam/types` only. No code writes the numbers `0`, `1` or `2` for a status.
- Functional style in Next and React code. No classes.
- Bounds (spec § 4.2), in `lib/form.ts` only, never copied: `slugField = z.string().trim().min(1).max(200)`; `nameField` = trimmed, at least 1 character, **at most 256 code points** (`[...value].length`); `prnField = z.string().trim().min(1).max(512)`. Create and rename use the same `nameField`.
- Rename sends ONLY the changed fields, compared on trimmed values (D6). With no change it still calls IAM with neither field, and IAM answers `nothing-to-rename`.
- A command takes no `mayI`. An action shell calls `iamClientsForAction()`, then `safeParse`, then the command, then `revalidatePath(TENANCY_PATH, 'layout')`. The five existing actions revalidate on success only. The nine new actions revalidate on success AND on presentation `forbidden` or `conflict`, and never on `invalid-input`.
- No `actions.ts` names `mayI`, `iamClients`, `redirect`, `forbidden` or `notFound`. No action calls `forbidden()` (D2).
- D7: the lifecycle control shows only the transition that the node's own status allows. No action and no command applies D7.
- Copy (exact): success texts `Renamed.`, `Archived.`, `Restored.`; buttons `Rename`, `Archive`, `Confirm archive`, `Cancel`, `Restore`; form titles `Rename organization`, `Rename team`, `Rename project`; section heading `Manage`.
- Archive confirmation (exact): `Archive <name>? Until you restore it, IAM refuses changes to it and to everything under it, and the AI Gateway refuses model calls for everything under it.`
- Note (exact): `A parent of this item is archived. Restore the parent to change this item.`
- Header badge: `active` → none, `archived` → `Archived`, `archived-parent` → `Archived (parent)`, `unknown` → `Status unknown`. Status column: `Active`, `Archived`, `Archived (parent)`, `Unknown`.
- `FORM_REASON_COPY[NOTHING_TO_RENAME]` = `Change the slug or the name first.`
- Test ids: `rename-<node>`, `archive-<node>`, `restore-<node>`, each with a `<id>-error` child; `node-status` (badge); `manage-note` (note). `<node>` is `organization`, `team` or `project`.
- Single-file test commands: `pnpm -C ts/apps/iam-console exec vitest run <file>`, `pnpm -C ts/packages/paigasus-console-core exec vitest run <file>`, `pnpm -C ts/packages/paigasus-sdk exec vitest run <file>` (paths relative to that package). Type checks: `moon run iam-console-ts:typecheck`, `moon run paigasus-console-core-ts:typecheck`, `moon run paigasus-sdk-ts:typecheck`. Whole suites: `moon run iam-console-ts:test`, `moon run paigasus-console-core-ts:test`, `moon run paigasus-sdk-ts:test`. E2e: `moon run iam-console-ts:test-e2e`. Use `--force` when you measure a mutation, because a Moon cache hit replays an old PASS.
- Before each commit: `pnpm -C ts exec prettier --write <changed files, relative to ts/>`, then `moon run ts:lint ts:fmt`. `ts:fmt` is a separate Prettier gate; lint can pass while fmt fails.
- Restore a temporary mutation with the Edit tool (remove the exact inserted text). NEVER use `git checkout --` or `git restore`: they also revert uncommitted work.
- Prose in comments and commit messages follows ASD-STE100 Simplified Technical English.

---

## File Structure

| File | Change | Responsibility |
| ---- | ------ | -------------- |
| `ts/packages/paigasus-sdk/src/iam/types.ts` | Create | Guard-free entry: re-exports `NodeStatus` (D8). |
| `ts/packages/paigasus-sdk/package.json` | Modify | Export `./iam/types`; entry comment says six entries. |
| `ts/packages/paigasus-sdk/src/index.ts` | Modify | Root barrel lists `NodeStatus`. |
| `ts/packages/paigasus-sdk/tests/iam-types.test.ts` | Create | `NodeStatus` is the registry enum, as a value. |
| `ts/packages/paigasus-sdk/tests/server-guard.test.ts` | Modify | `./iam/types` is an unguarded entry. |
| `ts/packages/paigasus-sdk/tests/index-barrel.test.ts` | Modify | The barrel covers `./iam/types`. |
| `ts/packages/paigasus-console-core/src/authorize.ts` | Modify | `IAM_ACTIONS` runtime array; `IamAction` derived; nine new names. |
| `ts/packages/paigasus-console-core/src/index.ts` | Modify | Export `IAM_ACTIONS`. |
| `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts` | Create | Parity of `IAM_ACTIONS` with `action.rs` wire names. |
| `ts/packages/paigasus-console-core/moon.yml` | Modify | `test` input `action.rs`. |
| `ts/apps/iam-console/lib/form.ts` | Modify | Shared bounds, `renameChange`, `refreshesAfterLifecycleAction`. |
| `ts/apps/iam-console/tests/unit/form.test.ts` | Modify | Bound, `renameChange` and refresh-rule cases. |
| `ts/apps/iam-console/app/(console)/orgs/commands.ts` | Modify | Use the shared bounds. |
| `ts/apps/iam-console/app/(console)/node-status.ts` | Create | `lifecycleOf`, `lifecycleView`, badge and column tables, the note text. |
| `ts/apps/iam-console/tests/unit/node-status.test.ts` | Create | Nine combinations, tables. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts` | Modify | Shared bounds; organization rename/archive/restore. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts` | Modify | Shared bounds; team rename/archive/restore. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts` | Create | Project rename/archive/restore. |
| `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts` | Create | Tier 2 for the nine commands. |
| `ts/apps/iam-console/app/_components/error-copy.ts` | Modify | `NOTHING_TO_RENAME` copy; comment. |
| `ts/apps/iam-console/tests/unit/error-copy.test.ts` | Modify | The new copy. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts` | Modify | Three organization actions. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts` | Modify | Three team actions. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts` | Create | Three project actions. |
| `ts/apps/iam-console/tests/unit/actions-structure.test.ts` | Modify | `EXPECTED`; navigation-helper ban with controls. |
| `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts` | Modify | The nine actions and their refresh rule. |
| `ts/apps/iam-console/package.json` | Modify | jsdom and Testing Library dev dependencies. |
| `ts/apps/iam-console/app/_components/form-action.ts` | Create | The `FormAction` type. |
| `ts/apps/iam-console/app/_components/rename-form.tsx` | Create | Controlled rename form. |
| `ts/apps/iam-console/app/_components/lifecycle-button.tsx` | Create | `ArchiveButton` (two-step) and `RestoreButton`. |
| `ts/apps/iam-console/tests/unit/rename-form.test.tsx` | Create | Typed values survive a failed action. |
| `ts/apps/iam-console/tests/unit/lifecycle-button.test.tsx` | Create | Two-step archive, cancel, restore. |
| `ts/apps/iam-console/app/(console)/manage-section.tsx` | Create | The § 5.3 table; D7 comment. |
| `ts/apps/iam-console/app/(console)/status-badge.tsx` | Create | Header badge. |
| `ts/apps/iam-console/tests/unit/manage-section.test.tsx` | Create | § 5.3 row by row. |
| `ts/apps/iam-console/tests/unit/status-badge.test.tsx` | Create | Badge per view. |
| `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` | Modify | One D7 pointer sentence in § 6.3. |
| `ts/apps/iam-console/app/(console)/orgs/load.ts`, `orgs/page.tsx` | Modify | Row lifecycle; Status column. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/load.ts`, `page.tsx` | Modify | Lifecycle, three `can*` flags, badge, column, Manage. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts`, `page.tsx` | Modify | The same for a team. |
| `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts`, `page.tsx` | Modify | The same for a project; a new `Promise.all`. |
| `ts/apps/iam-console/tests/integration/{orgs-page,org-page,team-project-pages}.test.ts` | Modify | Loader cases. |
| `ts/apps/iam-console/tests/e2e/support/world.ts` | Modify | Statuses, 16 actions with `satisfies`, nine handlers. |
| `ts/apps/iam-console/tests/unit/world-actions.test.ts` | Create | `ALL_ACTIONS` equals `IAM_ACTIONS` as a set. |
| `ts/apps/iam-console/tests/e2e/lifecycle.spec.ts` | Create | R14–R16. |
| `ts/apps/iam-console/tests/unit/e2e-rows.test.ts` | Modify | 16 rows. |

---

### Task 1: `NodeStatus` from a guard-free sdk entry (D8)

Spec § 4.5, F2.

**Layout decision.** The file is `src/iam/types.ts`, exported as `./iam/types`. The package already holds this shape without trouble: `src/errors.ts` next to `src/errors/types.ts`. A directory `src/iam/` next to `src/iam.ts` is safe because `./iam` resolves to the FILE `iam.ts` (there is no `src/iam/index.ts`, and this task adds none). `tests/server-guard.test.ts` derives the guard path from the depth of each entry, and `tests/http-surface.test.ts` already allows the specifier `@paigasus/proto/iam`. So the only structural test change is the unguarded list. The fallback name `src/iam-types.ts` is not necessary.

**Files:**
- Create: `ts/packages/paigasus-sdk/src/iam/types.ts`
- Create: `ts/packages/paigasus-sdk/tests/iam-types.test.ts`
- Modify: `ts/packages/paigasus-sdk/package.json:4` (comment), `:14-20` (exports)
- Modify: `ts/packages/paigasus-sdk/src/index.ts:26` (insert after)
- Modify: `ts/packages/paigasus-sdk/tests/server-guard.test.ts:14-17`
- Modify: `ts/packages/paigasus-sdk/tests/index-barrel.test.ts:7,18`, and after the last `it` block

**Interfaces:**
- Consumes: `NodeStatus` from `@paigasus/proto/iam` (a TS enum: `UNSPECIFIED = 0`, `ACTIVE = 1`, `ARCHIVED = 2`).
- Produces: `import { NodeStatus } from '@paigasus/sdk/iam/types'` (value and type), with no server guard. Also `NodeStatus` from the root `@paigasus/sdk`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/iam-types.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-630 spec § 4.5 (D8). An app may not import @paigasus/proto, so the loaders and the e2e world
// can name a node status only through this guard-free entry. It must be the registry enum ITSELF,
// as a runtime value, or a screen compares against a copy that can drift.
import { NodeStatus as ProtoNodeStatus } from '@paigasus/proto/iam';
import { describe, expect, it } from 'vitest';

import { NodeStatus } from '../src/iam/types.js';

describe('the guard-free ./iam/types entry', () => {
  it('re-exports NodeStatus as a runtime value', () => {
    expect(NodeStatus.UNSPECIFIED).toBe(0);
    expect(NodeStatus.ACTIVE).toBe(1);
    expect(NodeStatus.ARCHIVED).toBe(2);
  });

  it('re-exports the registry enum object, not a copy', () => {
    expect(NodeStatus).toBe(ProtoNodeStatus);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-types.test.ts`
Expected: FAIL with `Failed to resolve import "../src/iam/types.js"` (or `Cannot find module`).

- [ ] **Step 3: Create the entry**

Create `ts/packages/paigasus-sdk/src/iam/types.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The guard-free `./iam/types` entry (SMA-630 spec § 4.5, D8). This file carries NO
// `import '../server-guard'` and must not gain one: an app's client code and its Playwright support
// files import this entry, and `tests/server-guard.test.ts` lists './iam/types' in
// UNGUARDED_ENTRIES and asserts that the guard is absent.
//
// An app cannot import @paigasus/proto (the eslint boundary `paigasus/boundaries/apps` bans it,
// type imports included). Without this line a screen receives `status: 2` and cannot write the
// name. It is the same trade `./errors/types` makes for ErrorReason: one small frozen enum object
// in a client bundle.
export { NodeStatus } from '@paigasus/proto/iam';
```

- [ ] **Step 4: Export the entry**

In `ts/packages/paigasus-sdk/package.json`, replace the whole `"_comment_entries"` line with:

```json
  "_comment_entries": "Six entry points: `.` (the root barrel), `./iam` (the generated Connect-ES clients and createIamClient), `./iam/types` (the NodeStatus enum only, deliberately unguarded, SMA-630), `./chat` (the gateway's OpenAI-compatible chat client — the ONLY hand-written HTTP surface, enforced by tests/http-surface.test.ts, SMA-575), `./errors`, and `./errors/types` (types only, deliberately unguarded). tests/server-guard.test.ts is driven off THIS map, so a new entry is covered the day it is added. tests/http-surface.test.ts asserts every target stays under ./src/.",
```

Replace the `exports` object with:

```json
  "exports": {
    ".": "./src/index.ts",
    "./chat": "./src/chat.ts",
    "./iam": "./src/iam.ts",
    "./iam/types": "./src/iam/types.ts",
    "./errors": "./src/errors.ts",
    "./errors/types": "./src/errors/types.ts"
  },
```

- [ ] **Step 5: Add the name to the root barrel**

In `ts/packages/paigasus-sdk/src/index.ts`, after the line `export type { Auth, TransportOptions } from './iam';`, insert:

```ts

// The guard-free ./iam/types entry (SMA-630). tests/index-barrel.test.ts requires every subpath
// export in this barrel.
export { NodeStatus } from './iam/types';
```

- [ ] **Step 6: Mark the entry as unguarded**

In `ts/packages/paigasus-sdk/tests/server-guard.test.ts`, replace:

```ts
// Entries that deliberately carry NO guard. `./errors/types` (PR C) holds only types: under
// `verbatimModuleSyntax` an `import type` emits nothing, so a client component can name those types
// with no runtime import and no server-only evaluation (spec § 6.3). Every OTHER entry is guarded.
const UNGUARDED_ENTRIES = new Set(['./errors/types']);
```

with:

```ts
// Entries that deliberately carry NO guard. `./errors/types` (PR C) holds only types: under
// `verbatimModuleSyntax` an `import type` emits nothing, so a client component can name those types
// with no runtime import and no server-only evaluation (spec § 6.3). `./iam/types` (SMA-630 D8)
// re-exports the NodeStatus enum for the same readers. Every OTHER entry is guarded.
const UNGUARDED_ENTRIES = new Set(['./errors/types', './iam/types']);
```

- [ ] **Step 7: Cover the entry in the barrel test**

In `ts/packages/paigasus-sdk/tests/index-barrel.test.ts`, replace `import * as iam from '../src/iam.js';` with:

```ts
import * as iam from '../src/iam.js';
import * as iamTypes from '../src/iam/types.js';
```

Replace `const SUBMODULES = { chat, errors, iam } as const;` with:

```ts
const SUBMODULES = { chat, errors, iam, 'iam/types': iamTypes } as const;
```

After the last `it(...)` block (`'exposes ErrorReason as the registry enum, not a shadow'`), insert:

```ts

  it('exposes NodeStatus as the registry enum (SMA-630)', () => {
    expect(barrel.NodeStatus.ARCHIVED).toBe(2);
  });
```

- [ ] **Step 8: Run the tests and the type check**

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-types.test.ts tests/index-barrel.test.ts tests/server-guard.test.ts tests/http-surface.test.ts`
Expected: PASS. `server-guard.test.ts` shows the case `entry ./iam/types deliberately carries no guard`. `index-barrel.test.ts` shows the case `the barrel re-exports ./iam/types key NodeStatus`.

Run: `moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck`
Expected: PASS.

- [ ] **Step 9: Prove that the guard check bites, then remove the mutation**

Insert the line `import '../server-guard';` directly below the comment block of `src/iam/types.ts`. Run `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/server-guard.test.ts`. Expected: FAIL on `entry ./iam/types deliberately carries no guard`. Remove the inserted line with the Edit tool. Run the file again. Expected: PASS.

- [ ] **Step 10: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-sdk/package.json packages/paigasus-sdk/src/iam/types.ts packages/paigasus-sdk/src/index.ts packages/paigasus-sdk/tests/iam-types.test.ts packages/paigasus-sdk/tests/server-guard.test.ts packages/paigasus-sdk/tests/index-barrel.test.ts
moon run ts:lint ts:fmt
git add ts/packages/paigasus-sdk/package.json ts/packages/paigasus-sdk/src/iam/types.ts ts/packages/paigasus-sdk/src/index.ts ts/packages/paigasus-sdk/tests/iam-types.test.ts ts/packages/paigasus-sdk/tests/server-guard.test.ts ts/packages/paigasus-sdk/tests/index-barrel.test.ts
git commit -m "feat(ts): export NodeStatus from a guard-free sdk entry (SMA-630)" -m "The app cannot import @paigasus/proto. The new @paigasus/sdk/iam/types entry re-exports the NodeStatus enum without the server guard, and the root barrel lists it." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `IAM_ACTIONS` and the action-name parity test

Spec § 5.1, § 8 rows 1-2.

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/authorize.ts:26-27`
- Modify: `ts/packages/paigasus-console-core/src/index.ts:19`
- Modify: `ts/packages/paigasus-console-core/moon.yml:65-67` (the end of the `test` inputs)
- Create: `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts`

**Interfaces:**
- Consumes: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs` (the `as_wire` match arms, `Action::X => "X",`).
- Produces: `export const IAM_ACTIONS` (a `readonly` tuple of 16 names) and `export type IamAction = (typeof IAM_ACTIONS)[number]`, both from `@paigasus/console-core`. The nine new names are `RenameOrganization`, `ArchiveOrganization`, `RestoreOrganization`, `RenameTeam`, `ArchiveTeam`, `RestoreTeam`, `RenameProject`, `ArchiveProject`, `RestoreProject`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// IAM_ACTIONS against the action catalog of IAM (SMA-630 spec § 5.1). mayI() FAILS OPEN: IAM
// answers a misspelt action name with `invalid-action`, mayI() then answers true, and the control
// always shows. No page test can see that. This test holds every name to the wire names in the
// Rust catalog, which it reads as TEXT from the `as_wire` match arms.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { IAM_ACTIONS } from '../../src/authorize';

// tests/unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five `../`, the same
// depth as tests/unit/prn-tenancy.test.ts.
const REPO_ROOT = new URL('../../../../../', import.meta.url);
const ACTION_RS = 'rs/crates/libs/paigasus-iam-core/src/authz/action.rs';

/** The wire names of `Action::as_wire`: each arm reads `Action::Name => "Name",`. */
function wireNames(source: string): Set<string> {
  const names = new Set<string>();
  for (const match of source.matchAll(/Action::\w+ => "([A-Za-z]+)",/g)) {
    if (match[1] !== undefined) names.add(match[1]);
  }
  return names;
}

const LIFECYCLE = ['RenameOrganization', 'ArchiveOrganization', 'RestoreOrganization', 'RenameTeam', 'ArchiveTeam', 'RestoreTeam', 'RenameProject', 'ArchiveProject', 'RestoreProject'];

describe('IAM_ACTIONS against the Rust action catalog', () => {
  const wire = wireNames(readFileSync(fileURLToPath(new URL(ACTION_RS, REPO_ROOT)), 'utf8'));

  // Without this floor, a changed arm format gives an empty set, and the cases below fail for the
  // wrong reason.
  it('reads a non-trivial catalog', () => {
    expect(wire.size).toBeGreaterThanOrEqual(40);
    expect(wire.has('CreateTeam')).toBe(true);
  });

  it.each([...IAM_ACTIONS])('%s is a wire name that IAM accepts', (action) => {
    expect(wire.has(action)).toBe(true);
  });

  it('holds the nine lifecycle names, and no name twice', () => {
    expect(IAM_ACTIONS).toEqual(expect.arrayContaining(LIFECYCLE));
    expect(new Set(IAM_ACTIONS).size).toBe(IAM_ACTIONS.length);
  });

  it('refuses a misspelt name (negative control)', () => {
    expect(wire.has('RenameTeams')).toBe(false);
    expect(wire.has('renameTeam')).toBe(false);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/action-names.test.ts`
Expected: FAIL. `IAM_ACTIONS` is not exported, so `[...IAM_ACTIONS]` throws `IAM_ACTIONS is not iterable`.

- [ ] **Step 3: Make `IamAction` a runtime array**

In `ts/packages/paigasus-console-core/src/authorize.ts`, replace:

```ts
/** The PascalCase names IAM's Action::parse accepts (rs/crates/libs/paigasus-iam-core/src/authz/action.rs:114-163). */
export type IamAction = 'ListOrganizations' | 'CreateOrganization' | 'CreateTeam' | 'CreateProject' | 'AttachMembership' | 'DetachMembership' | 'ListAuditLog';
```

with:

```ts
/**
 * The PascalCase names IAM's Action::parse accepts (rs/crates/libs/paigasus-iam-core/src/authz/action.rs,
 * `as_wire`). A RUNTIME array, not only a type (SMA-630 spec § 5.1): mayI() fails open, so a
 * misspelt name would show its control for ever. tests/unit/action-names.test.ts holds every entry
 * to the Rust wire names, and the e2e world of the IAM console holds its ALL_ACTIONS to this list.
 */
export const IAM_ACTIONS = [
  'ListOrganizations',
  'CreateOrganization',
  'RenameOrganization',
  'ArchiveOrganization',
  'RestoreOrganization',
  'CreateTeam',
  'RenameTeam',
  'ArchiveTeam',
  'RestoreTeam',
  'CreateProject',
  'RenameProject',
  'ArchiveProject',
  'RestoreProject',
  'AttachMembership',
  'DetachMembership',
  'ListAuditLog',
] as const;

export type IamAction = (typeof IAM_ACTIONS)[number];
```

In `ts/packages/paigasus-console-core/src/index.ts`, replace:

```ts
export { createMayI, type IamAction, type MayI } from './authorize';
```

with:

```ts
export { IAM_ACTIONS, createMayI, type IamAction, type MayI } from './authorize';
```

- [ ] **Step 4: Key the Moon `test` task on the Rust file**

In `ts/packages/paigasus-console-core/moon.yml`, replace:

```yaml
      # The parity corpus and the Rust constant the PRN reader is held to.
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/**/*'
      - '/rs/crates/libs/paigasus-iam-core/src/authz/model.rs'
```

with:

```yaml
      # The parity corpus and the Rust constant the PRN reader is held to.
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/**/*'
      - '/rs/crates/libs/paigasus-iam-core/src/authz/model.rs'
      # SMA-630 spec § 5.1. tests/unit/action-names.test.ts reads the action catalog as TEXT. Without
      # this input, a changed Rust action name selects no paigasus-console-core-ts task, and Moon
      # serves a cached PASS on exactly the PR that breaks the parity. repo:input-liveness checks
      # `repo:*` tasks only, so the SMA-630 plan (Task 2) confirmed with `moon task … --json` that
      # this entry resolves to a tracked file.
      - '/rs/crates/libs/paigasus-iam-core/src/authz/action.rs'
```

- [ ] **Step 5: Run the tests**

Run: `pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/action-names.test.ts tests/unit/authorize.test.ts`
Expected: PASS, with 16 `is a wire name that IAM accepts` cases.

- [ ] **Step 6: Confirm that the Moon input resolves**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
set -o pipefail
moon task paigasus-console-core-ts:test --json | jq -e '.inputFiles | has("rs/crates/libs/paigasus-iam-core/src/authz/action.rs")'
git ls-files --error-unmatch rs/crates/libs/paigasus-iam-core/src/authz/action.rs
```

Expected: `true`, then the path. `jq -e` exits non-zero on `false`, and `pipefail` keeps a `moon` failure visible.

- [ ] **Step 7: Prove that the parity test bites, then remove the mutation**

In `src/authorize.ts`, change `'RenameTeam',` to `'RenameTeams',`. Run `pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/action-names.test.ts`. Expected: FAIL on `RenameTeams is a wire name that IAM accepts`. Change it back with the Edit tool and run again. Expected: PASS.

- [ ] **Step 8: Type-check the dependants**

Run: `moon run paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck`
Expected: PASS. `scriptedMayI` in the app is typed on `IamAction` and accepts the wider union.

- [ ] **Step 9: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-console-core/src/authorize.ts packages/paigasus-console-core/src/index.ts packages/paigasus-console-core/tests/unit/action-names.test.ts
moon run ts:lint ts:fmt
git add ts/packages/paigasus-console-core/src/authorize.ts ts/packages/paigasus-console-core/src/index.ts ts/packages/paigasus-console-core/tests/unit/action-names.test.ts ts/packages/paigasus-console-core/moon.yml
git commit -m "feat(ts): add the lifecycle action names with a rust parity test (SMA-630)" -m "IamAction becomes the IAM_ACTIONS runtime array with nine new names. A unit test holds each name to the wire names in action.rs, and the package test task keys on that file." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Shared field bounds and `renameChange` in `lib/form.ts`

Spec § 4.2, § 4.3 (D6), § 9.1 schema cases.

**Files:**
- Modify: `ts/apps/iam-console/lib/form.ts:1-8` (imports and new exports)
- Modify: `ts/apps/iam-console/app/(console)/orgs/commands.ts:7-22`
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts:5-12`
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts:5-12`
- Test: `ts/apps/iam-console/tests/unit/form.test.ts`

**Interfaces:**
- Consumes: `zod` (`z`), existing `toActionResult`, `formFields`, `invalidFormInput`, `ActionResult`.
- Produces (all from `lib/form.ts`):
  - `export const NAME_MAX_CODE_POINTS = 256;`
  - `export const slugField` (`z.string().trim().min(1).max(200)`)
  - `export const nameField` (trimmed, `min(1)`, at most 256 code points)
  - `export const prnField` (`z.string().trim().min(1).max(512)`)
  - `export const currentField` (`z.string().trim().max(4096)`, empty allowed)
  - `export type RenameFields = { readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };`
  - `export type RenameChange = { newSlug?: string; newName?: string };`
  - `export function renameChange(fields: RenameFields): RenameChange;`

**Choice made here (the spec does not fix it).** The spec names `currentSlug` and `currentName` but gives no bound. A stored name can be empty or longer than 256 code points, because the rename path of IAM does not validate names (F11). If the hidden current value used `nameField`, every rename of such a node would fail as `invalid-input`. So `currentField` allows the empty string and only limits the size (4096 UTF-16 units). It trims, so a stored name with outer spaces does not count as a change when the user leaves the field as it is.

- [ ] **Step 1: Write the failing tests**

In `ts/apps/iam-console/tests/unit/form.test.ts`, replace the import line:

```ts
import { formFields, invalidFormInput, toActionResult } from '../../lib/form';
```

with:

```ts
import { currentField, formFields, invalidFormInput, nameField, NAME_MAX_CODE_POINTS, prnField, renameChange, slugField, toActionResult } from '../../lib/form';
```

At the end of the file, append:

```ts

// SMA-630 spec § 4.2. The name bound counts CODE POINTS, as IAM's NAME_MAX_CHARS does. zod's
// `.max()` counts UTF-16 units, so the old 200-unit bound refused a valid name that IAM stored.
describe('the shared field bounds', () => {
  const GRIN = '\u{1F600}';

  it('accepts a name of 256 code points and refuses 257', () => {
    expect(NAME_MAX_CODE_POINTS).toBe(256);
    expect(nameField.safeParse('a'.repeat(256)).success).toBe(true);
    expect(nameField.safeParse('a'.repeat(257)).success).toBe(false);
  });

  it('counts code points, not UTF-16 units: 256 astral characters pass', () => {
    const astral = GRIN.repeat(256);
    expect(astral.length).toBe(512);
    expect(nameField.safeParse(astral).success).toBe(true);
    expect(nameField.safeParse(`${astral}${GRIN}`).success).toBe(false);
  });

  it('trims a name before it counts, and refuses a name that is only whitespace', () => {
    expect(nameField.safeParse(`  ${'a'.repeat(256)}  `).data).toBe('a'.repeat(256));
    expect(nameField.safeParse('   ').success).toBe(false);
  });

  it('keeps the slug bound at 200 and the PRN bound at 512, both trimmed', () => {
    expect(slugField.safeParse('s'.repeat(200)).success).toBe(true);
    expect(slugField.safeParse('s'.repeat(201)).success).toBe(false);
    expect(slugField.safeParse(' acme ').data).toBe('acme');
    expect(prnField.safeParse('p'.repeat(512)).success).toBe(true);
    expect(prnField.safeParse('p'.repeat(513)).success).toBe(false);
    expect(prnField.safeParse('  ').success).toBe(false);
  });

  it('accepts an empty current value, and a current name longer than the name bound', () => {
    expect(currentField.safeParse('').success).toBe(true);
    expect(currentField.safeParse('a'.repeat(300)).success).toBe(true);
    expect(currentField.safeParse(null).success).toBe(false);
  });
});

// SMA-630 spec D6, § 4.3. A rename sends only the fields that the user changed.
describe('renameChange', () => {
  const current = { currentSlug: 'acme', currentName: 'Acme' };

  it('sends only the slug when only the slug changed', () => {
    expect(renameChange({ ...current, slug: 'acme-2', name: 'Acme' })).toEqual({ newSlug: 'acme-2' });
  });

  it('sends only the name when only the name changed', () => {
    expect(renameChange({ ...current, slug: 'acme', name: 'Acme Two' })).toEqual({ newName: 'Acme Two' });
  });

  it('sends both when both changed', () => {
    expect(renameChange({ ...current, slug: 'acme-2', name: 'Acme Two' })).toEqual({ newSlug: 'acme-2', newName: 'Acme Two' });
  });

  it('sends neither field when nothing changed, so IAM answers nothing-to-rename', () => {
    const change = renameChange({ ...current, slug: 'acme', name: 'Acme' });
    expect(change).toEqual({});
    expect('newSlug' in change).toBe(false);
    expect('newName' in change).toBe(false);
  });

  it('compares trimmed values on both sides', () => {
    expect(renameChange({ slug: ' acme ', name: ' Acme ', currentSlug: 'acme', currentName: 'Acme' })).toEqual({});
    expect(renameChange({ slug: 'acme', name: 'Acme', currentSlug: ' acme', currentName: 'Acme ' })).toEqual({});
    expect(renameChange({ slug: ' new ', name: 'Acme', currentSlug: 'acme', currentName: 'Acme' })).toEqual({ newSlug: 'new' });
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/form.test.ts`
Expected: FAIL. `nameField`, `slugField`, `prnField`, `currentField`, `renameChange` and `NAME_MAX_CODE_POINTS` are `undefined` (for example `Cannot read properties of undefined (reading 'safeParse')`).

- [ ] **Step 3: Add the bounds and `renameChange`**

In `ts/apps/iam-console/lib/form.ts`, replace:

```ts
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { neverReachedIam, type ActionState, type IamResult } from '@paigasus/console-core';
```

with:

```ts
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { neverReachedIam, type ActionState, type IamResult } from '@paigasus/console-core';

// The field bounds of every tenancy form (SMA-630 spec § 4.2). They live ONLY here: each
// commands.ts imports them, so a create form and a rename form cannot disagree.

/** IAM's NAME_MAX_CHARS (paigasus-iam-core tenancy.rs): 256 Unicode scalar values, not UTF-16 units. */
export const NAME_MAX_CODE_POINTS = 256;

/** A slug. IAM refuses more than 64 bytes with `invalid-slug`; this bound only limits the request. */
export const slugField = z.string().trim().min(1).max(200);

/**
 * A name, for create AND rename. The bound counts code points (`[...value].length`), as IAM does:
 * zod's `.max()` counts UTF-16 units, and would refuse a valid name of astral characters. IAM's
 * rename path does not validate a name (spec F11), so for a rename this schema is the only guard.
 */
export const nameField = z
  .string()
  .trim()
  .min(1)
  .refine((value) => [...value].length <= NAME_MAX_CODE_POINTS);

/** A PRN: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
export const prnField = z.string().trim().min(1).max(512);

/**
 * A hidden "current value" of a rename form. It can be empty, and it can be longer than the name
 * bound, because IAM stores a renamed name without a check (spec F11). A stricter schema here would
 * refuse every rename of such a node. The bound only limits the request.
 */
export const currentField = z.string().trim().max(4096);

export type RenameFields = { readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };
export type RenameChange = { newSlug?: string; newName?: string };

/**
 * The fields a rename sends (SMA-630 spec D6, § 4.3): only the ones the user changed, compared on
 * trimmed values. With no change the result is `{}`. The command then still calls IAM with neither
 * field, and IAM answers `nothing-to-rename`: the console does not decide that refusal itself.
 *
 * This prevents one lost update: user A changes only the slug while user B changes only the name.
 * Two changes of the SAME field still end with the last write (spec § 11).
 */
export function renameChange(fields: RenameFields): RenameChange {
  const slug = fields.slug.trim();
  const name = fields.name.trim();
  return {
    ...(slug === fields.currentSlug.trim() ? {} : { newSlug: slug }),
    ...(name === fields.currentName.trim() ? {} : { newName: name }),
  };
}
```

- [ ] **Step 4: Replace every local copy of the bounds**

In `ts/apps/iam-console/app/(console)/orgs/commands.ts`, replace:

```ts
import { toActionResult, type ActionResult } from '../../../lib/form';

const text = z.string().trim().min(1).max(200);

/** The shape of the form, not IAM's rules. IAM validates the slug grammar and answers with a reason. */
export const createOrganizationForm = z.object({ slug: text, name: text });
```

with:

```ts
import { nameField, prnField, slugField, toActionResult, type ActionResult } from '../../../lib/form';

/** The shape of the form, not IAM's rules. IAM validates the slug grammar and answers with a reason. */
export const createOrganizationForm = z.object({ slug: slugField, name: nameField });
```

In the same file, replace:

```ts
/** A PRN field: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
const prn = z.string().trim().min(1).max(512);

export const attachMembershipForm = z.object({ principalPrn: prn, nodePrn: prn });
```

with:

```ts
export const attachMembershipForm = z.object({ principalPrn: prnField, nodePrn: prnField });
```

(`detachMembershipForm` keeps its own `max(64)` id bound: it is not a slug, name or PRN.)

In `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts`, replace:

```ts
import { toActionResult, type ActionResult } from '../../../../lib/form';

const text = z.string().trim().min(1).max(200);

/** `.trim()` like every other PRN field (../commands.ts's `prn`): a hidden field can carry whitespace. */
export const createTeamForm = z.object({ orgPrn: z.string().trim().min(1).max(512), slug: text, name: text });
```

with:

```ts
import { nameField, prnField, slugField, toActionResult, type ActionResult } from '../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createTeamForm = z.object({ orgPrn: prnField, slug: slugField, name: nameField });
```

In `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts`, replace:

```ts
import { toActionResult, type ActionResult } from '../../../../../../lib/form';

const text = z.string().trim().min(1).max(200);

/** `.trim()` like every other PRN field (../../../commands.ts's `prn`): a hidden field can carry whitespace. */
export const createProjectForm = z.object({ teamPrn: z.string().trim().min(1).max(512), slug: text, name: text });
```

with:

```ts
import { nameField, prnField, slugField, toActionResult, type ActionResult } from '../../../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createProjectForm = z.object({ teamPrn: prnField, slug: slugField, name: nameField });
```

- [ ] **Step 5: Confirm that no copy is left**

Run: `grep -rn "z.string()" "ts/apps/iam-console/app"`
Expected: exactly one hit, `detachMembershipForm` in `app/(console)/orgs/commands.ts`.

- [ ] **Step 6: Run the tests**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/form.test.ts tests/integration/orgs-commands.test.ts tests/integration/membership-commands.test.ts tests/integration/team-project-pages.test.ts`
Expected: PASS.

Run: `moon run iam-console-ts:typecheck`
Expected: PASS.

- [ ] **Step 7: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/lib/form.ts "apps/iam-console/app/(console)/orgs/commands.ts" "apps/iam-console/app/(console)/orgs/[org]/commands.ts" "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts" apps/iam-console/tests/unit/form.test.ts
moon run ts:lint ts:fmt
git add ts/apps/iam-console/lib/form.ts "ts/apps/iam-console/app/(console)/orgs/commands.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts" ts/apps/iam-console/tests/unit/form.test.ts
git commit -m "feat(ts): share the tenancy form bounds and add renameChange (SMA-630)" -m "The slug, name and PRN bounds move to lib/form.ts. The name bound is 256 code points, as in IAM. renameChange returns only the fields that the user changed." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `node-status.ts` — lifecycle, view and the two label tables

Spec § 5.2, § 6.3, § 9.2 (`node-status.test.ts`).

**Files:**
- Create: `ts/apps/iam-console/app/(console)/node-status.ts`
- Create: `ts/apps/iam-console/tests/unit/node-status.test.ts`

**Interfaces:**
- Consumes: `NodeStatus` from `@paigasus/sdk/iam/types` (Task 1).
- Produces (all from `app/(console)/node-status.ts`):
  - `export type NodeState = 'active' | 'archived' | 'unknown';`
  - `export type NodeLifecycle = { readonly own: NodeState; readonly effective: NodeState };`
  - `export type LifecycleView = 'active' | 'archived' | 'archived-parent' | 'unknown';`
  - `export function lifecycleOf(node: { readonly status: NodeStatus; readonly effectiveStatus: NodeStatus }): NodeLifecycle;`
  - `export function lifecycleView(lifecycle: NodeLifecycle): LifecycleView;`
  - `export const BADGE_LABEL: Readonly<Record<LifecycleView, string | null>>;`
  - `export const COLUMN_LABEL: Readonly<Record<LifecycleView, string>>;`
  - `export function statusColumnLabel(lifecycle: NodeLifecycle): string;`
  - `export const PARENT_ARCHIVED_NOTE: string;`

This module has no `server-only` import: its only import is the guard-free sdk entry. That lets the Playwright specs (Task 13) import `BADGE_LABEL` and `PARENT_ARCHIVED_NOTE` under plain Node.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/iam-console/tests/unit/node-status.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The lifecycle of a tenancy node as the screens show it (SMA-630 spec § 5.2, § 6.3): all nine
// (own × effective) combinations, a status value this build does not know, and the two label tables.
import { describe, expect, it } from 'vitest';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { BADGE_LABEL, COLUMN_LABEL, lifecycleOf, lifecycleView, PARENT_ARCHIVED_NOTE, statusColumnLabel, type LifecycleView, type NodeLifecycle } from '../../app/(console)/node-status';

type Row = { readonly status: NodeStatus; readonly effectiveStatus: NodeStatus; readonly lifecycle: NodeLifecycle; readonly view: LifecycleView };

const ROWS: readonly Row[] = [
  { status: NodeStatus.UNSPECIFIED, effectiveStatus: NodeStatus.UNSPECIFIED, lifecycle: { own: 'unknown', effective: 'unknown' }, view: 'unknown' },
  { status: NodeStatus.UNSPECIFIED, effectiveStatus: NodeStatus.ACTIVE, lifecycle: { own: 'unknown', effective: 'active' }, view: 'unknown' },
  { status: NodeStatus.UNSPECIFIED, effectiveStatus: NodeStatus.ARCHIVED, lifecycle: { own: 'unknown', effective: 'archived' }, view: 'unknown' },
  { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.UNSPECIFIED, lifecycle: { own: 'active', effective: 'unknown' }, view: 'unknown' },
  { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE, lifecycle: { own: 'active', effective: 'active' }, view: 'active' },
  { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ARCHIVED, lifecycle: { own: 'active', effective: 'archived' }, view: 'archived-parent' },
  { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.UNSPECIFIED, lifecycle: { own: 'archived', effective: 'unknown' }, view: 'unknown' },
  // IAM cannot send this pair. If it does, the node still shows as archived (spec § 5.2 rule 2).
  { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ACTIVE, lifecycle: { own: 'archived', effective: 'active' }, view: 'archived' },
  { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ARCHIVED, lifecycle: { own: 'archived', effective: 'archived' }, view: 'archived' },
];

describe('lifecycleOf and lifecycleView', () => {
  it('has all nine combinations', () => {
    expect(new Set(ROWS.map((row) => `${String(row.status)}/${String(row.effectiveStatus)}`)).size).toBe(9);
  });

  it.each(ROWS)('status $status / effective $effectiveStatus is $view', ({ status, effectiveStatus, lifecycle, view }) => {
    expect(lifecycleOf({ status, effectiveStatus })).toEqual(lifecycle);
    expect(lifecycleView(lifecycle)).toBe(view);
  });

  it('maps a status value this build does not know to unknown (version skew)', () => {
    const future = 7 as NodeStatus;
    expect(lifecycleOf({ status: future, effectiveStatus: NodeStatus.ACTIVE })).toEqual({ own: 'unknown', effective: 'active' });
    expect(lifecycleView(lifecycleOf({ status: NodeStatus.ACTIVE, effectiveStatus: future }))).toBe('unknown');
  });
});

describe('the badge and column tables (spec § 6.3)', () => {
  it('labels the header badge', () => {
    expect(BADGE_LABEL).toEqual({ active: null, archived: 'Archived', 'archived-parent': 'Archived (parent)', unknown: 'Status unknown' });
  });

  it('labels the Status column', () => {
    expect(COLUMN_LABEL).toEqual({ active: 'Active', archived: 'Archived', 'archived-parent': 'Archived (parent)', unknown: 'Unknown' });
    expect(statusColumnLabel({ own: 'active', effective: 'archived' })).toBe('Archived (parent)');
  });

  it('holds the exact parent-archived note', () => {
    expect(PARENT_ARCHIVED_NOTE).toBe('A parent of this item is archived. Restore the parent to change this item.');
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/node-status.test.ts`
Expected: FAIL with `Failed to resolve import "../../app/(console)/node-status"`.

- [ ] **Step 3: Write the module**

Create `ts/apps/iam-console/app/(console)/node-status.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The lifecycle of a tenancy node as the screens show it (SMA-630 spec § 5.2, § 6.3). IAM sends two
// NodeStatus values per node: `status` (the node itself) and `effectiveStatus` (archived when the
// node OR an ancestor is archived). This one module maps both, for the three detail pages and for
// the list rows.
//
// VERSION SKEW. UNSPECIFIED, and any value that this build does not know, map to 'unknown'. The
// screens then show "Status unknown" and offer no lifecycle control. They do not guess.
//
// A plain module with no `server-only` import. Its only import is @paigasus/sdk's guard-free
// ./iam/types entry, so the Playwright specs can read the labels and the note from here.
import { NodeStatus } from '@paigasus/sdk/iam/types';

export type NodeState = 'active' | 'archived' | 'unknown';
export type NodeLifecycle = { readonly own: NodeState; readonly effective: NodeState };
export type LifecycleView = 'active' | 'archived' | 'archived-parent' | 'unknown';

function stateOf(status: NodeStatus): NodeState {
  if (status === NodeStatus.ACTIVE) return 'active';
  if (status === NodeStatus.ARCHIVED) return 'archived';
  return 'unknown';
}

export function lifecycleOf(node: { readonly status: NodeStatus; readonly effectiveStatus: NodeStatus }): NodeLifecycle {
  return { own: stateOf(node.status), effective: stateOf(node.effectiveStatus) };
}

/**
 * The rules of spec § 5.2, in this order:
 * 1. an unknown half gives 'unknown';
 * 2. an archived node gives 'archived' (an archived/active pair cannot come from IAM; if it does,
 *    it still shows as archived);
 * 3. an archived ancestor gives 'archived-parent';
 * 4. else 'active'.
 */
export function lifecycleView(lifecycle: NodeLifecycle): LifecycleView {
  if (lifecycle.own === 'unknown' || lifecycle.effective === 'unknown') return 'unknown';
  if (lifecycle.own === 'archived') return 'archived';
  if (lifecycle.effective === 'archived') return 'archived-parent';
  return 'active';
}

/** The badge next to the `h1` of a detail page. An active node gets no badge. */
export const BADGE_LABEL: Readonly<Record<LifecycleView, string | null>> = {
  active: null,
  archived: 'Archived',
  'archived-parent': 'Archived (parent)',
  unknown: 'Status unknown',
};

/** The Status column of the organization, team and project tables. */
export const COLUMN_LABEL: Readonly<Record<LifecycleView, string>> = {
  active: 'Active',
  archived: 'Archived',
  'archived-parent': 'Archived (parent)',
  unknown: 'Unknown',
};

export function statusColumnLabel(lifecycle: NodeLifecycle): string {
  return COLUMN_LABEL[lifecycleView(lifecycle)];
}

/**
 * The Manage section shows this for 'archived-parent', also when it has no control (spec § 5.3).
 * It names no ancestor: the page does not know which ancestor is archived.
 */
export const PARENT_ARCHIVED_NOTE = 'A parent of this item is archived. Restore the parent to change this item.';
```

- [ ] **Step 4: Run the test**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/node-status.test.ts`
Expected: PASS (nine combination cases plus five more).

- [ ] **Step 5: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/node-status.ts" apps/iam-console/tests/unit/node-status.test.ts
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/node-status.ts" ts/apps/iam-console/tests/unit/node-status.test.ts
git commit -m "feat(ts): map node status to a lifecycle view (SMA-630)" -m "node-status.ts maps status and effective status to active, archived, archived-parent or unknown, and holds the badge and column labels." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Organization commands, the tier-2 lifecycle suite, and the `nothing-to-rename` copy

Spec § 4.1-§ 4.3, § 7, § 9.1. This task creates `lifecycle-commands.test.ts` with a table-driven suite. Tasks 6 and 7 call the same suite for a team and a project.

**Files:**
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts` (whole file)
- Create: `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts`
- Modify: `ts/apps/iam-console/app/_components/error-copy.ts:7-8`, `:38`
- Test: `ts/apps/iam-console/tests/unit/error-copy.test.ts`

**Interfaces:**
- Consumes: `slugField`, `nameField`, `prnField`, `currentField`, `renameChange`, `toActionResult`, `ActionResult` (Task 3); `callIam`, `IamClients` from `@paigasus/console-core`; `denial`, `startFakeIam`, `FakeIam`, `FakeIamHandlers`, `FakeIamMethod` from `@paigasus/console-core/testing`; `IDS`, `callsSince`, `clientsFor` from `tests/integration/support.ts`; `NodeStatus` (Task 1).
- Produces, from `app/(console)/orgs/[org]/commands.ts`:
  - `renameOrganizationForm` (zod: `{ prn, slug, name, currentSlug, currentName }`), `type RenameOrganizationInput`
  - `archiveOrganizationForm`, `restoreOrganizationForm` (both `z.object({ prn: prnField })`), `type ArchiveOrganizationInput`, `type RestoreOrganizationInput`
  - `renameOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameOrganization'> }, input: RenameOrganizationInput): Promise<ActionResult>`
  - `archiveOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveOrganization'> }, input: ArchiveOrganizationInput): Promise<ActionResult>`
  - `restoreOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreOrganization'> }, input: RestoreOrganizationInput): Promise<ActionResult>`
- Produces, in the test file: `function lifecycleSuite(suite: LifecycleSuite): void` and the helpers `nodeFields`, `answer`, `answerRename` (Tasks 6 and 7 use them in the same file).
- Produces: `FORM_REASON_COPY[ErrorReason.NOTHING_TO_RENAME] === 'Change the slug or the name first.'`

- [ ] **Step 1: Write the failing tier-2 suite**

Create `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The nine lifecycle commands (SMA-630 spec § 4.3, § 9.1) against the fake IAM. One table-driven
// suite runs for each node kind. For each command it proves the request that reaches IAM, a success,
// and a 403. For a rename it also proves D6 (only the changed fields go to IAM, compared on trimmed
// values), that an empty rename still calls IAM and returns IAM's nothing-to-rename, and that the
// reasons the form copy knows survive. A command takes no mayI, so nothing here scripts IsAuthorized.
import { Code } from '@connectrpc/connect';
import { ErrorReason, type Presentation } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { organizationPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers, type FakeIamMethod } from '@paigasus/console-core/testing';
import { archiveOrganization, renameOrganization, restoreOrganization } from '../../app/(console)/orgs/[org]/commands';
import type { ActionResult } from '../../lib/form';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const SLUG = 'current-slug';
const NAME = 'Current Name';

type Tenancy = ReturnType<typeof clientsFor>['tenancy'];
type Deps = { readonly tenancy: Tenancy };
type RenameInput = { readonly prn: string; readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };
type RenameRequest = { readonly prn: string; readonly newSlug?: string | undefined; readonly newName?: string | undefined };

/** How the scripted handlers answer: with the node, or by throwing `fail`. */
type Script = { readonly fail?: Error };

type LifecycleSuite = {
  readonly node: 'organization' | 'team' | 'project';
  readonly prn: string;
  readonly methods: { readonly rename: FakeIamMethod; readonly archive: FakeIamMethod; readonly restore: FakeIamMethod };
  readonly handlers: (script: Script) => FakeIamHandlers;
  readonly rename: (deps: Deps, input: RenameInput) => Promise<ActionResult>;
  readonly archive: (deps: Deps, input: { readonly prn: string }) => Promise<ActionResult>;
  readonly restore: (deps: Deps, input: { readonly prn: string }) => Promise<ActionResult>;
};

/** The node fields every answer carries. A rename answer shows the new values, as IAM does. */
function nodeFields(status: NodeStatus, change: { readonly newSlug?: string | undefined; readonly newName?: string | undefined } = {}) {
  return { slug: change.newSlug ?? SLUG, name: change.newName ?? NAME, status, effectiveStatus: status };
}

function answer(script: Script): void {
  if (script.fail !== undefined) throw script.fail;
}

/** IAM refuses a rename that carries neither field (pg_teams.rs:198-214 and the equivalents). */
function answerRename(script: Script, request: { readonly newSlug?: string | undefined; readonly newName?: string | undefined }): void {
  answer(script);
  if (request.newSlug === undefined && request.newName === undefined) throw denial({ code: Code.InvalidArgument, reason: 'nothing-to-rename' });
}

type RenameCase = { readonly label: string; readonly change: { readonly slug?: string; readonly name?: string }; readonly sent: { readonly newSlug?: string; readonly newName?: string } };

const RENAME_CASES: readonly RenameCase[] = [
  { label: 'only the slug changed: sends newSlug and no newName', change: { slug: 'new-slug' }, sent: { newSlug: 'new-slug' } },
  { label: 'only the name changed: sends newName and no newSlug', change: { name: 'New Name' }, sent: { newName: 'New Name' } },
  { label: 'both changed: sends both', change: { slug: 'new-slug', name: 'New Name' }, sent: { newSlug: 'new-slug', newName: 'New Name' } },
];

type RefusalCase = { readonly code: Code; readonly reason: string; readonly presentation: Presentation; readonly expected: ErrorReason };

const REFUSALS: readonly RefusalCase[] = [
  { code: Code.PermissionDenied, reason: 'forbidden', presentation: 'forbidden', expected: ErrorReason.FORBIDDEN },
  { code: Code.AlreadyExists, reason: 'slug-conflict', presentation: 'conflict', expected: ErrorReason.SLUG_CONFLICT },
  { code: Code.InvalidArgument, reason: 'invalid-slug', presentation: 'invalid-input', expected: ErrorReason.INVALID_SLUG },
  { code: Code.FailedPrecondition, reason: 'node-archived', presentation: 'conflict', expected: ErrorReason.NODE_ARCHIVED },
];

function lifecycleSuite(suite: LifecycleSuite): void {
  const deps = (): Deps => ({ tenancy: clientsFor(iam).tenancy });
  const input = (change: { readonly slug?: string; readonly name?: string } = {}): RenameInput => ({
    prn: suite.prn,
    slug: change.slug ?? SLUG,
    name: change.name ?? NAME,
    currentSlug: SLUG,
    currentName: NAME,
  });

  describe(`rename ${suite.node}`, () => {
    it.each(RENAME_CASES)('$label', async ({ change, sent }) => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      expect(await suite.rename(deps(), input(change))).toEqual({ ok: true });

      const requests = calls(suite.methods.rename).map((call) => call.request as RenameRequest);
      expect(requests).toHaveLength(1);
      expect(requests[0]?.prn).toBe(suite.prn);
      expect(requests[0]?.newSlug).toBe(sent.newSlug);
      expect(requests[0]?.newName).toBe(sent.newName);
    });

    it("still calls IAM when nothing changed, with neither field, and returns IAM's nothing-to-rename", async () => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      const result = await suite.rename(deps(), input());

      if (result.ok) throw new Error('expected nothing-to-rename');
      expect(result.error.presentation).toBe('invalid-input');
      expect(result.error.reason).toBe(ErrorReason.NOTHING_TO_RENAME);
      const requests = calls(suite.methods.rename).map((call) => call.request as RenameRequest);
      expect(requests).toHaveLength(1);
      expect(requests[0]?.newSlug).toBeUndefined();
      expect(requests[0]?.newName).toBeUndefined();
    });

    it('compares trimmed values: padded current values are no change', async () => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      const result = await suite.rename(deps(), input({ slug: ` ${SLUG} `, name: ` ${NAME} ` }));

      expect(result).toMatchObject({ ok: false, error: { reason: ErrorReason.NOTHING_TO_RENAME } });
      const requests = calls(suite.methods.rename).map((call) => call.request as RenameRequest);
      expect(requests[0]?.newSlug).toBeUndefined();
      expect(requests[0]?.newName).toBeUndefined();
    });

    it.each(REFUSALS)('keeps the reason $reason ($presentation)', async ({ code, reason, presentation, expected }) => {
      iam.setHandlers(suite.handlers({ fail: denial({ code, reason, correlationId: `corr-${suite.node}-${reason}` }) }));

      const result = await suite.rename(deps(), input({ slug: 'new-slug' }));

      if (result.ok) throw new Error(`expected ${reason}`);
      expect(result.error.presentation).toBe(presentation);
      expect(result.error.reason).toBe(expected);
      expect(result.error.correlationId).toBe(`corr-${suite.node}-${reason}`);
    });
  });

  describe.each([
    ['archive', suite.archive, suite.methods.archive],
    ['restore', suite.restore, suite.methods.restore],
  ] as const)(`%s ${suite.node}`, (_verb, command, method) => {
    it('sends only the node PRN and returns ok', async () => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      expect(await command(deps(), { prn: suite.prn })).toEqual({ ok: true });

      expect(calls(method).map((call) => call.request)).toEqual([expect.objectContaining({ prn: suite.prn })]);
    });

    it("returns IAM's 403 with the correlation id", async () => {
      iam.setHandlers(suite.handlers({ fail: denial({ correlationId: `corr-${suite.node}-lifecycle` }) }));

      const result = await command(deps(), { prn: suite.prn });

      if (result.ok) throw new Error('expected a denial');
      expect(result.error.presentation).toBe('forbidden');
      expect(result.error.reason).toBe(ErrorReason.FORBIDDEN);
      expect(result.error.correlationId).toBe(`corr-${suite.node}-lifecycle`);
    });
  });
}

lifecycleSuite({
  node: 'organization',
  prn: ORG_A,
  methods: { rename: 'tenancy.renameOrganization', archive: 'tenancy.archiveOrganization', restore: 'tenancy.restoreOrganization' },
  handlers: (script) => ({
    'tenancy.renameOrganization': (req) => {
      answerRename(script, req);
      return { organization: { prn: req.prn, ...nodeFields(NodeStatus.ACTIVE, req) } };
    },
    'tenancy.archiveOrganization': (req) => {
      answer(script);
      return { organization: { prn: req.prn, ...nodeFields(NodeStatus.ARCHIVED) } };
    },
    'tenancy.restoreOrganization': (req) => {
      answer(script);
      return { organization: { prn: req.prn, ...nodeFields(NodeStatus.ACTIVE) } };
    },
  }),
  rename: renameOrganization,
  archive: archiveOrganization,
  restore: restoreOrganization,
});

describe('the organization lifecycle forms', () => {
  it('trim every field, allow an empty current value, and refuse an empty PRN', async () => {
    const { archiveOrganizationForm, renameOrganizationForm, restoreOrganizationForm } = await import('../../app/(console)/orgs/[org]/commands');
    expect(renameOrganizationForm.safeParse({ prn: ` ${ORG_A} `, slug: ' acme ', name: ' Acme ', currentSlug: ' acme ', currentName: '' }).data).toEqual({
      prn: ORG_A,
      slug: 'acme',
      name: 'Acme',
      currentSlug: 'acme',
      currentName: '',
    });
    expect(renameOrganizationForm.safeParse({ prn: ORG_A, slug: 'acme', name: 'Acme', currentSlug: 'acme', currentName: null }).success).toBe(false);
    expect(archiveOrganizationForm.safeParse({ prn: ' ' }).success).toBe(false);
    expect(restoreOrganizationForm.safeParse({ prn: ORG_A }).data).toEqual({ prn: ORG_A });
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/lifecycle-commands.test.ts`
Expected: FAIL. `renameOrganization`, `archiveOrganization` and `restoreOrganization` are not exported (`TypeError: suite.rename is not a function`).

- [ ] **Step 3: Write the organization commands**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands of the organization page (spec § 5.3): create a team, and rename, archive and
// restore the organization (SMA-630 spec § 4). They take NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { currentField, nameField, prnField, renameChange, slugField, toActionResult, type ActionResult } from '../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createTeamForm = z.object({ orgPrn: prnField, slug: slugField, name: nameField });
export type CreateTeamInput = z.infer<typeof createTeamForm>;

export async function createTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createTeam'> }, input: CreateTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createTeam({ orgPrn: input.orgPrn, slug: input.slug, name: input.name })));
}

/**
 * The rename form holds the current values as hidden fields, so the command sends only what the
 * user changed (SMA-630 spec D6). A changed hidden `prn` is safe: IAM authorizes the action against
 * the STORED node that the PRN names (spec § 4.2).
 */
export const renameOrganizationForm = z.object({ prn: prnField, slug: slugField, name: nameField, currentSlug: currentField, currentName: currentField });
export type RenameOrganizationInput = z.infer<typeof renameOrganizationForm>;

export async function renameOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameOrganization'> }, input: RenameOrganizationInput): Promise<ActionResult> {
  const change = renameChange(input);
  return toActionResult(await callIam(() => deps.tenancy.renameOrganization({ prn: input.prn, ...change })));
}

export const archiveOrganizationForm = z.object({ prn: prnField });
export type ArchiveOrganizationInput = z.infer<typeof archiveOrganizationForm>;

export async function archiveOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveOrganization'> }, input: ArchiveOrganizationInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.archiveOrganization({ prn: input.prn })));
}

export const restoreOrganizationForm = archiveOrganizationForm;
export type RestoreOrganizationInput = z.infer<typeof restoreOrganizationForm>;

export async function restoreOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreOrganization'> }, input: RestoreOrganizationInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.restoreOrganization({ prn: input.prn })));
}
```

- [ ] **Step 4: Run the suite**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/lifecycle-commands.test.ts`
Expected: PASS (3 + 1 + 1 + 4 rename cases, 2 archive, 2 restore, 1 form case).

- [ ] **Step 5: Prove that D6 is tested, then remove the mutation**

In `renameOrganization`, temporarily replace `...change` with `newSlug: input.slug, newName: input.name`. Run the suite. Expected: FAIL on `only the slug changed…`, `only the name changed…`, and the two nothing-to-rename cases. Restore `...change` with the Edit tool and run again. Expected: PASS.

- [ ] **Step 6: Write the failing copy test**

In `ts/apps/iam-console/tests/unit/error-copy.test.ts`, after the `it('prefers the reason copy, …')` block, insert:

```ts

  // SMA-630 spec § 7. D6 makes this reason reachable: a rename submit with no change sends neither field.
  it('tells the user to change a field when IAM answers nothing-to-rename', () => {
    expect(formMessage(errorWith('invalid-input', ErrorReason.NOTHING_TO_RENAME))).toBe('Change the slug or the name first.');
  });
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-copy.test.ts`
Expected: FAIL. The message is `Check the values and try again.` (the presentation fallback).

- [ ] **Step 7: Add the copy**

In `ts/apps/iam-console/app/_components/error-copy.ts`, replace:

```ts
// Two tables. PRESENTATION_COPY is total over Presentation, so a tenth presentation fails the
// type-check. FORM_REASON_COPY covers the reasons the five forms can get; a test asserts every key
```

with:

```ts
// Two tables. PRESENTATION_COPY is total over Presentation, so a tenth presentation fails the
// type-check. FORM_REASON_COPY covers the reasons that the create, membership, rename, archive and
// restore forms can get (SMA-630 added the last three); a test asserts every key
```

and replace:

```ts
  [ErrorReason.NODE_ARCHIVED]: 'This item is archived.',
```

with:

```ts
  [ErrorReason.NODE_ARCHIVED]: 'This item is archived.',
  // SMA-630 spec § 7: a rename with no changed field reaches IAM with neither field (D6).
  [ErrorReason.NOTHING_TO_RENAME]: 'Change the slug or the name first.',
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-copy.test.ts`
Expected: PASS.

- [ ] **Step 8: Type-check, format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/orgs/[org]/commands.ts" apps/iam-console/tests/integration/lifecycle-commands.test.ts apps/iam-console/app/_components/error-copy.ts apps/iam-console/tests/unit/error-copy.test.ts
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts" ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts ts/apps/iam-console/app/_components/error-copy.ts ts/apps/iam-console/tests/unit/error-copy.test.ts
git commit -m "feat(ts): add the organization rename, archive and restore commands (SMA-630)" -m "The rename command sends only the changed fields. A table-driven tier-2 suite covers the three commands against the fake IAM. The form copy now covers nothing-to-rename." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Team commands

Spec § 4.1-§ 4.3, § 9.1.

**Files:**
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts` (whole file)
- Test: `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts` (append)

**Interfaces:**
- Consumes: Task 3 exports; `lifecycleSuite`, `nodeFields`, `answer`, `answerRename`, `ORG_A` in the test file (Task 5); `teamPrn` from `@paigasus/console-core`.
- Produces, from `app/(console)/orgs/[org]/teams/[team]/commands.ts`:
  - `renameTeamForm`, `type RenameTeamInput`; `archiveTeamForm`, `restoreTeamForm`, `type ArchiveTeamInput`, `type RestoreTeamInput`
  - `renameTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameTeam'> }, input: RenameTeamInput): Promise<ActionResult>`
  - `archiveTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveTeam'> }, input: ArchiveTeamInput): Promise<ActionResult>`
  - `restoreTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreTeam'> }, input: RestoreTeamInput): Promise<ActionResult>`

- [ ] **Step 1: Add the failing team suite**

In `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts`, replace:

```ts
import { organizationPrn } from '@paigasus/console-core';
```

with:

```ts
import { organizationPrn, teamPrn } from '@paigasus/console-core';
```

and after the line `import { archiveOrganization, renameOrganization, restoreOrganization } from '../../app/(console)/orgs/[org]/commands';` insert:

```ts
import { archiveTeam, renameTeam, restoreTeam } from '../../app/(console)/orgs/[org]/teams/[team]/commands';
```

After the line `const ORG_A = organizationPrn(IDS.orgA);` insert:

```ts
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
```

Before `describe('the organization lifecycle forms'`, insert:

```ts
lifecycleSuite({
  node: 'team',
  prn: TEAM_A1,
  methods: { rename: 'tenancy.renameTeam', archive: 'tenancy.archiveTeam', restore: 'tenancy.restoreTeam' },
  handlers: (script) => ({
    'tenancy.renameTeam': (req) => {
      answerRename(script, req);
      return { team: { prn: req.prn, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE, req) } };
    },
    'tenancy.archiveTeam': (req) => {
      answer(script);
      return { team: { prn: req.prn, orgPrn: ORG_A, ...nodeFields(NodeStatus.ARCHIVED) } };
    },
    'tenancy.restoreTeam': (req) => {
      answer(script);
      return { team: { prn: req.prn, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE) } };
    },
  }),
  rename: renameTeam,
  archive: archiveTeam,
  restore: restoreTeam,
});

```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/lifecycle-commands.test.ts`
Expected: FAIL. The `rename team`, `archive team` and `restore team` cases fail with `suite.rename is not a function` (or `command is not a function`). The organization cases still pass.

- [ ] **Step 3: Write the team commands**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands of the team page (spec § 5.3): create a project, and rename, archive and restore the
// team (SMA-630 spec § 4). They take NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { currentField, nameField, prnField, renameChange, slugField, toActionResult, type ActionResult } from '../../../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createProjectForm = z.object({ teamPrn: prnField, slug: slugField, name: nameField });
export type CreateProjectInput = z.infer<typeof createProjectForm>;

export async function createProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createProject'> }, input: CreateProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createProject({ teamPrn: input.teamPrn, slug: input.slug, name: input.name })));
}

/** See ../../commands.ts: the rename form holds the current values, and the command sends only the changes (D6). */
export const renameTeamForm = z.object({ prn: prnField, slug: slugField, name: nameField, currentSlug: currentField, currentName: currentField });
export type RenameTeamInput = z.infer<typeof renameTeamForm>;

export async function renameTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameTeam'> }, input: RenameTeamInput): Promise<ActionResult> {
  const change = renameChange(input);
  return toActionResult(await callIam(() => deps.tenancy.renameTeam({ prn: input.prn, ...change })));
}

export const archiveTeamForm = z.object({ prn: prnField });
export type ArchiveTeamInput = z.infer<typeof archiveTeamForm>;

export async function archiveTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveTeam'> }, input: ArchiveTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.archiveTeam({ prn: input.prn })));
}

export const restoreTeamForm = archiveTeamForm;
export type RestoreTeamInput = z.infer<typeof restoreTeamForm>;

export async function restoreTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreTeam'> }, input: RestoreTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.restoreTeam({ prn: input.prn })));
}
```

- [ ] **Step 4: Run the suites**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/lifecycle-commands.test.ts tests/integration/team-project-pages.test.ts`
Expected: PASS.

- [ ] **Step 5: Type-check, format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts" apps/iam-console/tests/integration/lifecycle-commands.test.ts
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts" ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts
git commit -m "feat(ts): add the team rename, archive and restore commands (SMA-630)" -m "The team commands follow the organization commands, and the tier-2 lifecycle suite runs for a team." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Project commands (new `commands.ts`)

Spec § 4.1-§ 4.3, § 9.1.

**Files:**
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts`
- Test: `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts` (append)

**Interfaces:**
- Consumes: Task 3 exports; `lifecycleSuite`, `nodeFields`, `answer`, `answerRename`, `ORG_A`, `TEAM_A1` in the test file; `projectPrn` from `@paigasus/console-core`.
- Produces, from `…/projects/[project]/commands.ts`:
  - `renameProjectForm`, `type RenameProjectInput`; `archiveProjectForm`, `restoreProjectForm`, `type ArchiveProjectInput`, `type RestoreProjectInput`
  - `renameProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameProject'> }, input: RenameProjectInput): Promise<ActionResult>`
  - `archiveProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveProject'> }, input: ArchiveProjectInput): Promise<ActionResult>`
  - `restoreProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreProject'> }, input: RestoreProjectInput): Promise<ActionResult>`

- [ ] **Step 1: Add the failing project suite**

In `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts`, replace:

```ts
import { organizationPrn, teamPrn } from '@paigasus/console-core';
```

with:

```ts
import { organizationPrn, projectPrn, teamPrn } from '@paigasus/console-core';
```

After the line `import { archiveTeam, renameTeam, restoreTeam } from '../../app/(console)/orgs/[org]/teams/[team]/commands';` insert:

```ts
import { archiveProject, renameProject, restoreProject } from '../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands';
```

After the line `const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);` insert:

```ts
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
```

Before `describe('the organization lifecycle forms'`, insert:

```ts
lifecycleSuite({
  node: 'project',
  prn: PROJECT_A1,
  methods: { rename: 'tenancy.renameProject', archive: 'tenancy.archiveProject', restore: 'tenancy.restoreProject' },
  handlers: (script) => ({
    'tenancy.renameProject': (req) => {
      answerRename(script, req);
      return { project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE, req) } };
    },
    'tenancy.archiveProject': (req) => {
      answer(script);
      return { project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, ...nodeFields(NodeStatus.ARCHIVED) } };
    },
    'tenancy.restoreProject': (req) => {
      answer(script);
      return { project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE) } };
    },
  }),
  rename: renameProject,
  archive: archiveProject,
  restore: restoreProject,
});

```

- [ ] **Step 2: Run it and see it fail**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/lifecycle-commands.test.ts`
Expected: FAIL with `Failed to resolve import "../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands"`.

- [ ] **Step 3: Write the project commands**

Create `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands of the project page: rename, archive and restore the project (SMA-630 spec § 4). They
// take NO mayI: IAM decides (spec § 6.3). The rename form holds the current values, and the command
// sends only the changes (D6).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { currentField, nameField, prnField, renameChange, slugField, toActionResult, type ActionResult } from '../../../../../../../../lib/form';

export const renameProjectForm = z.object({ prn: prnField, slug: slugField, name: nameField, currentSlug: currentField, currentName: currentField });
export type RenameProjectInput = z.infer<typeof renameProjectForm>;

export async function renameProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameProject'> }, input: RenameProjectInput): Promise<ActionResult> {
  const change = renameChange(input);
  return toActionResult(await callIam(() => deps.tenancy.renameProject({ prn: input.prn, ...change })));
}

export const archiveProjectForm = z.object({ prn: prnField });
export type ArchiveProjectInput = z.infer<typeof archiveProjectForm>;

export async function archiveProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveProject'> }, input: ArchiveProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.archiveProject({ prn: input.prn })));
}

export const restoreProjectForm = archiveProjectForm;
export type RestoreProjectInput = z.infer<typeof restoreProjectForm>;

export async function restoreProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreProject'> }, input: RestoreProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.restoreProject({ prn: input.prn })));
}
```

- [ ] **Step 4: Run the suite**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/lifecycle-commands.test.ts`
Expected: PASS. The file now has 3 × (8 rename + 4 archive/restore) cases plus the form case.

- [ ] **Step 5: Type-check, format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts" apps/iam-console/tests/integration/lifecycle-commands.test.ts
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts" ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts
git commit -m "feat(ts): add the project rename, archive and restore commands (SMA-630)" -m "The project folder gets its own commands.ts, and the tier-2 lifecycle suite runs for a project." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: The nine Server Action shells, the refresh rule and the structure checks

Spec § 4.1, § 4.4, § 8 rows `EXPECTED`, banned calls, `actions-revalidate.test.ts`.

**Files:**
- Modify: `ts/apps/iam-console/lib/form.ts` (append `refreshesAfterLifecycleAction`)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts` (whole file)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts` (whole file)
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts`
- Test: `ts/apps/iam-console/tests/unit/form.test.ts` (append)
- Test: `ts/apps/iam-console/tests/unit/actions-structure.test.ts:25-29`, `:117-120`, `:155-175`
- Test: `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts:14-88` (whole file)

**Interfaces:**
- Consumes: the nine commands and nine form schemas (Tasks 5-7); `formFields`, `invalidFormInput`, `ActionResult` (`lib/form.ts`); `iamClientsForAction` (`lib/console.ts`); `TENANCY_PATH` (`lib/tenancy-path.ts`); `revalidatePath` (`next/cache`); `ActionState` (`@paigasus/console-core`).
- Produces:
  - `export function refreshesAfterLifecycleAction(result: ActionResult): boolean` in `lib/form.ts`
  - `renameOrganizationAction`, `archiveOrganizationAction`, `restoreOrganizationAction` in `orgs/[org]/actions.ts`
  - `renameTeamAction`, `archiveTeamAction`, `restoreTeamAction` in `orgs/[org]/teams/[team]/actions.ts`
  - `renameProjectAction`, `archiveProjectAction`, `restoreProjectAction` in `…/projects/[project]/actions.ts`
  - Each has the signature `(previous: ActionState, form: FormData) => Promise<ActionState>`. The rename actions read the fields `prn`, `slug`, `name`, `currentSlug`, `currentName`; archive and restore read `prn`.

- [ ] **Step 1: Write the failing refresh-rule test**

In `ts/apps/iam-console/tests/unit/form.test.ts`, replace the import line with:

```ts
import type { Presentation } from '@paigasus/sdk/errors/types';
import { currentField, formFields, invalidFormInput, nameField, NAME_MAX_CODE_POINTS, prnField, refreshesAfterLifecycleAction, renameChange, slugField, toActionResult } from '../../lib/form';
```

Append at the end of the file:

```ts

// SMA-630 spec § 4.4. A lifecycle action refreshes on success, and ALSO on the two refusals that
// often mean the page is stale.
describe('refreshesAfterLifecycleAction', () => {
  const refused = (presentation: Presentation) => ({ ok: false as const, error: { ...invalidFormInput(), presentation } });

  it('refreshes on a success', () => {
    expect(refreshesAfterLifecycleAction({ ok: true })).toBe(true);
  });

  it.each<[Presentation, boolean]>([
    ['forbidden', true],
    ['conflict', true],
    ['invalid-input', false],
    ['relogin', false],
    ['not-found', false],
    ['degraded', false],
    ['rate-limited', false],
    ['disabled', false],
    ['generic', false],
  ])('a refusal with presentation %s refreshes: %s', (presentation, expected) => {
    expect(refreshesAfterLifecycleAction(refused(presentation))).toBe(expected);
  });
});
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/form.test.ts`
Expected: FAIL with `refreshesAfterLifecycleAction is not a function`.

- [ ] **Step 2: Add the refresh rule**

At the end of `ts/apps/iam-console/lib/form.ts`, append:

```ts

/**
 * Whether a rename, archive or restore action refreshes the tenancy pages (SMA-630 spec § 4.4): on a
 * success, as every action does, and ALSO on `forbidden` and `conflict`. For these actions a refusal
 * often means that the page is stale (another user archived the node or took the slug). Without the
 * refresh the page shows "Active" next to a 403. Other refusals change nothing on the page. The five
 * create and membership actions keep their success-only rule.
 */
export function refreshesAfterLifecycleAction(result: ActionResult): boolean {
  return result.ok || result.error.presentation === 'forbidden' || result.error.presentation === 'conflict';
}
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/form.test.ts`
Expected: PASS.

- [ ] **Step 3: Write the failing structure checks**

In `ts/apps/iam-console/tests/unit/actions-structure.test.ts`, replace the `EXPECTED` constant with:

```ts
const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/orgs/actions.ts': ['attachMembershipAction', 'createOrganizationAction', 'detachMembershipAction'],
  '(console)/orgs/[org]/actions.ts': ['archiveOrganizationAction', 'createTeamAction', 'renameOrganizationAction', 'restoreOrganizationAction'],
  '(console)/orgs/[org]/teams/[team]/actions.ts': ['archiveTeamAction', 'createProjectAction', 'renameTeamAction', 'restoreTeamAction'],
  '(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts': ['archiveProjectAction', 'renameProjectAction', 'restoreProjectAction'],
};

// SMA-630 spec § 4.4, D2. A Server Action never navigates. redirect() from an action carries no
// basePath and leaves the zone; forbidden() and notFound() would replace the inline form error that
// D2 requires. The check reads identifiers: a presentation STRING such as 'forbidden' is not one.
// It also matches a property NAME (`x.forbidden`), which no action uses.
const NAVIGATION_HELPERS = ['redirect', 'forbidden', 'notFound'] as const;
```

Replace:

```ts
  if (namesIdentifier(source, 'mayI')) violations.push(`${file}: consults mayI()`);
  return { names: values.map((value) => value.name).sort(), violations };
```

with:

```ts
  if (namesIdentifier(source, 'mayI')) violations.push(`${file}: consults mayI()`);
  for (const helper of NAVIGATION_HELPERS) {
    if (namesIdentifier(source, helper)) violations.push(`${file}: names the navigation helper ${helper}()`);
  }
  return { names: values.map((value) => value.name).sort(), violations };
```

In the `it.each([...])` table of negative controls, after the row `'an action that consults mayI'` (its closing `],`), insert:

```ts
      [
        'an action that redirects',
        `${header}import { redirect } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); redirect('/orgs'); }`,
        'navigation helper redirect()',
      ],
      [
        'an action that renders the 403 view',
        `${header}import { forbidden } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); forbidden(); }`,
        'navigation helper forbidden()',
      ],
      [
        'an action that renders the 404 view',
        `${header}import { notFound } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); notFound(); }`,
        'navigation helper notFound()',
      ],
```

After the block `it('accepts a correct action, and a comment that names mayI()', …);`, insert:

```ts

    it('accepts a presentation string and a comment that name forbidden', () => {
      expect(
        checkActionsSource(
          'probe.ts',
          `${header}// Never call forbidden() here.\nexport async function a() { const c = await iamClientsForAction(); return c.ok ? 'fine' : c.error.presentation === 'forbidden'; }`,
        ).violations,
      ).toEqual([]);
    });
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts`
Expected: FAIL only on `finds exactly the expected actions.ts files and exports` (the new exports and the project file do not exist yet). The three new negative controls and the new accept case PASS.

- [ ] **Step 4: Write the failing revalidation test**

Replace the whole content of `ts/apps/iam-console/tests/unit/actions-revalidate.test.ts` from the line `import { describe, expect, it, vi } from 'vitest';` to the end with:

```ts
import { describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { revalidatedPaths } from '../support/next-cache';

const { tenancy, failNext } = vi.hoisted(() => {
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
  };
});

// The session read is the one thing an action does before its command, and it needs a Next request
// scope. tests/unit/action-session.test.ts covers that read itself; here it always succeeds.
vi.mock('../../lib/console', () => ({ iamClientsForAction: () => Promise.resolve({ ok: true, value: { tenancy } }) }));

const { attachMembershipAction, createOrganizationAction, detachMembershipAction } = await import('../../app/(console)/orgs/actions');
const { archiveOrganizationAction, createTeamAction, renameOrganizationAction, restoreOrganizationAction } = await import('../../app/(console)/orgs/[org]/actions');
const { archiveTeamAction, createProjectAction, renameTeamAction, restoreTeamAction } = await import('../../app/(console)/orgs/[org]/teams/[team]/actions');
const { archiveProjectAction, renameProjectAction, restoreProjectAction } = await import('../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions');

const ORG_PRN = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1';
const PROJECT_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:project/0190a1c3-0000-7000-8000-0000000000a1';
const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';

function form(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [name, value] of Object.entries(fields)) data.append(name, value);
  return data;
}

const rename = (prn: string): Record<string, string> => ({ prn, slug: 'new-slug', name: 'New Name', currentSlug: 'old-slug', currentName: 'Old Name' });

/** Every create and membership action, with a form its zod schema accepts. The paths are basePath-RELATIVE. */
const ACTIONS = [
  ['createOrganizationAction', () => createOrganizationAction(null, form({ slug: 'acme', name: 'Acme' }))],
  ['attachMembershipAction', () => attachMembershipAction(null, form({ principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN }))],
  ['detachMembershipAction', () => detachMembershipAction(null, form({ id: '0190a1d4-0000-7000-8000-0000000000c1' }))],
  ['createTeamAction', () => createTeamAction(null, form({ orgPrn: ORG_PRN, slug: 'platform', name: 'Platform' }))],
  ['createProjectAction', () => createProjectAction(null, form({ teamPrn: TEAM_PRN, slug: 'api', name: 'API' }))],
] as const;

/** The nine lifecycle actions (SMA-630), each with a form its zod schema accepts. */
const LIFECYCLE_ACTIONS = [
  ['renameOrganizationAction', () => renameOrganizationAction(null, form(rename(ORG_PRN)))],
  ['archiveOrganizationAction', () => archiveOrganizationAction(null, form({ prn: ORG_PRN }))],
  ['restoreOrganizationAction', () => restoreOrganizationAction(null, form({ prn: ORG_PRN }))],
  ['renameTeamAction', () => renameTeamAction(null, form(rename(TEAM_PRN)))],
  ['archiveTeamAction', () => archiveTeamAction(null, form({ prn: TEAM_PRN }))],
  ['restoreTeamAction', () => restoreTeamAction(null, form({ prn: TEAM_PRN }))],
  ['renameProjectAction', () => renameProjectAction(null, form(rename(PROJECT_PRN)))],
  ['archiveProjectAction', () => archiveProjectAction(null, form({ prn: PROJECT_PRN }))],
  ['restoreProjectAction', () => restoreProjectAction(null, form({ prn: PROJECT_PRN }))],
] as const;

/** The same nine, each with a form its zod schema refuses (an empty PRN). */
const LIFECYCLE_INVALID = [
  ['renameOrganizationAction', () => renameOrganizationAction(null, form(rename('')))],
  ['archiveOrganizationAction', () => archiveOrganizationAction(null, form({ prn: '' }))],
  ['restoreOrganizationAction', () => restoreOrganizationAction(null, form({ prn: '' }))],
  ['renameTeamAction', () => renameTeamAction(null, form(rename('')))],
  ['archiveTeamAction', () => archiveTeamAction(null, form({ prn: '' }))],
  ['restoreTeamAction', () => restoreTeamAction(null, form({ prn: '' }))],
  ['renameProjectAction', () => renameProjectAction(null, form(rename('')))],
  ['archiveProjectAction', () => archiveProjectAction(null, form({ prn: '' }))],
  ['restoreProjectAction', () => restoreProjectAction(null, form({ prn: '' }))],
] as const;

const LAYOUT_REFRESH = [{ path: '/orgs', type: 'layout' }];

describe('every Server Action revalidates /orgs with the layout scope', () => {
  it.each([...ACTIONS, ...LIFECYCLE_ACTIONS])('%s', async (_name, run) => {
    const result = await run();

    expect(result).toEqual({ ok: true });
    // Strict equality on BOTH arguments: 'layout' is what refreshes the list the parent layout
    // renders, and 'page' would leave a created node invisible until a manual reload.
    expect(revalidatedPaths).toEqual(LAYOUT_REFRESH);
  });

  // The other direction: the call is inside `if (result.ok)`, so a refused action must revalidate
  // NOTHING. Without this, an action that revalidated unconditionally would still pass above.
  it.each(ACTIONS)('%s revalidates nothing when IAM refuses', async (_name, run) => {
    failNext(Code.PermissionDenied);

    const result = await run();

    expect(result).toMatchObject({ ok: false });
    expect(revalidatedPaths).toEqual([]);
  });

  // A form zod refuses never reaches IAM, so it revalidates nothing either.
  it('revalidates nothing when the form is invalid', async () => {
    const result = await createOrganizationAction(null, form({ slug: '', name: 'Acme' }));

    expect(result).toMatchObject({ ok: false });
    expect(revalidatedPaths).toEqual([]);
  });
});

// SMA-630 spec § 4.4. For a lifecycle action a `forbidden` or `conflict` refusal often means that
// the page is stale, so these also refresh. `invalid-input` does not.
describe('the nine lifecycle actions also revalidate on forbidden and conflict', () => {
  it.each(LIFECYCLE_ACTIONS)('%s revalidates when IAM answers forbidden', async (_name, run) => {
    failNext(Code.PermissionDenied);

    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual(LAYOUT_REFRESH);
  });

  it.each(LIFECYCLE_ACTIONS)('%s revalidates when IAM answers conflict', async (_name, run) => {
    failNext(Code.AlreadyExists);

    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'conflict' } });
    expect(revalidatedPaths).toEqual(LAYOUT_REFRESH);
  });

  it.each(LIFECYCLE_ACTIONS)('%s revalidates nothing when IAM answers invalid-input', async (_name, run) => {
    failNext(Code.InvalidArgument);

    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it.each(LIFECYCLE_INVALID)('%s revalidates nothing when the form is invalid', async (_name, run) => {
    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(revalidatedPaths).toEqual([]);
  });
});
```

Keep the file header comment (lines 1-13) as it is.

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/actions-revalidate.test.ts`
Expected: FAIL with `Failed to resolve import "../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions"` (and missing exports in the two other files).

- [ ] **Step 5: Write the organization actions**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClientsForAction() rule and bans the navigation helpers. The three lifecycle actions
// (SMA-630 spec § 4.4) also refresh the page after a forbidden or conflict answer, which often means
// that the page is stale.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '@paigasus/console-core';
import { formFields, invalidFormInput, refreshesAfterLifecycleAction } from '../../../../lib/form';
import { iamClientsForAction } from '../../../../lib/console';
import { TENANCY_PATH } from '../../../../lib/tenancy-path';
import {
  archiveOrganization,
  archiveOrganizationForm,
  createTeam,
  createTeamForm,
  renameOrganization,
  renameOrganizationForm,
  restoreOrganization,
  restoreOrganizationForm,
} from './commands';

export async function createTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createTeamForm.safeParse(formFields(form, ['orgPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function renameOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = renameOrganizationForm.safeParse(formFields(form, ['prn', 'slug', 'name', 'currentSlug', 'currentName']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await renameOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function archiveOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = archiveOrganizationForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function restoreOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = restoreOrganizationForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await restoreOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
```

- [ ] **Step 6: Write the team actions**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../../../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClientsForAction() rule and bans the navigation helpers. The three lifecycle actions
// (SMA-630 spec § 4.4) also refresh the page after a forbidden or conflict answer.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '@paigasus/console-core';
import { formFields, invalidFormInput, refreshesAfterLifecycleAction } from '../../../../../../lib/form';
import { iamClientsForAction } from '../../../../../../lib/console';
import { TENANCY_PATH } from '../../../../../../lib/tenancy-path';
import { archiveTeam, archiveTeamForm, createProject, createProjectForm, renameTeam, renameTeamForm, restoreTeam, restoreTeamForm } from './commands';

export async function createProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createProjectForm.safeParse(formFields(form, ['teamPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createProject({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function renameTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = renameTeamForm.safeParse(formFields(form, ['prn', 'slug', 'name', 'currentSlug', 'currentName']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await renameTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function archiveTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = archiveTeamForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function restoreTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = restoreTeamForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await restoreTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
```

- [ ] **Step 7: Write the project actions**

Create `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../../../../../actions.ts for the rules every action here follows.
// tests/unit/actions-structure.test.ts holds the iamClientsForAction() rule and bans the navigation
// helpers. The three lifecycle actions (SMA-630 spec § 4.4) also refresh the page after a forbidden
// or conflict answer.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '@paigasus/console-core';
import { formFields, invalidFormInput, refreshesAfterLifecycleAction } from '../../../../../../../../lib/form';
import { iamClientsForAction } from '../../../../../../../../lib/console';
import { TENANCY_PATH } from '../../../../../../../../lib/tenancy-path';
import { archiveProject, archiveProjectForm, renameProject, renameProjectForm, restoreProject, restoreProjectForm } from './commands';

export async function renameProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = renameProjectForm.safeParse(formFields(form, ['prn', 'slug', 'name', 'currentSlug', 'currentName']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await renameProject({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function archiveProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = archiveProjectForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveProject({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function restoreProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = restoreProjectForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await restoreProject({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
```

- [ ] **Step 8: Run the tests**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts tests/unit/actions-revalidate.test.ts tests/unit/form.test.ts tests/unit/action-session.test.ts`
Expected: PASS.

- [ ] **Step 9: Prove that the refresh test bites, then remove the mutation**

In `archiveTeamAction`, temporarily replace `refreshesAfterLifecycleAction(result)` with `result.ok`. Run `pnpm -C ts/apps/iam-console exec vitest run tests/unit/actions-revalidate.test.ts`. Expected: FAIL on `archiveTeamAction revalidates when IAM answers forbidden` and `… conflict`. Restore the call with the Edit tool and run again. Expected: PASS.

- [ ] **Step 10: Type-check, format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts exec prettier --write apps/iam-console/lib/form.ts "apps/iam-console/app/(console)/orgs/[org]/actions.ts" "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts" "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts" apps/iam-console/tests/unit/form.test.ts apps/iam-console/tests/unit/actions-structure.test.ts apps/iam-console/tests/unit/actions-revalidate.test.ts
moon run ts:lint ts:fmt
git add ts/apps/iam-console/lib/form.ts "ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts" ts/apps/iam-console/tests/unit/form.test.ts ts/apps/iam-console/tests/unit/actions-structure.test.ts ts/apps/iam-console/tests/unit/actions-revalidate.test.ts
git commit -m "feat(ts): add the nine lifecycle server actions (SMA-630)" -m "Each action follows the create action. It also refreshes the tenancy pages after a forbidden or conflict answer. The structure test now bans redirect, forbidden and notFound in every actions.ts, with a negative control for each." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: The client components — `RenameForm`, `ArchiveButton`, `RestoreButton`

Spec § 6.1, § 6.2, § 9.2 (`lifecycle-button.test.tsx`, `rename-form.test.tsx`).

The two component tests need a DOM and user events. The app has no jsdom tier today, so this task adds the same dev dependencies `@paigasus/discovery` uses (all from the pnpm catalog; nothing new enters the lockfile's package set). The two test files select jsdom with a `// @vitest-environment jsdom` line and call `cleanup()` themselves, because the app's vitest config keeps `globals: false`.

**Files:**
- Modify: `ts/apps/iam-console/package.json` (`devDependencies`), `ts/pnpm-lock.yaml` (by `pnpm install` only)
- Create: `ts/apps/iam-console/app/_components/form-action.ts`
- Create: `ts/apps/iam-console/app/_components/rename-form.tsx`
- Create: `ts/apps/iam-console/app/_components/lifecycle-button.tsx`
- Create: `ts/apps/iam-console/tests/unit/rename-form.test.tsx`
- Create: `ts/apps/iam-console/tests/unit/lifecycle-button.test.tsx`

**Interfaces:**
- Consumes: `ActionState` (type) from `@paigasus/console-core`; `FormError` (`app/_components/form-error.tsx`, props `{ error: PaigasusError | null }`); `Field`, `Input` from `@paigasus/ui`; `ZoneProvider` from `@paigasus/app-shell` (tests); `FORM_REASON_COPY` (tests).
- Produces:
  - `export type FormAction = (previous: ActionState, form: FormData) => Promise<ActionState>;` (`form-action.ts`)
  - `export type RenameFormProps = { readonly testId: string; readonly title: string; readonly prn: string; readonly slug: string; readonly name: string; readonly action: FormAction };` and `export function RenameForm(props: RenameFormProps): ReactElement` (`rename-form.tsx`)
  - `export type LifecycleButtonProps = { readonly testId: string; readonly prn: string; readonly action: FormAction };`, `export function RestoreButton(props: LifecycleButtonProps): ReactElement`, `export function ArchiveButton(props: LifecycleButtonProps & { readonly name: string }): ReactElement`, `export function archiveConfirmation(name: string): string` (`lifecycle-button.tsx`)
  - DOM contract: `RenameForm` is `<form aria-label={title} data-testid={testId}>` with hidden `prn`, `currentSlug`, `currentName`, the labelled inputs `Slug` and `Name`, a `Rename` submit button, `Renamed.` on success, and `<div data-testid={`${testId}-error`}>`. `RestoreButton` is `<form data-testid={testId}>` with hidden `prn`, a `Restore` submit button, `Restored.`, and the `-error` div. `ArchiveButton` is `<div data-testid={testId}>`: first an `Archive` button of `type="button"`; after the click the confirmation text and a form with hidden `prn`, a `Confirm archive` submit button and a `Cancel` button of `type="button"`; `Archived.` on success; and the `-error` div.

- [ ] **Step 1: Add the test dependencies**

In `ts/apps/iam-console/package.json`, replace the `devDependencies` object with:

```json
  "devDependencies": {
    "@playwright/test": "catalog:",
    "@testing-library/dom": "catalog:",
    "@testing-library/react": "catalog:",
    "@testing-library/user-event": "catalog:",
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "@types/react-dom": "catalog:",
    "jose": "catalog:",
    "jsdom": "catalog:",
    "msw": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
```

Run: `pnpm -C ts install`
Expected: exit 0. `git diff --stat ts/pnpm-lock.yaml` shows changes in the `apps/iam-console` importer only.

- [ ] **Step 2: Write the failing lifecycle-button test**

Create `ts/apps/iam-console/tests/unit/lifecycle-button.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The two lifecycle controls (SMA-630 spec § 6.2). ArchiveButton asks in two steps with local
// state, and has NO submit button before the first click. RestoreButton submits at once.
import type { ReactElement, ReactNode } from 'react';
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
import { ArchiveButton, archiveConfirmation, RestoreButton } from '../../app/_components/lifecycle-button';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

afterEach(() => {
  cleanup();
});

const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000e1:team/0190a1b2-0000-7000-8000-0000000000e2';
const CONFIRMATION =
  'Archive Platform Team? Until you restore it, IAM refuses changes to it and to everything under it, and the AI Gateway refuses model calls for everything under it.';

const FORBIDDEN: PaigasusError = {
  presentation: 'forbidden',
  domain: ErrorDomain.IAM,
  reason: ErrorReason.FORBIDDEN,
  rawReason: 'forbidden',
  rawDomain: 'iam.paigasus.io',
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-archive',
  requestId: null,
  retryable: false,
  metadata: {},
  transport: { kind: 'http', status: 403 },
};

function actionAnswering(state: ActionState) {
  return vi.fn((_previous: ActionState, _form: FormData): Promise<ActionState> => Promise.resolve(state));
}

function renderInZone(element: ReactElement) {
  return render(element, { wrapper: ({ children }: { children: ReactNode }) => <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>{children}</ZoneProvider> });
}

describe('ArchiveButton', () => {
  it('shows no form and no submit button before the first click', () => {
    const { container } = renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={actionAnswering({ ok: true })} />);

    expect(screen.getByRole('button', { name: 'Archive' }).getAttribute('type')).toBe('button');
    expect(container.querySelector('form')).toBeNull();
    expect(container.querySelector('button[type="submit"]')).toBeNull();
  });

  it('asks with the exact spec text, and Cancel returns to the first state', async () => {
    const user = userEvent.setup();
    const { container } = renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={actionAnswering({ ok: true })} />);

    await user.click(screen.getByRole('button', { name: 'Archive' }));

    expect(archiveConfirmation('Platform Team')).toBe(CONFIRMATION);
    expect(screen.getByText(CONFIRMATION).textContent).toBe(CONFIRMATION);
    expect(screen.getByRole('button', { name: 'Confirm archive' }).getAttribute('type')).toBe('submit');
    expect(screen.getByRole('button', { name: 'Cancel' }).getAttribute('type')).toBe('button');
    expect(screen.queryByRole('button', { name: 'Archive' })).toBeNull();

    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.getByRole('button', { name: 'Archive' }).getAttribute('type')).toBe('button');
    expect(screen.queryByText(CONFIRMATION)).toBeNull();
    expect(container.querySelector('button[type="submit"]')).toBeNull();
  });

  it('posts the node PRN on confirm and shows "Archived."', async () => {
    const user = userEvent.setup();
    const action = actionAnswering({ ok: true });
    renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={action} />);

    await user.click(screen.getByRole('button', { name: 'Archive' }));
    await user.click(screen.getByRole('button', { name: 'Confirm archive' }));

    expect(await screen.findByText('Archived.')).toBeDefined();
    expect(action).toHaveBeenCalledTimes(1);
    expect(action.mock.calls[0]?.[1].get('prn')).toBe(TEAM_PRN);
  });

  it('shows a refusal with its copy and the correlation id in its error area', async () => {
    const user = userEvent.setup();
    renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={actionAnswering({ ok: false, error: FORBIDDEN })} />);

    await user.click(screen.getByRole('button', { name: 'Archive' }));
    await user.click(screen.getByRole('button', { name: 'Confirm archive' }));

    const area = screen.getByTestId('archive-team-error');
    expect(await within(area).findByText(FORM_REASON_COPY[ErrorReason.FORBIDDEN] ?? '')).toBeDefined();
    expect(within(area).getByTestId('correlation-id').textContent).toBe('corr-unit-archive');
  });
});

describe('RestoreButton', () => {
  it('has a submit button at once, posts the node PRN and shows "Restored."', async () => {
    const user = userEvent.setup();
    const action = actionAnswering({ ok: true });
    renderInZone(<RestoreButton testId="restore-team" prn={TEAM_PRN} action={action} />);

    const button = screen.getByRole('button', { name: 'Restore' });
    expect(button.getAttribute('type')).toBe('submit');
    expect(screen.getByTestId('restore-team').tagName).toBe('FORM');
    expect(screen.getByTestId('restore-team-error')).toBeDefined();

    await user.click(button);

    expect(await screen.findByText('Restored.')).toBeDefined();
    expect(action.mock.calls[0]?.[1].get('prn')).toBe(TEAM_PRN);
  });
});
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/lifecycle-button.test.tsx`
Expected: FAIL with `Failed to resolve import "../../app/_components/lifecycle-button"`.

- [ ] **Step 3: Write the `FormAction` type and the lifecycle buttons**

Create `ts/apps/iam-console/app/_components/form-action.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The shape of a Server Action that a client form posts to. A TYPE-only module: the import below is
// erased (verbatimModuleSyntax), so no server-only module reaches a client bundle.
import type { ActionState } from '@paigasus/console-core';

export type FormAction = (previous: ActionState, form: FormData) => Promise<ActionState>;
```

Create `ts/apps/iam-console/app/_components/lifecycle-button.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useState, type ReactElement } from 'react';
import type { FormAction } from './form-action';
import { FormError } from './form-error';

/**
 * The lifecycle controls of the Manage section (SMA-630 spec § 6.2). The section renders ONE of the
 * two, with `key` set to the lifecycle view, so no `useActionState` result and no `confirming` state
 * passes from one to the other. The page renders the section only for a node where mayI() allowed
 * the transition; the Server Action still asks IAM (spec § 6.3).
 */
export type LifecycleButtonProps = {
  readonly testId: string;
  readonly prn: string;
  readonly action: FormAction;
};

const PRIMARY = 'bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50';
const SECONDARY = 'border-input rounded-pgs self-start border px-3 py-1.5 text-sm font-medium disabled:opacity-50';

/** The exact confirmation text (spec § 6.2). It names the AI Gateway effect (spec F10). */
export function archiveConfirmation(name: string): string {
  return `Archive ${name}? Until you restore it, IAM refuses changes to it and to everything under it, and the AI Gateway refuses model calls for everything under it.`;
}

/** One form, no confirmation: a restore starts traffic again, which is its purpose (spec § 10). */
export function RestoreButton({ testId, prn, action }: LifecycleButtonProps): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  return (
    <form action={formAction} aria-label="Restore this item" data-testid={testId} className="flex flex-col gap-2">
      <input type="hidden" name="prn" value={prn} />
      <button type="submit" disabled={pending} className={PRIMARY}>
        Restore
      </button>
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Restored.
        </p>
      ) : null}
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}

/**
 * Two steps, with local state only. The first state has NO form, so with JavaScript off an archive
 * is not possible at all (spec § 6.2 accepts this). After a successful archive the page renders
 * again, and the section replaces this component with RestoreButton.
 */
export function ArchiveButton({ testId, prn, name, action }: LifecycleButtonProps & { readonly name: string }): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const [confirming, setConfirming] = useState(false);
  return (
    <div data-testid={testId} className="flex flex-col gap-2">
      {confirming ? (
        <form action={formAction} aria-label="Confirm the archive" className="flex flex-col gap-2">
          <p className="text-sm">{archiveConfirmation(name)}</p>
          <input type="hidden" name="prn" value={prn} />
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={pending} className={PRIMARY}>
              Confirm archive
            </button>
            <button
              type="button"
              disabled={pending}
              className={SECONDARY}
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
          className={SECONDARY}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Archive
        </button>
      )}
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Archived.
        </p>
      ) : null}
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </div>
  );
}
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/lifecycle-button.test.tsx`
Expected: PASS (5 cases).

- [ ] **Step 4: Write the failing rename-form test**

Create `ts/apps/iam-console/tests/unit/rename-form.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The rename form (SMA-630 spec § 6.1). React 19 resets a form before it runs EVERY action, whatever
// the result. With uncontrolled inputs a failed rename would put the old values back, and "This
// slug is already in use" would show next to the old slug. The inputs are therefore controlled.
// The control case below proves that this harness really resets a form, so the main case is not
// green for the wrong reason.
import { useActionState, type ReactElement, type ReactNode } from 'react';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import type { FormAction } from '../../app/_components/form-action';
import { RenameForm } from '../../app/_components/rename-form';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

afterEach(() => {
  cleanup();
});

const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000e1:team/0190a1b2-0000-7000-8000-0000000000e2';

const SLUG_CONFLICT: PaigasusError = {
  presentation: 'conflict',
  domain: ErrorDomain.IAM,
  reason: ErrorReason.SLUG_CONFLICT,
  rawReason: 'slug-conflict',
  rawDomain: 'iam.paigasus.io',
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-rename',
  requestId: null,
  retryable: false,
  metadata: {},
  transport: { kind: 'http', status: 409 },
};

function failing() {
  return vi.fn((_previous: ActionState, _form: FormData): Promise<ActionState> => Promise.resolve({ ok: false, error: SLUG_CONFLICT }));
}

function renderInZone(element: ReactElement) {
  return render(element, { wrapper: ({ children }: { children: ReactNode }) => <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>{children}</ZoneProvider> });
}

/** An UNCONTROLLED form with the same action wiring: the control for the reset. */
function UncontrolledControl({ action }: { readonly action: FormAction }): ReactElement {
  const [state, formAction] = useActionState(action, null);
  return (
    <form action={formAction}>
      <input aria-label="Plain slug" name="slug" defaultValue="platform" />
      <button type="submit">Send</button>
      {state?.ok === false ? <p>failed</p> : null}
    </form>
  );
}

describe('RenameForm', () => {
  it('starts from the current values, and posts them as hidden fields with the typed values', async () => {
    const user = userEvent.setup();
    const action = failing();
    renderInZone(<RenameForm testId="rename-team" title="Rename team" prn={TEAM_PRN} slug="platform" name="Platform Team" action={action} />);

    const slug = screen.getByLabelText<HTMLInputElement>('Slug');
    const name = screen.getByLabelText<HTMLInputElement>('Name');
    expect(slug.value).toBe('platform');
    expect(name.value).toBe('Platform Team');
    expect(screen.getByRole('form', { name: 'Rename team' }).getAttribute('data-testid')).toBe('rename-team');

    await user.clear(slug);
    await user.type(slug, 'taken');
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await screen.findByText(FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT] ?? '');

    const posted = action.mock.calls[0]?.[1];
    expect(posted?.get('prn')).toBe(TEAM_PRN);
    expect(posted?.get('slug')).toBe('taken');
    expect(posted?.get('name')).toBe('Platform Team');
    expect(posted?.get('currentSlug')).toBe('platform');
    expect(posted?.get('currentName')).toBe('Platform Team');
  });

  it('keeps the typed values after an action that returns a failure', async () => {
    const user = userEvent.setup();
    renderInZone(<RenameForm testId="rename-team" title="Rename team" prn={TEAM_PRN} slug="platform" name="Platform Team" action={failing()} />);

    const slug = screen.getByLabelText<HTMLInputElement>('Slug');
    const name = screen.getByLabelText<HTMLInputElement>('Name');
    await user.clear(slug);
    await user.type(slug, 'taken');
    await user.clear(name);
    await user.type(name, 'Renamed Team');
    await user.click(screen.getByRole('button', { name: 'Rename' }));

    const error = await screen.findByText(FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT] ?? '');
    expect(screen.getByTestId('rename-team-error').contains(error)).toBe(true);
    expect(slug.value).toBe('taken');
    expect(name.value).toBe('Renamed Team');
  });

  it('shows "Renamed." after a success', async () => {
    const user = userEvent.setup();
    const action = vi.fn((_previous: ActionState, _form: FormData): Promise<ActionState> => Promise.resolve({ ok: true }));
    renderInZone(<RenameForm testId="rename-team" title="Rename team" prn={TEAM_PRN} slug="platform" name="Platform Team" action={action} />);

    await user.click(screen.getByRole('button', { name: 'Rename' }));

    expect(await screen.findByText('Renamed.')).toBeDefined();
  });

  // The control: the same wiring with an uncontrolled input LOSES the typed value. If this case
  // ever passes with the typed value, the harness no longer resets forms, and the case above proves
  // nothing.
  it('control: an uncontrolled input goes back to its default value after a failed action', async () => {
    const user = userEvent.setup();
    render(<UncontrolledControl action={failing()} />);

    const slug = screen.getByLabelText<HTMLInputElement>('Plain slug');
    await user.clear(slug);
    await user.type(slug, 'taken');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await screen.findByText('failed');

    expect(slug.value).toBe('platform');
  });
});
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/rename-form.test.tsx`
Expected: FAIL with `Failed to resolve import "../../app/_components/rename-form"`.

- [ ] **Step 5: Write the rename form**

Create `ts/apps/iam-console/app/_components/rename-form.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useId, useState, type ReactElement } from 'react';
import { Field, Input } from '@paigasus/ui';
import type { FormAction } from './form-action';
import { FormError } from './form-error';

/**
 * The rename form of a tenancy node (SMA-630 spec § 6.1). The hidden `currentSlug` and
 * `currentName` let the command send only the fields that the user changed (D6).
 *
 * THE INPUTS ARE CONTROLLED. React 19 resets a form before it runs every action, whatever the result
 * (react-dom-client.development.js:8954-8957). Controlled inputs keep their React state through that
 * reset, so after a failed rename the typed values stay next to the error. The Manage section
 * renders this component with `key={`${slug} ${name}`}`: after a successful rename the page renders
 * again with new props, the key changes, and the form starts again from the new values, with a new
 * `useActionState`, so "Renamed." does not stay.
 */
export type RenameFormProps = {
  readonly testId: string;
  readonly title: string;
  readonly prn: string;
  readonly slug: string;
  readonly name: string;
  readonly action: FormAction;
};

export function RenameForm({ testId, title, prn, slug: currentSlug, name: currentName, action }: RenameFormProps): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const [slug, setSlug] = useState(currentSlug);
  const [name, setName] = useState(currentName);
  const id = useId();
  return (
    <form action={formAction} aria-label={title} data-testid={testId} className="flex max-w-md flex-col gap-3">
      <h3 className="text-sm font-medium">{title}</h3>
      <input type="hidden" name="prn" value={prn} />
      <input type="hidden" name="currentSlug" value={currentSlug} />
      <input type="hidden" name="currentName" value={currentName} />
      <Field label="Slug" htmlFor={`${id}-slug`}>
        <Input
          name="slug"
          required
          autoComplete="off"
          value={slug}
          onChange={(event) => {
            setSlug(event.target.value);
          }}
        />
      </Field>
      <Field label="Name" htmlFor={`${id}-name`}>
        <Input
          name="name"
          required
          autoComplete="off"
          value={name}
          onChange={(event) => {
            setName(event.target.value);
          }}
        />
      </Field>
      <button type="submit" disabled={pending} className="bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50">
        Rename
      </button>
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Renamed.
        </p>
      ) : null}
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}
```

- [ ] **Step 6: Run the tests**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/rename-form.test.tsx tests/unit/lifecycle-button.test.tsx`
Expected: PASS (4 + 5 cases). If the control case fails with `taken`, STOP and report: the harness does not reset forms, so the main case cannot prove § 6.1.

- [ ] **Step 7: Prove that the controlled inputs matter, then remove the mutation**

In `rename-form.tsx`, temporarily replace `value={slug}` with `defaultValue={currentSlug}` and delete the slug `onChange` prop. Run `pnpm -C ts/apps/iam-console exec vitest run tests/unit/rename-form.test.tsx`. Expected: FAIL on `keeps the typed values after an action that returns a failure` (the slug is `platform`). Restore the two props with the Edit tool and run again. Expected: PASS.

- [ ] **Step 8: Type-check, run the whole app suite, format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts/apps/iam-console exec vitest run tests/unit tests/integration
pnpm -C ts exec prettier --write apps/iam-console/package.json apps/iam-console/app/_components/form-action.ts apps/iam-console/app/_components/rename-form.tsx apps/iam-console/app/_components/lifecycle-button.tsx apps/iam-console/tests/unit/rename-form.test.tsx apps/iam-console/tests/unit/lifecycle-button.test.tsx
moon run ts:lint ts:fmt
git add ts/apps/iam-console/package.json ts/pnpm-lock.yaml ts/apps/iam-console/app/_components/form-action.ts ts/apps/iam-console/app/_components/rename-form.tsx ts/apps/iam-console/app/_components/lifecycle-button.tsx ts/apps/iam-console/tests/unit/rename-form.test.tsx ts/apps/iam-console/tests/unit/lifecycle-button.test.tsx
git commit -m "feat(ts): add the rename form and the lifecycle buttons (SMA-630)" -m "The rename form uses controlled inputs, so a failed rename keeps the typed values. Archive asks in two steps with local state. The app gets a jsdom test tier for these two components." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: `ManageSection`, `StatusBadge`, and the D7 pointer in the 511 spec

Spec § 5.3 (the table, the note rule, D7), § 6.3, § 9.2 (`manage-section.test.tsx`), § 8 row "511 § 6.3".

**Files:**
- Create: `ts/apps/iam-console/app/(console)/manage-section.tsx`
- Create: `ts/apps/iam-console/app/(console)/status-badge.tsx`
- Create: `ts/apps/iam-console/tests/unit/manage-section.test.tsx`
- Create: `ts/apps/iam-console/tests/unit/status-badge.test.tsx`
- Modify: `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md:484` (insert after)

**Interfaces:**
- Consumes: `lifecycleView`, `BADGE_LABEL`, `PARENT_ARCHIVED_NOTE`, `NodeLifecycle` (Task 4); `RenameForm`, `ArchiveButton`, `RestoreButton`, `FormAction` (Task 9).
- Produces:
  - `export type ManageNode = 'organization' | 'team' | 'project';`
  - `export type ManageSectionProps = { readonly node: ManageNode; readonly prn: string; readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle; readonly can: { readonly rename: boolean; readonly archive: boolean; readonly restore: boolean }; readonly actions: { readonly rename: FormAction; readonly archive: FormAction; readonly restore: FormAction } };`
  - `export function ManageSection(props: ManageSectionProps): ReactElement | null` — a `<section aria-labelledby="manage-heading">` with the heading `Manage`, or `null`.
  - `export function StatusBadge({ lifecycle }: { readonly lifecycle: NodeLifecycle }): ReactElement | null` — `<span data-testid="node-status">`, or `null` for `active`.

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/iam-console/tests/unit/manage-section.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The Manage section (SMA-630 spec § 5.3), row by row. Its controls follow two inputs: mayI()
// (`can`) and the node's own status (D7). A server component, so the test renders it statically.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { FormAction } from '../../app/_components/form-action';
import { ManageSection, type ManageSectionProps } from '../../app/(console)/manage-section';
import { PARENT_ARCHIVED_NOTE, type NodeLifecycle } from '../../app/(console)/node-status';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

const noop: FormAction = () => Promise.resolve(null);

const LIFECYCLE = {
  active: { own: 'active', effective: 'active' },
  archived: { own: 'archived', effective: 'archived' },
  'archived-parent': { own: 'active', effective: 'archived' },
  unknown: { own: 'unknown', effective: 'active' },
} as const satisfies Record<string, NodeLifecycle>;

type Can = ManageSectionProps['can'];
const ALL: Can = { rename: true, archive: true, restore: true };
const NONE: Can = { rename: false, archive: false, restore: false };

function render(lifecycle: NodeLifecycle, can: Can): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <ManageSection node="team" prn="prn:pgs:iam::o:team/t" name="Platform Team" slug="platform" lifecycle={lifecycle} can={can} actions={{ rename: noop, archive: noop, restore: noop }} />
    </ZoneProvider>,
  );
}

type Shown = { readonly rename: boolean; readonly archive: boolean; readonly restore: boolean; readonly note: boolean };

function shown(html: string): Shown {
  return {
    rename: html.includes('data-testid="rename-team"'),
    archive: html.includes('data-testid="archive-team"'),
    restore: html.includes('data-testid="restore-team"'),
    note: html.includes(PARENT_ARCHIVED_NOTE),
  };
}

type Row = { readonly label: string; readonly lifecycle: NodeLifecycle; readonly can: Can; readonly expected: Shown };

// spec § 5.3: `active` → Archive; `archived` → Restore; `archived-parent` → Archive and the note;
// `unknown` → no lifecycle control. The rename form follows `can.rename` in every row.
const ROWS: readonly Row[] = [
  { label: 'active, all allowed', lifecycle: LIFECYCLE.active, can: ALL, expected: { rename: true, archive: true, restore: false, note: false } },
  { label: 'active, rename denied', lifecycle: LIFECYCLE.active, can: { ...ALL, rename: false }, expected: { rename: false, archive: true, restore: false, note: false } },
  { label: 'active, archive denied', lifecycle: LIFECYCLE.active, can: { ...ALL, archive: false }, expected: { rename: true, archive: false, restore: false, note: false } },
  { label: 'archived, all allowed', lifecycle: LIFECYCLE.archived, can: ALL, expected: { rename: true, archive: false, restore: true, note: false } },
  { label: 'archived, restore denied', lifecycle: LIFECYCLE.archived, can: { ...ALL, restore: false }, expected: { rename: true, archive: false, restore: false, note: false } },
  { label: 'archived, only restore allowed', lifecycle: LIFECYCLE.archived, can: { ...NONE, restore: true }, expected: { rename: false, archive: false, restore: true, note: false } },
  { label: 'archived-parent, all allowed', lifecycle: LIFECYCLE['archived-parent'], can: ALL, expected: { rename: true, archive: true, restore: false, note: true } },
  { label: 'archived-parent, archive denied', lifecycle: LIFECYCLE['archived-parent'], can: { ...ALL, archive: false }, expected: { rename: true, archive: false, restore: false, note: true } },
  { label: 'unknown, all allowed', lifecycle: LIFECYCLE.unknown, can: ALL, expected: { rename: true, archive: false, restore: false, note: false } },
  { label: 'unknown, rename denied', lifecycle: LIFECYCLE.unknown, can: { ...ALL, rename: false }, expected: { rename: false, archive: false, restore: false, note: false } },
];

describe('ManageSection (spec § 5.3)', () => {
  it.each(ROWS)('$label', ({ lifecycle, can, expected }) => {
    const html = render(lifecycle, can);
    expect(shown(html)).toEqual(expected);
    expect(html).toContain('>Manage</h2>');
  });

  it('renders nothing when it has no control and no note', () => {
    expect(render(LIFECYCLE.active, NONE)).toBe('');
    expect(render(LIFECYCLE.archived, { ...ALL, restore: false, rename: false })).toBe('');
    expect(render(LIFECYCLE.unknown, { ...NONE, archive: true, restore: true })).toBe('');
  });

  it('renders only the note for archived-parent with no control (the default policy, spec F9)', () => {
    const html = render(LIFECYCLE['archived-parent'], { ...NONE, restore: true });
    expect(shown(html)).toEqual({ rename: false, archive: false, restore: false, note: true });
    expect(html).toContain('data-testid="manage-note"');
  });

  it('names the controls after the node, and passes the name to the archive control', () => {
    const html = render(LIFECYCLE.active, ALL);
    expect(html).toContain('aria-label="Rename team"');
    expect(html).toContain('data-testid="rename-team-error"');
    expect(html).toContain('data-testid="archive-team-error"');
    expect(html).toContain('>Archive</button>');
    expect(html).not.toContain('Confirm archive');
  });
});
```

Create `ts/apps/iam-console/tests/unit/status-badge.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The header badge (SMA-630 spec § 6.3): no badge for an active node, one label per other view.
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { NodeLifecycle } from '../../app/(console)/node-status';
import { StatusBadge } from '../../app/(console)/status-badge';

describe('StatusBadge', () => {
  it('renders nothing for an active node', () => {
    expect(renderToStaticMarkup(<StatusBadge lifecycle={{ own: 'active', effective: 'active' }} />)).toBe('');
  });

  it.each<[string, NodeLifecycle, string]>([
    ['archived', { own: 'archived', effective: 'archived' }, 'Archived'],
    ['archived-parent', { own: 'active', effective: 'archived' }, 'Archived (parent)'],
    ['unknown', { own: 'unknown', effective: 'unknown' }, 'Status unknown'],
  ])('labels %s', (_view, lifecycle, label) => {
    const html = renderToStaticMarkup(<StatusBadge lifecycle={lifecycle} />);
    expect(html).toContain('data-testid="node-status"');
    expect(html).toContain(`>${label}</span>`);
  });
});
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/manage-section.test.tsx tests/unit/status-badge.test.tsx`
Expected: FAIL with `Failed to resolve import "../../app/(console)/manage-section"` and `… status-badge`.

- [ ] **Step 2: Write the status badge**

Create `ts/apps/iam-console/app/(console)/status-badge.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The badge next to the `h1` of a detail page (SMA-630 spec § 6.3). An active node gets none.
import type { ReactElement } from 'react';
import { BADGE_LABEL, lifecycleView, type NodeLifecycle } from './node-status';

export function StatusBadge({ lifecycle }: { readonly lifecycle: NodeLifecycle }): ReactElement | null {
  const label = BADGE_LABEL[lifecycleView(lifecycle)];
  if (label === null) return null;
  return (
    <span data-testid="node-status" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
      {label}
    </span>
  );
}
```

- [ ] **Step 3: Write the Manage section**

Create `ts/apps/iam-console/app/(console)/manage-section.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The Manage section of the organization, team and project pages (SMA-630 spec § 5.3). A SERVER
// component: it passes each page's own three Server Actions to the client controls as props.
//
// Two inputs select the controls:
//   - mayI() (`can`). It may hide an affordance and nothing more (511 § 6.3). Every action still
//     asks IAM.
//   - D7: the lifecycle control shows ONLY the transition that the node's own status allows. An
//     active node never shows Restore, and an archived node never shows Archive. This refuses no
//     real change: a restore of an active node changes nothing (IAM accepts it), and an archive of
//     an archived node changes nothing at the repository (IAM refuses it in the default policy). The
//     'archived-parent' view shows no Restore either: a restore of the node itself does not change
//     its effective status (spec F8). No action and no command applies D7; 511 § 6.3 points here.
import type { ReactElement } from 'react';
import type { FormAction } from '../_components/form-action';
import { ArchiveButton, RestoreButton } from '../_components/lifecycle-button';
import { RenameForm } from '../_components/rename-form';
import { lifecycleView, PARENT_ARCHIVED_NOTE, type LifecycleView, type NodeLifecycle } from './node-status';

export type ManageNode = 'organization' | 'team' | 'project';

export type ManageSectionProps = {
  /** Selects the test ids (`rename-team`, …) and the copy. */
  readonly node: ManageNode;
  readonly prn: string;
  readonly name: string;
  readonly slug: string;
  readonly lifecycle: NodeLifecycle;
  readonly can: { readonly rename: boolean; readonly archive: boolean; readonly restore: boolean };
  readonly actions: { readonly rename: FormAction; readonly archive: FormAction; readonly restore: FormAction };
};

type LifecycleControl = 'archive' | 'restore' | null;

/** The "Lifecycle control" column of the spec § 5.3 table. */
function lifecycleControl(view: LifecycleView, can: ManageSectionProps['can']): LifecycleControl {
  switch (view) {
    case 'active':
    case 'archived-parent':
      return can.archive ? 'archive' : null;
    case 'archived':
      return can.restore ? 'restore' : null;
    case 'unknown':
      return null;
  }
}

export function ManageSection({ node, prn, name, slug, lifecycle, can, actions }: ManageSectionProps): ReactElement | null {
  const view = lifecycleView(lifecycle);
  const control = lifecycleControl(view, can);
  // The note shows for 'archived-parent' even with no control: with the default policy mayI() denies
  // rename and archive there (spec F9), and without the note the user sees the badge and no hint.
  const note = view === 'archived-parent';
  if (!can.rename && control === null && !note) return null;

  return (
    <section aria-labelledby="manage-heading" className="flex flex-col gap-3">
      <h2 id="manage-heading" className="text-lg font-semibold">
        Manage
      </h2>
      {note ? (
        <p data-testid="manage-note" className="text-muted-foreground text-sm">
          {PARENT_ARCHIVED_NOTE}
        </p>
      ) : null}
      {can.rename ? <RenameForm key={`${slug} ${name}`} testId={`rename-${node}`} title={`Rename ${node}`} prn={prn} slug={slug} name={name} action={actions.rename} /> : null}
      {/* `key={view}`: the two control types never share state, and a view change starts a new control. */}
      {control === 'archive' ? <ArchiveButton key={view} testId={`archive-${node}`} prn={prn} name={name} action={actions.archive} /> : null}
      {control === 'restore' ? <RestoreButton key={view} testId={`restore-${node}`} prn={prn} action={actions.restore} /> : null}
    </section>
  );
}
```

- [ ] **Step 4: Run the tests**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/manage-section.test.tsx tests/unit/status-badge.test.tsx`
Expected: PASS (10 table rows plus 3 cases; 4 badge cases).

- [ ] **Step 5: Prove that D7 is tested, then remove the mutation**

In `lifecycleControl`, temporarily change `case 'archived': return can.restore ? 'restore' : null;` to `return can.archive ? 'archive' : null;`. Run `pnpm -C ts/apps/iam-console exec vitest run tests/unit/manage-section.test.tsx`. Expected: FAIL on `archived, all allowed`. Restore the line with the Edit tool and run again. Expected: PASS.

- [ ] **Step 6: Add the pointer to the 511 spec**

In `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md`, after the line `- The tests prove both directions and count the calls (§ 9.3, § 9.4).`, insert:

```markdown
- SMA-630 adds one rule for the lifecycle controls: a control shows only the transition that the
  node's own status allows (`docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md`,
  § 5.3, D7).
```

- [ ] **Step 7: Type-check, format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/manage-section.tsx" "apps/iam-console/app/(console)/status-badge.tsx" apps/iam-console/tests/unit/manage-section.test.tsx apps/iam-console/tests/unit/status-badge.test.tsx
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/manage-section.tsx" "ts/apps/iam-console/app/(console)/status-badge.tsx" ts/apps/iam-console/tests/unit/manage-section.test.tsx ts/apps/iam-console/tests/unit/status-badge.test.tsx docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md
git commit -m "feat(ts): add the manage section and the status badge (SMA-630)" -m "The manage section shows the rename form and the one lifecycle transition that the node status allows, and a note for a node under an archived parent. The SMA-511 spec points to the new rule." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: The organizations list and the organization page

Spec § 5.1 (loader queries), § 5.2, § 6.3 (badge, column), § 9.1 (loader tests).

**Files:**
- Modify: `ts/apps/iam-console/app/(console)/orgs/load.ts:6-10`, `:30-33`
- Modify: `ts/apps/iam-console/app/(console)/orgs/page.tsx:14`, `:52-72`
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/load.ts` (whole file)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/page.tsx` (whole file)
- Test: `ts/apps/iam-console/tests/integration/orgs-page.test.ts`
- Test: `ts/apps/iam-console/tests/integration/org-page.test.ts`

**Interfaces:**
- Consumes: `lifecycleOf`, `statusColumnLabel`, `NodeLifecycle` (Task 4); `ManageSection` (Task 10); `StatusBadge` (Task 10); the three organization actions (Task 8); `NodeStatus` (Task 1).
- Produces:
  - `OrganizationRow` gains `readonly lifecycle: NodeLifecycle`.
  - `TeamRow` gains `readonly lifecycle: NodeLifecycle`.
  - `OrganizationPageData` (`kind: 'ok'`) gains `organization.lifecycle: NodeLifecycle`, `canRename: boolean`, `canArchive: boolean`, `canRestore: boolean`.
  - `OrganizationPageDeps` and `OrganizationsPageDeps` are unchanged.

- [ ] **Step 1: Write the failing list-loader test**

In `ts/apps/iam-console/tests/integration/orgs-page.test.ts`, after `import { disposeTransports } from '@paigasus/sdk/iam';` insert:

```ts
import { NodeStatus } from '@paigasus/sdk/iam/types';
```

Replace `import { callsSince, clientsFor, scriptedMayI } from './support';` with:

```ts
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';
```

In `function organizations(count: number)`, replace:

```ts
    name: `Org ${String(index)}`,
  }));
```

with:

```ts
    name: `Org ${String(index)}`,
    status: NodeStatus.ACTIVE,
    effectiveStatus: NodeStatus.ACTIVE,
  }));
```

Replace:

```ts
    expect(first.all.value.rows[0]).toEqual({ prn: organizationPrn('0190a100-0000-7000-8000-000000000000'), orgId: '0190a100-0000-7000-8000-000000000000', slug: 'org-0', name: 'Org 0' });
```

with:

```ts
    expect(first.all.value.rows[0]).toEqual({
      prn: organizationPrn('0190a100-0000-7000-8000-000000000000'),
      orgId: '0190a100-0000-7000-8000-000000000000',
      slug: 'org-0',
      name: 'Org 0',
      lifecycle: { own: 'active', effective: 'active' },
    });
```

Before the final `});` of the file (the end of `describe('loadOrganizationsPage'`), insert:

```ts

  // SMA-630 spec § 6.3, § 9.1: the "All organizations" rows carry their lifecycle for the Status column.
  it('carries the lifecycle of each row, and maps UNSPECIFIED to unknown', async () => {
    iam.setHandlers({
      'tenancy.listOrganizations': () => ({
        organizations: [
          { prn: organizationPrn(IDS.orgA), slug: 'a', name: 'A', status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ARCHIVED },
          { prn: organizationPrn(IDS.orgB), slug: 'b', name: 'B' },
        ],
      }),
    });

    const data = await loadOrganizationsPage({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI({ ListOrganizations: true }) }, { offset: 0 });

    if (data.all?.ok !== true) throw new Error('expected a listed page');
    expect(data.all.value.rows.map((row) => row.lifecycle)).toEqual([
      { own: 'archived', effective: 'archived' },
      { own: 'unknown', effective: 'unknown' },
    ]);
  });
```

- [ ] **Step 2: Write the failing organization-loader test**

In `ts/apps/iam-console/tests/integration/org-page.test.ts`, after `import { disposeTransports } from '@paigasus/sdk/iam';` insert:

```ts
import { NodeStatus } from '@paigasus/sdk/iam/types';
```

Replace the functions `teams` and `world` with:

```ts
type StatusPair = { readonly status?: NodeStatus; readonly effectiveStatus?: NodeStatus };
const ACTIVE: StatusPair = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

function teams(count: number, status: StatusPair = ACTIVE) {
  return Array.from({ length: count }, (_, index) => ({
    prn: teamPrn(IDS.orgA, `0190a1b2-0000-7000-8000-${String(index).padStart(12, '0')}`),
    orgPrn: ORG_A,
    slug: `t-${String(index)}`,
    name: `Team ${String(index)}`,
    ...status,
  }));
}

function world(teamCount: number, organizationStatus: StatusPair = ACTIVE, teamStatus: StatusPair = ACTIVE): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme', ...organizationStatus } }),
    'tenancy.listTeams': () => ({ teams: teams(teamCount, teamStatus) }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({ memberships: [{ id: IDS.membership, principalPrn: IDS.principalPrn, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }] }),
  };
}
```

Replace:

```ts
    expect(data.organization).toEqual({ name: 'Acme', slug: 'acme' });
    expect(data.teams).toEqual({
      ok: true,
      value: { offset: 0, nextOffset: null, rows: teams(2).map((team) => ({ prn: team.prn, teamId: team.prn.slice(-36), slug: team.slug, name: team.name })) },
    });
```

with:

```ts
    expect(data.organization).toEqual({ name: 'Acme', slug: 'acme', lifecycle: { own: 'active', effective: 'active' } });
    expect(data.teams).toEqual({
      ok: true,
      value: {
        offset: 0,
        nextOffset: null,
        rows: teams(2).map((team) => ({ prn: team.prn, teamId: team.prn.slice(-36), slug: team.slug, name: team.name, lifecycle: { own: 'active', effective: 'active' } })),
      },
    });
```

Before the final `});` of the file, insert:

```ts

  // SMA-630 spec § 5.1: three more IsAuthorized questions, all about the organization's OWN PRN.
  it('asks the three lifecycle questions about the organization, and follows the answers', async () => {
    iam.setHandlers(world(1));
    const d = deps({ RenameOrganization: true, ArchiveOrganization: false, RestoreOrganization: true });

    const data = await loadOrganizationPage(d, { org: IDS.orgA, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect({ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }).toEqual({ rename: true, archive: false, restore: true });
    expect(d.mayI.asked).toEqual(
      expect.arrayContaining([
        ['RenameOrganization', ORG_A],
        ['ArchiveOrganization', ORG_A],
        ['RestoreOrganization', ORG_A],
      ]),
    );
  });

  it('maps the organization and team statuses to lifecycles, and UNSPECIFIED to unknown', async () => {
    iam.setHandlers(world(1, { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ARCHIVED }, { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ARCHIVED }));
    const archived = await loadOrganizationPage(deps(), { org: IDS.orgA, offset: 0, membersOffset: 0 });
    if (archived.kind !== 'ok' || !archived.teams.ok) throw new Error('expected an organization with a team list');
    expect(archived.organization.lifecycle).toEqual({ own: 'archived', effective: 'archived' });
    expect(archived.teams.value.rows.map((row) => row.lifecycle)).toEqual([{ own: 'active', effective: 'archived' }]);

    iam.setHandlers(world(1, {}, {}));
    const unknown = await loadOrganizationPage(deps(), { org: IDS.orgA, offset: 0, membersOffset: 0 });
    if (unknown.kind !== 'ok' || !unknown.teams.ok) throw new Error('expected an organization with a team list');
    expect(unknown.organization.lifecycle).toEqual({ own: 'unknown', effective: 'unknown' });
    expect(unknown.teams.value.rows.map((row) => row.lifecycle)).toEqual([{ own: 'unknown', effective: 'unknown' }]);
  });
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/orgs-page.test.ts tests/integration/org-page.test.ts`
Expected: FAIL. The rows have no `lifecycle`, and `canRename`, `canArchive`, `canRestore` are `undefined`.

- [ ] **Step 3: Add the row lifecycle to the list loader**

In `ts/apps/iam-console/app/(console)/orgs/load.ts`, replace:

```ts
import { callIam, ROOT_PRN, parseTenancyPrn, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';

export type OrganizationRow = { readonly prn: string; readonly orgId: string | null; readonly slug: string; readonly name: string };
```

with:

```ts
import { callIam, ROOT_PRN, parseTenancyPrn, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { lifecycleOf, type NodeLifecycle } from '../node-status';

export type OrganizationRow = { readonly prn: string; readonly orgId: string | null; readonly slug: string; readonly name: string; readonly lifecycle: NodeLifecycle };
```

and replace:

```ts
    return { prn: organization.prn, orgId: ref?.kind === 'organization' ? ref.id.toLowerCase() : null, slug: organization.slug, name: organization.name };
```

with:

```ts
    return {
      prn: organization.prn,
      orgId: ref?.kind === 'organization' ? ref.id.toLowerCase() : null,
      slug: organization.slug,
      name: organization.name,
      lifecycle: lifecycleOf(organization),
    };
```

- [ ] **Step 4: Add the Status column to the list page**

In `ts/apps/iam-console/app/(console)/orgs/page.tsx`, replace:

```ts
import { loadOrganizationsPage, type OrganizationList } from './load';
```

with:

```ts
import { statusColumnLabel } from '../node-status';
import { loadOrganizationsPage, type OrganizationList } from './load';
```

In `OrganizationTable`, replace:

```tsx
            <TableHead>Slug</TableHead>
          </TableRow>
```

with:

```tsx
            <TableHead>Slug</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
```

and replace:

```tsx
              <TableCell>{row.slug}</TableCell>
            </TableRow>
```

with:

```tsx
              <TableCell>{row.slug}</TableCell>
              <TableCell>{statusColumnLabel(row.lifecycle)}</TableCell>
            </TableRow>
```

("Your organizations" stays as it is: `myScopes()` returns no status, spec § 6.3, § 10.)

- [ ] **Step 5: Rewrite the organization loader**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/load.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org] (spec § 5.2). URLs use UUIDs because a PRN holds no slug. The
// organization comes first: when IAM denies it, the page is the 403 view and nothing else runs.
// The PRN comes from the URL, so an invalid-input answer (IAM's prn-mismatch, InvalidArgument,
// adapters/grpc/tenancy.rs:176-178) means that the URL names no such node: notFound().
//
// SMA-630 spec § 5.1: three more affordance questions, all about the organization's OWN PRN, and
// the lifecycle of the organization and of each team row.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { PAGE_SIZE, nextOffset } from '../../../../lib/paging';
import { callIam, isUuid, organizationPrn, parseTenancyPrn, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { loadMembers, type MembersData } from '../members';
import { lifecycleOf, type NodeLifecycle } from '../../node-status';

export type TeamRow = { readonly prn: string; readonly teamId: string | null; readonly slug: string; readonly name: string; readonly lifecycle: NodeLifecycle };
export type TeamList = { readonly rows: readonly TeamRow[]; readonly offset: number; readonly nextOffset: number | null };
export type OrganizationPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly orgPrn: string;
      readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly teams: IamResult<TeamList>;
      readonly canCreateTeam: boolean;
      readonly canRename: boolean;
      readonly canArchive: boolean;
      readonly canRestore: boolean;
      readonly members: MembersData;
    };
export type OrganizationPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'listTeams' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadOrganizationPage(deps: OrganizationPageDeps, params: { readonly org: string; readonly offset: number; readonly membersOffset: number }): Promise<OrganizationPageData> {
  if (!isUuid(params.org)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const orgPrn = organizationPrn(orgId);

  const got = await callIam(() => deps.tenancy.getOrganization({ prn: orgPrn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const organization = got.value.organization;
  if (organization === undefined) return { kind: 'not-found' };

  const [teams, canCreateTeam, canRename, canArchive, canRestore, members] = await Promise.all([
    callIam(() => deps.tenancy.listTeams({ orgPrn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateTeam', orgPrn),
    deps.mayI('RenameOrganization', orgPrn),
    deps.mayI('ArchiveOrganization', orgPrn),
    deps.mayI('RestoreOrganization', orgPrn),
    loadMembers(deps, orgPrn, params.membersOffset),
  ]);
  const teamList: IamResult<TeamList> = teams.ok
    ? {
        ok: true,
        value: {
          rows: teams.value.teams.map((team): TeamRow => {
            const ref = parseTenancyPrn(team.prn);
            return { prn: team.prn, teamId: ref?.kind === 'team' ? ref.id.toLowerCase() : null, slug: team.slug, name: team.name, lifecycle: lifecycleOf(team) };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, teams.value.teams.length),
        },
      }
    : teams;

  return {
    kind: 'ok',
    orgId,
    orgPrn,
    organization: { name: organization.name, slug: organization.slug, lifecycle: lifecycleOf(organization) },
    teams: teamList,
    canCreateTeam,
    canRename,
    canArchive,
    canRestore,
    members,
  };
}
```

- [ ] **Step 6: Rewrite the organization page**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org] (spec § 5.2). A navigation is a user action, so this page always asks IAM; the
// only things mayI() hides are the create, membership and manage controls (spec § 6.3, SMA-630
// spec § 5.3).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../../_components/create-form';
import { PageError } from '../../../_components/page-error';
import { Pager } from '../../../_components/pager';
import { SectionError } from '../../../_components/section-error';
import { isUuid } from '@paigasus/console-core';
import { iamClients, mayI } from '../../../../lib/console';
import { parseOffset } from '../../../../lib/paging';
import { ManageSection } from '../../manage-section';
import { statusColumnLabel } from '../../node-status';
import { StatusBadge } from '../../status-badge';
import { MembersSection } from '../members-section';
import { archiveOrganizationAction, createTeamAction, renameOrganizationAction, restoreOrganizationAction } from './actions';
import { loadOrganizationPage, type TeamList } from './load';

type Props = {
  params: Promise<{ org: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

function TeamTable({ orgId, list, membersOffset }: { readonly orgId: string; readonly list: TeamList; readonly membersOffset: number }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No teams yet" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.teamId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`/iam/orgs/${orgId}/teams/${row.teamId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
              <TableCell>{statusColumnLabel(row.lifecycle)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Team pages" path={`/iam/orgs/${orgId}`} param="offset" offset={list.offset} nextOffset={list.nextOffset} keep={{ moffset: membersOffset }} />
    </>
  );
}

export default async function OrganizationPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org)) notFound();
  // Each list's pager keeps the other list's offset in its links (pageHref, lib/paging.ts).
  const offset = parseOffset(query.offset);
  const membersOffset = parseOffset(query.moffset);
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadOrganizationPage({ tenancy: clients.tenancy, mayI: may }, { org, offset, membersOffset });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: data.organization.name }]} />
      <div>
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.organization.name}</h1>
          <StatusBadge lifecycle={data.organization.lifecycle} />
        </div>
        <p className="text-muted-foreground text-sm">{data.organization.slug}</p>
      </div>
      <section aria-labelledby="teams-heading" className="flex flex-col gap-3">
        <h2 id="teams-heading" className="text-lg font-semibold">
          Teams
        </h2>
        {data.teams.ok ? <TeamTable orgId={data.orgId} list={data.teams.value} membersOffset={membersOffset} /> : <SectionError error={data.teams.error} />}
        {data.canCreateTeam ? <CreateForm testId="create-team" title="Create team" submitLabel="Create" action={createTeamAction} hidden={{ orgPrn: data.orgPrn }} /> : null}
      </section>
      <MembersSection nodePrn={data.orgPrn} path={`/iam/orgs/${data.orgId}`} data={data.members} keep={{ offset }} />
      <ManageSection
        node="organization"
        prn={data.orgPrn}
        name={data.organization.name}
        slug={data.organization.slug}
        lifecycle={data.organization.lifecycle}
        can={{ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }}
        actions={{ rename: renameOrganizationAction, archive: archiveOrganizationAction, restore: restoreOrganizationAction }}
      />
    </div>
  );
}
```

- [ ] **Step 7: Run the tests and the type check**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/orgs-page.test.ts tests/integration/org-page.test.ts`
Expected: PASS.

Run: `moon run iam-console-ts:typecheck`
Expected: PASS.

- [ ] **Step 8: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/orgs/load.ts" "apps/iam-console/app/(console)/orgs/page.tsx" "apps/iam-console/app/(console)/orgs/[org]/load.ts" "apps/iam-console/app/(console)/orgs/[org]/page.tsx" apps/iam-console/tests/integration/orgs-page.test.ts apps/iam-console/tests/integration/org-page.test.ts
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/orgs/load.ts" "ts/apps/iam-console/app/(console)/orgs/page.tsx" "ts/apps/iam-console/app/(console)/orgs/[org]/load.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/page.tsx" ts/apps/iam-console/tests/integration/orgs-page.test.ts ts/apps/iam-console/tests/integration/org-page.test.ts
git commit -m "feat(ts): show the organization lifecycle and its manage section (SMA-630)" -m "The organization loader asks the three lifecycle questions and maps the statuses. The page shows a status badge, a Status column for teams and the manage section. The organization list gets a Status column." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: The team page and the project page

Spec § 5.1 (the project loader gets a `Promise.all`), § 5.2, § 6.3, § 9.1.

**Files:**
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts` (whole file)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx` (whole file)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts` (whole file)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx` (whole file)
- Test: `ts/apps/iam-console/tests/integration/team-project-pages.test.ts`

**Interfaces:**
- Consumes: as Task 11, plus the team and project actions (Task 8).
- Produces:
  - `ProjectRow` gains `readonly lifecycle: NodeLifecycle`.
  - `TeamPageData` (`kind: 'ok'`) gains `team.lifecycle`, `canRename`, `canArchive`, `canRestore`.
  - `ProjectPageData` (`kind: 'ok'`) gains `project.lifecycle`, `canRename`, `canArchive`, `canRestore`.

- [ ] **Step 1: Write the failing loader tests**

In `ts/apps/iam-console/tests/integration/team-project-pages.test.ts`, after `import { disposeTransports } from '@paigasus/sdk/iam';` insert:

```ts
import { NodeStatus } from '@paigasus/sdk/iam/types';
```

Replace the function `world` with:

```ts
type StatusPair = { readonly status?: NodeStatus; readonly effectiveStatus?: NodeStatus };
const ACTIVE: StatusPair = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

function world(
  overrides: { teamOrgPrn?: string; projectTeamPrn?: string; projectOrgPrn?: string; teamStatus?: StatusPair; projectStatus?: StatusPair; listedStatus?: StatusPair } = {},
): FakeIamHandlers {
  return {
    'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: overrides.teamOrgPrn ?? ORG_A, slug: 'platform', name: 'Platform', ...(overrides.teamStatus ?? ACTIVE) } }),
    'tenancy.getProject': (req: { prn: string }) => ({
      project: {
        prn: req.prn,
        teamPrn: overrides.projectTeamPrn ?? TEAM_A1,
        orgPrn: overrides.projectOrgPrn ?? ORG_A,
        slug: 'models',
        name: 'Models',
        ...(overrides.projectStatus ?? ACTIVE),
      },
    }),
    'tenancy.listProjects': () => ({ projects: [{ prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'models', name: 'Models', ...(overrides.listedStatus ?? ACTIVE) }] }),
    'tenancy.listMemberships': () => ({ memberships: [] }),
  };
}
```

Replace:

```ts
    expect(data.projects).toEqual({ ok: true, value: { offset: 0, nextOffset: null, rows: [{ prn: PROJECT_A1, projectId: IDS.projectA1, slug: 'models', name: 'Models' }] } });
```

with:

```ts
    expect(data.projects).toEqual({
      ok: true,
      value: { offset: 0, nextOffset: null, rows: [{ prn: PROJECT_A1, projectId: IDS.projectA1, slug: 'models', name: 'Models', lifecycle: { own: 'active', effective: 'active' } }] },
    });
```

In `describe('loadTeamPage'`, after the block `it('turns a denied GetTeam into a page error', …);`, insert:

```ts

  // SMA-630 spec § 5.1: three more IsAuthorized questions, all about the team's OWN PRN.
  it('asks the three lifecycle questions about the team, and follows the answers', async () => {
    iam.setHandlers(world());
    const d = deps({ RenameTeam: true, ArchiveTeam: false, RestoreTeam: true });

    const data = await loadTeamPage(d, { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect({ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }).toEqual({ rename: true, archive: false, restore: true });
    expect(d.mayI.asked).toEqual(
      expect.arrayContaining([
        ['RenameTeam', TEAM_A1],
        ['ArchiveTeam', TEAM_A1],
        ['RestoreTeam', TEAM_A1],
      ]),
    );
  });

  it('maps the team status and each project row status to a lifecycle, and UNSPECIFIED to unknown', async () => {
    iam.setHandlers(world({ teamStatus: { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ARCHIVED }, listedStatus: {} }));

    const data = await loadTeamPage(deps(), { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok' || !data.projects.ok) throw new Error('expected a team with a project list');
    expect(data.team.lifecycle).toEqual({ own: 'active', effective: 'archived' });
    expect(data.projects.value.rows.map((row) => row.lifecycle)).toEqual([{ own: 'unknown', effective: 'unknown' }]);
  });
```

In `describe('loadProjectPage'`, after the block `it('returns the project and its members', …);`, insert:

```ts

  // SMA-630 spec § 5.1: the project loader now runs the member load and the three questions together.
  it('asks the three lifecycle questions about the project, and follows the answers', async () => {
    iam.setHandlers(world());
    const d = deps({ RenameProject: false, ArchiveProject: true, RestoreProject: false });

    const data = await loadProjectPage(d, params);

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect({ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }).toEqual({ rename: false, archive: true, restore: false });
    expect(d.mayI.asked).toEqual(
      expect.arrayContaining([
        ['RenameProject', PROJECT_A1],
        ['ArchiveProject', PROJECT_A1],
        ['RestoreProject', PROJECT_A1],
      ]),
    );
  });

  it('maps the project status to a lifecycle, and UNSPECIFIED to unknown', async () => {
    iam.setHandlers(world({ projectStatus: { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ARCHIVED } }));
    const archived = await loadProjectPage(deps(), params);
    if (archived.kind !== 'ok') throw new Error(`expected ok, got ${archived.kind}`);
    expect(archived.project.lifecycle).toEqual({ own: 'archived', effective: 'archived' });

    iam.setHandlers(world({ projectStatus: {} }));
    const unknown = await loadProjectPage(deps(), params);
    if (unknown.kind !== 'ok') throw new Error(`expected ok, got ${unknown.kind}`);
    expect(unknown.project.lifecycle).toEqual({ own: 'unknown', effective: 'unknown' });
  });
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration/team-project-pages.test.ts`
Expected: FAIL. The project row has no `lifecycle`, and the `can*` flags are `undefined`.

- [ ] **Step 2: Rewrite the team loader**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team] (spec § 5.2). SMA-630 spec § 5.1 adds three affordance
// questions about the team's OWN PRN, and the lifecycle of the team and of each project row.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { PAGE_SIZE, nextOffset } from '../../../../../../lib/paging';
import { callIam, isUuid, parseTenancyPrn, teamPrn, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { loadMembers, type MembersData } from '../../../members';
import { sameNode } from '../../../node-ref';
import { lifecycleOf, type NodeLifecycle } from '../../../../node-status';

export type ProjectRow = { readonly prn: string; readonly projectId: string | null; readonly slug: string; readonly name: string; readonly lifecycle: NodeLifecycle };
export type ProjectList = { readonly rows: readonly ProjectRow[]; readonly offset: number; readonly nextOffset: number | null };
export type TeamPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly teamPrn: string;
      readonly team: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly projects: IamResult<ProjectList>;
      readonly canCreateProject: boolean;
      readonly canRename: boolean;
      readonly canArchive: boolean;
      readonly canRestore: boolean;
      readonly members: MembersData;
    };
export type TeamPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getTeam' | 'listProjects' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadTeamPage(deps: TeamPageDeps, params: { readonly org: string; readonly team: string; readonly offset: number; readonly membersOffset: number }): Promise<TeamPageData> {
  if (!isUuid(params.org) || !isUuid(params.team)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const teamId = params.team.toLowerCase();
  const prn = teamPrn(orgId, teamId);

  const got = await callIam(() => deps.tenancy.getTeam({ prn }));
  // The PRN comes from the URL: IAM answers a wrong [org] with prn-mismatch, which is invalid-input
  // (tenancy.rs:331-333). Spec § 5.2 makes a mismatched URL a 404 (ruling T18.a).
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const team = got.value.team;
  if (team === undefined || !sameNode(team.orgPrn, 'organization', orgId)) return { kind: 'not-found' };

  const [projects, canCreateProject, canRename, canArchive, canRestore, members] = await Promise.all([
    callIam(() => deps.tenancy.listProjects({ teamPrn: prn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateProject', prn),
    deps.mayI('RenameTeam', prn),
    deps.mayI('ArchiveTeam', prn),
    deps.mayI('RestoreTeam', prn),
    loadMembers(deps, prn, params.membersOffset),
  ]);
  const projectList: IamResult<ProjectList> = projects.ok
    ? {
        ok: true,
        value: {
          rows: projects.value.projects.map((project): ProjectRow => {
            const ref = parseTenancyPrn(project.prn);
            return { prn: project.prn, projectId: ref?.kind === 'project' ? ref.id.toLowerCase() : null, slug: project.slug, name: project.name, lifecycle: lifecycleOf(project) };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, projects.value.projects.length),
        },
      }
    : projects;

  return {
    kind: 'ok',
    orgId,
    teamId,
    teamPrn: prn,
    team: { name: team.name, slug: team.slug, lifecycle: lifecycleOf(team) },
    projects: projectList,
    canCreateProject,
    canRename,
    canArchive,
    canRestore,
    members,
  };
}
```

- [ ] **Step 3: Rewrite the project loader**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). A project PRN holds
// the org and the project, not the team, so [team] is checked through GetProject's team_prn.
// SMA-630 spec § 5.1: the member load and three affordance questions about the project's OWN PRN
// run together.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, isUuid, projectPrn, type IamClients, type MayI } from '@paigasus/console-core';
import { loadMembers, type MembersData } from '../../../../../members';
import { sameNode } from '../../../../../node-ref';
import { lifecycleOf, type NodeLifecycle } from '../../../../../../node-status';

export type ProjectPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly projectId: string;
      readonly projectPrn: string;
      readonly project: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly canRename: boolean;
      readonly canArchive: boolean;
      readonly canRestore: boolean;
      readonly members: MembersData;
    };
export type ProjectPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getProject' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadProjectPage(
  deps: ProjectPageDeps,
  params: { readonly org: string; readonly team: string; readonly project: string; readonly membersOffset: number },
): Promise<ProjectPageData> {
  if (!isUuid(params.org) || !isUuid(params.team) || !isUuid(params.project)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const teamId = params.team.toLowerCase();
  const projectId = params.project.toLowerCase();
  const prn = projectPrn(orgId, projectId);

  const got = await callIam(() => deps.tenancy.getProject({ prn }));
  // The PRN comes from the URL: IAM answers a wrong [org] with prn-mismatch, which is invalid-input
  // (tenancy.rs:480-482). Spec § 5.2 makes a mismatched URL a 404 (ruling T18.a).
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const project = got.value.project;
  if (project === undefined || !sameNode(project.teamPrn, 'team', teamId) || !sameNode(project.orgPrn, 'organization', orgId)) return { kind: 'not-found' };

  const [members, canRename, canArchive, canRestore] = await Promise.all([
    loadMembers(deps, prn, params.membersOffset),
    deps.mayI('RenameProject', prn),
    deps.mayI('ArchiveProject', prn),
    deps.mayI('RestoreProject', prn),
  ]);
  return {
    kind: 'ok',
    orgId,
    teamId,
    projectId,
    projectPrn: prn,
    project: { name: project.name, slug: project.slug, lifecycle: lifecycleOf(project) },
    canRename,
    canArchive,
    canRestore,
    members,
  };
}
```

- [ ] **Step 4: Rewrite the team page**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team] (spec § 5.2). The breadcrumb does not name the organization: a user
// with a team role only can have no access to GetOrganization, and this page must not need it.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../../../../_components/create-form';
import { PageError } from '../../../../../_components/page-error';
import { Pager } from '../../../../../_components/pager';
import { SectionError } from '../../../../../_components/section-error';
import { isUuid } from '@paigasus/console-core';
import { iamClients, mayI } from '../../../../../../lib/console';
import { parseOffset } from '../../../../../../lib/paging';
import { ManageSection } from '../../../../manage-section';
import { statusColumnLabel } from '../../../../node-status';
import { StatusBadge } from '../../../../status-badge';
import { MembersSection } from '../../../members-section';
import { archiveTeamAction, createProjectAction, renameTeamAction, restoreTeamAction } from './actions';
import { loadTeamPage, type ProjectList } from './load';

type Props = {
  params: Promise<{ org: string; team: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

function ProjectTable({ base, list, membersOffset }: { readonly base: string; readonly list: ProjectList; readonly membersOffset: number }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No projects yet" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.projectId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`${base}/projects/${row.projectId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
              <TableCell>{statusColumnLabel(row.lifecycle)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Project pages" path={base} param="offset" offset={list.offset} nextOffset={list.nextOffset} keep={{ moffset: membersOffset }} />
    </>
  );
}

export default async function TeamPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, team }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(team)) notFound();
  // Each list's pager keeps the other list's offset in its links (pageHref, lib/paging.ts).
  const offset = parseOffset(query.offset);
  const membersOffset = parseOffset(query.moffset);
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadTeamPage({ tenancy: clients.tenancy, mayI: may }, { org, team, offset, membersOffset });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;
  const base = `/iam/orgs/${data.orgId}/teams/${data.teamId}`;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: 'Organization', href: `/iam/orgs/${data.orgId}` }, { label: data.team.name }]} />
      <div>
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.team.name}</h1>
          <StatusBadge lifecycle={data.team.lifecycle} />
        </div>
        <p className="text-muted-foreground text-sm">{data.team.slug}</p>
      </div>
      <section aria-labelledby="projects-heading" className="flex flex-col gap-3">
        <h2 id="projects-heading" className="text-lg font-semibold">
          Projects
        </h2>
        {data.projects.ok ? <ProjectTable base={base} list={data.projects.value} membersOffset={membersOffset} /> : <SectionError error={data.projects.error} />}
        {data.canCreateProject ? <CreateForm testId="create-project" title="Create project" submitLabel="Create" action={createProjectAction} hidden={{ teamPrn: data.teamPrn }} /> : null}
      </section>
      <MembersSection nodePrn={data.teamPrn} path={base} data={data.members} keep={{ offset }} />
      <ManageSection
        node="team"
        prn={data.teamPrn}
        name={data.team.name}
        slug={data.team.slug}
        lifecycle={data.team.lifecycle}
        can={{ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }}
        actions={{ rename: renameTeamAction, archive: archiveTeamAction, restore: restoreTeamAction }}
      />
    </div>
  );
}
```

- [ ] **Step 5: Rewrite the project page**

Replace the whole content of `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). Mutations: attach and detach, and
// (SMA-630) rename, archive and restore.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { PageError } from '../../../../../../../_components/page-error';
import { isUuid } from '@paigasus/console-core';
import { iamClients, mayI } from '../../../../../../../../lib/console';
import { parseOffset } from '../../../../../../../../lib/paging';
import { ManageSection } from '../../../../../../manage-section';
import { StatusBadge } from '../../../../../../status-badge';
import { MembersSection } from '../../../../../members-section';
import { archiveProjectAction, renameProjectAction, restoreProjectAction } from './actions';
import { loadProjectPage } from './load';

type Props = {
  params: Promise<{ org: string; team: string; project: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function ProjectPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, team, project }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(team) || !isUuid(project)) notFound();
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadProjectPage({ tenancy: clients.tenancy, mayI: may }, { org, team, project, membersOffset: parseOffset(query.moffset) });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;
  const teamPath = `/iam/orgs/${data.orgId}/teams/${data.teamId}`;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs
        items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: 'Organization', href: `/iam/orgs/${data.orgId}` }, { label: 'Team', href: teamPath }, { label: data.project.name }]}
      />
      <div>
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.project.name}</h1>
          <StatusBadge lifecycle={data.project.lifecycle} />
        </div>
        <p className="text-muted-foreground text-sm">{data.project.slug}</p>
      </div>
      <MembersSection nodePrn={data.projectPrn} path={`${teamPath}/projects/${data.projectId}`} data={data.members} />
      <ManageSection
        node="project"
        prn={data.projectPrn}
        name={data.project.name}
        slug={data.project.slug}
        lifecycle={data.project.lifecycle}
        can={{ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }}
        actions={{ rename: renameProjectAction, archive: archiveProjectAction, restore: restoreProjectAction }}
      />
    </div>
  );
}
```

- [ ] **Step 6: Run the tests, the type check and the build**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/integration`
Expected: PASS.

Run: `moon run iam-console-ts:typecheck iam-console-ts:build`
Expected: PASS. The build proves that Turbopack resolves every new extensionless import and that each `actions.ts` is a valid Server Actions module.

- [ ] **Step 7: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts" "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx" "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts" "apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx" apps/iam-console/tests/integration/team-project-pages.test.ts
moon run ts:lint ts:fmt
git add "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx" "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts" "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx" ts/apps/iam-console/tests/integration/team-project-pages.test.ts
git commit -m "feat(ts): show the team and project lifecycle and their manage sections (SMA-630)" -m "The team and project loaders ask the three lifecycle questions and map the statuses. The pages show a status badge and the manage section, and the project table gets a Status column." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 13: The e2e world, R14–R16, and the row registry

Spec § 5.1 (second parity control), § 8 rows `ALL_ACTIONS`, world nodes, world handlers, `ROWS`, § 9.3.

**Files:**
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts` (whole file)
- Create: `ts/apps/iam-console/tests/unit/world-actions.test.ts`
- Create: `ts/apps/iam-console/tests/e2e/lifecycle.spec.ts`
- Modify: `ts/apps/iam-console/tests/unit/e2e-rows.test.ts:3-5`, `:12`, `:14`

**Interfaces:**
- Consumes: `IAM_ACTIONS`, `type IamAction` (Task 2); `NodeStatus` (Task 1); `BADGE_LABEL`, `PARENT_ARCHIVED_NOTE` (Task 4); `FORM_REASON_COPY` (`error-copy.ts`); `signIn`, `waitForHydration` (`support/login.ts`); `test`, `expect`, `harness.useWorld`, `harness.iam.callsTo` (`support/harness.ts`); `denial` (`@paigasus/console-core/testing`).
- Produces: `ALL_ACTIONS` (16 names, `satisfies readonly IamAction[]`); every scripted world node carries `status` and `effectiveStatus` = `NodeStatus.ACTIVE`; default handlers for the nine lifecycle RPCs, each returning the scripted node; the Playwright tests `R14`, `R15`, `R16`.

- [ ] **Step 1: Write the failing parity test for the world**

Create `ts/apps/iam-console/tests/unit/world-actions.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The e2e world's ALL_ACTIONS against @paigasus/console-core's IAM_ACTIONS (SMA-630 spec § 5.1). The
// world allows ALL_ACTIONS by default. A name missing here would make the e2e tier deny it in
// silence, and a control that the default world should show would stay hidden.
import { describe, expect, it } from 'vitest';
import { IAM_ACTIONS } from '@paigasus/console-core';
import { ALL_ACTIONS } from '../e2e/support/world';

describe('ALL_ACTIONS', () => {
  it('holds the same set as IAM_ACTIONS', () => {
    expect([...ALL_ACTIONS].sort()).toEqual([...IAM_ACTIONS].sort());
    expect(new Set(ALL_ACTIONS).size).toBe(ALL_ACTIONS.length);
  });
});
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/world-actions.test.ts`
Expected: FAIL. `ALL_ACTIONS` has 7 names and `IAM_ACTIONS` has 16.

- [ ] **Step 2: Rewrite the world**

Replace the whole content of `ts/apps/iam-console/tests/e2e/support/world.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The IAM that the e2e tier talks to: one organization with a team and a project, a grant on a
// project in a second organization, and an audit entry. `worldHandlers()` always returns the FULL
// handler set, because the fake's setHandlers() REPLACES the whole map (Task 11). Every key an
// override uses must therefore exist in the default set below.
//
// One piece of state: an organization that CreateOrganization makes is in every later
// ListOrganizations answer of the same world. Each worldHandlers() call starts with none, so a
// test cannot see the organizations of an earlier test. R6 uses it to prove that the action
// refreshes the page (P5b-16).
//
// PRNs are literal strings: @paigasus/console-core's prn-tenancy.ts imports server-only, which
// throws under Playwright. NodeStatus comes from @paigasus/sdk's guard-free ./iam/types entry, and
// IamAction is a TYPE import, which the compiler erases (SMA-630).
import { Code } from '@connectrpc/connect';
import type { IamAction } from '@paigasus/console-core';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { NodeStatus } from '@paigasus/sdk/iam/types';

export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e2';
export const PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e3';
export const OTHER_ORG_ID = '0190a100-0000-7000-8000-0000000000e4';
export const OTHER_TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e5';
export const OTHER_PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e6';
export const NEW_ORG_ID = '0190a100-0000-7000-8000-0000000000e7';

export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
export const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;
export const OTHER_TEAM_PRN = `prn:pgs:iam::${OTHER_ORG_ID}:team/${OTHER_TEAM_ID}`;
export const OTHER_PROJECT_PRN = `prn:pgs:iam::${OTHER_ORG_ID}:project/${OTHER_PROJECT_ID}`;

export const ORG_NAME = 'Acme Research';
export const TEAM_NAME = 'Platform Team';
export const PROJECT_NAME = 'Inference Gateway';
export const OTHER_PROJECT_NAME = 'Shared Models';
export const AUDIT_ACTION = 'CreateTeam';

/**
 * The Cedar action names the app asks IsAuthorized about. `satisfies` holds each entry to
 * @paigasus/console-core's IamAction, and tests/unit/world-actions.test.ts holds the SET to
 * IAM_ACTIONS (SMA-630 spec § 5.1).
 */
export const ALL_ACTIONS = [
  'ListOrganizations',
  'CreateOrganization',
  'RenameOrganization',
  'ArchiveOrganization',
  'RestoreOrganization',
  'CreateTeam',
  'RenameTeam',
  'ArchiveTeam',
  'RestoreTeam',
  'CreateProject',
  'RenameProject',
  'ArchiveProject',
  'RestoreProject',
  'AttachMembership',
  'DetachMembership',
  'ListAuditLog',
] as const satisfies readonly IamAction[];

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.audit'] };

export type WorldOptions = {
  /** The actions IsAuthorized allows. Default: all of them. */
  readonly allow?: readonly string[];
  /** false: a first-time identity with no membership and no grant. Default: true. */
  readonly memberships?: boolean;
  /** What GET /v1/service-info answers. Default: DEFAULT_DESCRIPTOR. */
  readonly descriptor?: Descriptor;
  /** Replace single handlers, for example with one that throws denial(). */
  readonly overrides?: FakeIamHandlers;
};

const notFound = (): Error => denial({ code: Code.NotFound, reason: 'not-found' });

/**
 * Every scripted node is active. Without a status a node reads as UNSPECIFIED, and every row and
 * header would show "Status unknown" (SMA-630 spec § 8).
 */
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

// Named constants, not `Map.get()` results, where a response lists them: the handlers are typed per
// method (Task 11), and a repeated field must not hold `undefined`.
const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME, ...ACTIVE };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME, ...ACTIVE };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME, ...ACTIVE };

const ORGANIZATIONS = new Map([[ORG_PRN, ORGANIZATION]]);
const TEAMS = new Map([
  [TEAM_PRN, TEAM],
  [OTHER_TEAM_PRN, { prn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'shared', name: 'Shared Team', ...ACTIVE }],
]);
const PROJECTS = new Map([
  [PROJECT_PRN, PROJECT],
  [OTHER_PROJECT_PRN, { prn: OTHER_PROJECT_PRN, teamPrn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'models', name: OTHER_PROJECT_NAME, ...ACTIVE }],
]);

function organizationAt(prn: string): typeof ORGANIZATION {
  const organization = ORGANIZATIONS.get(prn);
  if (organization === undefined) throw notFound();
  return organization;
}

function teamAt(prn: string): typeof TEAM {
  const team = TEAMS.get(prn);
  if (team === undefined) throw notFound();
  return team;
}

function projectAt(prn: string): typeof PROJECT {
  const project = PROJECTS.get(prn);
  if (project === undefined) throw notFound();
  return project;
}

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const allow = new Set<string>(options.allow ?? ALL_ACTIONS);
  const withScopes = options.memberships ?? true;
  const created: { prn: string; slug: string; name: string; status: NodeStatus; effectiveStatus: NodeStatus }[] = [];
  return {
    'authn.introspect': () => ({
      principalPrn: PRINCIPAL_PRN,
      status: 'active',
      issuer: 'fake-idp',
      subject: 'e2e-user',
      memberships: withScopes
        ? [
            { id: '0190a1d4-0000-7000-8000-0000000000f1', principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN },
            { id: '0190a1d4-0000-7000-8000-0000000000f2', principalPrn: PRINCIPAL_PRN, nodePrn: TEAM_PRN },
          ]
        : [],
    }),
    'authz.isAuthorized': (req: { action: string }) => ({ allowed: allow.has(req.action), determiningPolicies: [], reason: '' }),
    'authz.listRoleGrants': () => ({
      grants: withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: OTHER_PROJECT_PRN }] : [],
    }),
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.getTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.getProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.listOrganizations': () => ({ organizations: [...ORGANIZATIONS.values(), ...created] }),
    'tenancy.listTeams': () => ({ teams: [TEAM] }),
    'tenancy.listProjects': () => ({ projects: [PROJECT] }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'tenancy.createOrganization': (req: { slug: string; name: string }) => {
      const organization = { prn: `prn:pgs:iam:::organization/${NEW_ORG_ID}`, slug: req.slug, name: req.name, ...ACTIVE };
      created.push(organization);
      return { organization };
    },
    'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_PRN, orgPrn: req.orgPrn, slug: req.slug, name: req.name, ...ACTIVE } }),
    'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({
      project: { prn: PROJECT_PRN, teamPrn: req.teamPrn, orgPrn: ORG_PRN, slug: req.slug, name: req.name, ...ACTIVE },
    }),
    // SMA-630: the nine lifecycle RPCs. Each returns the scripted node unchanged. A test that needs
    // the node to change (R14) scripts stateful handlers through `overrides`.
    'tenancy.renameOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.archiveOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.restoreOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.renameTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.archiveTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.restoreTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.renameProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.archiveProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.restoreProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.attachMembership': (req: { principalPrn: string; nodePrn: string }) => ({
      membership: { id: '0190a1d4-0000-7000-8000-0000000000f5', principalPrn: req.principalPrn, nodePrn: req.nodePrn },
    }),
    'tenancy.detachMembership': () => ({}),
    'audit.listAuditEntries': () => ({
      entries: [
        {
          id: 'audit-e2e-1',
          occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
          actorPrn: PRINCIPAL_PRN,
          action: AUDIT_ACTION,
          resourcePrn: ORG_PRN,
          outcome: 'allow',
          determiningPolicies: [],
          detailJson: '{}',
          correlationId: 'corr-audit-e2e',
        },
      ],
      nextCursor: '',
    }),
    ...options.overrides,
  };
}
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/world-actions.test.ts`
Expected: PASS.

- [ ] **Step 3: Prove that `satisfies` bites, then remove the mutation**

In `world.ts`, temporarily change `'RenameTeam',` in `ALL_ACTIONS` to `'RenameTeams',`. Run `moon run iam-console-ts:typecheck --force`. Expected: FAIL with a TS error at `satisfies readonly IamAction[]` (`"RenameTeams"` is not assignable). Change it back with the Edit tool and run again. Expected: PASS.

- [ ] **Step 4: Raise the row registry to 16 (the failing step for the e2e rows)**

In `ts/apps/iam-console/tests/unit/e2e-rows.test.ts`, replace:

```ts
// Spec § 9.4 has thirteen rows. Each must have exactly one Playwright test whose title starts with
// its row id. A deleted or renamed scenario then fails this vitest suite, which runs in
// iam-console-ts:test on every PR that touches the app, even when the e2e task does not run.
```

with:

```ts
// The SMA-511 spec (§ 9.4) has thirteen rows, R1–R13. The SMA-630 spec
// (docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md, § 9.3) adds three, R14–R16.
// Each row must have exactly one Playwright test whose title starts with its row id. A deleted or
// renamed scenario then fails this vitest suite, which runs in iam-console-ts:test on every PR that
// touches the app, even when the e2e task does not run.
```

Replace:

```ts
const ROWS = Array.from({ length: 13 }, (_, index) => `R${String(index + 1)}`);
```

with:

```ts
const ROWS = Array.from({ length: 16 }, (_, index) => `R${String(index + 1)}`);
```

Replace:

```ts
describe('the e2e tier covers every row of spec § 9.4', () => {
```

with:

```ts
describe('the e2e tier covers every row of SMA-511 spec § 9.4 and SMA-630 spec § 9.3', () => {
```

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/e2e-rows.test.ts`
Expected: FAIL on `R14 has exactly one test`, `R15 …` and `R16 …` (0 tests each).

- [ ] **Step 5: Write R14–R16**

Create `ts/apps/iam-console/tests/e2e/lifecycle.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Rename, archive and restore (SMA-630 spec § 9.3, rows R14–R16).
//
// R14 scripts STATEFUL handlers: the state lives in the test's closure, and each call changes both
// `status` and `effectiveStatus`, as a real archive of the node itself does. Every button locator
// uses `exact: true`, because "Confirm archive" contains "archive".
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { denial } from '@paigasus/console-core/testing';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import { BADGE_LABEL, PARENT_ARCHIVED_NOTE } from '../../app/(console)/node-status';
import { signIn, waitForHydration } from './support/login';
import { ALL_ACTIONS, ORG_ID, ORG_PRN, PROJECT_ID, PROJECT_NAME, PROJECT_PRN, TEAM_ID, TEAM_NAME, TEAM_PRN } from './support/world';
import { expect, test } from './support/harness';

const TEAM_PATH = `/iam/orgs/${ORG_ID}/teams/${TEAM_ID}`;
const PROJECT_PATH = `${TEAM_PATH}/projects/${PROJECT_ID}`;

test('R14: archive and restore a team, with a two-step archive (SMA-630)', async ({ page, harness }) => {
  await signIn(page, harness);
  let status: NodeStatus = NodeStatus.ACTIVE;
  const team = () => ({ prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME, status, effectiveStatus: status });
  harness.useWorld({
    overrides: {
      'tenancy.getTeam': () => ({ team: team() }),
      'tenancy.archiveTeam': () => {
        status = NodeStatus.ARCHIVED;
        return { team: team() };
      },
      'tenancy.restoreTeam': () => {
        status = NodeStatus.ACTIVE;
        return { team: team() };
      },
    },
  });
  await page.goto(harness.url(TEAM_PATH));
  await waitForHydration(page);
  const archivesBefore = harness.iam.callsTo('tenancy.archiveTeam').length;
  const restoresBefore = harness.iam.callsTo('tenancy.restoreTeam').length;
  const badge = page.getByTestId('node-status');
  const archive = page.getByRole('button', { name: 'Archive', exact: true });
  const confirm = page.getByRole('button', { name: 'Confirm archive', exact: true });
  const restore = page.getByRole('button', { name: 'Restore', exact: true });
  await expect(badge).toHaveCount(0);

  // Step 1: the two-step archive.
  await archive.click();
  await confirm.click();
  await expect(badge).toHaveText(BADGE_LABEL.archived ?? '');
  await expect(restore).toBeVisible();
  await expect(archive).toHaveCount(0);
  await expect(confirm).toHaveCount(0);

  // Step 2: restore. The confirmation state of the first control is not kept.
  await restore.click();
  await expect(badge).toHaveCount(0);
  await expect(archive).toBeVisible();
  await expect(confirm).toHaveCount(0);

  // Step 3: exactly one call of each, with the team PRN.
  const prnOf = (call: { request: unknown }): string => (call.request as { prn: string }).prn;
  expect(harness.iam.callsTo('tenancy.archiveTeam').slice(archivesBefore).map(prnOf)).toEqual([TEAM_PRN]);
  expect(harness.iam.callsTo('tenancy.restoreTeam').slice(restoresBefore).map(prnOf)).toEqual([TEAM_PRN]);
});

test('R15: a denied rename shows an inline 403 and keeps the typed slug (SMA-630)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      'tenancy.renameProject': () => {
        throw denial({ correlationId: 'corr-e2e-rename-403' });
      },
    },
  });
  await page.goto(harness.url(PROJECT_PATH));
  await waitForHydration(page);
  const before = harness.iam.callsTo('tenancy.renameProject').length;
  const copy = FORM_REASON_COPY[ErrorReason.FORBIDDEN];
  if (copy === undefined) throw new Error('FORM_REASON_COPY has no FORBIDDEN entry');

  const form = page.getByTestId('rename-project');
  await form.getByLabel('Slug').fill('renamed-gateway');
  await form.getByRole('button', { name: 'Rename', exact: true }).click();

  const error = form.getByTestId('rename-project-error');
  await expect(error).toContainText(copy);
  // A form 403 shows the id in BOTH correlation modes (FormError).
  await expect(error.getByTestId('correlation-id')).toHaveText('corr-e2e-rename-403');
  await expect(form.getByLabel('Slug')).toHaveValue('renamed-gateway');
  await expect(form.getByLabel('Name')).toHaveValue(PROJECT_NAME);
  expect(new URL(page.url()).pathname).toBe(PROJECT_PATH);
  const renamed = harness.iam.callsTo('tenancy.renameProject').slice(before);
  expect(renamed).toHaveLength(1);
  expect(renamed[0]?.request).toMatchObject({ prn: PROJECT_PRN, newSlug: 'renamed-gateway' });
  expect((renamed[0]?.request as { newName?: string } | undefined)?.newName).toBeUndefined();
});

test('R16: a project under an archived parent shows the parent badge and the note, and no control (SMA-630)', async ({ page, harness }) => {
  await signIn(page, harness);
  // The starter policy `forbid-archived-writes` denies rename and archive on an effectively
  // archived node, and allows restore (spec F7, F9).
  harness.useWorld({
    allow: ALL_ACTIONS.filter((action) => action !== 'RenameProject' && action !== 'ArchiveProject'),
    overrides: {
      'tenancy.getProject': () => ({
        project: { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME, status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ARCHIVED },
      }),
    },
  });

  await page.goto(harness.url(PROJECT_PATH));
  await waitForHydration(page);

  await expect(page.getByTestId('node-status')).toHaveText(BADGE_LABEL['archived-parent'] ?? '');
  await expect(page.getByTestId('manage-note')).toHaveText(PARENT_ARCHIVED_NOTE);
  await expect(page.getByTestId('rename-project')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Archive', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Restore', exact: true })).toHaveCount(0);
});
```

- [ ] **Step 6: Run the unit tier**

Run: `pnpm -C ts/apps/iam-console exec vitest run tests/unit/e2e-rows.test.ts tests/unit/world-actions.test.ts`
Expected: PASS (16 row cases).

- [ ] **Step 7: Run the e2e tier**

Run: `moon run iam-console-ts:test-e2e`
Expected: PASS for R1–R16 and the harness test. The task has `cache: false` and depends on `iam-console-ts:build`.

If R14 fails at step 1 with the badge still absent, read the Server Action response: the action must return 200, and the world must return the archived team on the next `getTeam`. Do not add a `page.reload()`: R14 proves that the action refreshes the page.

- [ ] **Step 8: Prove that R16 bites, then remove the mutation**

In `manage-section.tsx`, temporarily change `case 'archived-parent':` so that it falls to the `archived` branch (move the line `case 'archived-parent':` directly above `case 'archived':`). Run `moon run iam-console-ts:build` and then `pnpm -C ts/apps/iam-console exec playwright test tests/e2e/lifecycle.spec.ts -g R16`. Expected: FAIL (a Restore button shows). Move the line back with the Edit tool, then run `moon run iam-console-ts:build` and the same Playwright command. Expected: PASS.

- [ ] **Step 9: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
pnpm -C ts exec prettier --write apps/iam-console/tests/e2e/support/world.ts apps/iam-console/tests/unit/world-actions.test.ts apps/iam-console/tests/e2e/lifecycle.spec.ts apps/iam-console/tests/unit/e2e-rows.test.ts
moon run ts:lint ts:fmt
git add ts/apps/iam-console/tests/e2e/support/world.ts ts/apps/iam-console/tests/unit/world-actions.test.ts ts/apps/iam-console/tests/e2e/lifecycle.spec.ts ts/apps/iam-console/tests/unit/e2e-rows.test.ts
git commit -m "test(ts): add the lifecycle e2e rows R14 to R16 (SMA-630)" -m "The e2e world now has node statuses, all sixteen action names and the nine lifecycle handlers. R14 archives and restores a team, R15 shows an inline 403 on rename, and R16 shows a project under an archived parent." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 14: Final verification (controller)

This task changes no code unless a check fails. Fix a failure in the task that owns the file, as a new commit (add, do not amend).

- [ ] **Step 1: Branch and tree**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-630-tenancy-lifecycle branch --show-current` and `git status --short`.
Expected: `feature/sma-630-tenancy-lifecycle`, and a clean tree.

- [ ] **Step 2: The per-project tasks**

Run: `moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck paigasus-console-core-ts:test paigasus-console-core-ts:typecheck iam-console-ts:typecheck iam-console-ts:test gateway-console-ts:typecheck ts:lint ts:fmt`
Expected: PASS. `iam-console-ts:test` also builds the app and runs the Tailwind source guard.

- [ ] **Step 3: The e2e tiers**

Run: `moon run iam-console-ts:test-e2e`
Expected: PASS (R1–R16).

Run: `moon run gateway-console-ts:test-e2e`
Expected: PASS. This tier NEEDS DOCKER and fails loudly without it. An `iam-console` edit selects it through its `inputs` (CLAUDE.md), so CI runs it on this PR. Its own world (`ts/apps/gateway-console/tests/e2e/support/world.ts`) visits only `/iam/orgs` and scripts no `ListOrganizations`, so no assertion there reads the new Status column. If a two-zone test reds, read its assertion before you change anything in this branch.

- [ ] **Step 4: The full CI graph**

Run the marker-delimited command from `CLAUDE.md` (`moon ci :build :test … --base origin/main --include-relations`).
Expected: PASS, with these local-only exceptions from CLAUDE.md: `repo:affected-smoke` needs system bash 3.2 (re-run it alone as `PATH="/bin:$PATH" moon run repo:affected-smoke --force`); `repo:ruff-ci` and `repo:next-public-free` need bash 4+ (re-run them as `/opt/homebrew/bin/bash ci/ruff/run.sh` and `/opt/homebrew/bin/bash ci/next-public/run.sh`); `repo:actionlint` has no working local bash, so its verdict comes from CI only. `repo:affected-smoke` must stay green: this branch adds a Rust file to a TS task's inputs, and no affected-graph case anchors on `paigasus-iam-core/src/authz/action.rs`.

- [ ] **Step 5: Spec cross-check**

Walk the "Spec coverage" table below. For each row, open the named test and confirm it exists and passed in Step 2 or Step 3.

---

## Spec coverage

| Spec item | Task |
| --------- | ---- |
| § 1 nine RPCs; D5 per-folder actions | 5, 6, 7, 8 |
| D1 Manage section on detail pages only | 10, 11, 12 |
| D2 inline 403, no `forbidden()` in actions | 8 (ban), 9 (`FormError` area), 13 (R15) |
| D3 two-step archive, local state | 9 |
| D4 badge and Status column | 4, 10, 11, 12 |
| D6 changed fields only (§ 4.3) | 3, 5, 6, 7 |
| D7 one transition per own status (§ 5.3) | 10, 13 (R14, R16) |
| D8 `@paigasus/sdk/iam/types` (§ 4.5) | 1 |
| § 4.2 bounds 200 / 256 code points / 512, all copies replaced | 3 |
| § 4.4 shell steps, `'layout'`, refresh on forbidden/conflict | 8 |
| § 5.1 `mayI` queries, project `Promise.all`, `IAM_ACTIONS` parity, Moon input | 2, 11, 12, 13 |
| § 5.2 `lifecycleOf`, `lifecycleView`, skew → unknown | 4 |
| § 5.3 table, note-only rule, props, test ids | 10 |
| § 6.1 controlled rename form, `key`, "Renamed." | 9, 10 |
| § 6.2 `ArchiveButton`/`RestoreButton`, exact text, `key={view}` | 9, 10 |
| § 6.3 badge and column tables; "Your organizations" unchanged | 4, 10, 11 |
| § 7 `NOTHING_TO_RENAME` copy and comment | 5 |
| § 8 `IamAction` → `IAM_ACTIONS` | 2 |
| § 8 parity test + `action.rs` input + `moon task` check | 2 |
| § 8 `ALL_ACTIONS` `satisfies` + set test | 13 |
| § 8 world node statuses | 13 |
| § 8 world default handlers (nine) | 13 |
| § 8 `EXPECTED` + project `actions.ts` | 8 |
| § 8 banned `redirect`/`forbidden`/`notFound` with controls | 8 |
| § 8 `actions-revalidate` (success, forbidden, conflict, not invalid-input) | 8 |
| § 8 `ROWS` 13 → 16, comment, title | 13 |
| § 8 sdk barrel test | 1 |
| § 8 511 § 6.3 pointer | 10 |
| § 9.1 tier-2 commands (request, success, 403, rename field cases, trim, reasons) | 5, 6, 7 |
| § 9.1 schema cases (256 / 257 / 256 astral) | 3 |
| § 9.1 loader cases (`can*`, own PRN, lifecycle, UNSPECIFIED, rows) | 11, 12 |
| § 9.2 unit tests (node-status, manage-section, lifecycle-button, rename-form, parity) | 2, 4, 9, 10, 13 |
| § 9.3 R14, R15, R16 | 13 |
| § 11 risks (no code) | — |
