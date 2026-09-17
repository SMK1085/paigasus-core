# SMA-630 — rename, archive and restore in iam-console

- **Issue:** [SMA-630](https://linear.app/smaschek/issue/SMA-630)
- **Parent spec:** `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` (SMA-511).
  This document uses "511 § n" for a section of that spec.
- **Status:** draft 2, 2026-09-17 (after the adversarial challenge, see § 12)

## 1. Goal

SMA-511 delivers five of the fourteen `TenancyService` mutations (511 D4). This issue adds the other
nine to `ts/apps/iam-console`:

| Node         | Rename               | Archive               | Restore               |
| ------------ | -------------------- | --------------------- | --------------------- |
| Organization | `RenameOrganization` | `ArchiveOrganization` | `RestoreOrganization` |
| Team         | `RenameTeam`         | `ArchiveTeam`         | `RestoreTeam`         |
| Project      | `RenameProject`      | `ArchiveProject`      | `RestoreProject`      |

The screens also show the archived state of each node, because an archive that the user cannot see
is not usable.

## 2. Decisions

| Id  | Decision |
| --- | -------- |
| D1  | The controls go on the **detail page** of each node, in a "Manage" section. The list pages get no row actions. |
| D2  | A denied mutation shows an **inline 403** in the form, with the correlation id (511 § 6.1, row R7). No action calls `forbidden()`. |
| D3  | Archive asks for confirmation in **two steps**, inline, with local client state. Restore and rename do not ask. |
| D4  | The detail header shows an archived badge. The org, team and project tables get a **Status** column. |
| D5  | Each detail route folder holds its own three actions. There is no generic action and no hidden `kind` field. |
| D6  | Rename sends **only the fields that the user changed**. The form holds the current values as hidden fields and compares with them (§ 4.3). |
| D7  | The lifecycle control shows **the one transition that the node's own status allows**: Archive for an active node, Restore for an archived node (§ 5.3). |
| D8  | `@paigasus/sdk` re-exports `NodeStatus` from a new guard-free entry, `@paigasus/sdk/iam/types` (§ 4.5). This is the only change outside the app and `@paigasus/console-core`. |

## 3. Facts this design depends on

All are read from the code on `main` at `c5388d32`, and the challenger checked them again.

- **F1.** The proto already declares all nine RPCs (`contracts/proto/paigasus/iam/v1/iam.proto`).
  `Rename*Request` is `{ prn, optional new_slug, optional new_name }`. `Archive*Request` and
  `Restore*Request` are `{ prn }`. Each response returns the node. No request has a version or an
  etag field, so the last write wins.
- **F2.** `@paigasus/console-core`'s `IamClients['tenancy']` is the generated client. It can already
  call all nine RPCs. But no entry that the app may import exports `NodeStatus`: the
  `paigasus/boundaries/apps` ESLint rule bans `@paigasus/proto` in `apps/**` (type imports
  included), with one exemption for `apps/*/tests/support/**`
  (`ts/packages/paigasus-next-config/src/eslint.mjs:236-245`). `@paigasus/sdk` re-exports
  `ErrorReason` for the same reason (`ts/packages/paigasus-sdk/src/errors/types.ts:15-19`), but
  not `NodeStatus`. D8 closes this gap.
- **F3.** Each node message has `status` and `effective_status`, of type `NodeStatus`
  (`UNSPECIFIED = 0`, `ACTIVE = 1`, `ARCHIVED = 2`). `effective_status` is `ARCHIVED` when the node
  or one of its ancestors is archived. The List and Get RPCs return archived nodes. There is no
  filter field.
- **F4.** A rename with no field set fails with `nothing-to-rename`. A rename whose values are equal
  to the current values succeeds and changes nothing. This is true for all three node kinds
  (`pg_organizations.rs:237-252`, `pg_teams.rs:198-214`, `pg_projects.rs:231-247`).
- **F5.** The persistence adapter refuses a rename of an effectively archived node with
  `node-archived` (`pg_teams.rs:198-201` and the equivalents).
- **F6.** At the **repository** level, `set_status` is idempotent and has no ancestor guard. An
  archive of an archived node, and a restore of an active node, change nothing
  (`pg_organizations.rs:287-292`, `pg_teams.rs:249-259`, `pg_projects.rs:282-292`). The RPC can
  still refuse such a call before the repository runs (F7).
- **F7.** Each of the nine gRPC handlers authorizes against the **stored** node's PRN before it
  calls the repository (`tenancy.rs:214-221`). `authz.enforce_tenancy` defaults to true
  (`config.rs:847`). The Cedar starter policy `forbid-archived-writes`
  (`paigasus-iam-core/src/authz/roles.rs:276-315`) forbids every write action except `Restore*` on
  a resource whose `effective_status` is `archived`. So, in the default configuration:
  - `Rename*` and `Archive*` on an effectively archived node are denied with `PermissionDenied`.
  - `Restore*` is not denied.
  - An operator **cannot** remove or change a starter policy (`pg_policies.rs:284-286`, `:378-380`).

  `node-archived` (F5) reaches the console in two cases only: `enforce_tenancy = false`, or a stale
  authorization decision (§ 11).
- **F8.** A restore of a node that is itself `ACTIVE` under an archived ancestor succeeds and does
  not change `effective_status` (F6, "D10: no ancestor guard" in `pg_teams.rs`). Only a restore of
  the archived ancestor helps. `parent-archived` applies to create only.
- **F9.** `IsAuthorized` loads `effective_status` into the entity slice (`pg_entity_slice.rs:7-13`,
  `:177-179`). So `mayI('Rename<Node>', prn)` and `mayI('Archive<Node>', prn)` are false on an
  effectively archived node, in steady state.
- **F10.** `forbid-archived-writes` also denies `InvokeModel` (`roles.rs:648-654`), which the AI
  Gateway checks on every request (`paigasus-gateway/src/adapters/http/auth.rs:18,46`). An archive
  of an organization therefore stops all AI Gateway traffic under it. Key issuance, key revocation
  and membership changes under it are blocked too.
- **F11.** The IAM name limit is 256 Unicode scalar values (`NAME_MAX_CHARS`, `tenancy.rs:11`). The
  slug limit is 64 bytes (`SLUG_MAX_LEN`). The rename path in IAM does **not** call
  `validate_name` (`application/teams.rs:178-187` and the equivalents), so IAM stores any renamed
  name, the empty string included. For a rename, the console's schema is the only name guard today.
- **F12.** `ErrorReason` to presentation is already total in `@paigasus/sdk`. The console's
  `FORM_REASON_COPY` (`app/_components/error-copy.ts`) has copy for `NODE_ARCHIVED`,
  `PARENT_ARCHIVED`, `SLUG_CONFLICT`, `INVALID_SLUG`, `FORBIDDEN` and `NOT_FOUND`, but not for
  `NOTHING_TO_RENAME`.

## 4. Commands and Server Actions

This follows 511 § 5.3.

### 4.1 Files

| Route folder (under `app/(console)/`) | `actions.ts` exports (new ones in bold) |
| ------------------------------------- | --------------------------------------- |
| `orgs/[org]/` | `createTeamAction`, **`renameOrganizationAction`**, **`archiveOrganizationAction`**, **`restoreOrganizationAction`** |
| `orgs/[org]/teams/[team]/` | `createProjectAction`, **`renameTeamAction`**, **`archiveTeamAction`**, **`restoreTeamAction`** |
| `orgs/[org]/teams/[team]/projects/[project]/` (new files) | **`renameProjectAction`**, **`archiveProjectAction`**, **`restoreProjectAction`** |

Each folder's `commands.ts` gets the three matching commands and zod schemas. The project folder
gets a new `commands.ts`.

### 4.2 Schemas

The schemas check the shape. IAM owns the slug grammar and the PRN grammar.

- `rename<Node>Form = z.object({ prn, slug, name, currentSlug, currentName })`
- `archive<Node>Form = restore<Node>Form = z.object({ prn })`

The bounds move to `lib/form.ts`, and **every** existing copy is replaced with an import. Today
`text` has three copies and the PRN bound has three (`orgs/commands.ts:11,22`,
`orgs/[org]/commands.ts:9,12`, `orgs/[org]/teams/[team]/commands.ts:9,12`). `lib/form.ts` already
imports `server-only`, and only server modules import it.

- `slug`: `z.string().trim().min(1).max(200)`, as today. IAM refuses more than 64 bytes with
  `invalid-slug`.
- `name`: trimmed, at least one character, **at most 256 code points** (`[...value].length`), to
  match `NAME_MAX_CHARS` (F11). The old bound was 200 UTF-16 units. With that bound, a node that has
  a longer name (created through the API) fills the rename form with a value that zod refuses.
  Create keeps using the same `name` schema, so the two forms agree.
- `prn`: `z.string().trim().min(1).max(512)`, as today.

**Why a changed hidden `prn` is safe.** A user can change a hidden field. IAM authorizes the action
against the **stored** node that the PRN names (F7), so the user can only act on a node that IAM
allows. A PRN of the wrong kind fails as `invalid-prn` in `convert::node_uuid` before anything runs
(`convert.rs:164-170`). A PRN whose org slot is forged passes authorization for the real node, the
write commits, and then `prn-mismatch` is returned (`tenancy.rs:222-230`). The console then shows an
error for a write that happened. That is an IAM defect (§ 10, follow-up 2). The console does not work
around it.

### 4.3 Commands

Each command takes a narrow port and returns an `ActionResult`. Rename sends only the changed
fields (D6):

```ts
export async function renameTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameTeam'> }, input: RenameTeamInput): Promise<ActionResult> {
  const change = renameChange(input); // { newSlug?: string; newName?: string }
  return toActionResult(await callIam(() => deps.tenancy.renameTeam({ prn: input.prn, ...change })));
}
```

`renameChange` lives in `lib/form.ts`. It compares the trimmed `slug` with `currentSlug`, and the
trimmed `name` with `currentName`. It sets a field only when the two values differ. When nothing
changed, the command still calls IAM with neither field, and IAM answers `nothing-to-rename`. The
command does not decide that refusal itself (511 § 6.3).

This prevents one lost update: user A changes only the slug while user B changes only the name. A
full-form rename would write A's old name over B's new name. Two concurrent changes of the **same**
field still end with the last write (§ 11).

Archive and restore send `{ prn: input.prn }`. A command takes no `mayI` (511 § 6.3).

### 4.4 Action shells

Each shell is a copy of `createOrganizationAction`, with one addition in step 4:

1. `iamClientsForAction()`. If it fails, return its result (the relogin state, row R13).
2. `safeParse(formFields(form, [...]))`. If it fails, return `invalidFormInput()`.
3. Call the command with `{ tenancy: clients.value.tenancy }`.
4. Call `revalidatePath(TENANCY_PATH, 'layout')` when the result is a success **or** has
   presentation `forbidden` or `conflict`. For these nine actions, a refusal often means that the
   page is stale: another user archived the node or took the slug. Without the refresh, the page
   shows an "Active" badge next to a 403.
5. Return the result.

`'layout'` is necessary. A rename changes the name in the parent list, in the breadcrumbs, and in
the organization switcher (which reads `myScopes()` → `GetOrganization`; Get returns archived nodes,
so an archive needs no switcher change).

An action never calls `mayI()`, `forbidden()`, `notFound()`, `redirect()` or `iamClients()`. The
`actions-structure` test bans `mayI` and `iamClients` today, but **not** the three navigation
helpers. This issue adds them to its banned list, with a negative control for each (§ 8).

### 4.5 D8 — `NodeStatus` for the app

New file `ts/packages/paigasus-sdk/src/iam/types.ts`, exported as `@paigasus/sdk/iam/types`:

```ts
// No server guard: an app's client code and its Playwright support files import this entry.
export { NodeStatus } from '@paigasus/proto/iam';
```

The root barrel (`src/index.ts`) lists `NodeStatus` too, because
`tests/index-barrel.test.ts` checks that every subpath export is in the barrel. The loaders and
`tests/e2e/support/world.ts` import the enum from this entry. No code writes `0`, `1` or `2`.

(If `src/iam.ts` and a directory `src/iam/` cannot both exist cleanly in this package, the file is
`src/iam-types.ts` with the export `./iam-types`. The plan decides after it reads the package
layout. The entry must be guard-free in both cases.)

## 5. Affordances

### 5.1 The mayI queries

`IamAction` in `ts/packages/paigasus-console-core/src/authorize.ts` gets the nine names from § 1.

Each detail loader adds three queries to its `Promise.all`, all against the node's own PRN:
`mayI('Rename<Node>', prn)`, `mayI('Archive<Node>', prn)` and `mayI('Restore<Node>', prn)`. The page
data gets `canRename`, `canArchive` and `canRestore`. This adds three `IsAuthorized` calls to each
detail page. The project loader has no `Promise.all` today. It gets one, with the member load and the
three queries.

**Name parity.** `mayI()` fails open. A misspelled action name gets `invalid-action` from IAM, so
`mayI()` answers true, and the control always shows. No test can see that. Two controls close it:

- A `@paigasus/console-core` unit test reads `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`
  and asserts that each `IamAction` name is one of its wire names. The package's Moon `test` task
  gets that file as an input. The same task already keys on a Rust file (the kernel's `model.rs`).
  `IamAction` must therefore be a runtime array (`IAM_ACTIONS`) with the type derived from it.
- `ALL_ACTIONS` in `tests/e2e/support/world.ts` gets `satisfies readonly IamAction[]` through an
  `import type`, and a unit test asserts that it holds the same set as `IAM_ACTIONS`.

### 5.2 Status

The loader maps each `NodeStatus` to `NodeState = 'active' | 'archived' | 'unknown'`. `UNSPECIFIED`
and any value that this build does not know map to `'unknown'` (version skew). One module,
`app/(console)/node-status.ts`, does this for all three node kinds and for the list rows:

```ts
export type NodeLifecycle = { readonly own: NodeState; readonly effective: NodeState };
export function lifecycleOf(node: { status: NodeStatus; effectiveStatus: NodeStatus }): NodeLifecycle;
export type LifecycleView = 'active' | 'archived' | 'archived-parent' | 'unknown';
export function lifecycleView(lifecycle: NodeLifecycle): LifecycleView;
```

`lifecycleView` applies these rules, in this order:

1. If `own` is `unknown` or `effective` is `unknown`: `'unknown'`.
2. If `own` is `archived`: `'archived'`. (`effective` is then `archived` too. An `archived`/`active`
   pair cannot come from IAM. If it does, it still shows as `'archived'`.)
3. If `effective` is `archived`: `'archived-parent'`.
4. Else: `'active'`.

The badge and the column use two small lookup tables over `LifecycleView` (§ 6.3).

### 5.3 The Manage section

`app/(console)/manage-section.tsx` is a server component. The three detail pages render it with
these props:

```ts
type ManageSectionProps = {
  readonly node: 'organization' | 'team' | 'project';   // test ids and copy
  readonly prn: string;
  readonly name: string;
  readonly slug: string;
  readonly lifecycle: NodeLifecycle;
  readonly can: { readonly rename: boolean; readonly archive: boolean; readonly restore: boolean };
  readonly actions: { readonly rename: FormAction; readonly archive: FormAction; readonly restore: FormAction };
};
```

`FormAction` is `(previous: ActionState, form: FormData) => Promise<ActionState>`. Each page passes
its own folder's three actions. Test ids use the `node` value: `rename-organization`,
`archive-team`, `restore-project`, and so on.

The section computes its controls first, and renders nothing when there are none:

| `lifecycleView`   | Rename form     | Lifecycle control        | Note |
| ----------------- | --------------- | ------------------------ | ---- |
| `active`          | if `can.rename` | Archive, if `can.archive` | none |
| `archived`        | if `can.rename` | Restore, if `can.restore` | none |
| `archived-parent` | if `can.rename` | Archive, if `can.archive` | "A parent of this item is archived. Restore the parent to change this item." |
| `unknown`         | if `can.rename` | none                     | none |

The note shows only when the section renders. For `archived-parent` with the default policy,
`can.rename` and `can.archive` are false (F9), so the section needs one more rule: **for
`archived-parent`, the section renders the note even when it has no controls.** Without this rule,
the user sees the badge and gets no hint where to act.

The rename form depends only on `mayI()`. In steady state `can.rename` is false on an effectively
archived node (F9), so the form hides. When the form still shows (`enforce_tenancy = false`, or a
stale decision), IAM answers `forbidden` or `node-archived`, and the form shows the copy.

**D7.** 511 § 6.3 says that `mayI()` may hide an affordance. This spec adds one rule: **the
lifecycle control shows only the transition that the node's own status allows.** An active node never
shows Restore, and an archived node never shows Archive. The rule does not refuse a real change:

- Restore on an active node changes nothing (F6), and IAM accepts it.
- Archive on an archived node changes nothing at the repository. In the default configuration, IAM
  refuses it with `PermissionDenied` (F7). So in this one case the rule hides a call that IAM would
  refuse. The call could not change anything in either case, so no user loses a capability.

No action and no command applies D7. This issue adds one sentence to 511 § 6.3 that points to this
section, and a comment in `manage-section.tsx`.

For the same reason, the `archived-parent` row shows no Restore button (F8). The note does not link
to the ancestor, because the page does not know which ancestor is archived.

## 6. Components

### 6.1 `app/_components/rename-form.tsx`

A client component. It has hidden `prn`, `currentSlug` and `currentName` fields, a slug field and a
name field. The success text is "Renamed.", and the result region shows it (§ 6.4). The error area
is `FormError`. Test id:
`rename-<node>`.

**The inputs are controlled.** React 19 resets a form before it runs **every** action, whatever the
result (`react-dom-client.development.js:8954-8957`). With uncontrolled inputs and `defaultValue`,
a failed rename would put the old values back, and the error "This slug is already in use" would
show next to the old slug. Controlled inputs keep their React state through the reset. So:

- The component holds `slug` and `name` in `useState`, initialised from the props.
- The page renders it with `` key={`${slug} ${name}`} ``. A space is a safe separator, because IAM
  allows only `[a-z0-9-]` in a slug. After a successful rename, the page
  renders again with new props, the key changes, and the component starts again from the new values
  (and with a new `useActionState`). The result region shows "Renamed." (§ 6.4).
- After a failed rename, the key does not change, so the typed values stay.

### 6.2 `app/_components/lifecycle-button.tsx`

Two client components in one file: `ArchiveButton` and `RestoreButton`. The Manage section renders
the one that D7 selects, with `key={view}`. Two component types in one position already remount,
and the key makes the intent explicit. So no `useActionState` result and no `confirming` state
passes from one to the other.

- **`RestoreButton`.** One form with a hidden `prn` and a "Restore" submit button. Success text:
  "Restored.".
- **`ArchiveButton`.** Local state `confirming: boolean`.
  - `confirming = false`: a button "Archive" (`type="button"`) sets `confirming = true`.
  - `confirming = true`: this text, then a form with a hidden `prn`, a "Confirm archive" submit
    button, and a "Cancel" button (`type="button"`) that sets `confirming = false`:

    > Archive <name>? Until you restore it, IAM refuses changes to it and to everything under it, and
    > the AI Gateway refuses model calls for everything under it.

  - Success text: "Archived.". After a successful archive, the page renders again, and
    `RestoreButton` replaces this component.

With JavaScript off, no manage control works. The first archive state has no form. Rename and
restore post to a client wrapper (§ 6.4), not directly to the Server Action, so React renders no
server form action for them. A submit before hydration still works, because React replays it. This
is accepted: the console is a React app, and the create forms need JavaScript for their state too.

Test ids: `archive-<node>` and `restore-<node>`, each with an `-error` child as in `CreateForm`.
The result region shows the success text of both controls (§ 6.4).

### 6.3 The status badge and column

| `LifecycleView`   | Header badge        | Status column       |
| ----------------- | ------------------- | ------------------- |
| `active`          | (none)              | "Active"            |
| `archived`        | "Archived"          | "Archived"          |
| `archived-parent` | "Archived (parent)" | "Archived (parent)" |
| `unknown`         | "Status unknown"    | "Unknown"           |

The badge goes next to the `h1` of each detail page. The column goes into the team table
(org page), the project table (team page), and the "All organizations" table (orgs page). The row
types (`OrganizationRow`, `TeamRow`, `ProjectRow`) get a `lifecycle: NodeLifecycle` field.

The "Your organizations" list on the orgs page and the organization switcher come from
`myScopes()`, which returns no status. They stay as they are (§ 10).

### 6.4 The result region

A rename, archive or restore action refreshes the page on a success, and on `forbidden` and
`conflict` (§ 4.4). React commits the action result and the refreshed page together. The refresh
can change the lifecycle view or the rename key. Then the control that ran the action unmounts, and
its `useActionState` result goes with it. Without a fix, "Renamed.", "Archived." and "Restored."
never show, and a 403 on a stale page loses its copy and its correlation id.

The fix is a client component, `app/_components/manage-controls.tsx`. The server section keeps its
§ 5.3 decisions and passes them, with the three actions, to `ManageControls`. `ManageControls`
renders the controls and one result region (test id `manage-result`) below them. The section
renders it with no `key` at a stable position, so React keeps its state through the refresh.
`ManageControls` wraps each action and keeps the last result in `useState`. The rule:

- A success shows only in the region: "Renamed.", "Archived." or "Restored.". The controls show no
  success text.
- A failure shows in the `-error` area of the control that produced it, while that control is
  mounted. The region shows the failure only after that control unmounts: its key changed, or the
  section does not render it. So the same error never shows twice.
- A new submission replaces the previous result.

The region is inside the section. When the refreshed section renders nothing (§ 5.3), the region
goes too. `tests/unit/manage-controls.test.tsx` pins this rule.

## 7. Error copy

`FORM_REASON_COPY` gets `NOTHING_TO_RENAME: 'Change the slug or the name first.'`. D6 makes this
reason reachable: a submit with no change sends neither field.

No other copy changes (F12). A denial by `forbid-archived-writes` is a `PermissionDenied` with
reason `FORBIDDEN`, so the form shows `FORM_REASON_COPY[FORBIDDEN]` and the correlation id.

The comment "the five forms" in `error-copy.ts:7-8` is updated.

## 8. Registries and controls to update

| Registry or control | Change |
| ------------------- | ------ |
| `IamAction` (`console-core/src/authorize.ts`) | Becomes `IAM_ACTIONS` (a runtime array) plus the derived type. Add the nine names. |
| New parity test (`console-core`) | § 5.1. Add `action.rs` to the package's Moon `test` inputs. `repo:input-liveness` does not check non-`repo` tasks, so the plan runs `moon query tasks` to confirm the input resolves. |
| `ALL_ACTIONS` (`tests/e2e/support/world.ts`) | Add the nine names, with `satisfies readonly IamAction[]`. |
| World nodes (`world.ts:60-62`) | `ORGANIZATION`, `TEAM`, `PROJECT` and the other scripted nodes get `status` and `effectiveStatus` set to `NodeStatus.ACTIVE`. Without this, every existing row would render "Status unknown". |
| World default handlers | Add the nine RPCs. Each returns the scripted node. |
| `EXPECTED` (`tests/unit/actions-structure.test.ts`) | Add the nine exports and the new project `actions.ts`. |
| Banned calls (same test) | Add `redirect`, `forbidden` and `notFound`, each with a negative control. |
| `actions-revalidate.test.ts` | Add the nine RPCs to the fake `tenancy`. Assert `revalidatePath(TENANCY_PATH, 'layout')` on success, on `forbidden` and on `conflict`, and **no** call on `invalid-input`. The five existing actions keep their success-only rule. |
| `ROWS` (`tests/unit/e2e-rows.test.ts`) | 13 → 16 rows. Update the comment and the `describe` title, and name this spec as the source of R14–R16. |
| `@paigasus/sdk` barrel test | Covers `NodeStatus` through the root barrel (§ 4.5). |
| 511 § 6.3 | One sentence that points to D7 (§ 5.3). |
| Moon `sources` of the `ts` project | No change. `apps/*/app/**/*` covers the new files. |

Checked, and no change is needed: the fake IAM types all nine RPCs from the service descriptor
(`console-core/testing/fake-iam.ts:63-97`). `tests/support/next-cache.ts` records both arguments of
`revalidatePath`. `scriptedMayI` is typed on `IamAction`. The gateway-console two-zone tier has its
own world and visits only `/iam/orgs`.

## 9. Tests

### 9.1 Tier 2 (integration, fake IAM, no Next runtime)

New file `tests/integration/lifecycle-commands.test.ts`. For each of the nine commands:

- The request that reaches the fake IAM has the expected fields.
- Success returns `{ ok: true }`.
- `PermissionDenied` with reason `forbidden` returns presentation `forbidden`.

For the three rename commands:

- Only the slug changed: the request has `newSlug` and no `newName`. Only the name changed: the
  reverse. Both changed: both fields. Nothing changed: neither field, and IAM's
  `nothing-to-rename` comes back in the result.
- A comparison uses trimmed values (`" acme "` against `"acme"` is no change).
- `AlreadyExists` with `slug-conflict`, `InvalidArgument` with `invalid-slug`, and
  `FailedPrecondition` with `node-archived` keep their reasons.

Schema cases (unit, `form.test.ts`): a 256-code-point name passes, a 257-code-point name fails, and a
name of 256 astral characters (512 UTF-16 units) passes.

The loader tests (`org-page.test.ts`, `team-project-pages.test.ts`, `orgs-page.test.ts`) get:

- The three `can*` flags follow the scripted `IsAuthorized` answers, and the queries use the node's
  own PRN and the right action name.
- `status` and `effective_status` map to `NodeLifecycle`, including `UNSPECIFIED` → `unknown`.
- The list rows carry `lifecycle`.

### 9.2 Unit

- `node-status.test.ts`: `lifecycleOf` and `lifecycleView` for all nine (own × effective)
  combinations, including the impossible `archived`/`active` pair.
- `manage-section.test.tsx`: the table in § 5.3, row by row. Extra cases: no controls renders
  nothing, and `archived-parent` with no controls renders only the note.
- `lifecycle-button.test.tsx`: `ArchiveButton` shows no submit button before the first click, and
  the confirmation text is exactly the § 6.2 text. Cancel returns to the first state.
  `RestoreButton` has a submit button at once.
- `rename-form.test.tsx`: after an action that returns a failure, the inputs keep the typed values.
- The two parity tests from § 5.1.

### 9.3 E2e (new rows)

The rows go in a new file, `tests/e2e/lifecycle.spec.ts`.

- **R14 — archive and restore a team.** The test scripts stateful `getTeam`, `archiveTeam` and
  `restoreTeam` handlers through `overrides`. The state is in the test's closure, and each call
  changes **both** `status` and `effectiveStatus`. The steps:
  1. The user clicks "Archive", then "Confirm archive". The header shows "Archived". The "Restore"
     button shows, and no button named "Archive" or "Confirm archive" exists.
  2. The user clicks "Restore". The badge goes away. A button named exactly "Archive" shows, and no
     "Confirm archive" button exists (so the confirmation state was not kept).
  3. The fake IAM call log, sliced from the count before step 1, holds exactly one `archiveTeam`
     and one `restoreTeam`, each with the team PRN.

  Every button locator uses `exact: true`, because "Confirm archive" contains "archive".
- **R15 — a denied rename shows an inline 403 and keeps the input.** `IsAuthorized` allows
  `RenameProject`, and `renameProject` throws `PermissionDenied` with reason `forbidden`. The user
  types a new slug in the rename form and submits. All locators are scoped to the
  `rename-project` form. The `rename-project-error` area shows `FORM_REASON_COPY[FORBIDDEN]` and
  the correlation id. The slug field still holds the typed slug. The page stays on the project URL.
- **R16 — a node under an archived parent.** The fake IAM returns a project with
  `status = ACTIVE` and `effectiveStatus = ARCHIVED`. `IsAuthorized` denies `RenameProject` and
  `ArchiveProject` and allows `RestoreProject`, as the starter policy does. The header shows
  "Archived (parent)", and the note from § 5.3 shows. No rename form, no Archive button and no
  Restore button exists.

## 10. Out of scope, and follow-ups

Out of scope:

- Row actions on the list pages.
- A filter that hides archived rows. Offset paging would show short pages.
- Status in "Your organizations" and in the organization switcher. Both read `myScopes()`, which
  has no status. This needs a `@paigasus/console-core` change and its own design.
- Confirmation for Restore. A restore starts traffic again, which is the purpose of the restore.
- A link from the "parent archived" note to the archived ancestor.
- Any change to the proto, `@paigasus/auth` or the Rust service.
- The gateway-console.

Proposed IAM follow-up issues (found during the challenge, not fixed here):

1. The rename path does not call `validate_name` (F11). IAM stores an empty or very long name.
2. `prn-mismatch` is returned after the write has committed (§ 4.2).

## 11. Risks

- **Three more `IsAuthorized` calls on each detail page.** `mayI()` runs them in parallel with the
  other reads. The cost is the same kind that 511 accepted for the create forms.
- **Stale decisions.** IAM caches decisions keyed on `entity_gen` (`cedar_authorizer.rs:167-175`).
  If a Redis bump is lost after an archive, `mayI()` can answer "allow" until the TTL ends (30 s for
  decisions, 60 s for slices). In that window the rename form can show on an archived node. IAM
  still refuses the rename (F5), and the form shows the copy. § 5.3's "the form hides" is a
  steady-state statement.
- **Last write wins on rename (F1).** D6 removes the cross-field lost update. Two concurrent changes
  of the same field still end with the later write, with no warning. The proto has no version
  field, so the console cannot detect this.
- **D7 is a new UI rule.** It is narrow, a unit test pins its table, and 511 § 6.3 points to it.
- **Another user renamed the node.** The refresh then changes the rename key, and the form starts
  again from the new values. The values that the user typed are lost. The page shows the new values,
  and the result region (§ 6.4) shows the refusal or the result. This is accepted.

## 12. Challenge log (Stage 2)

The adversarial challenger returned **APPROVE WITH CHANGES**. The changes in draft 2:

| Finding | Severity | Result |
| ------- | -------- | ------ |
| The app cannot import `NodeStatus` | BLOCKER | Fixed: D8, § 4.5, F2 corrected. |
| React 19 resets the form on every submit | MAJOR | Fixed: controlled inputs with a key (§ 6.1), a unit test, and an R15 assertion. |
| The lifecycle control keeps its state across modes | MAJOR | Fixed: two components with `key` (§ 6.2), and `exact: true` in R14. |
| A starter policy cannot be removed | MAJOR | Fixed: F7 and § 5.3 name the real conditions. |
| D7's invariant is false for Archive | MAJOR | Fixed: D7 restated, F6 qualified, and a pointer in 511 § 6.3. |
| The name bound blocks a valid rename, and IAM does not validate a renamed name | MAJOR | Fixed: a 256-code-point bound (§ 4.2), F11, and follow-up 1. |
| The confirmation text leaves out the gateway effect | MAJOR | Fixed: F10 and the § 6.2 text. |
| D6 reverts a field that the user did not change | MAJOR | Fixed: send only changed fields (D6, § 4.3). |
| Missing registries: world status, action-name parity, navigation-helper ban, stale comments | MAJOR | Fixed: § 5.1 and § 8. |
| The wrong control is named for PRN changes | MINOR | Fixed: § 4.2, and follow-up 2. |
| The switcher claim names the wrong call | MINOR | Fixed: § 4.4. |
| The bound duplication is larger than stated | MINOR | Fixed: § 4.2 replaces all copies. |
| One label function cannot serve both places | MINOR | Fixed: `LifecycleView` and two tables (§ 5.2, § 6.3). |
| Gaps in the § 5.3 table | MINOR | Fixed: ordered rules and the note-only case. |
| `ManageSection` props and test ids are not specified | MINOR | Fixed: § 5.3. |
| R14–R16 are under-specified | MINOR | Fixed: § 9.3. |
| The JavaScript-off statement is wrong | MINOR | Fixed: § 6.2. |
| The page stays stale after a refused action | MINOR | Fixed: revalidate on `forbidden` and `conflict` (§ 4.4). |
| `mayI()` can be stale after an archive | MINOR | Recorded: § 11. |
| Status in "Your organizations" and the switcher | QUESTION | Out of scope (§ 10): `myScopes()` has no status. |
| Open IAM follow-up issues? | QUESTION | Proposed in § 10. Waiting for a decision at Gate 1. |
| Confirm Restore of an organization? | QUESTION | No (§ 10). A restore starts traffic again on purpose. |
| An ADR for D7? | QUESTION | Not proposed. D7 is a narrow screen rule, and 511 § 6.3 points to it. Waiting for a decision at Gate 1. |
