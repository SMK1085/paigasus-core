# SMA-630 — rename, archive and restore in iam-console

- **Issue:** [SMA-630](https://linear.app/smaschek/issue/SMA-630)
- **Parent spec:** `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` (SMA-511).
  This document uses "511 § n" for a section of that spec.
- **Status:** draft, 2026-09-17

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

| Id  | Decision                                                                                                                                                                                             |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D1  | The controls go on the **detail page** of each node, in a "Manage" section. The list pages get no row actions.                                                                                        |
| D2  | A denied mutation shows an **inline 403** in the form, with the correlation id (511 § 6.1, row R7). No action calls `forbidden()`.                                                                    |
| D3  | Archive asks for confirmation in **two steps**, inline, with local client state. Restore and rename do not ask.                                                                                        |
| D4  | The detail header shows an archived badge. The org, team and project list tables get a **Status** column.                                                                                              |
| D5  | Each detail route folder holds its own three actions (approach A). There is no generic action and no hidden `kind` field.                                                                            |
| D6  | Rename always sends **both** `new_slug` and `new_name`. The form starts with the current values.                                                                                                     |
| D7  | The Archive and Restore buttons are selected by the node's **own** `status`. This is a second hide rule next to `mayI()` (§ 5).                                                                        |

## 3. Facts this design depends on

All are read from the code on `main` at `c5388d32`.

- **F1.** The proto already declares all nine RPCs (`contracts/proto/paigasus/iam/v1/iam.proto`).
  `Rename*Request` is `{ prn, optional new_slug, optional new_name }`. `Archive*Request` and
  `Restore*Request` are `{ prn }`. Each response returns the node. No request has a version or an
  etag field, so the last write wins.
- **F2.** `@paigasus/console-core`'s `IamClients['tenancy']` is the generated client. It can already
  call all nine RPCs. No SDK change is necessary.
- **F3.** Each node message has `status` and `effective_status`, of type `NodeStatus`
  (`UNSPECIFIED = 0`, `ACTIVE = 1`, `ARCHIVED = 2`). `effective_status` is `ARCHIVED` when the node
  or one of its ancestors is archived. The List and Get RPCs return archived nodes. There is no
  filter field.
- **F4.** A rename with no field set fails with `nothing-to-rename`. A rename whose values are equal
  to the current values **succeeds** and changes nothing (`pg_organizations.rs` `rename_in`: "a write
  that changes nothing stamps nothing"). So D6 cannot cause `nothing-to-rename`.
- **F5.** A rename of an effectively archived node fails with `node-archived` (a precondition) in
  the application layer.
- **F6.** `set_status` is idempotent. An archive of an archived node, and a restore of an active
  node, both succeed and change nothing.
- **F7.** The Cedar starter policy `forbid-archived-writes`
  (`rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`) forbids every write action **except**
  `Restore*` on a resource whose `effective_status` is `archived`. So on an effectively archived
  node, `Rename*` and `Archive*` are denied with `PermissionDenied`, and `Restore*` is not. This
  denial comes before F5 when tenancy enforcement is on. An operator can remove a starter policy,
  and then F5 applies.
- **F8.** A restore of a node that is itself `ACTIVE` but under an archived ancestor succeeds (F6)
  and does not change `effective_status`. Only a restore of the archived ancestor helps.
- **F9.** `ErrorReason` to presentation is already total in `@paigasus/sdk`. The console's
  `FORM_REASON_COPY` (`app/_components/error-copy.ts`) has copy for `NODE_ARCHIVED`,
  `PARENT_ARCHIVED`, `SLUG_CONFLICT`, `FORBIDDEN` and `NOT_FOUND`, but not for `NOTHING_TO_RENAME`.

## 4. Commands and Server Actions

This follows 511 § 5.3 without change.

### 4.1 Files

| Route folder (under `app/(console)/`)       | `actions.ts` exports (new ones in bold)                                                   |
| ------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `orgs/[org]/`                               | `createTeamAction`, **`renameOrganizationAction`**, **`archiveOrganizationAction`**, **`restoreOrganizationAction`** |
| `orgs/[org]/teams/[team]/`                  | `createProjectAction`, **`renameTeamAction`**, **`archiveTeamAction`**, **`restoreTeamAction`** |
| `orgs/[org]/teams/[team]/projects/[project]/` (new files) | **`renameProjectAction`**, **`archiveProjectAction`**, **`restoreProjectAction`**           |

Each folder's `commands.ts` gets the three matching commands and zod schemas. The project folder
gets a new `commands.ts`.

### 4.2 Schemas

The schemas check the shape only. IAM owns the slug grammar and the PRN grammar.

- `rename<Node>Form = z.object({ prn, slug: text, name: text })`
- `archive<Node>Form = restore<Node>Form = z.object({ prn })`

`text` and `prn` are the bounds that `orgs/commands.ts` already uses. To prevent a third copy, move
both to `lib/form.ts` and import them from there.

The PRN comes from a hidden field that the page renders. A user can change a hidden field, and that
is safe. IAM authorizes the action against the PRN in the request, and the `prn-mismatch` check in
each handler refuses a PRN of the wrong kind.

### 4.3 Commands

Each command takes a narrow port and returns an `ActionResult`:

```ts
export async function renameTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameTeam'> }, input: RenameTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.renameTeam({ prn: input.prn, newSlug: input.slug, newName: input.name })));
}
```

Archive and restore have the same shape, with `{ prn: input.prn }` as the request. A command takes no
`mayI` (511 § 6.3).

### 4.4 Action shells

Each shell is a line-by-line copy of `createOrganizationAction`:

1. `iamClientsForAction()`. If it fails, return its result (the relogin state, row R13).
2. `safeParse(formFields(form, [...]))`. If it fails, return `invalidFormInput()`.
3. Call the command with `{ tenancy: clients.value.tenancy }`.
4. On success, call `revalidatePath(TENANCY_PATH, 'layout')`.
5. Return the result.

`'layout'` is necessary. A rename changes the name in the parent list, and in the breadcrumbs, and
in the organization switcher.

An action never calls `mayI()`, `forbidden()`, `redirect()` or `iamClients()`. The existing
`actions-structure` test already enforces this for every `actions.ts`.

**Archive of the organization shown in the switcher.** An archived organization stays in
`ListOrganizations` (F3), so the switcher needs no special case. The user stays on the page.

## 5. Affordances

### 5.1 The mayI queries

`IamAction` in `ts/packages/paigasus-console-core/src/authorize.ts` gets the nine names from § 1.
The names must be equal to the Cedar action names (`Action::parse` in `paigasus-iam-core`).

Each detail loader adds three queries to its existing `Promise.all`, all against the node's own PRN:
`mayI('Rename<Node>', prn)`, `mayI('Archive<Node>', prn)` and `mayI('Restore<Node>', prn)`. The
page data gets `canRename`, `canArchive` and `canRestore`. This adds three `IsAuthorized` calls to
each detail page. `mayI()` fails open (it returns true when the query fails), as 511 § 6.3
specifies.

The project loader has no `Promise.all` today. It gets one, with the member load and the three
queries.

### 5.2 Status

The loader maps each `NodeStatus` to `NodeState = 'active' | 'archived' | 'unknown'`. `UNSPECIFIED`
and any value that this build does not know map to `'unknown'` (version skew). One function in
`app/(console)/node-status.ts` does this for all three node kinds and for the list rows:

```ts
export type NodeLifecycle = { readonly own: NodeState; readonly effective: NodeState };
```

### 5.3 The Manage section

`app/(console)/manage-section.tsx` is a server component. The three detail pages render it. It is
not rendered when all three of `canRename`, `canArchive` and `canRestore` are false.

| Own status | Effective status | Rename form     | Lifecycle control                   | Note                                          |
| ---------- | ---------------- | --------------- | ----------------------------------- | --------------------------------------------- |
| active     | active           | if `canRename`  | Archive, if `canArchive`            | none                                          |
| archived   | archived         | if `canRename`  | Restore, if `canRestore`            | none                                          |
| active     | archived         | if `canRename`  | Archive, if `canArchive`            | "A parent of this item is archived. Restore the parent to change this item." |
| unknown    | any              | if `canRename`  | none                                | none                                          |
| any        | unknown          | if `canRename`  | as the own-status row               | none                                          |

The rename form depends only on `mayI()`. With the starter policy in place, `canRename` is false on
an effectively archived node (F7), so the form hides. Without the starter policy, the form shows,
and IAM answers `node-archived` (F5). The form then shows the copy for that reason.

**D7 — the second hide rule.** 511 § 6.3 says that `mayI()` may hide an affordance. This spec adds
one more rule: **a page may hide an affordance whose action IAM would accept and that would change
nothing.** Archive on an archived node, and Restore on an active node, are such actions (F6). The
rule never hides an action that IAM would refuse, so the UI still does not decide a refusal. No
action and no command applies the rule. This spec is the one place that records it, and
511 § 6.3 is not edited.

For the same reason, the "parent archived" row shows no Restore button (F8). The note tells the
user where the restore must happen. It does not link to the ancestor, because the page does not know
which ancestor is archived.

## 6. Components

### 6.1 `app/_components/rename-form.tsx`

A client component, based on `CreateForm`. It has the hidden `prn`, a slug field and a name field.
Both fields have `defaultValue` set to the current values. The success text is "Renamed.". The error
area is `FormError`, as in `CreateForm`. Test id: `rename-<node>`.

After a successful rename, `revalidatePath` renders the page again with the new values. React keeps
the uncontrolled inputs as they are, so the fields keep what the user typed, which is the new value.

### 6.2 `app/_components/lifecycle-button.tsx`

A client component with `mode: 'archive' | 'restore'`, the node PRN, the node name, and the action.

- **Restore.** One form with a hidden `prn` and a "Restore" submit button. Success text:
  "Restored.".
- **Archive.** Local state `confirming: boolean`.
  - `confirming = false`: a plain "Archive" button (`type="button"`) sets `confirming = true`.
  - `confirming = true`: the text "Archive <name>? Writes under it are blocked until you restore
    it." A form with a hidden `prn` and a "Confirm archive" submit button. A "Cancel" button
    (`type="button"`) sets `confirming = false`.
  - After a successful archive, the page renders again and the Restore button replaces this one.

The first step is a client-only state. A form POST with JavaScript off reaches the action with no
confirmation. That is accepted: the confirmation is a guard against a wrong click, not a security
control, and archive is reversible.

Test ids: `archive-<node>`, `restore-<node>`, each with an `-error` child as in `CreateForm`.

### 6.3 The status badge and column

- **Header badge**, next to the `h1`: "Archived" when the own status is `archived`. "Archived
  (parent)" when the own status is `active` and the effective status is `archived`. No badge when
  the node is active. "Status unknown" when either value is `unknown`.
- **List column.** The org, team and project tables get a "Status" column with "Active",
  "Archived", "Archived (parent)" or "Unknown", by the same rules. The list row types
  (`OrganizationRow`, `TeamRow`, `ProjectRow`) get a `lifecycle: NodeLifecycle` field.

One function, `lifecycleLabel(lifecycle): string | null`, produces the text for both places.

## 7. Error copy

`FORM_REASON_COPY` gets `NOTHING_TO_RENAME: 'Change the slug or the name first.'`. D6 makes this
reason unreachable from the form. The entry is there for version skew only (a future IAM that
rejects an unchanged rename).

No other copy changes (F9). A denial by `forbid-archived-writes` is a `PermissionDenied` with
reason `FORBIDDEN`, so it shows the existing inline 403 copy and the correlation id.

## 8. Registries to update

| Registry                                                               | Change                                   |
| ---------------------------------------------------------------------- | ---------------------------------------- |
| `IamAction` (`console-core/src/authorize.ts`)                           | Add the nine names.                       |
| `EXPECTED` (`tests/unit/actions-structure.test.ts`)                     | Add the nine exports and the new project `actions.ts`. |
| `actions-revalidate.test.ts`                                           | Add the nine RPCs to the fake `tenancy` and assert `revalidatePath(TENANCY_PATH, 'layout')` for each action, on success only. |
| `ALL_ACTIONS` (`tests/e2e/support/world.ts`)                            | Add the nine names. Add default handlers for the nine RPCs that return the scripted node. |
| `ROWS` (`tests/unit/e2e-rows.test.ts`)                                  | 13 → 16 rows (§ 9.3). Its comment names 511 § 9.4; add this spec as the source of R14–R16. |
| The detail pages' `sources` in the `ts` Moon project                   | No change. `apps/*/app/**/*` covers the new files. |

## 9. Tests

### 9.1 Tier 2 (integration, fake IAM, no Next runtime)

New file `tests/integration/lifecycle-commands.test.ts`. For each of the nine commands:

- The request that reaches the fake IAM has the expected fields (rename: `prn`, `newSlug`, `newName`;
  archive and restore: `prn`).
- Success returns `{ ok: true }`.
- A `PermissionDenied` with reason `forbidden` returns `{ ok: false }` with presentation
  `forbidden`.

For rename only: `AlreadyExists` with `slug-conflict`, and `FailedPrecondition` with `node-archived`,
each keep their reason in the result.

The loader tests (`org-page.test.ts`, `team-project-pages.test.ts`, `orgs-page.test.ts`) get:

- The three `can*` flags follow the scripted `IsAuthorized` answers, and the queries use the node's
  own PRN and the right action name.
- `status` and `effective_status` map to `NodeLifecycle`, including `UNSPECIFIED` → `unknown`.
- The list rows carry `lifecycle`.

### 9.2 Unit

- `node-status.test.ts`: the `NodeStatus` mapping and `lifecycleLabel` for all combinations.
- `manage-section.test.tsx`: the table in § 5.3, row by row. Each row asserts which forms render.
  One extra case: all three flags false renders nothing.
- `lifecycle-button.test.tsx`: Archive shows no submit button before the first click. Cancel returns
  to the first state. Restore has a submit button at once.

### 9.3 E2e (new rows)

These rows go in a new file, `tests/e2e/lifecycle.spec.ts`. `e2e-rows.test.ts` counts them.

- **R14 — archive and restore a team.** The fake IAM holds the team's status and changes it on
  `ArchiveTeam` and `RestoreTeam`. The user clicks Archive, then Confirm archive. The header shows
  "Archived", and the Restore button replaces the Archive button. The user clicks Restore. The badge
  goes away, and the Archive button is back. The fake IAM records exactly one `ArchiveTeam` and one
  `RestoreTeam`, each with the team PRN.
- **R15 — a denied rename shows an inline 403.** `IsAuthorized` allows `RenameProject`, and
  `RenameProject` throws `PermissionDenied` with reason `forbidden`. The user submits the rename
  form. The `rename-project-error` area shows the 403 copy and the correlation id. The page stays on
  the project URL with HTTP 200.
- **R16 — a node under an archived parent.** The fake IAM returns a project with `status = ACTIVE`
  and `effective_status = ARCHIVED`, and `IsAuthorized` denies `RenameProject` and `ArchiveProject`
  (as the starter policy does). The header shows "Archived (parent)", the note from § 5.3 shows, and
  no rename form, Archive button or Restore button exists.

R14 needs a stateful fake handler. The `world.ts` default handlers are stateless today. R14 scripts
its own handlers through `overrides`, with the state inside the test's closure, so no other row
changes.

## 10. Out of scope

- Row actions on the list pages.
- A filter that hides archived rows. Offset paging would show short pages.
- Any change to the proto, the SDK, `@paigasus/auth` or the Rust service.
- The gateway-console. It shows no tenancy mutations.
- A link from the "parent archived" note to the archived ancestor (§ 5.3).

## 11. Risks

- **Three more `IsAuthorized` calls on each detail page.** `mayI()` runs them in parallel with the
  other reads. The cost is the same kind that 511 accepted for the create forms.
- **D7 is a new UI rule.** The rule is narrow (§ 5.3), and a test pins its table. If a later IAM
  makes archive of an archived node an error, the rule still hides only that no-op, and nothing
  breaks.
- **Last write wins on rename (F1).** Two users can rename the same node at the same time, and the
  later write wins. The proto has no version field, so the console cannot detect this. That is an
  IAM concern and is out of scope here.
