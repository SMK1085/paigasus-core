# SMA-636 Gateway Organization and Project Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The gateway zone gets an organization settings page and a project settings page. Each
page lists the service accounts that the node owns. A user can create an account (with the
`gateway_user` grant), allow model calls, issue an API key, revoke a key and archive an account.

**Architecture:** `@paigasus/console-core` gets the `serviceAccounts` IAM client, five new
`IAM_ACTIONS` names, a new log event, the pure form helpers and the fake-IAM route. The gateway
zone gets pure domain modules, one shared set of commands and Server Actions, one loader for the
service-accounts section, one loader per page, and client components with ONE result region per
section. The e2e world becomes stateful.

**Tech Stack:** TypeScript 5, Next 16.3 (App Router, Turbopack), React 19, zod 4,
`@connectrpc/connect` 2, vitest 5 (node and jsdom), Testing Library, Playwright 1.63, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-18-sma-636-gateway-org-project-settings-design.md`
(revision 2).

## SPEC DEVIATION NEEDED

Read these before Task 1. Each one names the evidence. The plan implements the text on the
right-hand side. A reviewer must accept or reject each one.

1. **`ZoneLink` gets a `prefetch` prop in `@paigasus/app-shell`.** Spec § 4.1 requires
   `prefetch={false}` on same-zone project, select and pager links. Spec § 3.4 records that
   `ZoneLink` has no `prefetch` prop (`ts/packages/paigasus-app-shell/src/zone/zone-link.tsx:10-15`).
   Spec § 4.5 lists no change to `@paigasus/app-shell`. The plan adds an optional
   `prefetch?: boolean` that only the same-zone `NextLink` branch receives (Task 6).
2. **`ApiKeyStatus` is exported from `@paigasus/sdk/iam/types`.** An app may not import
   `@paigasus/proto` (`ts/packages/paigasus-sdk/src/iam/types.ts:8-11`, eslint boundary
   `paigasus/boundaries/apps`). The key status mapping of § 5.7 needs the enum. Spec § 4.5 lists no
   SDK change. The plan adds the re-export (Task 1), in the pattern SMA-630 used for `NodeStatus`.
3. **The pager is NOT a copy of iam-console's.** D15 says "the `lib/paging.ts` pattern: … a
   request of limit+1". iam-console asks for `PAGE_SIZE` and treats a full page as "more"
   (`ts/apps/iam-console/lib/paging.ts:31-33`); it never asks for limit+1. The plan follows the
   spec's explicit numbers (limit 51, "exactly 50 gives no note") in a new gateway
   `lib/paging.ts` (Task 7).
4. **Keys load for an archived account too.** § 4.2 step 4 reads "for an active account:
   `modelCallState` … and `ListApiKeys` … if `iam.apikeys` is present". § 5.7 and § 7.2 row 1
   require archived keys to show "Inactive (account archived)", which needs `ListApiKeys` for an
   archived account. The plan calls `modelCallState` for every selected account (it makes no call
   for a non-active one, § 4.3) and `ListApiKeys` whenever `iam.apikeys` is present (Task 11).
5. **One `commands.ts` and one `actions.ts` serve both pages.** § 4.5 says "A `load.ts`,
   `commands.ts` and `actions.ts` per page". The five actions are identical on both pages: each one
   takes its owner from the form (create) or from IAM (D6), never from the page. The plan puts
   them once under `app/(console)/service-accounts/` and keeps one `load.ts` per page (Tasks 10,
   12, 13).
6. **A `relogin` result never revalidates.** §§ 5.3, 5.5, 5.6 say "revalidates after every
   result". A revalidation after `relogin` renders the `(console)` layout, whose
   `requireSession()` redirect carries no basePath and leaves the zone
   (`ts/packages/paigasus-console-core/src/runtime.ts:71-91`). The plan revalidates after every
   result except `relogin` (Task 13).
7. **Every mutation control renders its submit after hydration.** § 5.4 rule 7 requires this for
   the issue form only. The plan submits every control through a client transition (rule 2 needs
   that for issue, and one mechanism is simpler to review), so every submit control waits for
   hydration. With JavaScript off the section is read-only (Tasks 14, 15).
8. **An issue success revalidates.** § 5.4 does not say. The plan revalidates after a successful
   issue so the new key row shows at once. The revalidated render reads IAM, which never returns a
   token (Task 13).

## Global Constraints

- Worktree root: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-636`. Run
  every command from there. Work only there.
- Start every command block with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. The
  Bash tool's PATH lacks the proto-managed CLIs (moon, pnpm, node).
- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Commit messages are conventional with a workspace scope and the issue key:
  `feat(ts): <what> (SMA-636)` (use `test(ts)` for a test-only commit). End the message with one
  blank line and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Do not put a `#NNN` or
  a `word: value` line in the body (commitlint `footer-leading-blank`).
- Add, do not amend. Never `git commit --amend`. Never `--no-verify`.
- Run `pnpm --dir ts format:write` before every commit. `ts:fmt` is a separate Prettier gate
  (`printWidth: 200`). Do not hand-wrap to guess the width.
- Relative VALUE imports in `app/`, `lib/` and `packages/*/src/` are EXTENSIONLESS (`./view`,
  never `./view.js`). Turbopack does not map `.js` onto `.ts`, and
  `paigasus/no-js-relative-specifier` reports it.
- No file base name may be a Windows reserved device name (`prn`, `con`, `aux`, `nul`,
  `com1`…`com9`, `lpt1`…`lpt9`). The PRN helper of this plan is `service-account-id.ts` for that
  reason.
- `createConsoleRuntime` stays called once, in `lib/console.ts`. New code imports the accessors
  from `lib/console`.
- Every file in `ts/packages/paigasus-console-core/src/` imports `'server-only'`. A client
  component imports from `@paigasus/console-core` with `import type` only.
- An app never imports `@paigasus/proto`, also not as a type. It takes enums from
  `@paigasus/sdk/iam/types` and message types by inference from `IamClients`.
- A Server Action file starts with `'use server'`, exports only async functions, and each export
  calls `iamClientsForAction()` first. It never names `iamClients`, `mayI`, `redirect`,
  `permanentRedirect`, `forbidden`, `unauthorized` or `notFound`.
- The plaintext API key token is never logged, never put in a URL, a cookie or page data.
- Same-zone links to a project page, a select link and a pager link carry `prefetch={false}`.
- An e2e spec never calls `locator.waitFor(`: `tests/unit/hydration.test.ts` scans for it. Use
  `expect(locator).toBeVisible()` or `page.waitForURL(...)`.
- The fake IAM's `setHandlers()` REPLACES the whole map. A handler that a test needs must be in the
  map it passes.
- Per-task test runs use one vitest file:
  `pnpm --dir ts --filter <package> exec vitest run <file>`. The full `gateway-console-ts:test`
  task needs a `next build` first; only Task 21 runs it.
- Write prose (comments, copy, commit messages) in plain, short sentences.

## File Structure

| File | Responsibility |
| -- | -- |
| `ts/packages/paigasus-sdk/src/iam/types.ts` (modify) | Also re-exports `ApiKeyStatus`. |
| `ts/packages/paigasus-sdk/src/index.ts` (modify) | The barrel re-exports `ApiKeyStatus`. |
| `ts/packages/paigasus-console-core/src/iam-clients.ts` (modify) | `serviceAccounts` in `IamClients` and `createIamClients`. |
| `ts/packages/paigasus-console-core/src/authorize.ts` (modify) | Five new `IAM_ACTIONS` names. |
| `ts/packages/paigasus-console-core/src/logger.ts` (modify) | `gateway.sa.grant_failed` in `AppEventName`. |
| `ts/packages/paigasus-console-core/src/form.ts` (create) | The pure form helpers and the `FormAction` type, moved from iam-console. |
| `ts/packages/paigasus-console-core/src/index.ts` (modify) | Exports the form helpers. |
| `ts/packages/paigasus-console-core/package.json` (modify) | `zod` dependency. |
| `ts/packages/paigasus-console-core/testing/fake-iam.ts` (modify) | Routes `ServiceAccountService`. |
| `ts/packages/paigasus-ui/src/lib/button-class.ts` (create) | `PRIMARY_BUTTON_CLASS`, `SECONDARY_BUTTON_CLASS`. |
| `ts/packages/paigasus-ui/src/index.ts` (modify) | Exports the two classes. |
| `ts/packages/paigasus-app-shell/src/zone/zone-link.tsx` (modify) | Optional `prefetch` on the same-zone branch. |
| `ts/packages/paigasus-app-shell/tests/support/next-link-double.tsx` (modify) | Records `prefetch` as `data-prefetch`. |
| `ts/apps/iam-console/lib/form.ts` (modify) | Keeps only the rename helpers. |
| `ts/apps/iam-console/app/_components/form-action.ts` (delete) | Moved to console-core. |
| `ts/apps/iam-console/app/_components/button-class.ts` (delete) | Moved to `@paigasus/ui`. |
| `ts/apps/iam-console/app/**` (modify, import lines only) | Import the moved names from their new homes. |
| `ts/apps/iam-console/tests/e2e/support/world.ts` (modify) | `ALL_ACTIONS` gets the five names. |
| `ts/apps/gateway-console/lib/paging.ts` (create) | Limit+1 paging, `parseOffset`, `linkHref`. |
| `ts/apps/gateway-console/lib/concurrency.ts` (create) | `mapWithLimit` for the 8-in-flight fan-out. |
| `ts/apps/gateway-console/lib/settings-path.ts` (create) | The one revalidation path. |
| `ts/apps/gateway-console/lib/nav.ts` (modify) | `iamManageHref`. |
| `ts/apps/gateway-console/app/(console)/node-status.ts` (create) | Copy of iam-console's `node-status.ts`. |
| `ts/apps/gateway-console/app/(console)/service-accounts/view.ts` (create) | View model, result types, labels, action types. |
| `ts/apps/gateway-console/app/(console)/service-accounts/keys.ts` (create) | Timestamps, dates, key status, expiry. |
| `ts/apps/gateway-console/app/(console)/service-accounts/service-account-id.ts` (create) | Account PRN and id helpers. |
| `ts/apps/gateway-console/app/(console)/service-accounts/model-call-state.ts` (create) | `modelCallState`, fail-closed. |
| `ts/apps/gateway-console/app/(console)/service-accounts/commands.ts` (create) | The five pure commands and their zod shapes. |
| `ts/apps/gateway-console/app/(console)/service-accounts/revalidation.ts` (create) | Which result revalidates. |
| `ts/apps/gateway-console/app/(console)/service-accounts/actions.ts` (create) | The five Server Actions. |
| `ts/apps/gateway-console/app/(console)/service-accounts/load.ts` (create) | The section loader. |
| `ts/apps/gateway-console/app/(console)/service-accounts/block.tsx` (create) | The section frame for both pages. |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts` (create) | Organization page loader and the Projects list. |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/projects-block.tsx` (create) | The Projects section. |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx` (rewrite) | The organization settings page. |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/projects/[project]/load.ts` (create) | Project page loader. |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/projects/[project]/page.tsx` (create) | The project settings page. |
| `ts/apps/gateway-console/app/(console)/overview/page.tsx` (modify) | Adds "Your projects". |
| `ts/apps/gateway-console/app/_components/error-copy.ts` (modify) | `FORM_REASON_COPY`, `formMessage`. |
| `ts/apps/gateway-console/app/_components/form-error.tsx` (create) | Copy of iam-console's `FormError`, with a message override. |
| `ts/apps/gateway-console/app/_components/section-error.tsx` (create) | Copy of iam-console's `SectionError`. |
| `ts/apps/gateway-console/app/_components/confirm-button.tsx` (create) | The two-step confirm control (copy of `ArchiveButton`). |
| `ts/apps/gateway-console/app/_components/use-hydrated.ts` (create) | `useHydrated()`. |
| `ts/apps/gateway-console/app/_components/token-panel.tsx` (create) | `TokenPanel`. |
| `ts/apps/gateway-console/app/_components/pager.tsx` (create) | Pager links with `prefetch={false}`. |
| `ts/apps/gateway-console/app/_components/service-account-section.tsx` (create) | Section, create form, rows, result region. |
| `ts/apps/gateway-console/app/_components/service-account-panel.tsx` (create) | Selected-account panel, issue form, keys, revoke, archive. |
| `ts/apps/gateway-console/app/_components/gateway-state-line.tsx` (create) | The compact gateway state line. |
| `ts/apps/gateway-console/app/_components/your-projects.tsx` (create) | The overview's "Your projects" list. |
| `ts/apps/gateway-console/package.json` (modify) | jsdom and Testing Library dev dependencies. |
| `ts/apps/gateway-console/vitest.config.ts` (modify) | The `next/cache` double. |
| `ts/apps/gateway-console/tests/support/next-cache.ts` (create) | Records `revalidatePath` calls. |
| `ts/apps/gateway-console/tests/support/setup.ts` (modify) | Resets the `next/cache` double. |
| `ts/apps/gateway-console/tests/integration/support.ts` (modify) | `clientsFor`, `callsSince`, `scriptedMayI`, more ids. |
| `ts/apps/gateway-console/tests/e2e/support/world.ts` (rewrite) | The stateful world. |
| `ts/apps/gateway-console/tests/e2e/support/response-scan.ts` (create) | The response recorder of R16. |
| `ts/apps/gateway-console/tests/e2e/*.spec.ts` (create/modify) | R5 extension, R9/R11 re-baseline, R13–R21. |
| `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts` (modify) | Registers R13–R21. |
| `ts/apps/gateway-console/README.md` (modify) | The routes, the rows, the limits. |

Tests created per task are named in each task.

---

### Task 1: `ApiKeyStatus` in the guard-free SDK types entry

An app cannot import `@paigasus/proto`. The gateway loader and the e2e world need the API key
status enum by name (SMA-636 § 5.7). This is SPEC DEVIATION 2.

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/iam/types.ts:1-12` (whole file)
- Modify: `ts/packages/paigasus-sdk/src/index.ts:28-30`
- Test: `ts/packages/paigasus-sdk/tests/iam-types.test.ts` (whole file)

**Interfaces:**
- Consumes: `ApiKeyStatus` from `@paigasus/proto/iam` (generated: `UNSPECIFIED = 0`,
  `ACTIVE = 1`, `REVOKED = 2`).
- Produces: `ApiKeyStatus` from `@paigasus/sdk/iam/types` and from `@paigasus/sdk`, the registry
  enum object itself.

- [ ] **Step 1: Write the failing test**

Replace `ts/packages/paigasus-sdk/tests/iam-types.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-630 spec § 4.5 (D8), SMA-636 spec § 5.7. An app may not import @paigasus/proto, so the
// loaders and the e2e worlds can name a node status or an API key status only through this
// guard-free entry. Each must be the registry enum ITSELF, as a runtime value, or a screen compares
// against a copy that can drift.
import { ApiKeyStatus as ProtoApiKeyStatus, NodeStatus as ProtoNodeStatus } from '@paigasus/proto/iam';
import { describe, expect, it } from 'vitest';

import { ApiKeyStatus, NodeStatus } from '../src/iam/types.js';

describe('the guard-free ./iam/types entry', () => {
  it('re-exports NodeStatus as a runtime value', () => {
    expect(NodeStatus.UNSPECIFIED).toBe(0);
    expect(NodeStatus.ACTIVE).toBe(1);
    expect(NodeStatus.ARCHIVED).toBe(2);
  });

  it('re-exports the registry enum object, not a copy', () => {
    expect(NodeStatus).toBe(ProtoNodeStatus);
  });

  it('re-exports ApiKeyStatus as the registry enum object (SMA-636)', () => {
    expect(ApiKeyStatus.UNSPECIFIED).toBe(0);
    expect(ApiKeyStatus.ACTIVE).toBe(1);
    expect(ApiKeyStatus.REVOKED).toBe(2);
    expect(ApiKeyStatus).toBe(ProtoApiKeyStatus);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/sdk exec vitest run tests/iam-types.test.ts
```

Expected: FAIL in the third case with
`TypeError: Cannot read properties of undefined (reading 'UNSPECIFIED')`.

- [ ] **Step 3: Export the enum**

Replace `ts/packages/paigasus-sdk/src/iam/types.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The guard-free `./iam/types` entry (SMA-630 spec § 4.5, D8; SMA-636 spec § 5.7). This file
// carries NO `import '../server-guard'` and must not gain one: an app's client code and its
// Playwright support files import this entry, and `tests/server-guard.test.ts` lists './iam/types'
// in UNGUARDED_ENTRIES and asserts that the guard is absent.
//
// An app cannot import @paigasus/proto (the eslint boundary `paigasus/boundaries/apps` bans it,
// type imports included). Without these names a screen receives `status: 2` and cannot write the
// name. It is the same trade `./errors/types` makes for ErrorReason: small frozen enum objects in a
// client bundle. ApiKeyStatus serves the gateway zone's key list and its e2e world (SMA-636).
export { ApiKeyStatus, NodeStatus } from '@paigasus/proto/iam';
```

In `ts/packages/paigasus-sdk/src/index.ts`, replace lines 28-30 with:

```ts
// The guard-free ./iam/types entry (SMA-630, SMA-636). tests/index-barrel.test.ts requires every
// subpath export in this barrel.
export { ApiKeyStatus, NodeStatus } from './iam/types';
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/sdk exec vitest run tests/iam-types.test.ts tests/index-barrel.test.ts tests/server-guard.test.ts
pnpm --dir ts --filter @paigasus/sdk run typecheck
```

Expected: PASS. `index-barrel.test.ts` derives its cases from the module keys, so it now also
checks `ApiKeyStatus` in the barrel.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/packages/paigasus-sdk
git commit -F - <<'EOF'
feat(ts): export ApiKeyStatus from the guard-free sdk types entry (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 2: The `serviceAccounts` IAM client and the fake route

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/iam-clients.ts:13`, `:16-22`, `:44-56`
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts:3-5`, `:53`, `:63-69`
- Test: `ts/packages/paigasus-console-core/tests/integration/service-accounts-client.test.ts` (create)

**Interfaces:**
- Consumes: `ServiceAccountService` from `@paigasus/sdk/iam` (src) and from `@paigasus/proto/iam`
  (testing).
- Produces: `IamClients['serviceAccounts']: Client<typeof ServiceAccountService>`.
  `createIamClients()` returns it, with the correlation header. `FakeIamMethod` gains
  `serviceAccounts.createServiceAccount`, `serviceAccounts.getServiceAccount`,
  `serviceAccounts.listServiceAccounts`, `serviceAccounts.archiveServiceAccount`,
  `serviceAccounts.issueApiKey`, `serviceAccounts.revokeApiKey`, `serviceAccounts.listApiKeys`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/integration/service-accounts-client.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 4.5, § 7.1. IamClients carries a serviceAccounts client, and the fake IAM routes
// ServiceAccountService and records each call with its bearer and its correlation id.
//
// An unrouted service also answers Unimplemented, so "rejects" alone proves nothing about routing.
// The proof is the call log: the fake records a call only in its own dispatch.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createIamClients } from '../../src/iam-clients';
import { startFakeIam, type FakeIam } from '../../testing/index';

const OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const SA = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000aa';
const CID = '0198f2c1-8888-7000-8000-000000000636';
const METHODS = ['createServiceAccount', 'getServiceAccount', 'listServiceAccounts', 'archiveServiceAccount', 'issueApiKey', 'revokeApiKey', 'listApiKeys'] as const;

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

describe('the serviceAccounts client', () => {
  it('reaches the fake ServiceAccountService with the bearer and the correlation id', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': (req) => ({ serviceAccount: { prn: SA, ownerPrn: req.ownerPrn, name: req.name, status: 'active' } }),
    });
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-sa', correlationId: CID });

    const created = await clients.serviceAccounts.createServiceAccount({ ownerPrn: OWNER, name: 'ci-bot' });

    expect(created.serviceAccount?.prn).toBe(SA);
    const [call] = iam.callsTo('serviceAccounts.createServiceAccount');
    expect(call?.token).toBe('tok-sa');
    expect(call?.correlationId).toBe(CID);
    expect(call?.request).toMatchObject({ ownerPrn: OWNER, name: 'ci-bot' });
  });

  it('routes all seven RPCs, and an unscripted one answers Unimplemented', async () => {
    iam.setHandlers({});
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-sa' });
    const before = Object.fromEntries(METHODS.map((method) => [method, iam.callsTo(`serviceAccounts.${method}`).length]));

    const results = await Promise.allSettled([
      clients.serviceAccounts.createServiceAccount({ ownerPrn: OWNER, name: 'x' }),
      clients.serviceAccounts.getServiceAccount({ prn: SA }),
      clients.serviceAccounts.listServiceAccounts({ ownerPrn: OWNER, limit: 51, offset: 0n }),
      clients.serviceAccounts.archiveServiceAccount({ prn: SA }),
      clients.serviceAccounts.issueApiKey({ serviceAccountPrn: SA, scopePrn: OWNER }),
      clients.serviceAccounts.revokeApiKey({ id: 'key-1' }),
      clients.serviceAccounts.listApiKeys({ serviceAccountPrn: SA, limit: 51, offset: 0n }),
    ]);

    for (const result of results) {
      expect(result.status).toBe('rejected');
      if (result.status === 'rejected') expect((result.reason as ConnectError).code).toBe(Code.Unimplemented);
    }
    for (const method of METHODS) {
      expect(iam.callsTo(`serviceAccounts.${method}`).length - (before[method] ?? 0)).toBe(1);
    }
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/console-core exec vitest run tests/integration/service-accounts-client.test.ts
```

Expected: FAIL with
`TypeError: Cannot read properties of undefined (reading 'createServiceAccount')`.

- [ ] **Step 3: Add the client**

In `ts/packages/paigasus-console-core/src/iam-clients.ts`, replace line 13 with:

```ts
import { AuditService, AuthnService, AuthorizationService, createIamClient, ServiceAccountService, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
```

Replace lines 16-22 with:

```ts
export type IamClients = {
  tenancy: Client<typeof TenancyService>;
  authn: Client<typeof AuthnService>;
  authz: Client<typeof AuthorizationService>;
  audit: Client<typeof AuditService>;
  serviceInfo: Client<typeof ServiceInfoService>;
  /** Service accounts and their API keys (SMA-636): the gateway zone's settings. */
  serviceAccounts: Client<typeof ServiceAccountService>;
};
```

Replace lines 44-56 with:

```ts
/** The six clients for one bearer token, over one base URL. Pure: tests call it with a fake IAM. */
export function createIamClients(opts: { baseUrl: string; token: string; correlationId?: string | null }): IamClients {
  const transport = { baseUrl: opts.baseUrl };
  const auth = { bearer: opts.token };
  const id = opts.correlationId ?? null;
  return {
    tenancy: withCorrelation(createIamClient(TenancyService, transport, auth), id),
    authn: withCorrelation(createIamClient(AuthnService, transport, auth), id),
    authz: withCorrelation(createIamClient(AuthorizationService, transport, auth), id),
    audit: withCorrelation(createIamClient(AuditService, transport, auth), id),
    serviceInfo: withCorrelation(createIamClient(ServiceInfoService, transport, auth), id),
    serviceAccounts: withCorrelation(createIamClient(ServiceAccountService, transport, auth), id),
  };
}
```

- [ ] **Step 4: Route the service in the fake**

In `ts/packages/paigasus-console-core/testing/fake-iam.ts`, replace lines 3-5 with:

```ts
// An in-process fake of IAM for the integration and e2e tiers (spec § 9.1): a real gRPC server over
// h2c for the six services the console calls (SMA-636 added ServiceAccountService), and a plain
// HTTP server for `GET /v1/service-info`, which @paigasus/discovery probes.
```

Replace line 53 with:

```ts
import { AuditService, AuthnService, AuthorizationService, ServiceAccountService, TenancyService } from '@paigasus/proto/iam';
```

Replace lines 63-69 with:

```ts
const SERVICES = {
  tenancy: TenancyService,
  authn: AuthnService,
  authz: AuthorizationService,
  audit: AuditService,
  serviceInfo: ServiceInfoService,
  // SMA-636. No default answers: an unscripted service-account RPC answers Unimplemented, so a
  // test that forgets a handler fails loudly instead of reading a made-up account.
  serviceAccounts: ServiceAccountService,
} as const;
```

- [ ] **Step 5: Run the test and the package checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/console-core exec vitest run tests/integration/service-accounts-client.test.ts
pnpm --dir ts --filter @paigasus/console-core run typecheck
pnpm --dir ts --filter @paigasus/iam-console run typecheck
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS, 2 tests. The three typechecks exit 0.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/packages/paigasus-console-core
git commit -F - <<'EOF'
feat(ts): add the serviceAccounts IAM client and its fake route (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 3: Five `IAM_ACTIONS` names and the grant-failure event

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/authorize.ts:26-49`
- Modify: `ts/packages/paigasus-console-core/src/logger.ts:14`
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts:47-64`
- Test: `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts` (add one constant, one case)
- Test: `ts/packages/paigasus-console-core/tests/unit/logger.test.ts` (add one case)

**Interfaces:**
- Produces: `IamAction` includes `'CreateServiceAccount' | 'ArchiveServiceAccount' | 'IssueApiKey' |
  'RevokeApiKey' | 'GrantRole'`. It does NOT include `'InvokeModel'` or `'ListRoleGrants'`.
- Produces: `AppEventName` includes `'gateway.sa.grant_failed'`.

- [ ] **Step 1: Write the failing tests**

In `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts`, after line 26 (the
`LIFECYCLE` constant) add:

```ts
// SMA-636 spec § 4.5. mayI() asks about the CURRENT user only, so InvokeModel (asked about a service
// account, through modelCallState) and ListRoleGrants (Root-only for another principal) stay out.
const SERVICE_ACCOUNT = ['CreateServiceAccount', 'ArchiveServiceAccount', 'IssueApiKey', 'RevokeApiKey', 'GrantRole'];
```

Before the last `});` of the same file add:

```ts
  it('holds the five gateway-settings names of SMA-636, and neither InvokeModel nor ListRoleGrants', () => {
    expect(IAM_ACTIONS).toEqual(expect.arrayContaining(SERVICE_ACCOUNT));
    expect([...IAM_ACTIONS]).not.toContain('InvokeModel');
    expect([...IAM_ACTIONS]).not.toContain('ListRoleGrants');
  });
```

In `ts/packages/paigasus-console-core/tests/unit/logger.test.ts`, before the last `});` add:

```ts
  it('writes the gateway zone’s grant failure event (SMA-636 § 5.2)', () => {
    const { logger, parsed } = capture();
    logger.appEvent('gateway.sa.grant_failed', { presentation: 'generic', reason: null, correlation_id: 'cid-1' });
    expect(parsed().map((line) => [line.event, line.fields])).toEqual([['gateway.sa.grant_failed', { presentation: 'generic', reason: null, correlation_id: 'cid-1' }]]);
  });
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/console-core exec vitest run tests/unit/action-names.test.ts
pnpm --dir ts --filter @paigasus/console-core run typecheck
```

Expected: the vitest run FAILS the new case (the array does not contain `'CreateServiceAccount'`).
The typecheck FAILS with `TS2345` in `tests/unit/logger.test.ts`:
`'"gateway.sa.grant_failed"' is not assignable to parameter of type 'AppEventName'`. The logger
case passes at run time, because the logger writes any name; the type is the gate.

- [ ] **Step 3: Add the names and the event**

In `ts/packages/paigasus-console-core/src/authorize.ts`, replace lines 26-49 with:

```ts
/**
 * The PascalCase names IAM's Action::parse accepts (rs/crates/libs/paigasus-iam-core/src/authz/action.rs,
 * `as_wire`). A RUNTIME array, not only a type (SMA-630 spec § 5.1): mayI() fails open, so a
 * misspelt name would show its control for ever. tests/unit/action-names.test.ts holds every entry
 * to the Rust wire names, and the e2e world of the IAM console holds its ALL_ACTIONS to this list.
 *
 * SMA-636 added the five names of the gateway settings. InvokeModel and ListRoleGrants are
 * deliberately ABSENT: mayI() asks about the current user, and neither question is about the user.
 * The gateway zone asks InvokeModel about a service account through its own fail-closed
 * modelCallState, and ListRoleGrants for another principal needs Root.
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
  'CreateServiceAccount',
  'ArchiveServiceAccount',
  'IssueApiKey',
  'RevokeApiKey',
  'GrantRole',
] as const;
```

In `ts/packages/paigasus-console-core/src/logger.ts`, replace line 14 with:

```ts
export type AppEventName =
  | 'principal.resolve_failed'
  | 'principal.resolve_crashed'
  | 'authorize.query_failed'
  | 'authorize.no_principal'
  | 'discovery.redis_connect_failed'
  | 'iam.call_failed'
  // SMA-636 § 5.2: CreateServiceAccount succeeded, and the gateway_user grant after it failed.
  | 'gateway.sa.grant_failed';
```

In `ts/apps/iam-console/tests/e2e/support/world.ts`, replace lines 47-64 with:

```ts
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
  // SMA-636: the gateway settings. This zone asks none of them, but the SET must equal IAM_ACTIONS.
  'CreateServiceAccount',
  'ArchiveServiceAccount',
  'IssueApiKey',
  'RevokeApiKey',
  'GrantRole',
] as const satisfies readonly IamAction[];
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/console-core exec vitest run tests/unit/action-names.test.ts tests/unit/logger.test.ts tests/unit/authorize.test.ts
pnpm --dir ts --filter @paigasus/console-core run typecheck
pnpm --dir ts --filter @paigasus/iam-console exec vitest run tests/unit/world-actions.test.ts
pnpm --dir ts --filter @paigasus/iam-console run typecheck
```

Expected: PASS everywhere. `world-actions.test.ts` holds `ALL_ACTIONS` to the new `IAM_ACTIONS`
set (spec § 7.4).

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/packages/paigasus-console-core ts/apps/iam-console/tests/e2e/support/world.ts
git commit -F - <<'EOF'
feat(ts): add the service-account actions and the grant-failure event (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 4: Move the pure form helpers to `@paigasus/console-core`

D11. The helpers and their tests move. iam-console keeps its rename helpers and imports the moved
names from the new home. No product behaviour changes.

**Files:**
- Create: `ts/packages/paigasus-console-core/src/form.ts`
- Modify: `ts/packages/paigasus-console-core/src/index.ts` (add one line after line 12)
- Modify: `ts/packages/paigasus-console-core/package.json:19-27`
- Modify: `ts/apps/iam-console/lib/form.ts:1-117` (whole file)
- Delete: `ts/apps/iam-console/app/_components/form-action.ts`
- Modify, import lines only (the line numbers are the current ones):
  `ts/apps/iam-console/app/(console)/orgs/commands.ts:8-9`,
  `ts/apps/iam-console/app/(console)/orgs/actions.ts:10-11`,
  `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts:7-8`,
  `ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts:9-10`,
  `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts:7-8`,
  `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts:8-9`,
  `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts:8-9`,
  `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts:9-10`,
  `ts/apps/iam-console/app/(console)/manage-section.tsx:19`,
  `ts/apps/iam-console/app/_components/rename-form.tsx:7`,
  `ts/apps/iam-console/app/_components/manage-controls.tsx:5-7`,
  `ts/apps/iam-console/app/_components/lifecycle-button.tsx:6`,
  `ts/apps/iam-console/tests/unit/manage-section.test.tsx:9`,
  `ts/apps/iam-console/tests/unit/rename-form.test.tsx:14-17`,
  `ts/apps/iam-console/tests/unit/manage-controls.test.tsx:16-19`,
  `ts/apps/iam-console/tests/integration/lifecycle-commands.test.ts:18`
- Test: `ts/packages/paigasus-console-core/tests/unit/form.test.ts` (create)
- Test: `ts/apps/iam-console/tests/unit/form.test.ts` (whole file)

**Interfaces:**
- Produces, from `@paigasus/console-core`:
  - `NAME_MAX_CODE_POINTS = 256`
  - `nameField` (zod string: trimmed, 1 to 256 code points)
  - `prnField` (zod string: trimmed, 1 to 512 characters)
  - `type ActionResult = Exclude<ActionState, null>`
  - `toActionResult(result: IamResult<unknown>): ActionResult`
  - `formFields(form: FormData, names: readonly string[]): Record<string, ReturnType<FormData['get']>>`
  - `invalidFormInput(): PaigasusError`
  - `type FormAction = (previous: ActionState, form: FormData) => Promise<ActionState>`
- iam-console `lib/form.ts` keeps: `slugField`, `currentField`, `RenameFields`, `RenameChange`,
  `renameForm()`, `renameChange()`, `refreshesAfterLifecycleAction()`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/unit/form.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The pure form helpers (SMA-636 D11). They moved here from iam-console's lib/form.ts with these
// tests. The name bound counts CODE POINTS, as IAM's NAME_MAX_CHARS does; zod's `.max()` counts
// UTF-16 units and would refuse a valid name of astral characters (SMA-630 spec § 4.2).
import { describe, expect, it } from 'vitest';
import { NAME_MAX_CODE_POINTS, formFields, invalidFormInput, nameField, prnField, toActionResult } from '../../src/form';

describe('the form helpers', () => {
  it('reads the named fields and nothing else', () => {
    const form = new FormData();
    form.set('slug', 'acme');
    form.set('extra', 'ignored');
    expect(formFields(form, ['slug', 'name'])).toEqual({ slug: 'acme', name: null });
  });

  it('builds a local invalid-input error that claims nothing about IAM', () => {
    const error = invalidFormInput();
    expect(error.presentation).toBe('invalid-input');
    expect(error.domain).toBeNull();
    expect(error.reason).toBeNull();
    expect(error.correlationId).toBeNull();
    expect(error.retryable).toBe(false);
    // A plain object, so it crosses the Flight boundary as an action result.
    expect(structuredClone(error)).toEqual(error);
  });

  it('maps an IamResult to an action result and drops the value', () => {
    expect(toActionResult({ ok: true, value: { anything: 1 } })).toEqual({ ok: true });
    const error = invalidFormInput();
    expect(toActionResult({ ok: false, error })).toEqual({ ok: false, error });
  });
});

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

  it('keeps the PRN bound at 512, trimmed', () => {
    expect(prnField.safeParse('p'.repeat(512)).success).toBe(true);
    expect(prnField.safeParse('p'.repeat(513)).success).toBe(false);
    expect(prnField.safeParse('  ').success).toBe(false);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/console-core exec vitest run tests/unit/form.test.ts
```

Expected: FAIL: vitest cannot load `../../src/form` (the module does not exist).

- [ ] **Step 3: Add `zod` to console-core and create the module**

In `ts/packages/paigasus-console-core/package.json`, replace lines 19-27 with:

```json
  "dependencies": {
    "@bufbuild/protobuf": "catalog:",
    "@connectrpc/connect": "catalog:",
    "@paigasus/auth": "workspace:*",
    "@paigasus/discovery": "workspace:*",
    "@paigasus/sdk": "workspace:*",
    "redis": "catalog:",
    "server-only": "catalog:",
    "zod": "catalog:"
  },
```

Install:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts install
```

Expected: exit 0. `ts/pnpm-lock.yaml` gains `zod` under `packages/paigasus-console-core`.

Create `ts/packages/paigasus-console-core/src/form.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The PURE form helpers of the Server Action shells, shared by both console zones (SMA-636 D11,
// spec § 4.5). They moved here from iam-console's lib/form.ts. zod checks the SHAPE of a form only
// (present, trimmed, bounded). IAM owns every business rule — the name rules, the PRN grammar, who
// may act — and answers with a reason the form copy knows.
//
// The rename helpers (renameForm, renameChange, slugField, currentField,
// refreshesAfterLifecycleAction) stay in iam-console: only that zone renames a node.
//
// A client component imports `FormAction` and `ActionResult` with `import type`, which
// verbatimModuleSyntax erases, so this server-only module never reaches a client bundle.
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { neverReachedIam, type ActionState, type IamResult } from './errors';

/** IAM's NAME_MAX_CHARS (paigasus-iam-core tenancy.rs): 256 Unicode scalar values, not UTF-16 units. */
export const NAME_MAX_CODE_POINTS = 256;

/**
 * A name: of a tenancy node, and of a service account (IAM trims it and allows 1–256 characters,
 * SMA-636 spec § 3.1). The bound counts code points (`[...value].length`), as IAM does. The two
 * trims are not identical: this schema trims as JavaScript does, IAM trims Unicode `White_Space`
 * (SMA-642 spec F2), so a name of only U+0085 passes here and IAM answers `invalid-name`.
 */
export const nameField = z
  .string()
  .trim()
  .min(1)
  .refine((value) => [...value].length <= NAME_MAX_CODE_POINTS);

/** A PRN: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
export const prnField = z.string().trim().min(1).max(512);

/** What a command returns. `null` is only the initial state of a form. */
export type ActionResult = Exclude<ActionState, null>;

export function toActionResult(result: IamResult<unknown>): ActionResult {
  return result.ok ? { ok: true } : { ok: false, error: result.error };
}

/**
 * The named fields of a form, and NOTHING else (SMA-636 § 5.1, "accepted inputs"). A field the
 * action does not name never reaches its zod shape, whatever the client added to the form.
 */
export function formFields(form: FormData, names: readonly string[]): Record<string, ReturnType<FormData['get']>> {
  return Object.fromEntries(names.map((name) => [name, form.get(name)]));
}

/**
 * A form that zod refused never reached IAM (see `neverReachedIam`). `transport` says HTTP 400
 * because the BFF itself refused the request; the field is for logging only, and nothing branches
 * on it (ADR-0019 E8).
 */
export function invalidFormInput(): PaigasusError {
  return neverReachedIam({ presentation: 'invalid-input', message: 'Fill in every field of the form.', transport: { kind: 'http', status: 400 } });
}

/** The shape of a Server Action that a client form posts to. */
export type FormAction = (previous: ActionState, form: FormData) => Promise<ActionState>;
```

In `ts/packages/paigasus-console-core/src/index.ts`, after line 12 (the `./errors` export) add:

```ts
export { NAME_MAX_CODE_POINTS, formFields, invalidFormInput, nameField, prnField, toActionResult, type ActionResult, type FormAction } from './form';
```

- [ ] **Step 4: Run the new test and confirm it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/console-core exec vitest run tests/unit/form.test.ts
pnpm --dir ts --filter @paigasus/console-core run typecheck
```

Expected: PASS, 7 tests. Typecheck exit 0.

- [ ] **Step 5: Keep only the rename helpers in iam-console**

Replace `ts/apps/iam-console/lib/form.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The RENAME pieces of the Server Action shells (spec § 5.3, SMA-630 spec § 4.2). Only this zone
// renames a node, so they stay here. The pure helpers that both zones use — formFields,
// invalidFormInput, toActionResult, ActionResult, nameField, NAME_MAX_CODE_POINTS and prnField —
// moved to @paigasus/console-core (SMA-636 D11). zod checks the SHAPE of a form only; IAM owns
// every business rule and answers with a reason the form copy knows (spec § 6.5).
import 'server-only';
import { z } from 'zod';
import { nameField, prnField, type ActionResult } from '@paigasus/console-core';

/** A slug. IAM refuses more than 64 bytes with `invalid-slug`; this bound only limits the request. */
export const slugField = z.string().trim().min(1).max(200);

/**
 * A hidden "current value" of a rename form. It can be empty, and it can be longer than the name
 * bound, because IAM stored renamed names without a check before SMA-642 and those rows are kept as
 * they are (SMA-642 D4). A stricter schema here would refuse every rename of such a node. It
 * carries no `.max()` of its own: this field is only COMPARED (see `renameForm` and `renameChange`)
 * and never sent to IAM, and a Next Server Action request body is already bounded (1 MB by
 * default), which bounds how large it can arrive.
 */
export const currentField = z.string().trim();

export type RenameFields = { readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };
export type RenameChange = { newSlug?: string; newName?: string };

/**
 * The shared shape of the three rename forms (SMA-630 CR round 1, spec § 4.2): `prn`, `slug`,
 * `name` and the two hidden "current value" fields. IAM holds names longer than 256 code points
 * that it stored before SMA-642 and does not migrate (SMA-642 D4), so `nameField`'s bound applies
 * ONLY when the trimmed name changed. An unchanged name is accepted as it is: it is never sent to
 * IAM either way (`renameChange` omits an unchanged field). A CHANGED name still must pass
 * `nameField`. Do NOT make it unconditional now that IAM validates: that would refuse every
 * slug-only rename of such a node. This is the ONE place that builds a rename schema; each
 * `commands.ts` calls it instead of repeating the shape.
 */
export function renameForm() {
  return z.object({ prn: prnField, slug: slugField, name: z.string().trim(), currentSlug: currentField, currentName: currentField }).superRefine((value, ctx) => {
    if (value.name.trim() === value.currentName.trim()) return;
    const result = nameField.safeParse(value.name);
    if (!result.success) {
      for (const issue of result.error.issues) ctx.addIssue({ ...issue, path: ['name'] });
    }
  });
}

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

Replace `ts/apps/iam-console/tests/unit/form.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The rename helpers that stay in this zone. The shared helpers and their tests moved to
// @paigasus/console-core's tests/unit/form.test.ts (SMA-636 D11).
import { describe, expect, it } from 'vitest';
import type { Presentation } from '@paigasus/sdk/errors/types';
import { invalidFormInput } from '@paigasus/console-core';
import { currentField, refreshesAfterLifecycleAction, renameChange, renameForm, slugField } from '../../lib/form';

describe('the rename field bounds', () => {
  it('keeps the slug bound at 200, trimmed', () => {
    expect(slugField.safeParse('s'.repeat(200)).success).toBe(true);
    expect(slugField.safeParse('s'.repeat(201)).success).toBe(false);
    expect(slugField.safeParse(' acme ').data).toBe('acme');
  });

  it('accepts an empty current value, and a current name longer than the name bound', () => {
    expect(currentField.safeParse('').success).toBe(true);
    expect(currentField.safeParse('a'.repeat(300)).success).toBe(true);
    expect(currentField.safeParse(null).success).toBe(false);
  });

  it('carries no max of its own: a current value can pass 4096 characters (CR round 1)', () => {
    expect(currentField.safeParse('a'.repeat(5000)).success).toBe(true);
  });
});

// SMA-630 CR round 1, spec § 4.2, updated by SMA-642. The `name` bound applies only when the
// trimmed name changed: IAM holds names longer than 256 code points that it stored before SMA-642
// validated the rename path, it does not migrate them (SMA-642 D4), and a slug-only rename of such
// a node must still work.
describe('renameForm (the shared rename schema)', () => {
  const form = renameForm();
  const base = { prn: 'prn:pgs:iam::o:org/o', slug: 'acme', name: 'Acme', currentSlug: 'acme-old', currentName: 'Acme' };
  const longName = 'a'.repeat(300);

  it('parses a slug-only rename of a node whose stored name is 300 code points (unchanged)', () => {
    const result = form.safeParse({ ...base, slug: 'acme-2', name: longName, currentName: longName });
    expect(result.success).toBe(true);
  });

  it('refuses a CHANGED name of 300 code points', () => {
    const result = form.safeParse({ ...base, name: longName, currentName: 'Acme' });
    expect(result.success).toBe(false);
  });

  it('parses a slug-only rename of a node whose stored name is longer than 4096 characters (unchanged)', () => {
    const veryLongName = 'a'.repeat(5000);
    const result = form.safeParse({ ...base, slug: 'acme-2', name: veryLongName, currentName: veryLongName });
    expect(result.success).toBe(true);
  });

  it('still refuses an empty CHANGED name', () => {
    const result = form.safeParse({ ...base, name: '', currentName: 'Acme' });
    expect(result.success).toBe(false);
  });

  it('accepts an unchanged name equal to the bound and refuses a changed one past it', () => {
    expect(form.safeParse({ ...base, name: 'Acme', currentName: 'Acme' }).success).toBe(true);
    expect(form.safeParse({ ...base, slug: 'acme-2', name: 'a'.repeat(257), currentName: 'a'.repeat(257) }).success).toBe(true);
    expect(form.safeParse({ ...base, name: 'a'.repeat(257), currentName: 'Acme' }).success).toBe(false);
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

- [ ] **Step 6: Point every iam-console importer at the new home**

Delete the old type module:

```bash
git rm ts/apps/iam-console/app/_components/form-action.ts
```

Make these exact replacements. Paths are under `ts/apps/iam-console/`. The relative depth to
`lib/form` stays as it is in each file.

1. `app/(console)/orgs/commands.ts` lines 8-9 become:
   ```ts
   import { callIam, nameField, prnField, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
   import { slugField } from '../../../lib/form';
   ```
2. `app/(console)/orgs/actions.ts` lines 10-11 become:
   ```ts
   import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
   ```
3. `app/(console)/orgs/[org]/commands.ts` lines 7-8 become:
   ```ts
   import { callIam, nameField, prnField, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
   import { renameChange, renameForm, slugField } from '../../../../lib/form';
   ```
4. `app/(console)/orgs/[org]/actions.ts` lines 9-10 become:
   ```ts
   import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
   import { refreshesAfterLifecycleAction } from '../../../../lib/form';
   ```
5. `app/(console)/orgs/[org]/teams/[team]/commands.ts` lines 7-8 become:
   ```ts
   import { callIam, nameField, prnField, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
   import { renameChange, renameForm, slugField } from '../../../../../../lib/form';
   ```
6. `app/(console)/orgs/[org]/teams/[team]/actions.ts` lines 8-9 become:
   ```ts
   import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
   import { refreshesAfterLifecycleAction } from '../../../../../../lib/form';
   ```
7. `app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands.ts` lines 8-9 become:
   ```ts
   import { callIam, prnField, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
   import { renameChange, renameForm } from '../../../../../../../../lib/form';
   ```
8. `app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions.ts` lines 9-10 become:
   ```ts
   import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
   import { refreshesAfterLifecycleAction } from '../../../../../../../../lib/form';
   ```
9. `app/(console)/manage-section.tsx` line 19 becomes:
   ```ts
   import type { FormAction } from '@paigasus/console-core';
   ```
10. `app/_components/rename-form.tsx` line 7 becomes:
    ```ts
    import type { FormAction } from '@paigasus/console-core';
    ```
11. `app/_components/manage-controls.tsx` lines 5-7 become:
    ```ts
    import type { ActionState, FormAction } from '@paigasus/console-core';
    import type { PaigasusError } from '@paigasus/sdk/errors/types';
    ```
12. `app/_components/lifecycle-button.tsx` line 6 becomes:
    ```ts
    import type { FormAction } from '@paigasus/console-core';
    ```
13. `tests/unit/manage-section.test.tsx` line 9 becomes:
    ```ts
    import type { FormAction } from '@paigasus/console-core';
    ```
14. `tests/unit/rename-form.test.tsx` lines 14-17 become:
    ```ts
    import type { ActionState, FormAction } from '@paigasus/console-core';
    import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
    import { FORM_REASON_COPY } from '../../app/_components/error-copy';
    ```
15. `tests/unit/manage-controls.test.tsx` lines 16-19 become:
    ```ts
    import type { ActionState, FormAction } from '@paigasus/console-core';
    import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
    import { FORM_REASON_COPY } from '../../app/_components/error-copy';
    ```
16. `tests/integration/lifecycle-commands.test.ts` line 18 becomes:
    ```ts
    import type { ActionResult } from '@paigasus/console-core';
    import { renameChange } from '../../lib/form';
    ```

Prove that nothing names the deleted module:

```bash
grep -rn "form-action" ts/apps/iam-console/app ts/apps/iam-console/tests
```

Expected: no output.

- [ ] **Step 7: Run iam-console's checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/iam-console run typecheck
pnpm --dir ts --filter @paigasus/iam-console exec vitest run tests/unit/form.test.ts tests/unit/manage-controls.test.tsx tests/unit/rename-form.test.tsx tests/unit/manage-section.test.tsx tests/unit/lifecycle-button.test.tsx tests/unit/actions-revalidate.test.ts tests/unit/actions-structure.test.ts tests/unit/action-session.test.ts tests/integration/lifecycle-commands.test.ts tests/integration/orgs-commands.test.ts tests/integration/membership-commands.test.ts
pnpm --dir ts exec eslint apps/iam-console packages/paigasus-console-core
```

Expected: every command exits 0. No product behaviour changed (spec § 4.5).

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/packages/paigasus-console-core ts/apps/iam-console ts/pnpm-lock.yaml
git commit -F - <<'EOF'
feat(ts): move the pure form helpers to console-core (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 5: Move the button classes to `@paigasus/ui`

**Files:**
- Create: `ts/packages/paigasus-ui/src/lib/button-class.ts`
- Modify: `ts/packages/paigasus-ui/src/index.ts` (add one line after line 16)
- Delete: `ts/apps/iam-console/app/_components/button-class.ts`
- Modify: `ts/apps/iam-console/app/_components/rename-form.tsx:5-6`,
  `ts/apps/iam-console/app/_components/create-form.tsx:5-8`,
  `ts/apps/iam-console/app/_components/membership-form.tsx:5-7`,
  `ts/apps/iam-console/app/_components/lifecycle-button.tsx:5`
- Test: `ts/packages/paigasus-ui/tests/button-class.test.ts` (create)

**Interfaces:**
- Produces, from `@paigasus/ui`: `PRIMARY_BUTTON_CLASS: string` and `SECONDARY_BUTTON_CLASS: string`,
  with the exact class strings iam-console used.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-ui/tests/button-class.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 D11: the two button classes the console forms share live here. The exact strings are the
// ones iam-console used, so moving them changes no rendered class.
import { describe, expect, it } from 'vitest';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '../src/index';

describe('the shared button classes', () => {
  it('are exported from the package entry with the classes iam-console used', () => {
    expect(PRIMARY_BUTTON_CLASS).toBe('bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50');
    expect(SECONDARY_BUTTON_CLASS).toBe('border-input rounded-pgs self-start border px-3 py-1.5 text-sm font-medium disabled:opacity-50');
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/ui exec vitest run tests/button-class.test.ts
```

Expected: FAIL: `expected undefined to be 'bg-primary …'`.

- [ ] **Step 3: Create the module and export it**

Create `ts/packages/paigasus-ui/src/lib/button-class.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The two button classes the console forms share (SMA-636 D11, spec § 4.5). They moved here from
// iam-console's app/_components/button-class.ts, so the gateway zone uses the same two. Plain
// constants, not a component and not a client module: a server or a client component may import
// them. They live in src/, so every consumer's Tailwind `@source` line for this package scans them.

/** The submit button of every form: create, rename, add member, restore, and a confirmation. */
export const PRIMARY_BUTTON_CLASS = 'bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50';

/** The plain, non-primary button: the first step of a two-step confirm, and "Cancel" during it. */
export const SECONDARY_BUTTON_CLASS = 'border-input rounded-pgs self-start border px-3 py-1.5 text-sm font-medium disabled:opacity-50';
```

In `ts/packages/paigasus-ui/src/index.ts`, after line 16 (`export { cn } from './lib/cn';`) add:

```ts
export { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from './lib/button-class';
```

- [ ] **Step 4: Point iam-console at the package**

```bash
git rm ts/apps/iam-console/app/_components/button-class.ts
```

Under `ts/apps/iam-console/`:

1. `app/_components/rename-form.tsx` lines 5-6 become:
   ```ts
   import { Field, Input, PRIMARY_BUTTON_CLASS } from '@paigasus/ui';
   ```
2. `app/_components/create-form.tsx` lines 5-8 become:
   ```ts
   import { Field, Input, PRIMARY_BUTTON_CLASS } from '@paigasus/ui';
   // A TYPE import: verbatimModuleSyntax erases it, so no server-only module reaches the client bundle.
   import type { ActionState } from '@paigasus/console-core';
   ```
3. `app/_components/membership-form.tsx` lines 5-7 become:
   ```ts
   import { Field, Input, PRIMARY_BUTTON_CLASS } from '@paigasus/ui';
   import type { ActionState } from '@paigasus/console-core';
   ```
4. `app/_components/lifecycle-button.tsx` line 5 becomes:
   ```ts
   import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
   ```

- [ ] **Step 5: Run the checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/ui exec vitest run tests/button-class.test.ts tests/no-next-imports.test.ts
pnpm --dir ts --filter @paigasus/ui run typecheck
pnpm --dir ts --filter @paigasus/iam-console run typecheck
pnpm --dir ts --filter @paigasus/iam-console exec vitest run tests/unit/rename-form.test.tsx tests/unit/lifecycle-button.test.tsx tests/unit/manage-controls.test.tsx
grep -rn "button-class" ts/apps/iam-console/app ts/apps/iam-console/tests
```

Expected: tests PASS, typechecks exit 0, and `grep` prints nothing.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/packages/paigasus-ui ts/apps/iam-console
git commit -F - <<'EOF'
feat(ts): move the shared button classes to @paigasus/ui (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 6: `prefetch` on `ZoneLink`

SPEC DEVIATION 1. After hydration the Next router prefetches every visible same-zone link, and
each prefetch is a render (spec § 3.4). A list of project and pager links must opt out.

**Files:**
- Modify: `ts/packages/paigasus-app-shell/src/zone/zone-link.tsx:10-15`, `:37`, `:60-68`
- Modify: `ts/packages/paigasus-app-shell/tests/support/next-link-double.tsx:12-30`
- Test: `ts/packages/paigasus-app-shell/tests/zone/zone-link.test.tsx` (add three cases)

**Interfaces:**
- Produces: `ZoneLinkProps` gains `prefetch?: boolean`. Only the same-zone `NextLink` branch
  receives it. The plain `<a>` branches never receive it.

- [ ] **Step 1: Make the double show the prop, and write the failing tests**

Replace lines 12-30 (the props type and the whole function) of
`ts/packages/paigasus-app-shell/tests/support/next-link-double.tsx` with:

```tsx
type NextLinkDoubleProps = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { href: string; prefetch?: boolean; ref?: Ref<HTMLAnchorElement> };

/**
 * `prefetch` is written as `data-prefetch` (SMA-636), so a test can see what ZoneLink passed. The
 * attribute is absent when ZoneLink passed nothing.
 */
export default function NextLinkDouble({ href, onClick, prefetch, children, ...rest }: NextLinkDoubleProps): ReactElement {
  return (
    <a
      {...rest}
      data-next-link=""
      data-prefetch={prefetch === undefined ? undefined : String(prefetch)}
      href={href}
      onClick={(event) => {
        onClick?.(event);
        // Real next/link does not navigate when the consumer already prevented the default.
        if (!event.defaultPrevented) nextLinkClicks.push(href);
        event.preventDefault();
      }}
    >
      {children}
    </a>
  );
}
```

In `ts/packages/paigasus-app-shell/tests/zone/zone-link.test.tsx`, inside
`describe('ZoneLink (spec § 6.4)', …)`, after the "cross zone: a plain <a> …" case add:

```tsx
  it('same zone: passes prefetch={false} to next/link (SMA-636 spec § 4.1)', () => {
    render(
      inZone(
        <ZoneLink href="/iam/users" prefetch={false}>
          Users
        </ZoneLink>,
      ),
    );
    expect(screen.getByRole('link', { name: 'Users' })).toHaveAttribute('data-prefetch', 'false');
  });

  it('same zone: leaves prefetch unset when the caller gives none, so Next keeps its default', () => {
    render(inZone(<ZoneLink href="/iam/users">Users</ZoneLink>));
    expect(screen.getByRole('link', { name: 'Users' })).not.toHaveAttribute('data-prefetch');
  });

  it('cross zone: never writes prefetch onto the plain <a>', () => {
    render(
      inZone(
        <ZoneLink href="/gateway/usage" prefetch={false}>
          Usage
        </ZoneLink>,
      ),
    );
    const link = screen.getByRole('link', { name: 'Usage' });
    expect(link).not.toHaveAttribute('prefetch');
    expect(link).not.toHaveAttribute('data-prefetch');
  });
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/app-shell exec vitest run tests/zone/zone-link.test.tsx
```

Expected: FAIL. In the first new case the link has no `data-prefetch`. In the third new case the
`<a>` carries `prefetch="false"`, because ZoneLink spreads it with the anchor props.

- [ ] **Step 3: Add the prop**

In `ts/packages/paigasus-app-shell/src/zone/zone-link.tsx`, replace lines 10-15 with:

```tsx
/**
 * A superset of @paigasus/ui's LinkProps, so ZoneLink is assignable to LinkComponent and an app can
 * inject it: <LinkProvider link={ZoneLink}> (spec § 6.4). There is no `replace` or `scroll`: Next's
 * defaults apply, and nothing consumes them yet.
 *
 * `prefetch` reaches next/link on the SAME-ZONE branch only (SMA-636 spec § 4.1). After hydration
 * the router prefetches every visible same-zone link, and each prefetch is a server render, so a
 * long list of links passes `prefetch={false}`. A cross-zone or auth link is a plain <a>, which
 * never prefetches, so it never receives the prop.
 */
export type ZoneLinkProps = LinkProps & Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { ref?: Ref<HTMLAnchorElement>; prefetch?: boolean };
```

Replace line 37 with:

```tsx
export function ZoneLink({ href, children, onClick, onMouseEnter, onTouchStart, prefetch, ...anchorProps }: ZoneLinkProps): ReactElement | null {
```

Replace the `NextLink` element (lines 60-68) with:

```tsx
      <NextLink
        {...anchorProps}
        {...(onClick !== undefined ? { onClick } : {})}
        {...(onMouseEnter !== undefined ? { onMouseEnter } : {})}
        {...(onTouchStart !== undefined ? { onTouchStart } : {})}
        {...(prefetch !== undefined ? { prefetch } : {})}
        href={target.rest}
      >
        {children}
      </NextLink>
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/app-shell exec vitest run tests/zone/zone-link.test.tsx tests/shell/breadcrumbs.test.tsx tests/structure/public-surface.test.tsx
pnpm --dir ts --filter @paigasus/app-shell run typecheck
```

Expected: PASS. Typecheck exit 0.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/packages/paigasus-app-shell
git commit -F - <<'EOF'
feat(ts): let a same-zone ZoneLink opt out of prefetch (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 7: Gateway paging, the bounded fan-out, and the pager

D15 and § 4.2. SPEC DEVIATION 3: limit+1 is new here, not a copy of iam-console.

**Files:**
- Create: `ts/apps/gateway-console/lib/paging.ts`
- Create: `ts/apps/gateway-console/lib/concurrency.ts`
- Create: `ts/apps/gateway-console/app/_components/pager.tsx`
- Test: `ts/apps/gateway-console/tests/unit/paging.test.ts` (create)
- Test: `ts/apps/gateway-console/tests/unit/concurrency.test.ts` (create)
- Test: `ts/apps/gateway-console/tests/unit/pager.test.tsx` (create)

**Interfaces:**
- Produces (`lib/paging.ts`, no `server-only`, client-safe):
  - `PAGE_SIZE = 50`, `REQUEST_LIMIT = 51`
  - `parseOffset(raw: string | readonly string[] | undefined): number`
  - `type Page<T> = { readonly rows: readonly T[]; readonly offset: number; readonly nextOffset: number | null }`
  - `pageOf<T>(received: readonly T[], offset: number): Page<T>`
  - `linkHref(path: string, query: Readonly<Record<string, string | number | null>>): string`
- Produces (`lib/concurrency.ts`): `mapWithLimit<T, R>(items: readonly T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]>`
- Produces (`app/_components/pager.tsx`): `Pager(props: PagerProps): ReactElement | null` with
  `PagerProps = { label; path; param; offset; nextOffset; keep?: Readonly<Record<string, string | number | null>> }`.

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/paging.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The gateway settings paging (SMA-636 D15). A list asks IAM for PAGE_SIZE + 1 rows and shows
// PAGE_SIZE, so a page of EXACTLY PAGE_SIZE rows shows no "Next" and no "more exist" note.
import { describe, expect, it } from 'vitest';
import { PAGE_SIZE, REQUEST_LIMIT, linkHref, pageOf, parseOffset } from '../../lib/paging';

const items = (count: number): number[] => Array.from({ length: count }, (_, index) => index);

describe('parseOffset', () => {
  it('reads a plain non-negative integer', () => {
    expect(parseOffset('0')).toBe(0);
    expect(parseOffset('50')).toBe(50);
    expect(parseOffset(['100', '150'])).toBe(100);
  });

  it('reads anything else as 0, because a hand-edited query string is not worth an error page', () => {
    for (const raw of [undefined, '', '-50', '5.5', '1e3', 'abc', ' 50', '9999999999']) {
      expect(parseOffset(raw)).toBe(0);
    }
  });
});

describe('pageOf', () => {
  it('asks IAM for one row more than it shows', () => {
    expect(PAGE_SIZE).toBe(50);
    expect(REQUEST_LIMIT).toBe(51);
  });

  it('shows exactly 50 rows and no next page when IAM sent exactly 50', () => {
    const page = pageOf(items(50), 0);
    expect(page.rows).toHaveLength(50);
    expect(page.nextOffset).toBeNull();
  });

  it('shows 50 rows and a next page when IAM sent 51', () => {
    const page = pageOf(items(51), 100);
    expect(page.rows).toEqual(items(50));
    expect(page.offset).toBe(100);
    expect(page.nextOffset).toBe(150);
  });

  it('has no next page for an empty answer', () => {
    expect(pageOf([], 0)).toEqual({ rows: [], offset: 0, nextOffset: null });
  });
});

describe('linkHref', () => {
  it('drops a default value (0, empty text, null) and keeps the order of the rest', () => {
    expect(linkHref('/gateway/orgs/o', { saOffset: 0, sa: null, keyOffset: 50 })).toBe('/gateway/orgs/o?keyOffset=50');
    expect(linkHref('/gateway/orgs/o', { sa: 'abc', saOffset: 50 })).toBe('/gateway/orgs/o?sa=abc&saOffset=50');
  });

  it('is the bare path when nothing is left', () => {
    expect(linkHref('/gateway/orgs/o', { saOffset: 0, sa: '' })).toBe('/gateway/orgs/o');
  });

  it('lets the parameter a pager sets override a kept entry of the same name', () => {
    const keep = { saOffset: 50, sa: 'abc' };
    const param: string = 'saOffset';
    expect(linkHref('/p', { ...keep, [param]: 100 })).toBe('/p?saOffset=100&sa=abc');
  });
});
```

Create `ts/apps/gateway-console/tests/unit/concurrency.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// mapWithLimit (SMA-636 spec § 4.2): the organization page lists projects per team with at most
// 8 calls in flight, and keeps the order of the teams.
import { describe, expect, it } from 'vitest';
import { mapWithLimit } from '../../lib/concurrency';

const later = <T>(value: T, ms: number): Promise<T> => new Promise((resolve) => setTimeout(() => resolve(value), ms));
const items = (count: number): number[] => Array.from({ length: count }, (_, index) => index);

describe('mapWithLimit', () => {
  it('keeps the input order in the results, whatever order the calls finish in', async () => {
    expect(await mapWithLimit([30, 10, 20], 2, (ms) => later(ms * 10, ms))).toEqual([300, 100, 200]);
  });

  it('never has more than the limit in flight, and reaches the limit when there is work for it', async () => {
    let inFlight = 0;
    let peak = 0;
    await mapWithLimit(items(20), 8, async () => {
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await later(null, 5);
      inFlight -= 1;
    });
    expect(peak).toBe(8);
  });

  it('answers an empty list with no call', async () => {
    let calls = 0;
    const results = await mapWithLimit([], 8, () => {
      calls += 1;
      return Promise.resolve(null);
    });
    expect(results).toEqual([]);
    expect(calls).toBe(0);
  });

  it('rejects a limit below 1', async () => {
    await expect(mapWithLimit([1], 0, (n) => Promise.resolve(n))).rejects.toThrow('limit');
  });
});
```

Create `ts/apps/gateway-console/tests/unit/pager.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The settings pager (SMA-636 D15, § 4.1): full-path links through ZoneLink, the other list's
// parameters kept, and NO prefetch. The next/link double writes `prefetch` as `data-prefetch`.
import type { ReactElement, ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { Pager } from '../../app/_components/pager';

vi.mock('next/link', () => ({
  default: ({ href, prefetch, children }: { href: string; prefetch?: boolean; children?: ReactNode }) => (
    <a href={href} data-prefetch={String(prefetch)}>
      {children}
    </a>
  ),
}));

function render(element: ReactElement): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      {element}
    </ZoneProvider>,
  );
}

describe('Pager', () => {
  it('renders nothing on a single page', () => {
    expect(render(<Pager label="Pages" path="/gateway/orgs/o" param="saOffset" offset={0} nextOffset={null} />)).toBe('');
  });

  it('links both ways, keeps the other parameters, and never prefetches', () => {
    const html = render(<Pager label="Pages" path="/gateway/orgs/o" param="saOffset" offset={50} nextOffset={100} keep={{ sa: 'id-1', keyOffset: 0 }} />);
    // ZoneLink hands next/link the zone-relative remainder (Next adds the base path back).
    expect(html).toContain('href="/orgs/o?sa=id-1"');
    expect(html).toContain('href="/orgs/o?sa=id-1&amp;saOffset=100"');
    expect(html.match(/data-prefetch="false"/g)).toHaveLength(2);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/paging.test.ts tests/unit/concurrency.test.ts tests/unit/pager.test.tsx
```

Expected: FAIL, three files: vitest cannot load `../../lib/paging`, `../../lib/concurrency` and
`../../app/_components/pager`.

- [ ] **Step 3: Implement the three modules**

Create `ts/apps/gateway-console/lib/paging.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Offset paging for the gateway settings lists (SMA-636 D15). It follows the pattern of
// ts/apps/iam-console/lib/paging.ts with ONE change: a list asks IAM for PAGE_SIZE + 1 rows and
// shows PAGE_SIZE. So the page knows whether more rows exist, and a page of exactly PAGE_SIZE rows
// shows no "Next". IAM's list responses carry no total. A missing or malformed offset reads as 0.
//
// A plain module with no `server-only` import: the pager, which the client section renders, builds
// its links here.

/** The rows a list shows. IAM's server maximum is 200. */
export const PAGE_SIZE = 50;

/** The rows a list asks IAM for: one more than it shows. */
export const REQUEST_LIMIT = PAGE_SIZE + 1;

const MAX_OFFSET_DIGITS = 9;

function first(raw: string | readonly string[] | undefined): string | undefined {
  return typeof raw === 'string' ? raw : raw?.[0];
}

export function parseOffset(raw: string | readonly string[] | undefined): number {
  const value = first(raw);
  if (value === undefined || value.length > MAX_OFFSET_DIGITS || !/^\d+$/.test(value)) return 0;
  return Number(value);
}

export type Page<T> = { readonly rows: readonly T[]; readonly offset: number; readonly nextOffset: number | null };

/** One page from an answer to a REQUEST_LIMIT request. */
export function pageOf<T>(received: readonly T[], offset: number): Page<T> {
  return { rows: received.slice(0, PAGE_SIZE), offset, nextOffset: received.length > PAGE_SIZE ? offset + PAGE_SIZE : null };
}

/**
 * `path` plus a query of the given entries, in their order. An entry of 0, '' or null is a
 * default and is left out, so the first page of a list has no offset in its URL.
 */
export function linkHref(path: string, query: Readonly<Record<string, string | number | null>>): string {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === null || value === 0 || value === '') continue;
    params.set(name, String(value));
  }
  const text = params.toString();
  return text === '' ? path : `${path}?${text}`;
}
```

Create `ts/apps/gateway-console/lib/concurrency.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A bounded fan-out (SMA-636 spec § 4.2): the organization page lists the projects of each team
// with at most 8 calls in flight. The results keep the input order.

export async function mapWithLimit<T, R>(items: readonly T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  if (!Number.isInteger(limit) || limit < 1) throw new RangeError('mapWithLimit: the limit must be a positive integer');
  const results = new Array<R>(items.length);
  let next = 0;
  const worker = async (): Promise<void> => {
    while (next < items.length) {
      const index = next;
      next += 1;
      results[index] = await fn(items[index] as T);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, () => worker()));
  return results;
}
```

Create `ts/apps/gateway-console/app/_components/pager.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// Offset paging links (SMA-636 D15). It follows iam-console's app/_components/pager.tsx with two
// changes: every link has prefetch={false} (spec § 4.1), and `keep` may carry the selected account
// id as well as the other list's offset. No 'use client' and no server-only: the client section
// renders it, and ZoneLink is a client component. Hrefs are full paths (/gateway/…).
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { PAGE_SIZE, linkHref } from '../../lib/paging';

export type PagerProps = {
  readonly label: string;
  readonly path: string;
  readonly param: string;
  readonly offset: number;
  readonly nextOffset: number | null;
  /** The other query parameters of the page, kept in every link. */
  readonly keep?: Readonly<Record<string, string | number | null>>;
};

export function Pager({ label, path, param, offset, nextOffset, keep = {} }: PagerProps): ReactElement | null {
  if (offset === 0 && nextOffset === null) return null;
  const previous = Math.max(0, offset - PAGE_SIZE);
  return (
    <nav aria-label={label} className="flex gap-4 text-sm">
      {offset > 0 ? (
        <ZoneLink prefetch={false} href={linkHref(path, { ...keep, [param]: previous })} className="hover:underline">
          Previous
        </ZoneLink>
      ) : null}
      {nextOffset === null ? null : (
        <ZoneLink prefetch={false} href={linkHref(path, { ...keep, [param]: nextOffset })} className="hover:underline">
          Next
        </ZoneLink>
      )}
    </nav>
  );
}
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/paging.test.ts tests/unit/concurrency.test.ts tests/unit/pager.test.tsx
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS. Typecheck exit 0.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add limit+1 paging and a bounded fan-out to the gateway zone (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 8: The section's view model, key facts, account ids and node status

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/node-status.ts` (copy)
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/view.ts`
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/keys.ts`
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/service-account-id.ts`
- Test: `ts/apps/gateway-console/tests/unit/node-status.test.ts` (copy)
- Test: `ts/apps/gateway-console/tests/unit/keys.test.ts` (create)
- Test: `ts/apps/gateway-console/tests/unit/service-account-id.test.ts` (create)

**Interfaces:**
- Produces (`node-status.ts`, a copy of iam-console's): `NodeState`, `NodeLifecycle`,
  `LifecycleView`, `lifecycleOf(node)`, `lifecycleView(lifecycle)`, `BADGE_LABEL`, `COLUMN_LABEL`,
  `statusColumnLabel()`, `PARENT_ARCHIVED_NOTE`.
- Produces (`view.ts`, client-safe): `ModelCallState`, `KeyStatus`, `KEY_STATUS_LABEL`,
  `EXPIRY_CHOICES`, `ExpiryChoice`, `EXPIRY_LABEL`, `CreateState`, `IssueKeyState`, `PageLinks`,
  `ServiceAccountRowView`, `ApiKeyRowView`, `KeysView`, `PanelControls`, `SelectedView`,
  `ReadOnlyView`, `SectionOk`, `SectionView`, `OwnerKind`, `CreateAction`, `IssueKeyAction`,
  `ServiceAccountActions`, `SimpleControl`, `SectionResult` (exact shapes in Step 3).
- Produces (`keys.ts`): `timestampMs(value?: { seconds: bigint; nanos: number }): number | null`,
  `formatDate(ms: number | null): string | null`,
  `keyStatus(key: { status: ApiKeyStatus; expiresAtMs: number | null }, accountActive: boolean, nowMs: number): KeyStatus`,
  `expiresAtFor(choice: ExpiryChoice, nowMs: number): { seconds: bigint; nanos: number } | undefined`.
- Produces (`service-account-id.ts`, client-safe): `serviceAccountPrn(id: string): string`,
  `serviceAccountIdOf(prn: string): string | null`,
  `parseAccountParam(raw: string | readonly string[] | undefined): string | null`.

- [ ] **Step 1: Copy the node-status module and its test**

```bash
cp "ts/apps/iam-console/app/(console)/node-status.ts" "ts/apps/gateway-console/app/(console)/node-status.ts"
cp ts/apps/iam-console/tests/unit/node-status.test.ts ts/apps/gateway-console/tests/unit/node-status.test.ts
```

In the gateway copy of `node-status.ts`, replace lines 1-12 (the header, up to the line before
`import { NodeStatus } …`) with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// COPY of ts/apps/iam-console/app/(console)/node-status.ts (SMA-636 D11, spec § 9). Nothing gates a
// divergence between the two copies. The gateway settings read an owner node's lifecycle here: the
// service-accounts section is read-only unless the owner is active (spec § 5.8).
//
// IAM sends two NodeStatus values per node: `status` (the node itself) and `effectiveStatus`
// (archived when the node OR an ancestor is archived). UNSPECIFIED, and any value that this build
// does not know, map to 'unknown'.
//
// A plain module with no `server-only` import. Its only import is @paigasus/sdk's guard-free
// ./iam/types entry, so a client component can read the note from here.
```

In the gateway copy of `node-status.test.ts`, replace line 3-4 (its header prose) with:

```ts
// The copy of iam-console's node-status module in this zone (SMA-636 § 9): all nine
// (own × effective) combinations, a status value this build does not know, and the label tables.
```

- [ ] **Step 2: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/keys.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// API key facts as the screens show them (SMA-636 spec § 5.4, § 5.7). The clock is injected, so
// every case is exact.
import { describe, expect, it } from 'vitest';
import { ApiKeyStatus } from '@paigasus/sdk/iam/types';
import { expiresAtFor, formatDate, keyStatus, timestampMs } from '../../app/(console)/service-accounts/keys';
import { EXPIRY_CHOICES, EXPIRY_LABEL, KEY_STATUS_LABEL } from '../../app/(console)/service-accounts/view';

const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;

describe('keyStatus (§ 5.7, in this order)', () => {
  it('makes every key inactive when the account is not active, whatever IAM says', () => {
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: null }, false, NOW)).toBe('inactive');
    expect(keyStatus({ status: ApiKeyStatus.REVOKED, expiresAtMs: NOW - DAY }, false, NOW)).toBe('inactive');
  });

  it('reads a revoked key as revoked, also when it expired', () => {
    expect(keyStatus({ status: ApiKeyStatus.REVOKED, expiresAtMs: NOW - DAY }, true, NOW)).toBe('revoked');
  });

  it('reads a key past its expiry as expired, and the expiry instant itself as past', () => {
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: NOW - 1 }, true, NOW)).toBe('expired');
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: NOW }, true, NOW)).toBe('expired');
  });

  it('reads a key with no expiry or a future expiry as active', () => {
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: null }, true, NOW)).toBe('active');
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: NOW + 1 }, true, NOW)).toBe('active');
  });

  it('reads a status this build does not know as active: IAM checks every call itself', () => {
    expect(keyStatus({ status: 9 as ApiKeyStatus, expiresAtMs: null }, true, NOW)).toBe('active');
  });
});

describe('expiresAtFor (§ 5.4, D7)', () => {
  it('sends no expires_at for the IAM default', () => {
    expect(expiresAtFor('default', NOW)).toBeUndefined();
  });

  it.each([
    ['30', 30],
    ['90', 90],
    ['365', 365],
  ] as const)('computes %s days from the injected clock, in whole seconds', (choice, days) => {
    expect(expiresAtFor(choice, NOW + 999)).toEqual({ seconds: BigInt(Math.floor((NOW + 999 + days * DAY) / 1000)), nanos: 0 });
  });
});

describe('timestamps and dates', () => {
  it('reads a Timestamp as milliseconds, and an unset one as null', () => {
    expect(timestampMs({ seconds: 1_788_000_000n, nanos: 500_000_000 })).toBe(1_788_000_000_500);
    expect(timestampMs(undefined)).toBeNull();
  });

  it('formats a date as YYYY-MM-DD in UTC, so the server and the client never disagree', () => {
    expect(formatDate(NOW)).toBe('2026-09-18');
    expect(formatDate(null)).toBeNull();
  });
});

describe('the labels', () => {
  it('names each key status', () => {
    expect(KEY_STATUS_LABEL).toEqual({ active: 'Active', revoked: 'Revoked', expired: 'Expired', inactive: 'Inactive (account archived)' });
  });

  it('offers exactly the four expiry choices of D7, with the default label of § 5.4', () => {
    expect([...EXPIRY_CHOICES]).toEqual(['default', '30', '90', '365']);
    expect(EXPIRY_LABEL.default).toBe('IAM default (the key may not expire)');
  });
});
```

Create `ts/apps/gateway-console/tests/unit/service-account-id.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A service account is a principal: IAM names it `prn:pgs:iam:::principal/<uuid>`
// (paigasus-iam-core value.rs). The `sa` search parameter carries the UUID only (SMA-636 D14).
import { describe, expect, it } from 'vitest';
import { parseAccountParam, serviceAccountIdOf, serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';

const ID = '0190a1e5-0000-7000-8000-0000000000a1';

describe('service account ids', () => {
  it('builds the principal PRN, lower case', () => {
    expect(serviceAccountPrn(ID.toUpperCase())).toBe(`prn:pgs:iam:::principal/${ID}`);
  });

  it('reads the id back, lower case, and refuses anything else', () => {
    expect(serviceAccountIdOf(`prn:pgs:iam:::principal/${ID.toUpperCase()}`)).toBe(ID);
    expect(serviceAccountIdOf(`prn:pgs:iam:::organization/${ID}`)).toBeNull();
    expect(serviceAccountIdOf('prn:pgs:iam:::principal/not-a-uuid')).toBeNull();
    expect(serviceAccountIdOf('')).toBeNull();
  });

  it('reads ?sa= as a UUID or as no selection, never as an error', () => {
    expect(parseAccountParam(ID)).toBe(ID);
    expect(parseAccountParam([ID.toUpperCase(), 'x'])).toBe(ID);
    for (const raw of [undefined, '', 'abc', `${ID} `]) expect(parseAccountParam(raw)).toBeNull();
  });
});
```

- [ ] **Step 3: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/keys.test.ts tests/unit/service-account-id.test.ts tests/unit/node-status.test.ts
```

Expected: `keys.test.ts` and `service-account-id.test.ts` FAIL (the modules do not exist).
`node-status.test.ts` PASSES already: it tests a verbatim copy.

- [ ] **Step 4: Implement the three modules**

Create `ts/apps/gateway-console/app/(console)/service-accounts/view.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The view model of the service-accounts section (SMA-636 spec § 4.2, § 4.6), the result types of
// its actions (§ 5.2, § 5.4) and their labels. PLAIN DATA ONLY: a server loader builds it and a
// client component receives it, so it never holds a proto message. No `server-only` import and no
// runtime import: a client component may import it. The type imports are erased.
import type { ActionState, FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

/** Spec § 4.3. */
export type ModelCallState = 'yes' | 'no' | 'archived' | 'unknown';

/** Spec § 5.7. */
export type KeyStatus = 'active' | 'revoked' | 'expired' | 'inactive';

export const KEY_STATUS_LABEL: Readonly<Record<KeyStatus, string>> = {
  active: 'Active',
  revoked: 'Revoked',
  expired: 'Expired',
  inactive: 'Inactive (account archived)',
};

/** D7: the IAM default or 30, 90 or 365 days. The form sends the choice; the server computes the date. */
export const EXPIRY_CHOICES = ['default', '30', '90', '365'] as const;
export type ExpiryChoice = (typeof EXPIRY_CHOICES)[number];

export const EXPIRY_LABEL: Readonly<Record<ExpiryChoice, string>> = {
  default: 'IAM default (the key may not expire)',
  '30': '30 days',
  '90': '90 days',
  '365': '365 days',
};

/** § 5.2. `granted: false` means `iam.authz.cedar` is absent. */
export type CreateState =
  | { readonly kind: 'created'; readonly saPrn: string; readonly granted: boolean }
  | { readonly kind: 'partial'; readonly saPrn: string; readonly error: PaigasusError }
  | { readonly kind: 'failed'; readonly error: PaigasusError }
  | null;

/** § 5.4. The token is in this value only (rule 1). */
export type IssueKeyState = { readonly ok: true; readonly token: string; readonly prefix: string } | { readonly ok: false; readonly error: PaigasusError } | null;

export type PageLinks = { readonly offset: number; readonly nextOffset: number | null };

export type ServiceAccountRowView = {
  readonly prn: string;
  /** The UUID of the PRN, or null when the PRN is not a principal PRN (then the row has no Select link). */
  readonly id: string | null;
  readonly name: string;
  /** YYYY-MM-DD, or null. */
  readonly created: string | null;
  readonly active: boolean;
};

export type ApiKeyRowView = {
  readonly id: string;
  readonly prefix: string;
  readonly status: KeyStatus;
  readonly created: string | null;
  /** Null: the key never expires. */
  readonly expires: string | null;
  /** Null: never used. */
  readonly lastUsed: string | null;
  /** The key's scope when it is NOT the owner node (a key made outside the console), else null. */
  readonly otherScope: string | null;
};

export type KeysView = { readonly kind: 'hidden' } | { readonly kind: 'error'; readonly error: PaigasusError } | { readonly kind: 'ok'; readonly rows: readonly ApiKeyRowView[]; readonly page: PageLinks };

/** Which panel controls show. The loader already combined mayI(), the capabilities and both lifecycles. */
export type PanelControls = { readonly allow: boolean; readonly issue: boolean; readonly revoke: boolean; readonly archive: boolean };

export type SelectedView =
  | { readonly kind: 'none' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | { readonly kind: 'other-scope' }
  | { readonly kind: 'ok'; readonly account: ServiceAccountRowView; readonly modelCalls: ModelCallState; readonly keys: KeysView; readonly controls: PanelControls };

/** § 5.8. Null: the owner node is active and the section is writable. */
export type ReadOnlyView = 'archived' | 'archived-parent' | 'unknown' | null;

export type SectionOk = {
  readonly kind: 'ok';
  readonly ownerPrn: string;
  readonly readOnly: ReadOnlyView;
  readonly rows: readonly ServiceAccountRowView[];
  readonly page: PageLinks;
  readonly canCreate: boolean;
  readonly selected: SelectedView;
  /** The `sa` search parameter, kept in the pager links. */
  readonly sa: string | null;
  readonly saOffset: number;
  readonly keyOffset: number;
};

export type SectionView = { readonly kind: 'denied' } | { readonly kind: 'error'; readonly error: PaigasusError } | SectionOk;

export type OwnerKind = 'organization' | 'project';

export type CreateAction = (previous: CreateState, form: FormData) => Promise<CreateState>;

/** § 5.4 rule 2: the client calls it DIRECTLY, with `null` as the previous state. */
export type IssueKeyAction = (previous: null, form: FormData) => Promise<IssueKeyState>;

export type ServiceAccountActions = {
  readonly create: CreateAction;
  readonly allow: FormAction;
  readonly issue: IssueKeyAction;
  readonly revoke: FormAction;
  readonly archive: FormAction;
};

export type SimpleControl = 'allow' | 'revoke' | 'archive' | 'issue';

/** The ONE result region of a section (§ 4.6): the last result of any of its actions. */
export type SectionResult = { readonly control: 'create'; readonly state: Exclude<CreateState, null> } | { readonly control: SimpleControl; readonly state: Exclude<ActionState, null> } | null;
```

Create `ts/apps/gateway-console/app/(console)/service-accounts/keys.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// API key facts as the screens show them (SMA-636 spec § 5.4, § 5.7). Pure, with an injected
// clock. The loader and the issue command use it on the server. Its only runtime import is
// @paigasus/sdk's guard-free ./iam/types entry.
import { ApiKeyStatus } from '@paigasus/sdk/iam/types';
import type { ExpiryChoice, KeyStatus } from './view';

const DAY_MS = 86_400_000;

const EXPIRY_DAYS: Readonly<Record<Exclude<ExpiryChoice, 'default'>, number>> = { '30': 30, '90': 90, '365': 365 };

/** A protobuf Timestamp as milliseconds, or null when it is not set. */
export function timestampMs(value: { readonly seconds: bigint; readonly nanos: number } | undefined): number | null {
  if (value === undefined) return null;
  return Number(value.seconds) * 1000 + Math.floor(value.nanos / 1_000_000);
}

/** YYYY-MM-DD in UTC, or null. The server formats every date, so a client render never differs. */
export function formatDate(ms: number | null): string | null {
  return ms === null ? null : new Date(ms).toISOString().slice(0, 10);
}

/** § 5.7, in this order: account not active, revoked, expired, else active. */
export function keyStatus(key: { readonly status: ApiKeyStatus; readonly expiresAtMs: number | null }, accountActive: boolean, nowMs: number): KeyStatus {
  if (!accountActive) return 'inactive';
  if (key.status === ApiKeyStatus.REVOKED) return 'revoked';
  if (key.expiresAtMs !== null && key.expiresAtMs <= nowMs) return 'expired';
  return 'active';
}

/**
 * The `expires_at` of IssueApiKey (§ 5.4). Unset for 'default': IAM then applies its
 * `default_expiry_days`, and with none set the key never expires. Else the clock plus N days, in
 * whole seconds. IAM enforces no maximum, so the console invents none (D7).
 */
export function expiresAtFor(choice: ExpiryChoice, nowMs: number): { seconds: bigint; nanos: number } | undefined {
  if (choice === 'default') return undefined;
  return { seconds: BigInt(Math.floor((nowMs + EXPIRY_DAYS[choice] * DAY_MS) / 1000)), nanos: 0 };
}
```

Create `ts/apps/gateway-console/app/(console)/service-accounts/service-account-id.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A service account is a principal, and IAM names it `prn:pgs:iam:::principal/<uuid>`
// (rs/crates/libs/paigasus-iam-core/src/value.rs). The `sa` search parameter carries the UUID only
// (SMA-636 D14). Client-safe: no import at all, so the result region can build a select link.
//
// The file name is deliberate: a base name `prn` is a Windows reserved device name (CLAUDE.md).

const PRINCIPAL_PREFIX = 'prn:pgs:iam:::principal/';
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function serviceAccountPrn(id: string): string {
  return `${PRINCIPAL_PREFIX}${id.toLowerCase()}`;
}

/** The UUID of a principal PRN, lower case, or null for any other text. */
export function serviceAccountIdOf(prn: string): string | null {
  if (!prn.startsWith(PRINCIPAL_PREFIX)) return null;
  const id = prn.slice(PRINCIPAL_PREFIX.length);
  return UUID_RE.test(id) ? id.toLowerCase() : null;
}

/** `?sa=`: a UUID selects one account. Any other value is ignored, in the parseOffset pattern (§ 4.1). */
export function parseAccountParam(raw: string | readonly string[] | undefined): string | null {
  const value = typeof raw === 'string' ? raw : raw?.[0];
  return value !== undefined && UUID_RE.test(value) ? value.toLowerCase() : null;
}
```

- [ ] **Step 5: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/keys.test.ts tests/unit/service-account-id.test.ts tests/unit/node-status.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS. Typecheck exit 0.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the service-account view model and key facts (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 9: `modelCallState`

§ 4.3 and D8. It fails CLOSED. It is not `mayI()`.

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/model-call-state.ts`
- Test: `ts/apps/gateway-console/tests/unit/model-call-state.test.ts` (create)

**Interfaces:**
- Consumes: `callIam`, `IamClients` from `@paigasus/console-core`; `ModelCallState` from `./view`.
- Produces: `modelCallState(authz: Pick<IamClients['authz'], 'isAuthorized'>, saPrn: string, ownerPrn: string, status: string): Promise<ModelCallState>`.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/gateway-console/tests/unit/model-call-state.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// modelCallState (SMA-636 spec § 4.3, D8). It is NOT mayI(): mayI() fails open and asks about the
// current user. This asks IAM about a SERVICE ACCOUNT, and it FAILS CLOSED: a failed call never
// reads as "yes". No Cedar policy reads the principal's status, so IAM answers `allowed` for an
// archived account; that is why a status other than `active` never asks.
import { describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { denial } from '@paigasus/console-core/testing';
import { modelCallState } from '../../app/(console)/service-accounts/model-call-state';

const SA = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000a1';
const OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';

function authz(answer: boolean | Error) {
  const asked: unknown[] = [];
  const client = {
    isAuthorized: (request: unknown) => {
      asked.push(request);
      return answer instanceof Error ? Promise.reject(answer) : Promise.resolve({ allowed: answer, determiningPolicies: [], reason: '' });
    },
  };
  return { asked, client: client as never };
}

describe('modelCallState', () => {
  it('answers yes when IAM allows InvokeModel for the account at its owner node', async () => {
    const { asked, client } = authz(true);
    expect(await modelCallState(client, SA, OWNER, 'active')).toBe('yes');
    expect(asked).toEqual([{ principalPrn: SA, action: 'InvokeModel', resourcePrn: OWNER }]);
  });

  it('answers no when IAM denies it', async () => {
    const { client } = authz(false);
    expect(await modelCallState(client, SA, OWNER, 'active')).toBe('no');
  });

  it.each(['disabled', 'suspended', ''])('answers archived for status %j, and asks IAM nothing', async (status) => {
    const { asked, client } = authz(true);
    expect(await modelCallState(client, SA, OWNER, status)).toBe('archived');
    expect(asked).toEqual([]);
  });

  it('answers unknown, never yes, when the call fails — a refusal of the question included', async () => {
    for (const failure of [denial({ code: Code.PermissionDenied, reason: 'forbidden' }), new ConnectError('down', Code.Unavailable)]) {
      const { client } = authz(failure);
      expect(await modelCallState(client, SA, OWNER, 'active')).toBe('unknown');
    }
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/model-call-state.test.ts
```

Expected: FAIL: vitest cannot load `model-call-state` (the module does not exist).

- [ ] **Step 3: Implement it**

Create `ts/apps/gateway-console/app/(console)/service-accounts/model-call-state.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// "Can this service account call models?" (SMA-636 spec § 4.3, D8). It asks IAM's IsAuthorized
// about the ACCOUNT, not about the current user. IAM allows that question only to a caller that
// holds ListRoleGrants at the resource, which every *_admin role holds (spec § 3.2).
//
// IT FAILS CLOSED. Any failed call — `forbidden` included — answers `unknown`, never `yes`. It is
// NOT mayI(), which fails open and asks about the current user.
//
// A status other than `active` answers `archived` with NO call: no Cedar policy reads the
// principal's status, so IAM would answer `allowed` for an archived account that still holds
// gateway_user (spec § 3.2).
import 'server-only';
import { callIam, type IamClients } from '@paigasus/console-core';
import type { ModelCallState } from './view';

export async function modelCallState(authz: Pick<IamClients['authz'], 'isAuthorized'>, saPrn: string, ownerPrn: string, status: string): Promise<ModelCallState> {
  if (status !== 'active') return 'archived';
  const answer = await callIam(() => authz.isAuthorized({ principalPrn: saPrn, action: 'InvokeModel', resourcePrn: ownerPrn }));
  if (!answer.ok) return 'unknown';
  return answer.value.allowed ? 'yes' : 'no';
}
```

- [ ] **Step 4: Run the test and confirm it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/model-call-state.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS, 6 tests. Typecheck exit 0.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the fail-closed model-call state of a service account (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 10: The five commands

§ 5.1-5.6 and D6. Pure and dependency-injected. The integration support file gains the helpers
that Tasks 10-12 use.

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/commands.ts`
- Modify: `ts/apps/gateway-console/tests/integration/support.ts:14-25` and append at the end
- Test: `ts/apps/gateway-console/tests/integration/service-account-commands.test.ts` (create)

**Interfaces:**
- Consumes: `callIam`, `nameField`, `neverReachedIam`, `prnField`, `toActionResult`,
  `ActionResult`, `ConsoleLogger`, `IamClients`, `IamResult` from `@paigasus/console-core`;
  `expiresAtFor` from `./keys`; `EXPIRY_CHOICES`, `CreateState`, `IssueKeyState` from `./view`.
- Produces:
  - `GATEWAY_ROLE = 'gateway_user'`
  - zod shapes `createServiceAccountForm` (`ownerPrn`, `name`), `serviceAccountForm` (`saPrn`),
    `issueApiKeyForm` (`saPrn`, `expiry`), `revokeApiKeyForm` (`saPrn`, `keyId`), and their
    `z.infer` types `CreateServiceAccountInput`, `ServiceAccountInput`, `IssueApiKeyInput`,
    `RevokeApiKeyInput`
  - `createServiceAccount(deps: { serviceAccounts: Pick<SA, 'createServiceAccount'>; authz: Pick<Authz, 'grantRole'>; cedar: boolean; logger: Pick<ConsoleLogger, 'appEvent'> }, input: CreateServiceAccountInput): Promise<Exclude<CreateState, null>>`
  - `allowModelCalls(deps: { serviceAccounts: Pick<SA, 'getServiceAccount'>; authz: Pick<Authz, 'grantRole'> }, input: ServiceAccountInput): Promise<ActionResult>`
  - `issueApiKey(deps: { serviceAccounts: Pick<SA, 'getServiceAccount' | 'issueApiKey'>; now: () => number }, input: IssueApiKeyInput): Promise<Exclude<IssueKeyState, null>>`
  - `revokeApiKey(deps: { serviceAccounts: Pick<SA, 'revokeApiKey'> }, input: RevokeApiKeyInput): Promise<ActionResult>`
  - `archiveServiceAccount(deps: { serviceAccounts: Pick<SA, 'archiveServiceAccount'> }, input: ServiceAccountInput): Promise<ActionResult>`
  - (`SA` is `IamClients['serviceAccounts']`, `Authz` is `IamClients['authz']`.)
- Produces (test support): `IDS.teamA1`, `IDS.projectA1`, `IDS.saA`, `IDS.saB`;
  `clientsFor(iam: FakeIam, token?: string): IamClients`;
  `callsSince(iam: FakeIam): (method: FakeIamMethod | 'http.getServiceInfo') => FakeIamCall[]`;
  `scriptedMayI(allowed: Partial<Record<IamAction, boolean>>): ScriptedMayI`.

- [ ] **Step 1: Extend the integration support**

In `ts/apps/gateway-console/tests/integration/support.ts`, replace lines 14-25 (the imports and
`IDS`) with:

```ts
import { randomUUID } from 'node:crypto';
import { SESSION_COOKIE, type SessionRecord } from '@paigasus/auth/server';
import { createIamClients, type IamAction, type IamClients, type MayI } from '@paigasus/console-core';
import { startFakeGateway, startFakeIam, type FakeGateway, type FakeIam, type FakeIamCall, type FakeIamMethod } from '@paigasus/console-core/testing';
import { authRuntime } from '../../lib/auth';
import { setRequestCookies } from '../support/next-headers';
import { stubConsoleEnv } from '../support/env';

/** Fixed UUIDs so a test's expectations read as data, not as `expect.any(String)`. */
export const IDS = {
  orgA: '0190a100-0000-7000-8000-00000000000a',
  orgB: '0190a100-0000-7000-8000-00000000000b',
  teamA1: '0190a1b2-0000-7000-8000-0000000000a1',
  projectA1: '0190a1c3-0000-7000-8000-0000000000a1',
  saA: '0190a1e5-0000-7000-8000-0000000000a1',
  saB: '0190a1e5-0000-7000-8000-0000000000b1',
} as const;
```

At the end of the same file, append:

```ts
/** The six IAM clients for `token` over the fake's gRPC address: the factory the app itself uses. */
export function clientsFor(iam: FakeIam, token = 'tok-integration'): IamClients {
  return createIamClients({ baseUrl: iam.grpcUrl, token });
}

/** The calls the fake saw from now on, by method. The fake's log is shared by every test in a file. */
export function callsSince(iam: FakeIam): (method: FakeIamMethod | 'http.getServiceInfo') => FakeIamCall[] {
  const start = iam.calls.length;
  return (method) => iam.calls.slice(start).filter((call) => call.method === method);
}

export type ScriptedMayI = MayI & { readonly asked: readonly (readonly [IamAction, string])[] };

/**
 * A MayI that answers from a table (absent = false) and records every question. The pattern of
 * iam-console's tests/integration/support.ts. It makes no IAM call, so a test's isAuthorized log
 * holds only the questions the code under test asked about a SERVICE ACCOUNT.
 */
export function scriptedMayI(allowed: Partial<Record<IamAction, boolean>>): ScriptedMayI {
  const asked: (readonly [IamAction, string])[] = [];
  const mayI = (action: IamAction, resourcePrn: string): Promise<boolean> => {
    asked.push([action, resourcePrn]);
    return Promise.resolve(allowed[action] ?? false);
  };
  return Object.assign(mayI, { asked });
}
```

- [ ] **Step 2: Write the failing test**

Create `ts/apps/gateway-console/tests/integration/service-account-commands.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The five commands of the gateway settings (SMA-636 spec § 5, § 7.1) against the fake IAM. A
// command takes no mayI: IAM decides. D6: a grant's scope and a key's scope come from IAM — from
// CreateServiceAccount's response, or from GetServiceAccount — never from an input.
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { organizationPrn, projectPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { GATEWAY_ROLE, allowModelCalls, archiveServiceAccount, createServiceAccount, issueApiKey, revokeApiKey } from '../../app/(console)/service-accounts/commands';
import { serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';
import { IDS, callsSince, clientsFor } from './support';

const OWNER = organizationPrn(IDS.orgA);
/** The same node as OWNER, as a client could send it: IAM answers with the canonical lower-case PRN. */
const OWNER_AS_SENT = `prn:pgs:iam:::organization/${IDS.orgA.toUpperCase()}`;
const PROJECT = projectPrn(IDS.orgA, IDS.projectA1);
const SA = serviceAccountPrn(IDS.saA);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;
const TOKEN = 'pgs_int_0123456789abcdef0123456789abcdef';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function storedAccount(ownerPrn: string) {
  return { serviceAccount: { prn: SA, ownerPrn, name: 'ci-bot', status: 'active' } };
}

describe('createServiceAccount (§ 5.2)', () => {
  it('creates, then grants gateway_user at the owner PRN of IAM’s RESPONSE', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': (req) => ({ serviceAccount: { prn: SA, ownerPrn: req.ownerPrn.toLowerCase(), name: req.name, status: 'active' } }),
      'authz.grantRole': (req) => ({ grant: { id: 'g-1', principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }),
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);
    const appEvent = vi.fn();

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: true, logger: { appEvent } }, { ownerPrn: OWNER_AS_SENT, name: 'ci-bot' });

    expect(result).toEqual({ kind: 'created', saPrn: SA, granted: true });
    expect(calls('serviceAccounts.createServiceAccount')[0]?.request).toMatchObject({ ownerPrn: OWNER_AS_SENT, name: 'ci-bot' });
    const grants = calls('authz.grantRole');
    expect(grants).toHaveLength(1);
    expect(grants[0]?.request).toMatchObject({ principalPrn: SA, roleKey: 'gateway_user', scopePrn: OWNER });
    expect(GATEWAY_ROLE).toBe('gateway_user');
    expect(appEvent).not.toHaveBeenCalled();
  });

  it('answers partial and logs gateway.sa.grant_failed, without IAM’s message, when the grant fails', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': () => storedAccount(OWNER),
      'authz.grantRole': () => {
        throw new ConnectError('grant exploded with internal detail', Code.Internal);
      },
    });
    const clients = clientsFor(iam);
    const appEvent = vi.fn();

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: true, logger: { appEvent } }, { ownerPrn: OWNER, name: 'ci-bot' });

    expect(result.kind).toBe('partial');
    if (result.kind !== 'partial') throw new Error('expected partial');
    expect(result.saPrn).toBe(SA);
    expect(result.error.presentation).toBe('generic');
    expect(appEvent).toHaveBeenCalledTimes(1);
    expect(appEvent).toHaveBeenCalledWith('gateway.sa.grant_failed', expect.objectContaining({ presentation: 'generic' }));
    expect(JSON.stringify(appEvent.mock.calls)).not.toContain('internal detail');
  });

  it('answers failed with the name-conflict reason, and makes no grant', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': () => {
        throw denial({ code: Code.AlreadyExists, reason: 'service-account-name-conflict' });
      },
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: true, logger: { appEvent: vi.fn() } }, { ownerPrn: OWNER, name: 'ci-bot' });

    expect(result.kind).toBe('failed');
    if (result.kind !== 'failed') throw new Error('expected failed');
    expect(result.error.presentation).toBe('conflict');
    expect(result.error.reason).toBe(ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT);
    expect(calls('authz.grantRole')).toHaveLength(0);
  });

  it('makes no grant when IAM does not offer role administration (iam.authz.cedar absent)', async () => {
    iam.setHandlers({ 'serviceAccounts.createServiceAccount': () => storedAccount(OWNER) });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: false, logger: { appEvent: vi.fn() } }, { ownerPrn: OWNER, name: 'ci-bot' });

    expect(result).toEqual({ kind: 'created', saPrn: SA, granted: false });
    expect(calls('authz.grantRole')).toHaveLength(0);
  });
});

describe('allowModelCalls (§ 5.3)', () => {
  it('grants gateway_user at the owner that GetServiceAccount names', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(PROJECT),
      'authz.grantRole': (req) => ({ grant: { id: 'g-2', principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }),
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    expect(await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA })).toEqual({ ok: true });
    expect(calls('serviceAccounts.getServiceAccount')[0]?.request).toMatchObject({ prn: SA });
    expect(calls('authz.grantRole')[0]?.request).toMatchObject({ principalPrn: SA, roleKey: 'gateway_user', scopePrn: PROJECT });
  });

  it('makes no grant when IAM refuses to show the account', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => {
        throw denial();
      },
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA });

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error('expected a refusal');
    expect(result.error.presentation).toBe('forbidden');
    expect(calls('authz.grantRole')).toHaveLength(0);
  });

  it('returns a duplicate grant as IAM answers it: an internal error (spec § 3.2)', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(OWNER),
      'authz.grantRole': () => {
        throw new ConnectError('duplicate', Code.Internal);
      },
    });
    const clients = clientsFor(iam);

    const result = await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA });

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error('expected a refusal');
    expect(result.error.presentation).toBe('generic');
  });
});

describe('issueApiKey (§ 5.4)', () => {
  it.each([
    ['default', null],
    ['30', 30],
    ['90', 90],
    ['365', 365],
  ] as const)('sends expiry %s from the injected clock, the owner scope from IAM, and empty scope lists', async (expiry, days) => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(PROJECT),
      'serviceAccounts.issueApiKey': (req) => ({ apiKey: { id: 'k-1', serviceAccountPrn: req.serviceAccountPrn, scopePrn: req.scopePrn, prefix: 'pgs_int_0123' }, token: TOKEN }),
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await issueApiKey({ serviceAccounts: clients.serviceAccounts, now: () => NOW }, { saPrn: SA, expiry });

    expect(result).toEqual({ ok: true, token: TOKEN, prefix: 'pgs_int_0123' });
    const request = calls('serviceAccounts.issueApiKey')[0]?.request as { expiresAt?: unknown; scopeActions: string[]; scopeRoles: string[] };
    expect(request).toMatchObject({ serviceAccountPrn: SA, scopePrn: PROJECT, scopeActions: [], scopeRoles: [] });
    if (days === null) expect(request.expiresAt).toBeUndefined();
    else expect(request.expiresAt).toMatchObject({ seconds: BigInt(Math.floor((NOW + days * DAY) / 1000)), nanos: 0 });
  });

  it('issues nothing when GetServiceAccount fails', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => {
        throw denial({ code: Code.NotFound, reason: 'not-found' });
      },
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await issueApiKey({ serviceAccounts: clients.serviceAccounts, now: () => NOW }, { saPrn: SA, expiry: '30' });

    expect(result.ok).toBe(false);
    expect(calls('serviceAccounts.issueApiKey')).toHaveLength(0);
  });
});

describe('revokeApiKey and archiveServiceAccount (§ 5.5, § 5.6)', () => {
  it('revokes the key by id', async () => {
    iam.setHandlers({ 'serviceAccounts.revokeApiKey': () => ({}) });
    const calls = callsSince(iam);

    expect(await revokeApiKey({ serviceAccounts: clientsFor(iam).serviceAccounts }, { saPrn: SA, keyId: 'k-1' })).toEqual({ ok: true });
    expect(calls('serviceAccounts.revokeApiKey')[0]?.request).toMatchObject({ id: 'k-1' });
  });

  it('returns IAM’s refusal of a revoke', async () => {
    iam.setHandlers({
      'serviceAccounts.revokeApiKey': () => {
        throw denial();
      },
    });

    const result = await revokeApiKey({ serviceAccounts: clientsFor(iam).serviceAccounts }, { saPrn: SA, keyId: 'k-1' });

    expect(result.ok).toBe(false);
  });

  it('archives the account by PRN', async () => {
    iam.setHandlers({ 'serviceAccounts.archiveServiceAccount': () => ({}) });
    const calls = callsSince(iam);

    expect(await archiveServiceAccount({ serviceAccounts: clientsFor(iam).serviceAccounts }, { saPrn: SA })).toEqual({ ok: true });
    expect(calls('serviceAccounts.archiveServiceAccount')[0]?.request).toMatchObject({ prn: SA });
  });
});
```

- [ ] **Step 3: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/integration/service-account-commands.test.ts
```

Expected: FAIL: vitest cannot load `commands` (the module does not exist).

- [ ] **Step 4: Implement the commands**

Create `ts/apps/gateway-console/app/(console)/service-accounts/commands.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands of the gateway settings (SMA-636 spec § 5). Pure and dependency-injected: the
// actions pass in the clients, the clock and the logger. They take NO mayI: IAM decides (§ 5.1).
//
// D6. A grant's scope and a key's scope are ALWAYS the account's owner node, read from IAM on the
// server: from CreateServiceAccount's response in the create flow, and from GetServiceAccount in
// the other two. No input below carries a scope, so a form field cannot choose one.
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, nameField, neverReachedIam, prnField, toActionResult, type ActionResult, type ConsoleLogger, type IamClients, type IamResult } from '@paigasus/console-core';
import { expiresAtFor } from './keys';
import { EXPIRY_CHOICES, type CreateState, type IssueKeyState } from './view';

/** The role a console-made account gets at its owner node (spec § 1.2). It holds only InvokeModel. */
export const GATEWAY_ROLE = 'gateway_user';

type ServiceAccounts = IamClients['serviceAccounts'];
type Authz = IamClients['authz'];
type AccountMessage = NonNullable<Awaited<ReturnType<ServiceAccounts['getServiceAccount']>>['serviceAccount']>;

/** `ownerPrn` is under client control, and that is safe: IAM checks CreateServiceAccount at that node (§ 5.1). */
export const createServiceAccountForm = z.object({ ownerPrn: prnField, name: nameField });
export type CreateServiceAccountInput = z.infer<typeof createServiceAccountForm>;

export const serviceAccountForm = z.object({ saPrn: prnField });
export type ServiceAccountInput = z.infer<typeof serviceAccountForm>;

export const issueApiKeyForm = z.object({ saPrn: prnField, expiry: z.enum(EXPIRY_CHOICES) });
export type IssueApiKeyInput = z.infer<typeof issueApiKeyForm>;

/** A key id is IAM's; this bound only limits the request. `saPrn` only keeps the panel selected (§ 5.5). */
export const revokeApiKeyForm = z.object({ saPrn: prnField, keyId: z.string().trim().min(1).max(128) });
export type RevokeApiKeyInput = z.infer<typeof revokeApiKeyForm>;

/**
 * IAM answered a successful call with no account. That is version skew, not a state IAM produces.
 * It is reported as not-found, and no grant and no key follow. `neverReachedIam` only fills the
 * fields that carry no IAM data.
 */
function missingAccount(): PaigasusError {
  return neverReachedIam({ presentation: 'not-found', message: 'IAM returned no service account.', transport: { kind: 'grpc', code: 5, codeName: 'NotFound' } });
}

/** GetServiceAccount: the account as IAM stores it, for the owner PRN that D6 takes from IAM. */
async function storedAccount(serviceAccounts: Pick<ServiceAccounts, 'getServiceAccount'>, saPrn: string): Promise<IamResult<AccountMessage>> {
  const got = await callIam(() => serviceAccounts.getServiceAccount({ prn: saPrn }));
  if (!got.ok) return got;
  const account = got.value.serviceAccount;
  return account === undefined ? { ok: false, error: missingAccount() } : { ok: true, value: account };
}

/**
 * § 5.2. Two RPCs, not one atomic RPC (D3): CreateServiceAccount, then — when IAM offers role
 * administration (D13) — GrantRole(account, gateway_user, owner). A failed grant leaves an account
 * that cannot call models; the answer is `partial`, and the screen offers the repair control.
 */
export async function createServiceAccount(
  deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'createServiceAccount'>; readonly authz: Pick<Authz, 'grantRole'>; readonly cedar: boolean; readonly logger: Pick<ConsoleLogger, 'appEvent'> },
  input: CreateServiceAccountInput,
): Promise<Exclude<CreateState, null>> {
  const created = await callIam(() => deps.serviceAccounts.createServiceAccount({ ownerPrn: input.ownerPrn, name: input.name }));
  if (!created.ok) return { kind: 'failed', error: created.error };
  const account = created.value.serviceAccount;
  if (account === undefined) return { kind: 'failed', error: missingAccount() };
  if (!deps.cedar) return { kind: 'created', saPrn: account.prn, granted: false };
  const granted = await callIam(() => deps.authz.grantRole({ principalPrn: account.prn, roleKey: GATEWAY_ROLE, scopePrn: account.ownerPrn }));
  if (granted.ok) return { kind: 'created', saPrn: account.prn, granted: true };
  // Scalars only, and never IAM's message (the logger's redaction contract).
  deps.logger.appEvent('gateway.sa.grant_failed', { presentation: granted.error.presentation, reason: granted.error.rawReason, correlation_id: granted.error.correlationId });
  return { kind: 'partial', saPrn: account.prn, error: granted.error };
}

/** § 5.3: GetServiceAccount, then GrantRole(account, gateway_user, owner_prn). */
export async function allowModelCalls(deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'getServiceAccount'>; readonly authz: Pick<Authz, 'grantRole'> }, input: ServiceAccountInput): Promise<ActionResult> {
  const account = await storedAccount(deps.serviceAccounts, input.saPrn);
  if (!account.ok) return { ok: false, error: account.error };
  return toActionResult(await callIam(() => deps.authz.grantRole({ principalPrn: account.value.prn, roleKey: GATEWAY_ROLE, scopePrn: account.value.ownerPrn })));
}

/**
 * § 5.4: GetServiceAccount, then IssueApiKey(account, scope = owner_prn, expires_at). The scope
 * lists stay empty (IAM stores them and does not enforce them in v1). The token is in the result
 * ONLY: this function logs nothing, and callIam logs only a FAILED call, which carries no token.
 */
export async function issueApiKey(deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'getServiceAccount' | 'issueApiKey'>; readonly now: () => number }, input: IssueApiKeyInput): Promise<Exclude<IssueKeyState, null>> {
  const account = await storedAccount(deps.serviceAccounts, input.saPrn);
  if (!account.ok) return { ok: false, error: account.error };
  const expiresAt = expiresAtFor(input.expiry, deps.now());
  const issued = await callIam(() =>
    deps.serviceAccounts.issueApiKey({ serviceAccountPrn: account.value.prn, scopePrn: account.value.ownerPrn, ...(expiresAt === undefined ? {} : { expiresAt }), scopeActions: [], scopeRoles: [] }),
  );
  if (!issued.ok) return { ok: false, error: issued.error };
  return { ok: true, token: issued.value.token, prefix: issued.value.apiKey?.prefix ?? '' };
}

/** § 5.5. IAM authorizes the revoke at the owner node of the key's own account. */
export async function revokeApiKey(deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'revokeApiKey'> }, input: RevokeApiKeyInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.serviceAccounts.revokeApiKey({ id: input.keyId })));
}

/** § 5.6. IAM disables the principal and evicts its keys from its API-key cache. No restore RPC exists. */
export async function archiveServiceAccount(deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'archiveServiceAccount'> }, input: ServiceAccountInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.serviceAccounts.archiveServiceAccount({ prn: input.saPrn })));
}
```

- [ ] **Step 5: Run the test and confirm it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/integration/service-account-commands.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS, 15 tests. Typecheck exit 0.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the service-account commands of the gateway zone (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 11: The service-accounts section loader

§ 4.2 (the section), § 4.3, § 5.7, § 5.8, D13, D14, D15. SPEC DEVIATION 4 (keys of an archived
account).

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/load.ts`
- Test: `ts/apps/gateway-console/tests/integration/service-account-section.test.ts` (create)

**Interfaces:**
- Consumes: `callIam`, `cedarCapabilityOf`, `IamClients`, `MayI` from `@paigasus/console-core`;
  `REQUEST_LIMIT`, `pageOf` from `lib/paging`; `lifecycleView`, `NodeLifecycle` from
  `../node-status`; `formatDate`, `keyStatus`, `timestampMs` from `./keys`; `modelCallState` from
  `./model-call-state`; `serviceAccountIdOf`, `serviceAccountPrn` from `./service-account-id`; the
  view types from `./view`.
- Produces:
  - `API_KEYS_CAPABILITY = 'iam.apikeys'`
  - `apiKeysCapabilityOf(state: ServiceState): boolean`
  - `type SectionDeps = { serviceAccounts: Pick<SA, 'listServiceAccounts' | 'getServiceAccount' | 'listApiKeys'>; authz: Pick<Authz, 'isAuthorized'>; mayI: MayI; iam: ServiceState; now: () => number }` (all readonly)
  - `type SectionParams = { ownerPrn: string; lifecycle: NodeLifecycle; saOffset: number; keyOffset: number; sa: string | null }` (all readonly)
  - `loadServiceAccountSection(deps: SectionDeps, params: SectionParams): Promise<SectionView>`

- [ ] **Step 1: Write the failing test**

Create `ts/apps/gateway-console/tests/integration/service-account-section.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section loader (SMA-636 spec § 4.2, § 7.1) against the fake IAM. mayI() is
// scripted, so the fake's isAuthorized log holds only the questions about a SERVICE ACCOUNT
// (modelCallState).
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { ApiKeyStatus } from '@paigasus/sdk/iam/types';
import { organizationPrn, projectPrn, type IamAction } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import type { NodeLifecycle } from '../../app/(console)/node-status';
import { loadServiceAccountSection, type SectionDeps, type SectionParams } from '../../app/(console)/service-accounts/load';
import { serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';
import type { SectionOk, SelectedView } from '../../app/(console)/service-accounts/view';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const OWNER = organizationPrn(IDS.orgA);
const OTHER_OWNER = organizationPrn(IDS.orgB);
const PROJECT = projectPrn(IDS.orgA, IDS.projectA1);
const SA = serviceAccountPrn(IDS.saA);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;
const ACTIVE: NodeLifecycle = { own: 'active', effective: 'active' };
const ALL: Partial<Record<IamAction, boolean>> = { CreateServiceAccount: true, IssueApiKey: true, RevokeApiKey: true, ArchiveServiceAccount: true, GrantRole: true };

const at = (ms: number) => ({ seconds: BigInt(Math.floor(ms / 1000)), nanos: 0 });

function iamState(capabilities: string[]): ServiceState {
  return { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities }, capabilities };
}
const FULL = iamState(['iam.authz.cedar', 'iam.apikeys']);

function accounts(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    prn: serviceAccountPrn(`0190a1e5-0000-7000-8000-${String(index).padStart(12, '0')}`),
    ownerPrn: OWNER,
    name: `bot-${String(index)}`,
    status: 'active',
    audit: { createdAt: at(NOW) },
  }));
}

/** An account SA owned by `ownerPrn`, keys, and an IsAuthorized that answers `allowed` about SA. */
function selectedWorld(opts: { ownerPrn?: string; status?: string; allowed?: boolean } = {}): FakeIamHandlers {
  return {
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(1) }),
    'serviceAccounts.getServiceAccount': () => ({ serviceAccount: { prn: SA, ownerPrn: opts.ownerPrn ?? OWNER, name: 'ci-bot', status: opts.status ?? 'active', audit: { createdAt: at(NOW) } } }),
    'authz.isAuthorized': () => ({ allowed: opts.allowed ?? false, determiningPolicies: [], reason: '' }),
    'serviceAccounts.listApiKeys': () => ({
      apiKeys: [
        { id: 'k-active', serviceAccountPrn: SA, scopePrn: OWNER, prefix: 'pgs_a', status: ApiKeyStatus.ACTIVE, expiresAt: at(NOW + DAY), audit: { createdAt: at(NOW - DAY) } },
        { id: 'k-revoked', serviceAccountPrn: SA, scopePrn: OWNER, prefix: 'pgs_r', status: ApiKeyStatus.REVOKED },
        { id: 'k-expired', serviceAccountPrn: SA, scopePrn: OWNER, prefix: 'pgs_e', status: ApiKeyStatus.ACTIVE, expiresAt: at(NOW - DAY) },
        { id: 'k-other', serviceAccountPrn: SA, scopePrn: PROJECT, prefix: 'pgs_o', status: ApiKeyStatus.ACTIVE, lastUsedAt: at(NOW) },
      ],
    }),
  };
}

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(overrides: Partial<SectionDeps> = {}): SectionDeps {
  const clients = clientsFor(iam);
  return { serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: scriptedMayI(ALL), iam: FULL, now: () => NOW, ...overrides };
}

function params(overrides: Partial<SectionParams> = {}): SectionParams {
  return { ownerPrn: OWNER, lifecycle: ACTIVE, saOffset: 0, keyOffset: 0, sa: null, ...overrides };
}

async function loadOk(d: SectionDeps, p: SectionParams): Promise<SectionOk> {
  const view = await loadServiceAccountSection(d, p);
  if (view.kind !== 'ok') throw new Error(`expected ok, got ${view.kind}`);
  return view;
}

function okSelected(selected: SelectedView): Extract<SelectedView, { kind: 'ok' }> {
  if (selected.kind !== 'ok') throw new Error(`expected a selected account, got ${selected.kind}`);
  return selected;
}

describe('loadServiceAccountSection: the list', () => {
  it('asks for 51 accounts at the owner, asks the five affordances there, and lists the rows', async () => {
    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(2) }) });
    const calls = callsSince(iam);
    const mayI = scriptedMayI(ALL);

    const view = await loadOk(deps({ mayI }), params({ saOffset: 50 }));

    expect(calls('serviceAccounts.listServiceAccounts')[0]?.request).toMatchObject({ ownerPrn: OWNER, limit: 51, offset: 50n });
    expect([...mayI.asked].map(([action, prn]) => `${action}@${prn}`).sort()).toEqual(
      ['ArchiveServiceAccount', 'CreateServiceAccount', 'GrantRole', 'IssueApiKey', 'RevokeApiKey'].map((action) => `${action}@${OWNER}`),
    );
    expect(view.rows.map((row) => row.name)).toEqual(['bot-0', 'bot-1']);
    expect(view.rows[0]).toMatchObject({ id: '0190a1e5-0000-7000-8000-000000000000', created: '2026-09-18', active: true });
    expect(view.canCreate).toBe(true);
    expect(view.selected).toEqual({ kind: 'none' });
    expect(calls('serviceAccounts.getServiceAccount')).toHaveLength(0);
  });

  it('shows exactly 50 rows with no next page for 50, and a next page for 51 (limit+1)', async () => {
    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(50) }) });
    const fifty = await loadOk(deps(), params());
    expect(fifty.rows).toHaveLength(50);
    expect(fifty.page).toEqual({ offset: 0, nextOffset: null });

    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(51) }) });
    const more = await loadOk(deps(), params());
    expect(more.rows).toHaveLength(50);
    expect(more.page).toEqual({ offset: 0, nextOffset: 50 });
  });

  it('answers denied for a forbidden list, so only this section shows the denial', async () => {
    iam.setHandlers({
      'serviceAccounts.listServiceAccounts': () => {
        throw denial();
      },
    });
    expect(await loadServiceAccountSection(deps(), params())).toEqual({ kind: 'denied' });
  });

  it('answers the section error, with a correlation id, for any other failure', async () => {
    iam.setHandlers({
      'serviceAccounts.listServiceAccounts': () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    });
    const view = await loadServiceAccountSection(deps(), params());
    expect(view.kind).toBe('error');
    if (view.kind !== 'error') throw new Error('expected error');
    expect(view.error.presentation).toBe('degraded');
    expect(view.error.correlationId).not.toBeNull();
  });

  it.each([
    [{ own: 'archived', effective: 'archived' }, 'archived'],
    [{ own: 'active', effective: 'archived' }, 'archived-parent'],
    [{ own: 'unknown', effective: 'active' }, 'unknown'],
  ] as const)('is read-only for owner lifecycle %j (%s): no create, and no panel control', async (lifecycle, readOnly) => {
    iam.setHandlers(selectedWorld());
    const view = await loadOk(deps(), params({ lifecycle, sa: IDS.saA }));
    expect(view.readOnly).toBe(readOnly);
    expect(view.canCreate).toBe(false);
    expect(okSelected(view.selected).controls).toEqual({ allow: false, issue: false, revoke: false, archive: false });
  });

  it('hides create when mayI(CreateServiceAccount) is false', async () => {
    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }) });
    const view = await loadOk(deps({ mayI: scriptedMayI({ ...ALL, CreateServiceAccount: false }) }), params());
    expect(view.canCreate).toBe(false);
  });
});

describe('loadServiceAccountSection: the selected account (D14)', () => {
  it('asks IsAuthorized about the account, lists 51 keys, and maps every key status (§ 5.7)', async () => {
    iam.setHandlers(selectedWorld({ allowed: false }));
    const calls = callsSince(iam);

    const view = await loadOk(deps(), params({ sa: IDS.saA, keyOffset: 50 }));
    const selected = okSelected(view.selected);

    expect(calls('authz.isAuthorized').map((call) => call.request)).toEqual([expect.objectContaining({ principalPrn: SA, action: 'InvokeModel', resourcePrn: OWNER })]);
    expect(calls('serviceAccounts.listApiKeys')[0]?.request).toMatchObject({ serviceAccountPrn: SA, limit: 51, offset: 50n });
    expect(selected.modelCalls).toBe('no');
    expect(selected.keys).toEqual({
      kind: 'ok',
      page: { offset: 50, nextOffset: null },
      rows: [
        { id: 'k-active', prefix: 'pgs_a', status: 'active', created: '2026-09-17', expires: '2026-09-19', lastUsed: null, otherScope: null },
        { id: 'k-revoked', prefix: 'pgs_r', status: 'revoked', created: null, expires: null, lastUsed: null, otherScope: null },
        { id: 'k-expired', prefix: 'pgs_e', status: 'expired', created: null, expires: '2026-09-17', lastUsed: null, otherScope: null },
        { id: 'k-other', prefix: 'pgs_o', status: 'active', created: null, expires: null, lastUsed: '2026-09-18', otherScope: PROJECT },
      ],
    });
    expect(selected.controls).toEqual({ allow: true, issue: true, revoke: true, archive: true });
  });

  it('reads an archived account as archived, asks IAM nothing about it, and shows every key inactive', async () => {
    iam.setHandlers(selectedWorld({ status: 'disabled', allowed: true }));
    const calls = callsSince(iam);

    const selected = okSelected((await loadOk(deps(), params({ sa: IDS.saA }))).selected);

    expect(selected.modelCalls).toBe('archived');
    expect(calls('authz.isAuthorized')).toHaveLength(0);
    expect(selected.keys.kind === 'ok' ? selected.keys.rows.map((row) => row.status) : []).toEqual(['inactive', 'inactive', 'inactive', 'inactive']);
    expect(selected.controls).toEqual({ allow: false, issue: false, revoke: false, archive: false });
  });

  it('offers no "Allow model calls" when the account can already call models', async () => {
    iam.setHandlers(selectedWorld({ allowed: true }));
    const selected = okSelected((await loadOk(deps(), params({ sa: IDS.saA }))).selected);
    expect(selected.modelCalls).toBe('yes');
    expect(selected.controls.allow).toBe(false);
  });

  it('makes no ListApiKeys call and shows no key control when iam.apikeys is absent (D13)', async () => {
    iam.setHandlers(selectedWorld());
    const calls = callsSince(iam);

    const selected = okSelected((await loadOk(deps({ iam: iamState(['iam.authz.cedar']) }), params({ sa: IDS.saA }))).selected);

    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
    expect(selected.keys).toEqual({ kind: 'hidden' });
    expect(selected.controls).toMatchObject({ issue: false, revoke: false, archive: true });
  });

  it('offers no "Allow model calls" when iam.authz.cedar is absent (D13)', async () => {
    iam.setHandlers(selectedWorld({ allowed: false }));
    const selected = okSelected((await loadOk(deps({ iam: iamState(['iam.apikeys']) }), params({ sa: IDS.saA }))).selected);
    expect(selected.modelCalls).toBe('no');
    expect(selected.controls.allow).toBe(false);
  });

  it('counts a degraded IAM as offering neither capability, whatever its last descriptor said (D13)', async () => {
    iam.setHandlers(selectedWorld({ allowed: false }));
    const calls = callsSince(iam);
    const degraded: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: null, capabilities: ['iam.authz.cedar', 'iam.apikeys'] };

    const selected = okSelected((await loadOk(deps({ iam: degraded }), params({ sa: IDS.saA }))).selected);

    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
    expect(selected.controls).toMatchObject({ allow: false, issue: false, revoke: false });
  });

  it('answers other-scope for an account of another owner, and asks nothing more about it', async () => {
    iam.setHandlers(selectedWorld({ ownerPrn: OTHER_OWNER }));
    const calls = callsSince(iam);

    const view = await loadOk(deps(), params({ sa: IDS.saA }));

    expect(view.selected).toEqual({ kind: 'other-scope' });
    expect(calls('authz.isAuthorized')).toHaveLength(0);
    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
  });

  it('answers the panel error when GetServiceAccount fails, and the keys error when ListApiKeys fails', async () => {
    iam.setHandlers({
      ...selectedWorld(),
      'serviceAccounts.getServiceAccount': () => {
        throw denial();
      },
    });
    expect((await loadOk(deps(), params({ sa: IDS.saA }))).selected.kind).toBe('error');

    iam.setHandlers({
      ...selectedWorld(),
      'serviceAccounts.listApiKeys': () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    });
    const selected = okSelected((await loadOk(deps(), params({ sa: IDS.saA }))).selected);
    expect(selected.keys.kind).toBe('error');
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/integration/service-account-section.test.ts
```

Expected: FAIL: vitest cannot load `service-accounts/load` (the module does not exist).

- [ ] **Step 3: Implement the loader**

Create `ts/apps/gateway-console/app/(console)/service-accounts/load.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section of the organization and project pages (SMA-636 spec § 4.2). It
// returns a VIEW MODEL (./view): plain data that a client component can receive, never a proto
// message. All IAM calls go through callIam.
//
// The fan-out is bounded (D14): the list and the five affordances always; the account, its
// model-call state and its keys only for the ONE account that `?sa=` selects.
//
// Keys load for an archived account too (plan SPEC DEVIATION 4): § 5.7 shows them as "Inactive
// (account archived)". modelCallState makes no call for such an account (§ 4.3).
import 'server-only';
import type { ServiceState } from '@paigasus/discovery/types';
import { callIam, cedarCapabilityOf, type IamClients, type MayI } from '@paigasus/console-core';
import { REQUEST_LIMIT, pageOf } from '../../../lib/paging';
import { lifecycleView, type NodeLifecycle } from '../node-status';
import { formatDate, keyStatus, timestampMs } from './keys';
import { modelCallState } from './model-call-state';
import { serviceAccountIdOf, serviceAccountPrn } from './service-account-id';
import type { ApiKeyRowView, KeysView, ReadOnlyView, SectionView, SelectedView, ServiceAccountRowView } from './view';

/** The IAM capability that gates every key control and every ListApiKeys call (D13). */
export const API_KEYS_CAPABILITY = 'iam.apikeys';

/** D13: the rule of cedarCapabilityOf, for iam.apikeys. A degraded IAM counts as absent. */
export function apiKeysCapabilityOf(state: ServiceState): boolean {
  return state.state === 'available' && state.capabilities.includes(API_KEYS_CAPABILITY);
}

type ServiceAccounts = IamClients['serviceAccounts'];
type AccountMessage = Awaited<ReturnType<ServiceAccounts['listServiceAccounts']>>['serviceAccounts'][number];
type KeyMessage = Awaited<ReturnType<ServiceAccounts['listApiKeys']>>['apiKeys'][number];

export type SectionDeps = {
  readonly serviceAccounts: Pick<ServiceAccounts, 'listServiceAccounts' | 'getServiceAccount' | 'listApiKeys'>;
  readonly authz: Pick<IamClients['authz'], 'isAuthorized'>;
  readonly mayI: MayI;
  /** This request's IAM discovery state (memoized per request by discovery()). */
  readonly iam: ServiceState;
  readonly now: () => number;
};

export type SectionParams = {
  readonly ownerPrn: string;
  readonly lifecycle: NodeLifecycle;
  readonly saOffset: number;
  readonly keyOffset: number;
  /** The UUID `?sa=` selects, or null. */
  readonly sa: string | null;
};

type Affordances = { readonly issue: boolean; readonly revoke: boolean; readonly archive: boolean; readonly grant: boolean };
type Flags = { readonly writable: boolean; readonly apiKeys: boolean; readonly cedar: boolean; readonly can: Affordances };

function accountRow(account: AccountMessage): ServiceAccountRowView {
  return { prn: account.prn, id: serviceAccountIdOf(account.prn), name: account.name, created: formatDate(timestampMs(account.audit?.createdAt)), active: account.status === 'active' };
}

/** § 5.8: read-only unless the owner node itself and every ancestor are active. */
function readOnlyOf(lifecycle: NodeLifecycle): ReadOnlyView {
  const view = lifecycleView(lifecycle);
  return view === 'active' ? null : view;
}

function keyRow(key: KeyMessage, ownerPrn: string, accountActive: boolean, now: number): ApiKeyRowView {
  const expiresAtMs = timestampMs(key.expiresAt);
  return {
    id: key.id,
    prefix: key.prefix,
    status: keyStatus({ status: key.status, expiresAtMs }, accountActive, now),
    created: formatDate(timestampMs(key.audit?.createdAt)),
    expires: formatDate(expiresAtMs),
    lastUsed: formatDate(timestampMs(key.lastUsedAt)),
    otherScope: key.scopePrn === ownerPrn ? null : key.scopePrn,
  };
}

async function loadKeys(deps: SectionDeps, saPrn: string, params: SectionParams, accountActive: boolean): Promise<KeysView> {
  const listed = await callIam(() => deps.serviceAccounts.listApiKeys({ serviceAccountPrn: saPrn, limit: REQUEST_LIMIT, offset: BigInt(params.keyOffset) }));
  if (!listed.ok) return { kind: 'error', error: listed.error };
  const now = deps.now();
  const page = pageOf(
    listed.value.apiKeys.map((key) => keyRow(key, params.ownerPrn, accountActive, now)),
    params.keyOffset,
  );
  return { kind: 'ok', rows: page.rows, page: { offset: page.offset, nextOffset: page.nextOffset } };
}

async function loadSelected(deps: SectionDeps, params: SectionParams, sa: string, flags: Flags): Promise<SelectedView> {
  const got = await callIam(() => deps.serviceAccounts.getServiceAccount({ prn: serviceAccountPrn(sa) }));
  if (!got.ok) return { kind: 'error', error: got.error };
  const account = got.value.serviceAccount;
  // Another owner's account is never shown here, not even its name (§ 4.2 step 4). An answer with
  // no account (version skew) is treated the same way: nothing about it is shown.
  if (account === undefined || account.ownerPrn !== params.ownerPrn) return { kind: 'other-scope' };
  const row = accountRow(account);
  const [modelCalls, keys] = await Promise.all([
    modelCallState(deps.authz, account.prn, params.ownerPrn, account.status),
    flags.apiKeys ? loadKeys(deps, account.prn, params, row.active) : Promise.resolve<KeysView>({ kind: 'hidden' }),
  ]);
  // An archived account has no controls (§ 5.6); an owner that is not active has none (§ 5.8).
  const live = flags.writable && row.active;
  return {
    kind: 'ok',
    account: row,
    modelCalls,
    keys,
    controls: {
      // § 5.3: all four conditions.
      allow: live && modelCalls === 'no' && flags.cedar && flags.can.grant,
      issue: live && flags.apiKeys && flags.can.issue,
      revoke: live && flags.apiKeys && flags.can.revoke,
      archive: live && flags.can.archive,
    },
  };
}

export async function loadServiceAccountSection(deps: SectionDeps, params: SectionParams): Promise<SectionView> {
  const owner = params.ownerPrn;
  const [listed, canCreate, canIssue, canRevoke, canArchive, canGrant] = await Promise.all([
    callIam(() => deps.serviceAccounts.listServiceAccounts({ ownerPrn: owner, limit: REQUEST_LIMIT, offset: BigInt(params.saOffset) })),
    deps.mayI('CreateServiceAccount', owner),
    deps.mayI('IssueApiKey', owner),
    deps.mayI('RevokeApiKey', owner),
    deps.mayI('ArchiveServiceAccount', owner),
    deps.mayI('GrantRole', owner),
  ]);
  if (!listed.ok) return listed.error.presentation === 'forbidden' ? { kind: 'denied' } : { kind: 'error', error: listed.error };

  const readOnly = readOnlyOf(params.lifecycle);
  const flags: Flags = {
    writable: readOnly === null,
    apiKeys: apiKeysCapabilityOf(deps.iam),
    cedar: cedarCapabilityOf(deps.iam),
    can: { issue: canIssue, revoke: canRevoke, archive: canArchive, grant: canGrant },
  };
  const page = pageOf(listed.value.serviceAccounts.map(accountRow), params.saOffset);
  const selected: SelectedView = params.sa === null ? { kind: 'none' } : await loadSelected(deps, params, params.sa, flags);
  return {
    kind: 'ok',
    ownerPrn: owner,
    readOnly,
    rows: page.rows,
    page: { offset: page.offset, nextOffset: page.nextOffset },
    canCreate: flags.writable && canCreate,
    selected,
    sa: params.sa,
    saOffset: params.saOffset,
    keyOffset: params.keyOffset,
  };
}
```

- [ ] **Step 4: Run the test and confirm it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/integration/service-account-section.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS, 16 tests. Typecheck exit 0.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): load the service-accounts section of a settings page (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 12: The organization and project page loaders

§ 4.1 (mixed URL), § 4.2 (both pages), D4, D5, D15.

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts`
- Create: `ts/apps/gateway-console/app/(console)/orgs/[org]/projects/[project]/load.ts`
- Test: `ts/apps/gateway-console/tests/integration/org-settings-load.test.ts` (create)
- Test: `ts/apps/gateway-console/tests/integration/project-settings-load.test.ts` (create)

**Interfaces:**
- Consumes: `loadServiceAccountSection`, `SectionDeps` (Task 11); `mapWithLimit` (Task 7);
  `REQUEST_LIMIT`, `pageOf` (Task 7); `lifecycleOf`, `NodeLifecycle` (Task 8); `callIam`,
  `isUuid`, `organizationPrn`, `projectPrn`, `parseTenancyPrn`, `IamClients` from
  `@paigasus/console-core`.
- Produces (`orgs/[org]/load.ts`):
  - `PROJECT_CALLS_IN_FLIGHT = 8`
  - `type SettingsParams = { saOffset: number; keyOffset: number; sa: string | null }`
  - `type ProjectLink = { projectId: string | null; name: string; slug: string }`
  - `type TeamGroup = { teamId: string | null; name: string; projects: readonly ProjectLink[]; moreProjects: boolean }`
  - `type ProjectsView = { kind: 'error'; error: PaigasusError } | { kind: 'ok'; teams: readonly TeamGroup[]; moreTeams: boolean }`
  - `type OrganizationSettings = { kind: 'not-found' } | { kind: 'error'; error: PaigasusError } | { kind: 'ok'; orgId: string; orgPrn: string; organization: { name: string; slug: string; lifecycle: NodeLifecycle }; section: SectionView; projects: ProjectsView }`
  - `type OrganizationSettingsDeps = SectionDeps & { tenancy: Pick<Tenancy, 'getOrganization' | 'listTeams' | 'listProjects'> }`
  - `loadOrganizationSettings(deps: OrganizationSettingsDeps, params: SettingsParams & { org: string }): Promise<OrganizationSettings>`
  - (all object types readonly)
- Produces (`orgs/[org]/projects/[project]/load.ts`):
  - `type ProjectSettings = { kind: 'not-found' } | { kind: 'error'; error: PaigasusError } | { kind: 'ok'; orgId: string; projectId: string; projectPrn: string; project: { name: string; slug: string; lifecycle: NodeLifecycle }; team: { id: string | null; name: string | null }; organizationName: string | null; section: SectionView }`
  - `type ProjectSettingsDeps = SectionDeps & { tenancy: Pick<Tenancy, 'getProject' | 'getTeam' | 'getOrganization'> }`
  - `loadProjectSettings(deps: ProjectSettingsDeps, params: SettingsParams & { org: string; project: string }): Promise<ProjectSettings>`

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/integration/org-settings-load.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The organization settings loader (SMA-636 spec § 4.2). GetOrganization first: a denial is the
// PAGE error (403), and nothing else runs. Then the section and the Projects list, in parallel.
// A Projects failure is a SECTION error: the page stays 200.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { organizationPrn, projectPrn, teamPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { loadOrganizationSettings, type OrganizationSettingsDeps } from '../../app/(console)/orgs/[org]/load';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const ORG = organizationPrn(IDS.orgA);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };
const IAM: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar', 'iam.apikeys'] }, capabilities: ['iam.authz.cedar', 'iam.apikeys'] };

const uuid = (tag: string, index: number): string => `0190a1${tag}-0000-7000-8000-${String(index).padStart(12, '0')}`;

function teams(count: number) {
  return Array.from({ length: count }, (_, index) => ({ prn: teamPrn(IDS.orgA, uuid('b2', index)), orgPrn: ORG, slug: `t-${String(index)}`, name: `Team ${String(index)}`, ...ACTIVE }));
}

function projects(teamPrnValue: string, count: number) {
  return Array.from({ length: count }, (_, index) => ({ prn: projectPrn(IDS.orgA, uuid('c3', index)), teamPrn: teamPrnValue, orgPrn: ORG, slug: `p-${String(index)}`, name: `Project ${String(index)}`, ...ACTIVE }));
}

function world(opts: { teams?: number; projectsPerTeam?: number } = {}): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme', ...ACTIVE } }),
    'tenancy.listTeams': () => ({ teams: teams(opts.teams ?? 1) }),
    'tenancy.listProjects': (req) => ({ projects: projects(req.teamPrn, opts.projectsPerTeam ?? 1) }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }),
  };
}

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(): OrganizationSettingsDeps {
  const clients = clientsFor(iam);
  return { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: scriptedMayI({}), iam: IAM, now: () => NOW };
}

const params = (org: string = IDS.orgA) => ({ org, saOffset: 0, keyOffset: 0, sa: null });

describe('loadOrganizationSettings', () => {
  it('answers not-found for a segment that is not a UUID, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadOrganizationSettings(deps(), params('acme'))).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('returns the organization, its section and its projects grouped by team', async () => {
    iam.setHandlers(world({ teams: 2 }));
    const calls = callsSince(iam);

    const data = await loadOrganizationSettings(deps(), params(IDS.orgA.toUpperCase()));

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data.orgId).toBe(IDS.orgA);
    expect(data.orgPrn).toBe(ORG);
    expect(data.organization).toEqual({ name: 'Acme', slug: 'acme', lifecycle: { own: 'active', effective: 'active' } });
    expect(data.section).toMatchObject({ kind: 'ok', ownerPrn: ORG });
    expect(data.projects).toEqual({
      kind: 'ok',
      moreTeams: false,
      teams: [0, 1].map((index) => ({
        teamId: uuid('b2', index),
        name: `Team ${String(index)}`,
        moreProjects: false,
        projects: [{ projectId: uuid('c3', 0), name: 'Project 0', slug: 'p-0' }],
      })),
    });
    expect(calls('tenancy.listTeams')[0]?.request).toMatchObject({ orgPrn: ORG, limit: 51, offset: 0n });
    expect(calls('tenancy.listProjects').map((call) => call.request)).toEqual([0, 1].map((index) => expect.objectContaining({ teamPrn: teamPrn(IDS.orgA, uuid('b2', index)), limit: 51 })));
  });

  it('is the page error for a denied organization, and runs nothing else', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getOrganization': () => {
        throw denial();
      },
    });
    const calls = callsSince(iam);

    const data = await loadOrganizationSettings(deps(), params());

    expect(data.kind).toBe('error');
    if (data.kind !== 'error') throw new Error('expected error');
    expect(data.error.presentation).toBe('forbidden');
    expect(calls('tenancy.listTeams')).toHaveLength(0);
    expect(calls('serviceAccounts.listServiceAccounts')).toHaveLength(0);
  });

  it.each([
    [Code.InvalidArgument, 'prn-mismatch'],
    [Code.NotFound, 'not-found'],
  ])('answers not-found for code %s (%s)', async (code, reason) => {
    iam.setHandlers({
      ...world(),
      'tenancy.getOrganization': () => {
        throw denial({ code, reason });
      },
    });
    expect(await loadOrganizationSettings(deps(), params())).toEqual({ kind: 'not-found' });
  });

  it('shows 50 teams with no note for exactly 50, and 50 teams with the note for 51', async () => {
    iam.setHandlers(world({ teams: 50 }));
    const fifty = await loadOrganizationSettings(deps(), params());
    if (fifty.kind !== 'ok' || fifty.projects.kind !== 'ok') throw new Error('expected ok');
    expect(fifty.projects.teams).toHaveLength(50);
    expect(fifty.projects.moreTeams).toBe(false);

    iam.setHandlers(world({ teams: 51 }));
    const calls = callsSince(iam);
    const more = await loadOrganizationSettings(deps(), params());
    if (more.kind !== 'ok' || more.projects.kind !== 'ok') throw new Error('expected ok');
    expect(more.projects.teams).toHaveLength(50);
    expect(more.projects.moreTeams).toBe(true);
    // Only the SHOWN teams get a ListProjects call.
    expect(calls('tenancy.listProjects')).toHaveLength(50);
  });

  it('notes more projects for a team that has 51', async () => {
    iam.setHandlers(world({ projectsPerTeam: 51 }));
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok' || data.projects.kind !== 'ok') throw new Error('expected ok');
    expect(data.projects.teams[0]?.projects).toHaveLength(50);
    expect(data.projects.teams[0]?.moreProjects).toBe(true);
  });

  it('keeps the page when ListTeams fails: the Projects section carries the error', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.listTeams': () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    });
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error('expected ok');
    expect(data.projects).toMatchObject({ kind: 'error', error: { presentation: 'degraded' } });
    expect(data.section.kind).toBe('ok');
  });

  it('fails the Projects section when one ListProjects fails', async () => {
    iam.setHandlers({
      ...world({ teams: 3 }),
      'tenancy.listProjects': (req) => {
        if (req.teamPrn === teamPrn(IDS.orgA, uuid('b2', 1))) throw denial();
        return { projects: [] };
      },
    });
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error('expected ok');
    expect(data.projects).toMatchObject({ kind: 'error', error: { presentation: 'forbidden' } });
  });

  it('never has more than 8 ListProjects calls in flight', async () => {
    let inFlight = 0;
    let peak = 0;
    iam.setHandlers({
      ...world({ teams: 20 }),
      'tenancy.listProjects': async () => {
        inFlight += 1;
        peak = Math.max(peak, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 20));
        inFlight -= 1;
        return { projects: [] };
      },
    });
    await loadOrganizationSettings(deps(), params());
    expect(peak).toBe(8);
  });
});
```

Create `ts/apps/gateway-console/tests/integration/project-settings-load.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The project settings loader (SMA-636 spec § 4.1, § 4.2). GetProject first. Then GetTeam and
// GetOrganization for the header and the breadcrumbs, and the section, in parallel. A failed
// GetTeam or GetOrganization keeps the page: the header shows the UUID instead of the name.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { organizationPrn, projectPrn, teamPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { loadProjectSettings, type ProjectSettingsDeps } from '../../app/(console)/orgs/[org]/projects/[project]/load';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const ORG = organizationPrn(IDS.orgA);
const TEAM = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT = projectPrn(IDS.orgA, IDS.projectA1);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };
const IAM: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar'] }, capabilities: ['iam.authz.cedar'] };

function world(): FakeIamHandlers {
  return {
    'tenancy.getProject': (req) => ({ project: { prn: req.prn, teamPrn: TEAM, orgPrn: ORG, slug: 'gw', name: 'Gateway', ...ACTIVE } }),
    'tenancy.getTeam': () => ({ team: { prn: TEAM, orgPrn: ORG, slug: 'platform', name: 'Platform', ...ACTIVE } }),
    'tenancy.getOrganization': () => ({ organization: { prn: ORG, slug: 'acme', name: 'Acme', ...ACTIVE } }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }),
  };
}

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(): ProjectSettingsDeps {
  const clients = clientsFor(iam);
  return { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: scriptedMayI({}), iam: IAM, now: () => NOW };
}

const params = (project: string = IDS.projectA1, org: string = IDS.orgA) => ({ org, project, saOffset: 0, keyOffset: 0, sa: null });

describe('loadProjectSettings', () => {
  it('answers not-found for a segment that is not a UUID, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadProjectSettings(deps(), params('gateway'))).toEqual({ kind: 'not-found' });
    expect(await loadProjectSettings(deps(), params(IDS.projectA1, 'acme'))).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('returns the project, its team and organization names, and a section owned by the project', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);

    const data = await loadProjectSettings(deps(), params());

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data).toMatchObject({
      orgId: IDS.orgA,
      projectId: IDS.projectA1,
      projectPrn: PROJECT,
      project: { name: 'Gateway', slug: 'gw', lifecycle: { own: 'active', effective: 'active' } },
      team: { id: IDS.teamA1, name: 'Platform' },
      organizationName: 'Acme',
      section: { kind: 'ok', ownerPrn: PROJECT },
    });
    // The PRN is built from the URL's organization and project (§ 4.1).
    expect(calls('tenancy.getProject')[0]?.request).toMatchObject({ prn: PROJECT });
    expect(calls('tenancy.getTeam')[0]?.request).toMatchObject({ prn: TEAM });
    expect(calls('tenancy.getOrganization')[0]?.request).toMatchObject({ prn: ORG });
    expect(calls('serviceAccounts.listServiceAccounts')[0]?.request).toMatchObject({ ownerPrn: PROJECT });
  });

  it('keeps the page when GetTeam and GetOrganization fail: the names are null, the team id stays', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getTeam': () => {
        throw denial();
      },
      'tenancy.getOrganization': () => {
        throw denial();
      },
    });
    const data = await loadProjectSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error('expected ok');
    expect(data.team).toEqual({ id: IDS.teamA1, name: null });
    expect(data.organizationName).toBeNull();
  });

  it('is the page error for a denied project, and not-found for a mixed or missing one', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial();
      },
    });
    expect((await loadProjectSettings(deps(), params())).kind).toBe('error');

    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch' });
      },
    });
    expect(await loadProjectSettings(deps(), params())).toEqual({ kind: 'not-found' });
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/integration/org-settings-load.test.ts tests/integration/project-settings-load.test.ts
```

Expected: FAIL: vitest cannot load the two `load` modules.

- [ ] **Step 3: Implement the two loaders**

Create `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /gateway/orgs/[org] (SMA-636 spec § 4.2). URLs use UUIDs because a PRN holds no
// slug. GetOrganization comes first: when IAM denies it, the page is the 403 view and nothing else
// runs. The PRN comes from the URL, so an invalid-input answer (IAM's prn-mismatch) or a not-found
// answer means the URL names no such node: not-found.
//
// Then, in parallel: the service-accounts section, and the Projects list — ListTeams (limit 51),
// then ListProjects (limit 51) for each SHOWN team, at most 8 calls in flight. Any Projects failure
// is the Projects section's error; the page stays 200.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, isUuid, organizationPrn, parseTenancyPrn, type IamClients, type IamResult } from '@paigasus/console-core';
import { mapWithLimit } from '../../../../lib/concurrency';
import { REQUEST_LIMIT, pageOf } from '../../../../lib/paging';
import { lifecycleOf, type NodeLifecycle } from '../../node-status';
import { loadServiceAccountSection, type SectionDeps } from '../../service-accounts/load';
import type { SectionView } from '../../service-accounts/view';

/** § 4.2: the org page makes at most this many ListProjects calls at once. */
export const PROJECT_CALLS_IN_FLIGHT = 8;

type Tenancy = IamClients['tenancy'];
type ProjectsAnswer = Awaited<ReturnType<Tenancy['listProjects']>>;

export type SettingsParams = { readonly saOffset: number; readonly keyOffset: number; readonly sa: string | null };

export type ProjectLink = { readonly projectId: string | null; readonly name: string; readonly slug: string };
export type TeamGroup = { readonly teamId: string | null; readonly name: string; readonly projects: readonly ProjectLink[]; readonly moreProjects: boolean };
export type ProjectsView = { readonly kind: 'error'; readonly error: PaigasusError } | { readonly kind: 'ok'; readonly teams: readonly TeamGroup[]; readonly moreTeams: boolean };

export type OrganizationSettings =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly orgPrn: string;
      readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly section: SectionView;
      readonly projects: ProjectsView;
    };

export type OrganizationSettingsDeps = SectionDeps & { readonly tenancy: Pick<Tenancy, 'getOrganization' | 'listTeams' | 'listProjects'> };

function idOf(prn: string, kind: 'team' | 'project'): string | null {
  const ref = parseTenancyPrn(prn);
  return ref?.kind === kind ? ref.id.toLowerCase() : null;
}

function failure<T>(results: readonly IamResult<T>[]): PaigasusError | null {
  for (const result of results) {
    if (!result.ok) return result.error;
  }
  return null;
}

async function loadProjects(tenancy: Pick<Tenancy, 'listTeams' | 'listProjects'>, orgPrn: string): Promise<ProjectsView> {
  const teams = await callIam(() => tenancy.listTeams({ orgPrn, limit: REQUEST_LIMIT, offset: 0n }));
  if (!teams.ok) return { kind: 'error', error: teams.error };
  const shown = pageOf(teams.value.teams, 0);
  const lists = await mapWithLimit(shown.rows, PROJECT_CALLS_IN_FLIGHT, (team) => callIam(() => tenancy.listProjects({ teamPrn: team.prn, limit: REQUEST_LIMIT, offset: 0n })));
  const failed = failure(lists);
  if (failed !== null) return { kind: 'error', error: failed };
  const groups = shown.rows.map((team, index): TeamGroup => {
    const list = lists[index];
    const received: ProjectsAnswer['projects'] = list?.ok === true ? list.value.projects : [];
    const projects = pageOf(received, 0);
    return {
      teamId: idOf(team.prn, 'team'),
      name: team.name,
      projects: projects.rows.map((project) => ({ projectId: idOf(project.prn, 'project'), name: project.name, slug: project.slug })),
      moreProjects: projects.nextOffset !== null,
    };
  });
  return { kind: 'ok', teams: groups, moreTeams: shown.nextOffset !== null };
}

export async function loadOrganizationSettings(deps: OrganizationSettingsDeps, params: SettingsParams & { readonly org: string }): Promise<OrganizationSettings> {
  if (!isUuid(params.org)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const orgPrn = organizationPrn(orgId);

  const got = await callIam(() => deps.tenancy.getOrganization({ prn: orgPrn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' || got.error.presentation === 'not-found' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const organization = got.value.organization;
  if (organization === undefined) return { kind: 'not-found' };
  const lifecycle = lifecycleOf(organization);

  const [section, projects] = await Promise.all([
    loadServiceAccountSection(deps, { ownerPrn: orgPrn, lifecycle, saOffset: params.saOffset, keyOffset: params.keyOffset, sa: params.sa }),
    loadProjects(deps.tenancy, orgPrn),
  ]);
  return { kind: 'ok', orgId, orgPrn, organization: { name: organization.name, slug: organization.slug, lifecycle }, section, projects };
}
```

Create `ts/apps/gateway-console/app/(console)/orgs/[org]/projects/[project]/load.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /gateway/orgs/[org]/projects/[project] (SMA-636 spec § 4.1, § 4.2, D5: a flat
// route with no team segment). The project PRN is built from the URL's organization and project.
// IAM checks authorization first and the PRN second, so a URL that pairs one organization with
// another organization's project answers 403 or 404 and never shows the project (§ 4.1). The
// console does no pairing check of its own.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, isUuid, parseTenancyPrn, projectPrn, type IamClients } from '@paigasus/console-core';
import { lifecycleOf, type NodeLifecycle } from '../../../../node-status';
import { loadServiceAccountSection, type SectionDeps } from '../../../../service-accounts/load';
import type { SectionView } from '../../../../service-accounts/view';
import type { SettingsParams } from '../../load';

export type ProjectSettings =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly projectId: string;
      readonly projectPrn: string;
      readonly project: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      /** The team's UUID from the project's team_prn, and its name when GetTeam answered. */
      readonly team: { readonly id: string | null; readonly name: string | null };
      /** The organization's name when GetOrganization answered, else null. */
      readonly organizationName: string | null;
      readonly section: SectionView;
    };

export type ProjectSettingsDeps = SectionDeps & { readonly tenancy: Pick<IamClients['tenancy'], 'getProject' | 'getTeam' | 'getOrganization'> };

export async function loadProjectSettings(deps: ProjectSettingsDeps, params: SettingsParams & { readonly org: string; readonly project: string }): Promise<ProjectSettings> {
  if (!isUuid(params.org) || !isUuid(params.project)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const projectId = params.project.toLowerCase();
  const prn = projectPrn(orgId, projectId);

  const got = await callIam(() => deps.tenancy.getProject({ prn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' || got.error.presentation === 'not-found' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const project = got.value.project;
  if (project === undefined) return { kind: 'not-found' };
  const lifecycle = lifecycleOf(project);
  const teamRef = parseTenancyPrn(project.teamPrn);

  const [team, organization, section] = await Promise.all([
    callIam(() => deps.tenancy.getTeam({ prn: project.teamPrn })),
    callIam(() => deps.tenancy.getOrganization({ prn: project.orgPrn })),
    loadServiceAccountSection(deps, { ownerPrn: prn, lifecycle, saOffset: params.saOffset, keyOffset: params.keyOffset, sa: params.sa }),
  ]);
  return {
    kind: 'ok',
    orgId,
    projectId,
    projectPrn: prn,
    project: { name: project.name, slug: project.slug, lifecycle },
    team: { id: teamRef?.kind === 'team' ? teamRef.id.toLowerCase() : null, name: team.ok ? (team.value.team?.name ?? null) : null },
    organizationName: organization.ok ? (organization.value.organization?.name ?? null) : null,
    section,
  };
}
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/integration/org-settings-load.test.ts tests/integration/project-settings-load.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
```

Expected: PASS (10 + 4 tests). Typecheck exit 0.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): load the organization and project settings pages (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 13: The five Server Actions

§ 5.1-5.6, § 7.1 (token and logs), § 7.4 (the structure test copy). SPEC DEVIATIONS 5, 6 and 8.

**Files:**
- Create: `ts/apps/gateway-console/lib/settings-path.ts`
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/revalidation.ts`
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/actions.ts`
- Create: `ts/apps/gateway-console/tests/support/next-cache.ts` (copy of iam-console's)
- Modify: `ts/apps/gateway-console/tests/support/setup.ts:1-10` (whole file)
- Modify: `ts/apps/gateway-console/vitest.config.ts:16-19`, `:50`
- Test: `ts/apps/gateway-console/tests/unit/actions.test.ts` (create)
- Test: `ts/apps/gateway-console/tests/unit/actions-structure.test.ts` (copy of iam-console's, two edits)

**Interfaces:**
- Consumes: the commands and zod shapes of Task 10; `cedarCapabilityOf`, `formFields`,
  `invalidFormInput`, `logger`, `ActionState`, `ActionResult` from `@paigasus/console-core`;
  `discovery`, `iamClientsForAction`, `optionalSession` from `lib/console`.
- Produces (`lib/settings-path.ts`): `SETTINGS_PATH = '/orgs'`.
- Produces (`revalidation.ts`): `refreshesAfterCreate(state: Exclude<CreateState, null>): boolean`,
  `refreshesAfterMutation(result: ActionResult): boolean`,
  `refreshesAfterIssue(state: Exclude<IssueKeyState, null>): boolean`.
- Produces (`actions.ts`, `'use server'`):
  - `createServiceAccountAction(previous: CreateState, form: FormData): Promise<CreateState>` — accepts `ownerPrn`, `name`
  - `allowModelCallsAction(previous: ActionState, form: FormData): Promise<ActionState>` — accepts `saPrn`
  - `issueApiKeyAction(previous: null, form: FormData): Promise<IssueKeyState>` — accepts `saPrn`, `expiry`
  - `revokeApiKeyAction(previous: ActionState, form: FormData): Promise<ActionState>` — accepts `saPrn`, `keyId`
  - `archiveServiceAccountAction(previous: ActionState, form: FormData): Promise<ActionState>` — accepts `saPrn`
- Produces (test support): `revalidatedPaths`, `resetNextCache()` from `tests/support/next-cache.ts`.

- [ ] **Step 1: Add the `next/cache` double**

```bash
cp ts/apps/iam-console/tests/support/next-cache.ts ts/apps/gateway-console/tests/support/next-cache.ts
```

Replace `ts/apps/gateway-console/tests/support/setup.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Runs before every test file (vitest.config.ts `setupFiles`). It resets the two Next doubles, so
// no test sees another test's request headers, cookies or revalidations. Same pattern as
// ts/apps/iam-console/tests/support/setup.ts.
import { beforeEach } from 'vitest';
import { resetNextCache } from './next-cache';
import { resetNextHeaders } from './next-headers';

beforeEach(() => {
  resetNextHeaders();
  resetNextCache();
});
```

In `ts/apps/gateway-console/vitest.config.ts`, replace lines 16-19 with:

```ts
// Both real modules throw outside a Next request scope. These doubles let a test set the request
// headers and cookies, and record revalidatePath() calls (SMA-636: this zone now runs Server
// Actions). tests/support/setup.ts resets both before every test.
const nextHeadersDouble = fileURLToPath(new URL('./tests/support/next-headers.ts', import.meta.url));
const nextCacheDouble = fileURLToPath(new URL('./tests/support/next-cache.ts', import.meta.url));
```

and replace the `resolve:` line (line 50 before this edit, line 51 after it) with:

```ts
  resolve: { conditions, alias: { 'server-only': serverOnlyStub, 'next/headers': nextHeadersDouble, 'next/cache': nextCacheDouble } },
```

- [ ] **Step 2: Copy the structure test and give it this zone's list**

```bash
cp ts/apps/iam-console/tests/unit/actions-structure.test.ts ts/apps/gateway-console/tests/unit/actions-structure.test.ts
```

In the gateway copy, FIRST replace lines 25-30 (the `EXPECTED` constant) with:

```ts
const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/service-accounts/actions.ts': ['allowModelCallsAction', 'archiveServiceAccountAction', 'createServiceAccountAction', 'issueApiKeyAction', 'revokeApiKeyAction'],
};
```

THEN replace lines 3-16 (the header prose) with:

```ts
// COPY of ts/apps/iam-console/tests/unit/actions-structure.test.ts (SMA-636 spec § 7.4), with this
// zone's EXPECTED list. Every exported Server Action obtains its client through
// iamClientsForAction(), so a session read runs for EVERY action. The (console) layout does not
// guard Server Actions: an action is a POST to the page URL, and Next does not render the layout
// for it.
//
// The redirecting page accessor is BANNED here: it redirects through requireSession(), and a
// Server Action's redirect carries no basePath, so the browser leaves the /gateway zone
// (@paigasus/console-core's src/runtime.ts, on iamClientsForAction's doc comment).
//
// No actions.ts names mayI (SMA-636 spec § 5.1): a hidden button is cosmetic, and IAM decides. That
// check reads identifiers, not text, so a comment that names mayI() is not a violation.
//
// EXPECTED is a strict-equality list. A new action is a review point: add it here. The negative
// cases at the bottom prove the checker can fail, so a green here is not vacuous.
```

- [ ] **Step 3: Write the failing action test**

Create `ts/apps/gateway-console/tests/unit/actions.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The five Server Actions of the gateway settings (SMA-636 spec § 5.1-5.6, § 7.1): what each one
// sends to IAM, which inputs it accepts, what it revalidates, and that an issued token reaches no
// log line (§ 5.4 rule 1). The session read and the discovery read are mocked. Every IAM call still
// goes through the real callIam, so a ConnectError maps to the real presentation.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { denial } from '@paigasus/console-core/testing';
import { resetNextCache, revalidatedPaths } from '../support/next-cache';

const OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const PROJECT = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:project/0190a1c3-0000-7000-8000-0000000000a1';
const SA = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000a1';
const TOKEN = 'pgs_secret_0123456789abcdef0123456789abcdef';
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;
const LAYOUT = [{ path: '/orgs', type: 'layout' }];

type Handler = (request: Record<string, unknown>) => unknown;

const fake = vi.hoisted(() => ({
  session: true,
  iam: null as unknown,
  calls: [] as { method: string; request: Record<string, unknown> }[],
  handlers: {} as Record<string, (request: Record<string, unknown>) => unknown>,
}));

vi.mock('../../lib/console', () => {
  const call =
    (method: string) =>
    async (request: Record<string, unknown>): Promise<unknown> => {
      fake.calls.push({ method, request });
      const handler = fake.handlers[method];
      if (handler === undefined) throw new Error(`no handler for ${method}`);
      return handler(request);
    };
  const serviceAccounts = {
    createServiceAccount: call('serviceAccounts.createServiceAccount'),
    getServiceAccount: call('serviceAccounts.getServiceAccount'),
    issueApiKey: call('serviceAccounts.issueApiKey'),
    revokeApiKey: call('serviceAccounts.revokeApiKey'),
    archiveServiceAccount: call('serviceAccounts.archiveServiceAccount'),
  };
  const authz = { grantRole: call('authz.grantRole') };
  const relogin = {
    presentation: 'relogin',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'The session has ended.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 401 },
  };
  return {
    iamClientsForAction: () => Promise.resolve(fake.session ? { ok: true, value: { serviceAccounts, authz } } : { ok: false, error: relogin }),
    optionalSession: () => Promise.resolve(fake.session ? { accessToken: 'tok-action' } : null),
    discovery: () => ({ getServiceState: () => Promise.resolve(fake.iam) }),
  };
});

const { allowModelCallsAction, archiveServiceAccountAction, createServiceAccountAction, issueApiKeyAction, revokeApiKeyAction } = await import('../../app/(console)/service-accounts/actions');

function capabilities(list: string[]): ServiceState {
  return { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: list }, capabilities: list };
}

function form(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [name, value] of Object.entries(fields)) data.set(name, value);
  return data;
}

function requestsTo(method: string): Record<string, unknown>[] {
  return fake.calls.filter((call) => call.method === method).map((call) => call.request);
}

function happy(): Record<string, Handler> {
  return {
    // IAM answers with its canonical, lower-case owner PRN.
    'serviceAccounts.createServiceAccount': (req) => ({ serviceAccount: { prn: SA, ownerPrn: (req['ownerPrn'] as string).toLowerCase(), name: 'ci-bot', status: 'active' } }),
    'serviceAccounts.getServiceAccount': () => ({ serviceAccount: { prn: SA, ownerPrn: PROJECT, name: 'ci-bot', status: 'active' } }),
    'serviceAccounts.issueApiKey': () => ({ apiKey: { id: 'key-1', prefix: 'pgs_secret_01' }, token: TOKEN }),
    'serviceAccounts.revokeApiKey': () => ({}),
    'serviceAccounts.archiveServiceAccount': () => ({}),
    'authz.grantRole': (req) => ({ grant: { id: 'g-1', ...req } }),
  };
}

function failing(method: string, error: Error): Record<string, Handler> {
  return {
    ...happy(),
    [method]: () => {
      throw error;
    },
  };
}

/** Every line written to stdout or stderr from now on. The logger is module-level. */
function captureOutput(): () => string[] {
  const out = vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
  const err = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
  return () => [...out.mock.calls, ...err.mock.calls].map(([chunk]) => String(chunk));
}

beforeEach(() => {
  fake.session = true;
  fake.iam = capabilities(['iam.authz.cedar', 'iam.apikeys']);
  fake.calls.length = 0;
  fake.handlers = happy();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('createServiceAccountAction (§ 5.2)', () => {
  it('creates, grants gateway_user at the owner of IAM’s response, and revalidates', async () => {
    const result = await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }));

    expect(result).toEqual({ kind: 'created', saPrn: SA, granted: true });
    expect(requestsTo('authz.grantRole')).toEqual([{ principalPrn: SA, roleKey: 'gateway_user', scopePrn: OWNER }]);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('accepts only ownerPrn and name: extra fields change no request (§ 5.1, D6)', async () => {
    await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot', scopePrn: PROJECT, roleKey: 'org_admin', principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000ff' }));

    expect(requestsTo('serviceAccounts.createServiceAccount')).toEqual([{ ownerPrn: OWNER, name: 'ci-bot' }]);
    expect(requestsTo('authz.grantRole')).toEqual([{ principalPrn: SA, roleKey: 'gateway_user', scopePrn: OWNER }]);
  });

  it('makes no grant when discovery says IAM offers no role administration (D13)', async () => {
    fake.iam = capabilities(['iam.apikeys']);

    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }))).toEqual({ kind: 'created', saPrn: SA, granted: false });
    expect(requestsTo('authz.grantRole')).toHaveLength(0);
  });

  it('answers partial, logs gateway.sa.grant_failed, and revalidates when the grant fails', async () => {
    fake.handlers = failing('authz.grantRole', new ConnectError('boom', Code.Internal));
    const lines = captureOutput();

    const result = await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }));

    expect(result).toMatchObject({ kind: 'partial', saPrn: SA, error: { presentation: 'generic' } });
    expect(lines().some((line) => line.includes('"event":"gateway.sa.grant_failed"'))).toBe(true);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it.each([
    ['conflict', denial({ code: Code.AlreadyExists, reason: 'service-account-name-conflict' }), true],
    ['forbidden', denial(), true],
    ['degraded', new ConnectError('down', Code.Unavailable), true],
    ['generic', new ConnectError('boom', Code.Internal), true],
    ['invalid-input', denial({ code: Code.InvalidArgument, reason: 'invalid-name' }), false],
    ['relogin', new ConnectError('expired', Code.Unauthenticated), false],
  ] as const)('a failed create with presentation %s revalidates: %s', async (presentation, error, revalidates) => {
    fake.handlers = failing('serviceAccounts.createServiceAccount', error);

    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }))).toMatchObject({ kind: 'failed', error: { presentation } });
    expect(revalidatedPaths).toEqual(revalidates ? LAYOUT : []);
  });

  it('refuses a name that is only whitespace without calling IAM or revalidating', async () => {
    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: '   ' }))).toMatchObject({ kind: 'failed', error: { presentation: 'invalid-input' } });
    expect(fake.calls).toHaveLength(0);
    expect(revalidatedPaths).toEqual([]);
  });

  it('answers relogin, calls nothing and revalidates nothing when the session ended', async () => {
    fake.session = false;

    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }))).toMatchObject({ kind: 'failed', error: { presentation: 'relogin' } });
    expect(fake.calls).toHaveLength(0);
    expect(revalidatedPaths).toEqual([]);
  });
});

describe('allow, revoke and archive (§ 5.3, § 5.5, § 5.6)', () => {
  it('allow: grants at the owner that GetServiceAccount names, never at a form field', async () => {
    expect(await allowModelCallsAction(null, form({ saPrn: SA, scopePrn: OWNER }))).toEqual({ ok: true });

    expect(requestsTo('serviceAccounts.getServiceAccount')).toEqual([{ prn: SA }]);
    expect(requestsTo('authz.grantRole')).toEqual([{ principalPrn: SA, roleKey: 'gateway_user', scopePrn: PROJECT }]);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it.each([
    ['a success', null, true],
    ['a generic error (a duplicate grant)', new ConnectError('duplicate', Code.Internal), true],
    ['a refusal', denial(), true],
    ['relogin', new ConnectError('expired', Code.Unauthenticated), false],
  ] as const)('allow revalidates after %s: %s', async (_label, error, revalidates) => {
    if (error !== null) fake.handlers = failing('authz.grantRole', error);

    await allowModelCallsAction(null, form({ saPrn: SA }));

    expect(revalidatedPaths).toEqual(revalidates ? LAYOUT : []);
  });

  it('revoke: sends the key id, and revalidates after a success and after a refusal', async () => {
    expect(await revokeApiKeyAction(null, form({ saPrn: SA, keyId: 'key-1' }))).toEqual({ ok: true });
    expect(requestsTo('serviceAccounts.revokeApiKey')).toEqual([{ id: 'key-1' }]);
    expect(revalidatedPaths).toEqual(LAYOUT);

    resetNextCache();
    fake.handlers = failing('serviceAccounts.revokeApiKey', denial());
    expect(await revokeApiKeyAction(null, form({ saPrn: SA, keyId: 'key-1' }))).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('archive: sends the account PRN, and revalidates after a success and after a refusal', async () => {
    expect(await archiveServiceAccountAction(null, form({ saPrn: SA }))).toEqual({ ok: true });
    expect(requestsTo('serviceAccounts.archiveServiceAccount')).toEqual([{ prn: SA }]);
    expect(revalidatedPaths).toEqual(LAYOUT);

    resetNextCache();
    fake.handlers = failing('serviceAccounts.archiveServiceAccount', denial());
    expect(await archiveServiceAccountAction(null, form({ saPrn: SA }))).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('refuses a missing key id without calling IAM', async () => {
    expect(await revokeApiKeyAction(null, form({ saPrn: SA }))).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(fake.calls).toHaveLength(0);
  });
});

describe('issueApiKeyAction (§ 5.4)', () => {
  it('scopes the key to the owner from GetServiceAccount; a scopePrn form field changes nothing', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(NOW);

    const result = await issueApiKeyAction(null, form({ saPrn: SA, expiry: '30', scopePrn: OWNER }));

    expect(result).toEqual({ ok: true, token: TOKEN, prefix: 'pgs_secret_01' });
    expect(requestsTo('serviceAccounts.issueApiKey')).toEqual([
      { serviceAccountPrn: SA, scopePrn: PROJECT, expiresAt: { seconds: BigInt(Math.floor((NOW + 30 * DAY) / 1000)), nanos: 0 }, scopeActions: [], scopeRoles: [] },
    ]);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('sends no expires_at for the IAM default', async () => {
    await issueApiKeyAction(null, form({ saPrn: SA, expiry: 'default' }));
    expect(requestsTo('serviceAccounts.issueApiKey')[0]).not.toHaveProperty('expiresAt');
  });

  it('refuses an expiry outside the four choices without calling IAM', async () => {
    expect(await issueApiKeyAction(null, form({ saPrn: SA, expiry: '7' }))).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(fake.calls).toHaveLength(0);
  });

  it('revalidates nothing after a refusal', async () => {
    fake.handlers = failing('serviceAccounts.issueApiKey', denial());

    expect(await issueApiKeyAction(null, form({ saPrn: SA, expiry: '30' }))).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it('writes the token to no log line, on stdout or on stderr (§ 5.4 rule 1, § 7.1)', async () => {
    const lines = captureOutput();

    // Vacuity: a failed call DOES write a line, so the spies see what the logger writes.
    fake.handlers = failing('authz.grantRole', new ConnectError('boom', Code.Internal));
    await allowModelCallsAction(null, form({ saPrn: SA }));
    fake.handlers = happy();
    expect(await issueApiKeyAction(null, form({ saPrn: SA, expiry: '90' }))).toMatchObject({ ok: true, token: TOKEN });
    // A failed issue after it: its log line must not carry the earlier token either.
    fake.handlers = failing('serviceAccounts.issueApiKey', new ConnectError('boom', Code.Internal));
    await issueApiKeyAction(null, form({ saPrn: SA, expiry: '90' }));

    expect(lines().length).toBeGreaterThan(0);
    expect(lines().filter((line) => line.includes(TOKEN))).toEqual([]);
  });
});
```

- [ ] **Step 4: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/actions.test.ts tests/unit/actions-structure.test.ts
```

Expected: FAIL. `actions.test.ts` cannot load `service-accounts/actions`. In
`actions-structure.test.ts` the case "finds exactly the expected actions.ts files and exports"
fails: no `actions.ts` exists under `app/`. Its negative controls pass.

- [ ] **Step 5: Implement the path, the rules and the actions**

Create `ts/apps/gateway-console/lib/settings-path.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The one path the gateway settings actions revalidate (SMA-636 § 5). basePath-relative. Route
// groups such as (console) are not part of the URL, so '/orgs' with 'layout' covers the
// organization page and every project page under it. The pattern of iam-console's
// lib/tenancy-path.ts. A plain module: a Server Actions file may export only async functions.
export const SETTINGS_PATH = '/orgs';
```

Create `ts/apps/gateway-console/app/(console)/service-accounts/revalidation.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Which action results refresh the settings pages (SMA-636 spec § 5.2-5.6). A plain module: a
// Server Actions file may export only async functions, so these rules cannot live in actions.ts.
//
// A `relogin` result NEVER revalidates (plan SPEC DEVIATION 6). The post-action render runs the
// (console) layout's requireSession(), whose redirect carries no basePath and leaves the zone
// (@paigasus/console-core's src/runtime.ts, on iamClientsForAction).
import type { ActionResult } from '@paigasus/console-core';
import type { CreateState, IssueKeyState } from './view';

/**
 * § 5.2's table: every create result except `invalid-input` (and `relogin`). A `degraded` or
 * `generic` failure revalidates too: step 1 may have committed before the response was lost.
 */
export function refreshesAfterCreate(state: Exclude<CreateState, null>): boolean {
  if (state.kind !== 'failed') return true;
  return state.error.presentation !== 'invalid-input' && state.error.presentation !== 'relogin';
}

/** §§ 5.3, 5.5, 5.6: after every result. A `generic` allow can be a duplicate grant (§ 3.2). */
export function refreshesAfterMutation(result: ActionResult): boolean {
  return result.ok || result.error.presentation !== 'relogin';
}

/**
 * § 5.4, after a success only (plan SPEC DEVIATION 8), so the new key row shows at once. The
 * revalidated render reads IAM, which never returns a token.
 */
export function refreshesAfterIssue(state: Exclude<IssueKeyState, null>): boolean {
  return state.ok;
}
```

Create `ts/apps/gateway-console/app/(console)/service-accounts/actions.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// The five Server Actions of the gateway settings (SMA-636 spec § 5). Both settings pages pass
// these same five to ServiceAccountSection (plan SPEC DEVIATION 5): an action takes its owner from
// the form (create) or from IAM (D6), never from the page.
//
// Every action follows § 5.1, in this order: iamClientsForAction() first; then a zod shape check
// of ONLY the fields it accepts (formFields names them, and the rest of the form is ignored); then
// a pure command. No action consults mayI(): IAM decides. tests/unit/actions-structure.test.ts
// holds these rules.
//
// Revalidation follows ./revalidation.ts. A `relogin` result returns before any revalidatePath.
//
// THE TOKEN (§ 5.4 rule 1). issueApiKeyAction returns the plaintext token in its result, and that
// result is the ONE response body that may carry it. Nothing here logs it or puts it in a URL, a
// cookie or page data.
import { revalidatePath } from 'next/cache';
import { cedarCapabilityOf, formFields, invalidFormInput, logger, type ActionState } from '@paigasus/console-core';
import { discovery, iamClientsForAction, optionalSession } from '../../../lib/console';
import { SETTINGS_PATH } from '../../../lib/settings-path';
import { allowModelCalls, archiveServiceAccount, createServiceAccount, createServiceAccountForm, issueApiKey, issueApiKeyForm, revokeApiKey, revokeApiKeyForm, serviceAccountForm } from './commands';
import { refreshesAfterCreate, refreshesAfterIssue, refreshesAfterMutation } from './revalidation';
import type { CreateState, IssueKeyState } from './view';

/** D13: grant only when IAM is available and reports iam.authz.cedar (the rule of cedarCapabilityOf). */
async function roleAdministrationOffered(): Promise<boolean> {
  const session = await optionalSession();
  if (session === null) return false;
  return cedarCapabilityOf(await discovery().getServiceState('iam', session.accessToken));
}

export async function createServiceAccountAction(_previous: CreateState, form: FormData): Promise<CreateState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return { kind: 'failed', error: clients.error };
  const parsed = createServiceAccountForm.safeParse(formFields(form, ['ownerPrn', 'name']));
  if (!parsed.success) return { kind: 'failed', error: invalidFormInput() };
  const result = await createServiceAccount({ serviceAccounts: clients.value.serviceAccounts, authz: clients.value.authz, cedar: await roleAdministrationOffered(), logger }, parsed.data);
  if (refreshesAfterCreate(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function allowModelCallsAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = serviceAccountForm.safeParse(formFields(form, ['saPrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await allowModelCalls({ serviceAccounts: clients.value.serviceAccounts, authz: clients.value.authz }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

/** § 5.4 rule 2: the client calls this DIRECTLY in a transition, with `null` as the previous state. */
export async function issueApiKeyAction(_previous: null, form: FormData): Promise<IssueKeyState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = issueApiKeyForm.safeParse(formFields(form, ['saPrn', 'expiry']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await issueApiKey({ serviceAccounts: clients.value.serviceAccounts, now: Date.now }, parsed.data);
  if (refreshesAfterIssue(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function revokeApiKeyAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = revokeApiKeyForm.safeParse(formFields(form, ['saPrn', 'keyId']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await revokeApiKey({ serviceAccounts: clients.value.serviceAccounts }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function archiveServiceAccountAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = serviceAccountForm.safeParse(formFields(form, ['saPrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveServiceAccount({ serviceAccounts: clients.value.serviceAccounts }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}
```

- [ ] **Step 6: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/actions.test.ts tests/unit/actions-structure.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
pnpm --dir ts exec eslint apps/gateway-console
```

Expected: PASS. Typecheck and lint exit 0.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the gateway settings Server Actions (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 14: Client building blocks: error copy, `FormError`, `SectionError`, confirm, token panel

§ 4.6, § 5.4 rules 3, 6 and 7, § 5.5, § 5.6, § 9 (the recorded copies). jsdom and Testing Library
enter this app here.

**Files:**
- Modify: `ts/apps/gateway-console/package.json:32-40` (devDependencies)
- Modify: `ts/apps/gateway-console/app/_components/error-copy.ts:1-26` (whole file)
- Create: `ts/apps/gateway-console/app/_components/form-error.tsx`
- Create: `ts/apps/gateway-console/app/_components/section-error.tsx` (copy of iam-console's)
- Create: `ts/apps/gateway-console/app/_components/confirm-button.tsx`
- Create: `ts/apps/gateway-console/app/_components/use-hydrated.ts`
- Create: `ts/apps/gateway-console/app/_components/token-panel.tsx`
- Modify: `ts/apps/gateway-console/tests/unit/error-views.test.tsx:3-5` (header prose only)
- Test: `ts/apps/gateway-console/tests/unit/error-copy.test.ts` (whole file)
- Test: `ts/apps/gateway-console/tests/unit/form-error.test.tsx` (create)
- Test: `ts/apps/gateway-console/tests/unit/confirm-button.test.tsx` (create)
- Test: `ts/apps/gateway-console/tests/unit/token-panel.test.tsx` (create)
- Test: `ts/apps/gateway-console/tests/unit/use-hydrated.test.tsx` (create)

**Interfaces:**
- Produces:
  - `FORM_REASON_COPY: Partial<Record<ErrorReason, string>>`, `formMessage(error: PaigasusError): string` (error-copy)
  - `FormError(props: { error: PaigasusError | null; message?: string | undefined }): ReactElement | null`
  - `SectionError(props: { error: PaigasusError }): Promise<ReactElement>` (awaited as a function)
  - `ConfirmButton(props: { testId: string; label: string; confirmLabel: string; confirmation: string; hidden: Readonly<Record<string, string>>; disabled: boolean; onConfirm: (form: FormData) => void }): ReactElement`
  - `useHydrated(): boolean`
  - `type TokenPanelHandle = { show(token: string, prefix: string): void }`;
    `TokenPanel(props: { ref: Ref<TokenPanelHandle> }): ReactElement | null`

- [ ] **Step 1: Add the jsdom tier's dependencies**

In `ts/apps/gateway-console/package.json`, replace lines 32-40 with:

```json
  "devDependencies": {
    "@playwright/test": "catalog:",
    "@testing-library/dom": "catalog:",
    "@testing-library/react": "catalog:",
    "@testing-library/user-event": "catalog:",
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "@types/react-dom": "catalog:",
    "jsdom": "catalog:",
    "testcontainers": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts install
```

Expected: exit 0. `ts/pnpm-lock.yaml` gains the four entries under `apps/gateway-console`. They
are the same catalog versions iam-console uses.

- [ ] **Step 2: Write the failing tests**

Replace `ts/apps/gateway-console/tests/unit/error-copy.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The error copy tables (spec § 6.5, § 9.2, D16; SMA-636 § 5.1). The form table covers the
// reasons the service-account forms can get.
import { describe, expect, it } from 'vitest';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY, PRESENTATION_COPY, formMessage } from '../../app/_components/error-copy';

const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'rate-limited', 'invalid-input', 'conflict', 'disabled', 'generic'];

function errorWith(presentation: Presentation, reason: ErrorReason | null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'cid-1',
    requestId: null,
    retryable: null,
    metadata: {},
    transport: { kind: 'transport', cause: 'network' },
  };
}

describe('the error copy', () => {
  it('has a title and a body for every presentation', () => {
    // Iterates the literal list above, not Object.keys(PRESENTATION_COPY): a table replaced by
    // `{}` widened to `any` would still pass a key-driven assertion.
    expect(Object.keys(PRESENTATION_COPY).sort()).toEqual([...PRESENTATIONS].sort());
    for (const presentation of PRESENTATIONS) {
      expect(PRESENTATION_COPY[presentation].title.length).toBeGreaterThan(0);
      expect(PRESENTATION_COPY[presentation].body.length).toBeGreaterThan(0);
    }
  });

  it('keys the form table only by real, non-sentinel ErrorReason values', () => {
    const keys = Object.keys(FORM_REASON_COPY).map(Number);
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) {
      expect(ErrorReason[key]).toBeDefined();
      expect(key).not.toBe(ErrorReason.UNSPECIFIED);
    }
  });

  it('says the service-account name conflict in its own words (SMA-636 § 5.2)', () => {
    expect(formMessage(errorWith('conflict', ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT))).toBe('A service account with this name already exists here.');
  });

  it('falls back to the presentation copy for a reason the table does not have, and never shows IAM’s message', () => {
    const message = formMessage(errorWith('degraded', null));
    expect(message).toBe(PRESENTATION_COPY.degraded.body);
    expect(message).not.toContain('IAM text');
  });
});
```

Create `ts/apps/gateway-console/tests/unit/form-error.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// FormError and SectionError, the copies of iam-console's (SMA-636 spec § 9). FormError has ONE
// addition: a `message` that replaces the copy, for the results whose words depend on the control.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FormError } from '../../app/_components/form-error';
import { SectionError } from '../../app/_components/section-error';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/o' }));

const CID = '0198f2c1-8888-7000-8000-00000000abcd';

function errorWith(presentation: Presentation, reason: ErrorReason | null = null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: CID,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 7, codeName: 'PermissionDenied' },
  };
}

function render(node: ReactNode): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      {node}
    </ZoneProvider>,
  );
}

describe('FormError', () => {
  it('renders nothing without an error', () => {
    expect(render(<FormError error={null} />)).toBe('');
  });

  it('shows the reason copy and the correlation id, never IAM’s message', () => {
    const html = render(<FormError error={errorWith('conflict', ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT)} />);
    expect(html).toContain('A service account with this name already exists here.');
    expect(html).toContain(`data-testid="correlation-id">${CID}<`);
    expect(html).not.toContain('IAM text');
  });

  it('shows a given message in place of the copy (SMA-636 § 5.3, § 5.4)', () => {
    const html = render(<FormError error={errorWith('generic')} message="Model calls may already be allowed. The page was reloaded." />);
    expect(html).toContain('Model calls may already be allowed. The page was reloaded.');
    expect(html).not.toContain('The request failed.');
  });

  it('links "Sign in again" back to this page, inside the zone, for relogin', () => {
    expect(render(<FormError error={errorWith('relogin')} />)).toContain('href="/gateway/auth/login?returnTo=%2Fgateway%2Forgs%2Fo"');
  });
});

describe('SectionError', () => {
  it('shows the error state with the correlation id for a refused section, and the empty state for a disabled one', async () => {
    const refused = renderToStaticMarkup(await SectionError({ error: errorWith('degraded') }));
    expect(refused).toContain('data-testid="section-error"');
    expect(refused).toContain('data-presentation="degraded"');
    expect(refused).toContain(CID);
    expect(refused).not.toContain('IAM text');

    const disabled = renderToStaticMarkup(await SectionError({ error: errorWith('disabled') }));
    expect(disabled).toContain('This feature is not enabled on this IAM');
  });
});
```

Create `ts/apps/gateway-console/tests/unit/confirm-button.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The two-step confirm control (SMA-636 spec § 5.5, § 5.6): a copy of iam-console's ArchiveButton.
// The first state has NO form and NO submit, so with JavaScript off the action cannot run.
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ConfirmButton, type ConfirmButtonProps } from '../../app/_components/confirm-button';

afterEach(() => {
  cleanup();
});

function renderButton(overrides: Partial<ConfirmButtonProps> = {}) {
  const onConfirm = vi.fn<(form: FormData) => void>();
  render(
    <ConfirmButton
      testId="revoke-key-1"
      label="Revoke"
      confirmLabel="Confirm revoke"
      confirmation="Revoke the key pgs_a?"
      hidden={{ saPrn: 'sa-1', keyId: 'key-1' }}
      disabled={false}
      onConfirm={onConfirm}
      {...overrides}
    />,
  );
  return onConfirm;
}

describe('ConfirmButton', () => {
  it('has no form and no submit before the first click', () => {
    renderButton();
    expect(screen.queryByRole('form')).toBeNull();
    expect(screen.getByRole('button', { name: 'Revoke' }).getAttribute('type')).toBe('button');
  });

  it('asks first, then hands the hidden fields to onConfirm', async () => {
    const user = userEvent.setup();
    const onConfirm = renderButton();

    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(screen.getByText('Revoke the key pgs_a?')).toBeDefined();
    await user.click(screen.getByRole('button', { name: 'Confirm revoke' }));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const form = onConfirm.mock.calls[0]?.[0];
    expect(form?.get('saPrn')).toBe('sa-1');
    expect(form?.get('keyId')).toBe('key-1');
  });

  it('goes back without a call on Cancel', async () => {
    const user = userEvent.setup();
    const onConfirm = renderButton();

    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.queryByRole('form')).toBeNull();
  });

  it('disables the first step while disabled', () => {
    renderButton({ disabled: true });
    expect((screen.getByRole('button', { name: 'Revoke' }) as HTMLButtonElement).disabled).toBe(true);
  });
});
```

Create `ts/apps/gateway-console/tests/unit/token-panel.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// TokenPanel (SMA-636 spec § 5.4 rules 3 and 6): the token lives in this component's own state. It
// closes on "Done" and on `pagehide`, and on nothing else.
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef } from 'react';
import { afterEach, describe, expect, it } from 'vitest';
import { TokenPanel, type TokenPanelHandle } from '../../app/_components/token-panel';

const TOKEN = 'pgs_unit_0123456789abcdef0123456789abcdef';
const SECOND = 'pgs_unit_fedcba9876543210fedcba9876543210';

afterEach(() => {
  cleanup();
});

function mount() {
  const ref = createRef<TokenPanelHandle>();
  render(<TokenPanel ref={ref} />);
  return ref;
}

describe('TokenPanel', () => {
  it('shows nothing until a key is issued', () => {
    mount();
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('shows the token, its prefix and the one-time warning', () => {
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    expect(screen.getByText('You cannot see this token again.')).toBeDefined();
    expect(screen.getByTestId('token-panel').textContent).toContain('pgs_unit_01');
  });

  it('closes on Done', async () => {
    const user = userEvent.setup();
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    await user.click(screen.getByRole('button', { name: 'Done' }));
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('closes on pagehide', () => {
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    act(() => {
      window.dispatchEvent(new Event('pagehide'));
    });
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('shows the second token when a second key is issued', () => {
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    act(() => {
      ref.current?.show(SECOND, 'pgs_unit_fe');
    });
    expect(screen.getByTestId('token-value').textContent).toBe(SECOND);
  });
});
```

Create `ts/apps/gateway-console/tests/unit/use-hydrated.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// useHydrated (SMA-636 spec § 5.4 rule 7): false in the server render, true in a client render.
import { cleanup, render, screen } from '@testing-library/react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';
import { useHydrated } from '../../app/_components/use-hydrated';

afterEach(() => {
  cleanup();
});

function Probe() {
  return <p>{useHydrated() ? 'hydrated' : 'server'}</p>;
}

describe('useHydrated', () => {
  it('is false in a server render, so a JavaScript-only control is not in the HTML', () => {
    expect(renderToStaticMarkup(<Probe />)).toBe('<p>server</p>');
  });

  it('is true in a client render', () => {
    render(<Probe />);
    expect(screen.getByText('hydrated')).toBeDefined();
  });
});
```

- [ ] **Step 3: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/error-copy.test.ts tests/unit/form-error.test.tsx tests/unit/confirm-button.test.tsx tests/unit/token-panel.test.tsx tests/unit/use-hydrated.test.tsx
```

Expected: FAIL. `error-copy.test.ts` fails on `FORM_REASON_COPY` (undefined). The other four
files cannot load their component modules.

- [ ] **Step 4: Implement the copy table**

Replace `ts/apps/gateway-console/app/_components/error-copy.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The user-facing words for every error (spec § 6.5, D16; SMA-636 § 5.1). CLIENT-SAFE: no
// `server-only`, and the only import is @paigasus/sdk's guard-free ./errors/types entry
// (ts/packages/paigasus-sdk/src/errors/types.ts), so form-error.tsx (a client component) can use it.
//
// Every error this zone shows comes from a call to IAM, so the tables are IAM-worded throughout,
// same as iam-console's. `iam-console` keeps its OWN copy rather than sharing one (plan D16): the
// two zones are separate products whose wording may legitimately differ, and
// @paigasus/console-core's boundary rule bans a React component from its `src/`.
//
// Two tables. PRESENTATION_COPY is total over Presentation, so a tenth presentation fails the
// type-check. FORM_REASON_COPY covers the reasons the service-account forms can get (SMA-636); a
// test asserts every key is a real ErrorReason. A reason with no entry — including one this build
// does not know, which the SDK reports as reason null — falls back to the presentation's copy (the
// version-skew rule). No copy ever repeats IAM's message.
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';

export const PRESENTATION_COPY: Record<Presentation, { title: string; body: string }> = {
  relogin: { title: 'Your session has ended', body: 'Sign in again to continue.' },
  forbidden: { title: 'You do not have access', body: 'IAM refused this request for your account.' },
  'not-found': { title: 'Not found', body: 'This item does not exist, or it was removed.' },
  degraded: { title: 'IAM is not available', body: 'IAM did not answer in time. Try again in a moment.' },
  'rate-limited': { title: 'Too many requests', body: 'Wait a moment, then try again.' },
  'invalid-input': { title: 'The request was not valid', body: 'Check the values and try again.' },
  conflict: { title: 'The request conflicts with the current state', body: 'Reload the page and try again.' },
  disabled: { title: 'This feature is not enabled on this IAM', body: 'An operator can enable it in the IAM configuration.' },
  generic: { title: 'Something went wrong', body: 'The request failed. Try again, and give the reference below to support if it fails again.' },
};

export const FORM_REASON_COPY: Partial<Record<ErrorReason, string>> = {
  // SMA-636 § 5.2.
  [ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT]: 'A service account with this name already exists here.',
  [ErrorReason.INVALID_NAME]: 'Enter a name.',
  [ErrorReason.NOT_FOUND]: 'The item was not found. It may have been removed.',
  [ErrorReason.MISSING_REQUIRED_FIELD]: 'Fill in every required field.',
  [ErrorReason.FORBIDDEN]: 'You do not have permission to do this.',
  [ErrorReason.PARENT_ARCHIVED]: 'The parent of this item is archived.',
  [ErrorReason.NODE_ARCHIVED]: 'This item is archived.',
  // IAM answers UNIMPLEMENTED with this reason when a capability is off (spec § 3.3).
  [ErrorReason.CAPABILITY_DISABLED]: 'This feature is not enabled on this IAM.',
  [ErrorReason.PRINCIPAL_INACTIVE]: 'Your account is not active in IAM.',
};

/** The one sentence a form shows for an error. */
export function formMessage(error: PaigasusError): string {
  const byReason = error.reason === null ? undefined : FORM_REASON_COPY[error.reason];
  return byReason ?? PRESENTATION_COPY[error.presentation].body;
}
```

- [ ] **Step 5: Implement the components**

Create `ts/apps/gateway-console/app/_components/form-error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// A Server Action failed (SMA-636 § 5.1). COPY of iam-console's app/_components/form-error.tsx
// (spec § 9; nothing gates a divergence), with ONE addition: `message` replaces the copy, for the
// two results whose words depend on the control (§ 5.3 "may already be allowed" and § 5.4 "…grant
// every role this account holds"). CLIENT component. It shows the reason's copy when the table
// knows the reason, else the presentation's copy, and always the correlation id — never IAM's
// message.
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

Copy the section error and replace its header prose:

```bash
cp ts/apps/iam-console/app/_components/section-error.tsx ts/apps/gateway-console/app/_components/section-error.tsx
```

In the gateway copy, replace lines 3-6 with:

```tsx
// COPY of ts/apps/iam-console/app/_components/section-error.tsx (SMA-636 spec § 9; nothing gates a
// divergence). A SECTION read failed: the service accounts or the Projects list of a settings page.
// SERVER component. It NEVER throws: the rest of the page still renders. Its callers AWAIT it as a
// function (see error-tail.tsx), so a page returns a fully resolved tree.
```

Create `ts/apps/gateway-console/app/_components/use-hydrated.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// true after hydration; false in the server render and while React hydrates (SMA-636 spec § 5.4
// rule 7, plan SPEC DEVIATION 7). useSyncExternalStore gives the server snapshot during hydration
// and the client snapshot after it, with no effect and no state. A control that needs JavaScript
// renders only when this is true, so with JavaScript off it does not exist, and no document-level
// POST can run it.
'use client';

import { useSyncExternalStore } from 'react';

const subscribe = (): (() => void) => () => undefined;

export function useHydrated(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => true,
    () => false,
  );
}
```

Create `ts/apps/gateway-console/app/_components/confirm-button.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// A two-step confirm control (SMA-636 spec § 4.6, § 5.5, § 5.6). COPY of iam-console's
// ArchiveButton (app/_components/lifecycle-button.tsx), recorded in spec § 9; nothing gates a
// divergence. Two changes: it takes its words and its hidden fields as props, and it hands the
// submitted form to `onConfirm` instead of posting it, because the section runs every action in
// its own transition and shows every result in ONE result region (§ 4.6). So it has no error area.
//
// The first state has NO form, so with JavaScript off the action is not possible at all.
'use client';

import { useState, type ReactElement } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';

export type ConfirmButtonProps = {
  readonly testId: string;
  readonly label: string;
  readonly confirmLabel: string;
  readonly confirmation: string;
  readonly hidden: Readonly<Record<string, string>>;
  readonly disabled: boolean;
  readonly onConfirm: (form: FormData) => void;
};

export function ConfirmButton({ testId, label, confirmLabel, confirmation, hidden, disabled, onConfirm }: ConfirmButtonProps): ReactElement {
  const [confirming, setConfirming] = useState(false);
  return (
    <div data-testid={testId} className="flex flex-col gap-2">
      {confirming ? (
        <form
          aria-label={confirmLabel}
          className="flex flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            onConfirm(new FormData(event.currentTarget));
            setConfirming(false);
          }}
        >
          <p className="text-sm">{confirmation}</p>
          {Object.entries(hidden).map(([name, value]) => (
            <input key={name} type="hidden" name={name} value={value} />
          ))}
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
              {confirmLabel}
            </button>
            <button
              type="button"
              disabled={disabled}
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
          disabled={disabled}
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            setConfirming(true);
          }}
        >
          {label}
        </button>
      )}
    </div>
  );
}
```

Create `ts/apps/gateway-console/app/_components/token-panel.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The new API key's token (SMA-636 spec § 5.4). The section's result region renders it, OUTSIDE
// every row and panel that a revalidation re-renders (rule 3). The token lives in THIS component's
// useState and nowhere else in the browser: the section hands it over through `show()` the moment
// the issue call returns.
//
// It closes on "Done" and on `pagehide` ONLY (rule 6). It does not close on another submission, so
// key rotation works: issue, copy, then revoke the old key with the new token still on screen.
'use client';

import { useEffect, useImperativeHandle, useState, type ReactElement, type Ref } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';

export type TokenPanelHandle = { show(token: string, prefix: string): void };

type Issued = { readonly token: string; readonly prefix: string };

export function TokenPanel({ ref }: { readonly ref: Ref<TokenPanelHandle> }): ReactElement | null {
  const [issued, setIssued] = useState<Issued | null>(null);
  const [copied, setCopied] = useState(false);

  useImperativeHandle(
    ref,
    () => ({
      show: (token: string, prefix: string) => {
        setIssued({ token, prefix });
        setCopied(false);
      },
    }),
    [],
  );

  useEffect(() => {
    if (issued === null) return undefined;
    const close = (): void => {
      setIssued(null);
    };
    window.addEventListener('pagehide', close);
    return () => {
      window.removeEventListener('pagehide', close);
    };
  }, [issued]);

  if (issued === null) return null;
  return (
    <div role="region" aria-label="New API key" data-testid="token-panel" className="border-input rounded-pgs flex flex-col gap-2 border p-3">
      <p className="text-sm">
        API key <code>{issued.prefix}</code> was issued. Copy the token now.
      </p>
      <code data-testid="token-value" className="text-sm break-all">
        {issued.token}
      </code>
      <p className="text-sm font-medium">You cannot see this token again.</p>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            navigator.clipboard.writeText(issued.token).then(
              () => {
                setCopied(true);
              },
              () => {
                setCopied(false);
              },
            );
          }}
        >
          Copy
        </button>
        <button
          type="button"
          className={PRIMARY_BUTTON_CLASS}
          onClick={() => {
            setIssued(null);
          }}
        >
          Done
        </button>
      </div>
      {copied ? (
        <p role="status" className="text-sm">
          Copied.
        </p>
      ) : null}
    </div>
  );
}
```

In `ts/apps/gateway-console/tests/unit/error-views.test.tsx`, replace lines 3-5 with:

```tsx
// PageError and the 403 view (spec § 6.1, § 6.2). Server components are async functions here, so a
// test awaits them and renders the element they return. FormError and SectionError have their own
// file, tests/unit/form-error.test.tsx (SMA-636).
```

- [ ] **Step 6: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/error-copy.test.ts tests/unit/form-error.test.tsx tests/unit/confirm-button.test.tsx tests/unit/token-panel.test.tsx tests/unit/use-hydrated.test.tsx tests/unit/error-views.test.tsx
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
pnpm --dir ts exec eslint apps/gateway-console
```

Expected: PASS. Typecheck and lint exit 0.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console ts/pnpm-lock.yaml
git commit -F - <<'EOF'
feat(ts): add the form error, confirm and token panel pieces of the gateway zone (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 15: The section and the selected-account panel

§ 4.6, § 5.2-5.8, § 7.1 (the jsdom rows). SPEC DEVIATION 7.

**Files:**
- Create: `ts/apps/gateway-console/app/_components/service-account-section.tsx`
- Create: `ts/apps/gateway-console/app/_components/service-account-panel.tsx`
- Test: `ts/apps/gateway-console/tests/unit/service-account-section.test.tsx` (create)

**Interfaces:**
- Consumes: the view types of Task 8; `Pager`, `linkHref` (Task 7); `FormError`, `ConfirmButton`,
  `TokenPanel`, `TokenPanelHandle`, `useHydrated` (Task 14); `PARENT_ARCHIVED_NOTE` (Task 8);
  `serviceAccountIdOf` (Task 8); `FormAction` (type, Task 4).
- Produces (`service-account-section.tsx`, `'use client'`):
  - `type ServiceAccountSectionProps = { ownerKind: OwnerKind; path: string; view: SectionOk; actions: ServiceAccountActions }`
  - `ServiceAccountSection(props): ReactElement`
  - `CreateServiceAccountForm(props: { ownerPrn: string; disabled: boolean; onSubmit: (form: FormData) => void }): ReactElement`
  - `ServiceAccountRow(props: { row: ServiceAccountRowView; path: string; saOffset: number; selected: boolean }): ReactElement`
- Produces (`service-account-panel.tsx`, `'use client'`):
  - `type ServiceAccountPanelProps = { selected: SelectedView; path: string; saOffset: number; sa: string | null; hydrated: boolean; disabled: boolean; onAllow; onIssue; onRevoke; onArchive: (form: FormData) => void }`
  - `ServiceAccountPanel(props): ReactElement | null`, `IssueKeyForm`, `RevokeKeyButton`,
    `ArchiveServiceAccountButton`, `ARCHIVE_CONFIRMATION`
- Test ids the e2e tier uses: `sa-result`, `sa-create-form`, `sa-row`, `sa-row-archived`,
  `sa-read-only`, `sa-panel`, `sa-panel-archived`, `sa-panel-other`, `sa-panel-error`,
  `model-calls` (with `data-state`), `issue-key-form`, `api-keys`, `api-keys-error`,
  `api-key-row` (with `data-status`), `api-key-scope`, `archive-service-account`,
  `revoke-key-<id>`, `token-panel`, `token-value`.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/gateway-console/tests/unit/service-account-section.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The service-accounts section (SMA-636 spec § 4.6, § 5.4, § 7.1). Each case renders the real
// section with real transitions and real form submissions, then renders it AGAIN with the props
// that the revalidated page sends — the technique of iam-console's manage-controls.test.tsx.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { FormAction } from '@paigasus/console-core';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import { ServiceAccountSection } from '../../app/_components/service-account-section';
import type { ApiKeyRowView, IssueKeyState, SectionOk, SelectedView, ServiceAccountActions, ServiceAccountRowView } from '../../app/(console)/service-accounts/view';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/0190a100-0000-7000-8000-00000000000a' }));

afterEach(() => {
  cleanup();
});

const ORG_ID = '0190a100-0000-7000-8000-00000000000a';
const OWNER = `prn:pgs:iam:::organization/${ORG_ID}`;
const PATH = `/gateway/orgs/${ORG_ID}`;
const SA_ID = '0190a1e5-0000-7000-8000-0000000000a1';
const SA_PRN = `prn:pgs:iam:::principal/${SA_ID}`;
const TOKEN = 'pgs_unit_0123456789abcdef0123456789abcdef';

const ACCOUNT: ServiceAccountRowView = { prn: SA_PRN, id: SA_ID, name: 'ci-bot', created: '2026-09-18', active: true };
const ARCHIVED_ACCOUNT: ServiceAccountRowView = { ...ACCOUNT, active: false };
const KEY: ApiKeyRowView = { id: 'key-1', prefix: 'pgs_unit_old', status: 'active', created: '2026-09-18', expires: null, lastUsed: null, otherScope: null };
const ALL_CONTROLS = { allow: true, issue: true, revoke: true, archive: true };
const NO_CONTROLS = { allow: false, issue: false, revoke: false, archive: false };

function errorWith(presentation: Presentation, reason: ErrorReason | null = null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'corr-unit-sa',
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 13, codeName: 'Internal' },
  };
}

function selected(overrides: Partial<Extract<SelectedView, { kind: 'ok' }>> = {}): SelectedView {
  return { kind: 'ok', account: ACCOUNT, modelCalls: 'no', keys: { kind: 'ok', rows: [KEY], page: { offset: 0, nextOffset: null } }, controls: ALL_CONTROLS, ...overrides };
}

function view(overrides: Partial<SectionOk> = {}): SectionOk {
  return { kind: 'ok', ownerPrn: OWNER, readOnly: null, rows: [ACCOUNT], page: { offset: 0, nextOffset: null }, canCreate: true, selected: selected(), sa: SA_ID, saOffset: 0, keyOffset: 0, ...overrides };
}

const ok: FormAction = () => Promise.resolve({ ok: true });

function actions(overrides: Partial<ServiceAccountActions> = {}): ServiceAccountActions {
  return {
    create: () => Promise.resolve({ kind: 'created', saPrn: SA_PRN, granted: true }),
    allow: ok,
    issue: () => Promise.resolve({ ok: true, token: TOKEN, prefix: 'pgs_unit_new' }),
    revoke: ok,
    archive: ok,
    ...overrides,
  };
}

function section(v: SectionOk, a: ServiceAccountActions): ReactNode {
  return (
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      <ServiceAccountSection key={v.ownerPrn} ownerKind="organization" path={PATH} view={v} actions={a} />
    </ZoneProvider>
  );
}

/** Renders the props of the revalidated page, with an awaited act() (see manage-controls.test.tsx). */
async function refresh(rerender: (ui: ReactNode) => void, ui: ReactNode): Promise<void> {
  await act(() => {
    rerender(ui);
    return Promise.resolve();
  });
}

const region = (): HTMLElement => screen.getByTestId('sa-result');
const panel = (): HTMLElement => screen.getByTestId('sa-panel');

async function revokeFirstKey(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  const row = within(panel()).getByTestId('api-key-row');
  await user.click(within(row).getByRole('button', { name: 'Revoke' }));
  await user.click(within(row).getByRole('button', { name: 'Confirm revoke' }));
}

describe('one result region: a result survives the refresh that unmounts its control (§ 4.6)', () => {
  it('keeps "Service account archived." after the archived account loses every control', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(section(view(), a));

    await user.click(within(panel()).getByRole('button', { name: 'Archive' }));
    await user.click(within(panel()).getByRole('button', { name: 'Confirm archive' }));
    await within(region()).findByText('Service account archived.');
    await refresh(rerender, section(view({ rows: [ARCHIVED_ACCOUNT], selected: selected({ account: ARCHIVED_ACCOUNT, modelCalls: 'archived', controls: NO_CONTROLS }) }), a));

    expect(within(panel()).queryByRole('button', { name: 'Archive' })).toBeNull();
    expect(within(region()).getByText('Service account archived.')).toBeDefined();
  });

  it('keeps "Key revoked." after the revoked key loses its Revoke button', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(section(view(), a));

    await revokeFirstKey(user);
    await within(region()).findByText('Key revoked.');
    await refresh(rerender, section(view({ selected: selected({ keys: { kind: 'ok', rows: [{ ...KEY, status: 'revoked' }], page: { offset: 0, nextOffset: null } } }) }), a));

    expect(within(panel()).queryByRole('button', { name: 'Revoke' })).toBeNull();
    expect(within(region()).getByText('Key revoked.')).toBeDefined();
  });

  it('keeps "Model calls allowed." after the Allow control goes away', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(section(view(), a));

    await user.click(within(panel()).getByRole('button', { name: 'Allow model calls' }));
    await within(region()).findByText('Model calls allowed.');
    await refresh(rerender, section(view({ selected: selected({ modelCalls: 'yes', controls: { ...ALL_CONTROLS, allow: false } }) }), a));

    expect(within(panel()).queryByRole('button', { name: 'Allow model calls' })).toBeNull();
    expect(within(region()).getByText('Model calls allowed.')).toBeDefined();
  });

  it('says "may already be allowed" for a generic answer to Allow (§ 5.3)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ allow: () => Promise.resolve({ ok: false, error: errorWith('generic') }) })));

    await user.click(within(panel()).getByRole('button', { name: 'Allow model calls' }));

    expect(await within(region()).findByText('Model calls may already be allowed. The page was reloaded.')).toBeDefined();
    expect(within(region()).getByTestId('correlation-id').textContent).toBe('corr-unit-sa');
  });
});

describe('the create result (§ 5.2)', () => {
  async function create(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    const form = screen.getByRole('form', { name: 'Create service account' });
    await user.type(within(form).getByLabelText('Name'), 'ci-bot');
    await user.click(within(form).getByRole('button', { name: 'Create' }));
  }

  it('says the account can call models, and links to select it', async () => {
    const user = userEvent.setup();
    render(section(view({ selected: { kind: 'none' }, sa: null }), actions()));

    await create(user);

    expect(await within(region()).findByText('Service account created. It can call models.')).toBeDefined();
    expect(within(region()).getByRole('link', { name: 'Select it' }).getAttribute('href')).toBe(`/orgs/${ORG_ID}?sa=${SA_ID}`);
  });

  it('says a partial create cannot call models yet, with the grant error and the select link', async () => {
    const user = userEvent.setup();
    render(section(view({ selected: { kind: 'none' }, sa: null }), actions({ create: () => Promise.resolve({ kind: 'partial', saPrn: SA_PRN, error: errorWith('generic') }) })));

    await create(user);

    expect(await within(region()).findByText('Service account created, but it cannot call models yet.')).toBeDefined();
    expect(within(region()).getByTestId('form-error')).toBeDefined();
    expect(within(region()).getByRole('link', { name: 'Select it' })).toBeDefined();
  });

  it('says a name conflict in its own words', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ create: () => Promise.resolve({ kind: 'failed', error: errorWith('conflict', ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT) }) })));

    await create(user);

    expect(await within(region()).findByText(FORM_REASON_COPY[ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT] ?? '')).toBeDefined();
  });
});

describe('the token (§ 5.4)', () => {
  it('calls the issue action directly with null, and keeps the token through the refresh (rules 2, 3)', async () => {
    const user = userEvent.setup();
    const issue = vi.fn<ServiceAccountActions['issue']>(() => Promise.resolve({ ok: true, token: TOKEN, prefix: 'pgs_unit_new' }));
    const a = actions({ issue });
    const { rerender } = render(section(view(), a));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));
    await waitFor(() => {
      expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    });
    expect(issue).toHaveBeenCalledTimes(1);
    expect(issue.mock.calls[0]?.[0]).toBeNull();
    expect(issue.mock.calls[0]?.[1].get('expiry')).toBe('90');
    expect(issue.mock.calls[0]?.[1].get('saPrn')).toBe(SA_PRN);

    const NEW_KEY: ApiKeyRowView = { ...KEY, id: 'key-2', prefix: 'pgs_unit_new' };
    await refresh(rerender, section(view({ selected: selected({ keys: { kind: 'ok', rows: [KEY, NEW_KEY], page: { offset: 0, nextOffset: null } } }) }), a));

    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });

  it('keeps the token panel open after a revoke, so key rotation works (rule 6)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions()));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));
    await waitFor(() => {
      expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    });
    await revokeFirstKey(user);
    await within(region()).findByText('Key revoked.');

    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });

  it('shows an issue result although a later action started and finished first (rule 4)', async () => {
    const user = userEvent.setup();
    let finishIssue: (state: IssueKeyState) => void = () => undefined;
    const issue = (): Promise<IssueKeyState> =>
      new Promise((resolve) => {
        finishIssue = resolve;
      });
    render(section(view(), actions({ issue })));
    const row = within(panel()).getByTestId('api-key-row');
    // Open the revoke confirmation BEFORE the issue starts.
    await user.click(within(row).getByRole('button', { name: 'Revoke' }));
    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));

    // Rule 5 disables the submit, so the later action is started past it, as a stale page could.
    fireEvent.submit(within(row).getByRole('form', { name: 'Confirm revoke' }));
    await within(region()).findByText('Key revoked.');
    await act(async () => {
      finishIssue({ ok: true, token: TOKEN, prefix: 'pgs_unit_new' });
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    });
  });

  it('disables every other submit in the section while an issue is pending (rule 5)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ issue: () => new Promise<IssueKeyState>(() => undefined) })));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));

    await waitFor(() => {
      const enabled = screen.getAllByRole('button').filter((button) => !(button as HTMLButtonElement).disabled);
      expect(enabled.map((button) => button.textContent)).toEqual([]);
    });
  });

  it('says what a forbidden issue needs (§ 5.4)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ issue: () => Promise.resolve<IssueKeyState>({ ok: false, error: errorWith('forbidden') }) })));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));

    expect(await within(region()).findByText('You need permission to issue keys here and to grant every role this account holds.')).toBeDefined();
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('renders no issue form and no create form in the server HTML (rule 7)', () => {
    const html = renderToStaticMarkup(section(view(), actions()));
    expect(html).toContain('data-testid="sa-panel"');
    expect(html).not.toContain('data-testid="issue-key-form"');
    expect(html).not.toContain('data-testid="sa-create-form"');
    expect(html).not.toContain('Allow model calls');
  });
});

describe('what the section shows', () => {
  it('says why an archived owner is read-only (§ 5.8)', () => {
    render(section(view({ readOnly: 'archived', canCreate: false, selected: selected({ controls: NO_CONTROLS }) }), actions()));
    expect(screen.getByTestId('sa-read-only').textContent).toBe('This organization is archived. IAM refuses changes to its service accounts and keys.');
    expect(screen.queryByRole('form', { name: 'Create service account' })).toBeNull();
  });

  it('says that an account of another scope is not shown (§ 4.2)', () => {
    render(section(view({ selected: { kind: 'other-scope' } }), actions()));
    expect(screen.getByTestId('sa-panel-other').textContent).toBe('This service account belongs to another scope.');
  });

  it('shows the key status labels and the note for a key of another scope (§ 5.7)', () => {
    const PROJECT = `prn:pgs:iam::${ORG_ID}:project/0190a1c3-0000-7000-8000-0000000000a1`;
    render(section(view({ selected: selected({ keys: { kind: 'ok', rows: [{ ...KEY, status: 'expired', otherScope: PROJECT }], page: { offset: 0, nextOffset: null } } }) }), actions()));
    const row = within(panel()).getByTestId('api-key-row');
    expect(row.getAttribute('data-status')).toBe('expired');
    expect(row.textContent).toContain('Expired');
    expect(within(row).getByTestId('api-key-scope').textContent).toBe(`Scope: ${PROJECT}. The gateway checks model calls against this scope.`);
    expect(within(row).queryByRole('button', { name: 'Revoke' })).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/service-account-section.test.tsx
```

Expected: FAIL: vitest cannot load `service-account-section`.

- [ ] **Step 3: Implement the panel**

Create `ts/apps/gateway-console/app/_components/service-account-panel.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The selected service account (SMA-636 spec § 4.6, D14): "Can call models", "Allow model calls",
// the issue-key form, the keys list with its pager, and "Archive". CLIENT component. It holds NO
// result: every control hands its form to the section, whose ONE result region shows every result.
// The loader decided which controls show (PanelControls); this component only renders them.
'use client';

import { useId, type FormEvent, type ReactElement } from 'react';
import { EmptyState, PRIMARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { EXPIRY_CHOICES, EXPIRY_LABEL, KEY_STATUS_LABEL, type KeysView, type ModelCallState, type SelectedView } from '../(console)/service-accounts/view';
import { ConfirmButton } from './confirm-button';
import { FormError } from './form-error';
import { Pager } from './pager';

export type ServiceAccountPanelProps = {
  readonly selected: SelectedView;
  readonly path: string;
  readonly saOffset: number;
  readonly sa: string | null;
  /** Submit controls render only after hydration (§ 5.4 rule 7, plan SPEC DEVIATION 7). */
  readonly hydrated: boolean;
  /** true while any action of the section runs (§ 5.4 rule 5). */
  readonly disabled: boolean;
  readonly onAllow: (form: FormData) => void;
  readonly onIssue: (form: FormData) => void;
  readonly onRevoke: (form: FormData) => void;
  readonly onArchive: (form: FormData) => void;
};

const MODEL_CALLS_TEXT: Readonly<Record<ModelCallState, string>> = { yes: 'Yes', no: 'No', archived: 'No (account archived)', unknown: 'Unknown' };

/** § 5.6: the exact confirm text. */
export const ARCHIVE_CONFIRMATION = 'All keys of this account stop working. You cannot undo this in the console.';

function submitting(onSubmit: (form: FormData) => void) {
  return (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSubmit(new FormData(event.currentTarget));
  };
}

function AllowModelCallsForm({ saPrn, disabled, onSubmit }: { readonly saPrn: string; readonly disabled: boolean; readonly onSubmit: (form: FormData) => void }): ReactElement {
  return (
    <form aria-label="Allow model calls" onSubmit={submitting(onSubmit)}>
      <input type="hidden" name="saPrn" value={saPrn} />
      <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
        Allow model calls
      </button>
    </form>
  );
}

/** § 5.4: the expiry choice only (D7). The server computes the date, and the scope comes from IAM (D6). */
export function IssueKeyForm({ saPrn, disabled, onSubmit }: { readonly saPrn: string; readonly disabled: boolean; readonly onSubmit: (form: FormData) => void }): ReactElement {
  const id = useId();
  return (
    <form aria-label="Issue API key" data-testid="issue-key-form" className="flex max-w-md flex-col gap-2" onSubmit={submitting(onSubmit)}>
      <input type="hidden" name="saPrn" value={saPrn} />
      <label htmlFor={`${id}-expiry`} className="text-sm font-medium">
        Expiry
      </label>
      <select id={`${id}-expiry`} name="expiry" defaultValue="90" className="border-input rounded-pgs border px-2 py-1 text-sm">
        {EXPIRY_CHOICES.map((choice) => (
          <option key={choice} value={choice}>
            {EXPIRY_LABEL[choice]}
          </option>
        ))}
      </select>
      <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
        Issue key
      </button>
    </form>
  );
}

export function ArchiveServiceAccountButton({ saPrn, disabled, onConfirm }: { readonly saPrn: string; readonly disabled: boolean; readonly onConfirm: (form: FormData) => void }): ReactElement {
  return <ConfirmButton testId="archive-service-account" label="Archive" confirmLabel="Confirm archive" confirmation={ARCHIVE_CONFIRMATION} hidden={{ saPrn }} disabled={disabled} onConfirm={onConfirm} />;
}

export function RevokeKeyButton({
  saPrn,
  keyId,
  prefix,
  disabled,
  onConfirm,
}: {
  readonly saPrn: string;
  readonly keyId: string;
  readonly prefix: string;
  readonly disabled: boolean;
  readonly onConfirm: (form: FormData) => void;
}): ReactElement {
  return (
    <ConfirmButton
      testId={`revoke-key-${keyId}`}
      label="Revoke"
      confirmLabel="Confirm revoke"
      confirmation={`Revoke the key ${prefix}? A client that sends it can no longer call models.`}
      hidden={{ saPrn, keyId }}
      disabled={disabled}
      onConfirm={onConfirm}
    />
  );
}

type KeysListProps = {
  readonly keys: KeysView;
  readonly saPrn: string;
  readonly path: string;
  readonly saOffset: number;
  readonly sa: string | null;
  readonly revoke: boolean;
  readonly disabled: boolean;
  readonly onRevoke: (form: FormData) => void;
};

/** § 5.7: IAM's order, with the pager. Columns: prefix, status, created, expires, last used. */
function KeysList({ keys, saPrn, path, saOffset, sa, revoke, disabled, onRevoke }: KeysListProps): ReactElement | null {
  if (keys.kind === 'hidden') return null;
  if (keys.kind === 'error') {
    return (
      <div data-testid="api-keys-error">
        <FormError error={keys.error} />
      </div>
    );
  }
  return (
    <div data-testid="api-keys" className="flex flex-col gap-2">
      <h4 className="text-sm font-medium">API keys</h4>
      {keys.rows.length === 0 && keys.page.offset === 0 ? (
        <EmptyState title="No API keys yet" />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Prefix</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Created</TableHead>
              <TableHead>Expires</TableHead>
              <TableHead>Last used</TableHead>
              <TableHead>
                <span className="sr-only">Revoke</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {keys.rows.map((key) => (
              <TableRow key={key.id} data-testid="api-key-row" data-status={key.status}>
                <TableCell>
                  <code>{key.prefix}</code>
                  {key.otherScope === null ? null : (
                    <p data-testid="api-key-scope" className="text-muted-foreground text-xs">
                      {`Scope: ${key.otherScope}. The gateway checks model calls against this scope.`}
                    </p>
                  )}
                </TableCell>
                <TableCell>{KEY_STATUS_LABEL[key.status]}</TableCell>
                <TableCell>{key.created ?? ''}</TableCell>
                <TableCell>{key.expires ?? 'Never'}</TableCell>
                <TableCell>{key.lastUsed ?? 'Never'}</TableCell>
                <TableCell>{revoke && key.status === 'active' ? <RevokeKeyButton saPrn={saPrn} keyId={key.id} prefix={key.prefix} disabled={disabled} onConfirm={onRevoke} /> : null}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <Pager label="API key pages" path={path} param="keyOffset" offset={keys.page.offset} nextOffset={keys.page.nextOffset} keep={{ saOffset, sa }} />
    </div>
  );
}

export function ServiceAccountPanel(props: ServiceAccountPanelProps): ReactElement | null {
  const { selected } = props;
  if (selected.kind === 'none') return null;
  if (selected.kind === 'other-scope') {
    return (
      <p data-testid="sa-panel-other" className="text-muted-foreground text-sm">
        This service account belongs to another scope.
      </p>
    );
  }
  if (selected.kind === 'error') {
    return (
      <div data-testid="sa-panel-error">
        <FormError error={selected.error} />
      </div>
    );
  }
  const { account, modelCalls, keys, controls } = selected;
  return (
    <section aria-label={`Service account ${account.name}`} data-testid="sa-panel" className="flex flex-col gap-3 border-t pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-base font-semibold">{account.name}</h3>
        {account.active ? null : (
          <span data-testid="sa-panel-archived" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
            Archived
          </span>
        )}
      </div>
      <p data-testid="model-calls" data-state={modelCalls} className="text-sm">
        {`Can call models: ${MODEL_CALLS_TEXT[modelCalls]}`}
      </p>
      {controls.allow && props.hydrated ? <AllowModelCallsForm saPrn={account.prn} disabled={props.disabled} onSubmit={props.onAllow} /> : null}
      {controls.issue && props.hydrated ? <IssueKeyForm saPrn={account.prn} disabled={props.disabled} onSubmit={props.onIssue} /> : null}
      <KeysList keys={keys} saPrn={account.prn} path={props.path} saOffset={props.saOffset} sa={props.sa} revoke={controls.revoke} disabled={props.disabled} onRevoke={props.onRevoke} />
      {controls.archive ? <ArchiveServiceAccountButton saPrn={account.prn} disabled={props.disabled} onConfirm={props.onArchive} /> : null}
    </section>
  );
}
```

- [ ] **Step 4: Implement the section**

Create `ts/apps/gateway-console/app/_components/service-account-section.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section (SMA-636 spec § 4.6): the frame, ONE result region for every action
// of the section, the create form, the list, the pager and the selected-account panel. CLIENT
// component. The server block renders it with `key={ownerPrn}`, so its state — the result region
// and the token panel — stays through a revalidation of the SAME owner, and a move to another owner
// starts a new, empty instance (the ManageControls pattern, SMA-630).
//
// WHY ONE REGION. A row or panel control that succeeds often unmounts itself: a revoked key loses
// its Revoke button, an archived account loses every control. So every result, success or error,
// goes to this region, and no control holds the only copy of its result.
//
// HOW AN ACTION RUNS. Every control hands its form to this component, which calls the action
// DIRECTLY in a transition, with `null` as the previous state. § 5.4 rule 2 requires that for the
// issue action, because useActionState sends the previous state — the token — back to the server;
// the other actions follow the same path. A newer submission replaces the result of an older one
// (a generation counter), EXCEPT an issue result: a token that IAM minted is always shown (rule 4).
// While any action runs, every submit in the section is disabled (rule 5 needs that for an issue).
// Every submit control renders only after hydration (rule 7; plan SPEC DEVIATION 7 for the rest).
'use client';

import { useId, useRef, useState, useTransition, type ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import type { FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { EmptyState, Field, Input, PRIMARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { linkHref } from '../../lib/paging';
import { PARENT_ARCHIVED_NOTE } from '../(console)/node-status';
import { serviceAccountIdOf } from '../(console)/service-accounts/service-account-id';
import type { OwnerKind, ReadOnlyView, SectionOk, SectionResult, ServiceAccountActions, ServiceAccountRowView, SimpleControl } from '../(console)/service-accounts/view';
import { FormError } from './form-error';
import { Pager } from './pager';
import { ServiceAccountPanel } from './service-account-panel';
import { TokenPanel, type TokenPanelHandle } from './token-panel';
import { useHydrated } from './use-hydrated';

export type ServiceAccountSectionProps = {
  readonly ownerKind: OwnerKind;
  /** The page's full path (/gateway/orgs/<org>[/projects/<project>]), for the select and pager links. */
  readonly path: string;
  readonly view: SectionOk;
  readonly actions: ServiceAccountActions;
};

const SUCCESS_TEXT: Readonly<Record<SimpleControl, string>> = {
  allow: 'Model calls allowed.',
  revoke: 'Key revoked.',
  archive: 'Service account archived.',
  issue: 'Key issued.',
};

/** § 5.3: a generic answer to "Allow model calls" can be a duplicate grant (§ 3.2). */
const ALLOW_MAY_ALREADY = 'Model calls may already be allowed. The page was reloaded.';
/** § 5.4: a plain denial and IAM's D15 check both answer forbidden. */
const ISSUE_FORBIDDEN = 'You need permission to issue keys here and to grant every role this account holds.';

function overrideFor(control: SimpleControl, error: PaigasusError): string | undefined {
  if (control === 'allow' && error.presentation === 'generic') return ALLOW_MAY_ALREADY;
  if (control === 'issue' && error.presentation === 'forbidden') return ISSUE_FORBIDDEN;
  return undefined;
}

/** § 5.8. `unknown` is read-only with no note. */
function readOnlyNote(readOnly: ReadOnlyView, ownerKind: OwnerKind): string | null {
  if (readOnly === 'archived') return `This ${ownerKind} is archived. IAM refuses changes to its service accounts and keys.`;
  if (readOnly === 'archived-parent') return PARENT_ARCHIVED_NOTE;
  return null;
}

function ResultMessage({ result, path, saOffset }: { readonly result: SectionResult; readonly path: string; readonly saOffset: number }): ReactElement | null {
  if (result === null) return null;
  if (result.control === 'create') {
    const state = result.state;
    if (state.kind === 'failed') return <FormError error={state.error} />;
    const id = serviceAccountIdOf(state.saPrn);
    const select =
      id === null ? null : (
        <ZoneLink prefetch={false} href={linkHref(path, { saOffset, sa: id })} className="underline">
          Select it
        </ZoneLink>
      );
    if (state.kind === 'partial') {
      return (
        <>
          <p role="status" className="flex flex-wrap gap-2 text-sm">
            <span>Service account created, but it cannot call models yet.</span>
            {select}
          </p>
          <FormError error={state.error} />
        </>
      );
    }
    return (
      <p role="status" className="flex flex-wrap gap-2 text-sm">
        <span>{state.granted ? 'Service account created. It can call models.' : 'Service account created. This IAM does not offer role administration, so it cannot call models from here.'}</span>
        {select}
      </p>
    );
  }
  if (result.state.ok) {
    return (
      <p role="status" className="text-sm">
        {SUCCESS_TEXT[result.control]}
      </p>
    );
  }
  return <FormError error={result.state.error} message={overrideFor(result.control, result.state.error)} />;
}

export function CreateServiceAccountForm({ ownerPrn, disabled, onSubmit }: { readonly ownerPrn: string; readonly disabled: boolean; readonly onSubmit: (form: FormData) => void }): ReactElement {
  const id = useId();
  return (
    <form
      aria-label="Create service account"
      data-testid="sa-create-form"
      className="flex max-w-md flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit(new FormData(event.currentTarget));
      }}
    >
      <h3 className="text-sm font-medium">Create service account</h3>
      {/* Under client control, and safe: IAM checks CreateServiceAccount at this node (§ 5.1). */}
      <input type="hidden" name="ownerPrn" value={ownerPrn} />
      <Field label="Name" htmlFor={`${id}-name`}>
        <Input name="name" required autoComplete="off" />
      </Field>
      <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
        Create
      </button>
    </form>
  );
}

export function ServiceAccountRow({ row, path, saOffset, selected }: { readonly row: ServiceAccountRowView; readonly path: string; readonly saOffset: number; readonly selected: boolean }): ReactElement {
  return (
    <TableRow data-testid="sa-row" data-selected={String(selected)}>
      <TableCell>{row.name}</TableCell>
      <TableCell>{row.created ?? ''}</TableCell>
      <TableCell>
        {row.active ? (
          'Active'
        ) : (
          <span data-testid="sa-row-archived" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
            Archived
          </span>
        )}
      </TableCell>
      <TableCell>
        {row.id === null ? null : (
          <ZoneLink prefetch={false} href={linkHref(path, { saOffset, sa: row.id })} aria-label={`Select ${row.name}`} className="hover:underline">
            Select
          </ZoneLink>
        )}
      </TableCell>
    </TableRow>
  );
}

function ServiceAccountList({ view, path }: { readonly view: SectionOk; readonly path: string }): ReactElement {
  if (view.rows.length === 0 && view.saOffset === 0) return <EmptyState title="No service accounts yet" />;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Created</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>
            <span className="sr-only">Select</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {view.rows.map((row) => (
          <ServiceAccountRow key={row.prn} row={row} path={path} saOffset={view.saOffset} selected={row.id !== null && row.id === view.sa} />
        ))}
      </TableBody>
    </Table>
  );
}

export function ServiceAccountSection({ ownerKind, path, view, actions }: ServiceAccountSectionProps): ReactElement {
  const hydrated = useHydrated();
  const [result, setResult] = useState<SectionResult>(null);
  const [working, startWork] = useTransition();
  const [issuing, startIssue] = useTransition();
  const generation = useRef(0);
  const tokenPanel = useRef<TokenPanelHandle>(null);
  const busy = working || issuing;
  const note = readOnlyNote(view.readOnly, ownerKind);

  function runCreate(form: FormData): void {
    generation.current += 1;
    const mine = generation.current;
    setResult(null);
    startWork(async () => {
      const state = await actions.create(null, form);
      if (state !== null && generation.current === mine) setResult({ control: 'create', state });
    });
  }

  function run(control: 'allow' | 'revoke' | 'archive', action: FormAction, form: FormData): void {
    generation.current += 1;
    const mine = generation.current;
    setResult(null);
    startWork(async () => {
      const state = await action(null, form);
      if (state !== null && generation.current === mine) setResult({ control, state });
    });
  }

  function runIssue(form: FormData): void {
    generation.current += 1;
    setResult(null);
    startIssue(async () => {
      const state = await actions.issue(null, form);
      // NO generation check (§ 5.4 rule 4): a token that IAM minted is always shown.
      if (state === null) return;
      if (state.ok) tokenPanel.current?.show(state.token, state.prefix);
      else setResult({ control: 'issue', state });
    });
  }

  return (
    <div className="flex flex-col gap-4">
      {note === null ? null : (
        <p data-testid="sa-read-only" className="text-muted-foreground text-sm">
          {note}
        </p>
      )}
      <div data-testid="sa-result" className="flex flex-col gap-2">
        <ResultMessage result={result} path={path} saOffset={view.saOffset} />
        <TokenPanel ref={tokenPanel} />
      </div>
      {view.canCreate && hydrated ? <CreateServiceAccountForm ownerPrn={view.ownerPrn} disabled={busy} onSubmit={runCreate} /> : null}
      <ServiceAccountList view={view} path={path} />
      <Pager label="Service account pages" path={path} param="saOffset" offset={view.page.offset} nextOffset={view.page.nextOffset} keep={{ sa: view.sa }} />
      <ServiceAccountPanel
        selected={view.selected}
        path={path}
        saOffset={view.saOffset}
        sa={view.sa}
        hydrated={hydrated}
        disabled={busy}
        onAllow={(form) => {
          run('allow', actions.allow, form);
        }}
        onRevoke={(form) => {
          run('revoke', actions.revoke, form);
        }}
        onArchive={(form) => {
          run('archive', actions.archive, form);
        }}
        onIssue={runIssue}
      />
    </div>
  );
}
```

- [ ] **Step 5: Run the test and confirm it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/service-account-section.test.tsx
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
pnpm --dir ts exec eslint apps/gateway-console
```

Expected: PASS, 16 tests. Typecheck and lint exit 0.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the service-accounts section with one result region (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 16: The organization and project settings pages

§ 4.1, § 4.4, D4, D5, D10, § 7.4 (the scope-page re-baseline). The gateway README records the
limits of § 9.

**Files:**
- Create: `ts/apps/gateway-console/app/_components/gateway-state-line.tsx`
- Create: `ts/apps/gateway-console/app/(console)/service-accounts/block.tsx`
- Create: `ts/apps/gateway-console/app/(console)/orgs/[org]/projects-block.tsx`
- Modify: `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx:1-39` (whole file)
- Create: `ts/apps/gateway-console/app/(console)/orgs/[org]/projects/[project]/page.tsx`
- Modify: `ts/apps/gateway-console/lib/nav.ts` (append after line 31)
- Modify: `ts/apps/gateway-console/README.md:5-6`, `:85`
- Test: `ts/apps/gateway-console/tests/unit/nav.test.ts:16` and three new cases
- Test: `ts/apps/gateway-console/tests/integration/scope-page.test.ts` (whole file)
- Test: `ts/apps/gateway-console/tests/integration/project-page.test.ts` (create)

**Interfaces:**
- Consumes: `loadOrganizationSettings`, `projectsBlock` input `ProjectsView` (Task 12);
  `loadProjectSettings` (Task 12); the five actions (Task 13); `ServiceAccountSection` (Task 15);
  `SectionError` (Task 14); `parseOffset` (Task 7); `parseAccountParam` (Task 8); `BADGE_LABEL`,
  `lifecycleView` (Task 8).
- Produces:
  - `type ManageTarget = { kind: 'organization'; orgId: string } | { kind: 'project'; orgId: string; teamId: string | null; projectId: string }`
  - `iamManageHref(zones: ZoneMap, target: ManageTarget): string | null` (`lib/nav.ts`)
  - `GatewayStateLine(props: { view: GatewayView }): ReactElement`
  - `serviceAccountsBlock(props: { view: SectionView; ownerKind: OwnerKind; path: string }): Promise<ReactElement>`
  - `projectsBlock(props: { orgId: string; projects: ProjectsView }): Promise<ReactElement>`
  - Page test ids: `org-settings`, `project-settings`, `node-status`, `manage-in-iam`,
    `gateway-state-line`, `service-accounts`, `service-accounts-denied`, `projects`,
    `project-group`, `more-teams`, `more-projects`, `project-team`.

- [ ] **Step 1: Write the failing tests**

In `ts/apps/gateway-console/tests/unit/nav.test.ts`, replace line 16 with:

```ts
const { GATEWAY_BASE_PATH, buildNavEntries, iamManageHref } = await import('../../lib/nav');
```

and append at the end of the file:

```ts
describe('iamManageHref (SMA-636 spec § 4.4)', () => {
  const ORG = '0190a100-0000-7000-8000-00000000000a';
  const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';
  const PROJECT = '0190a1c3-0000-7000-8000-0000000000a1';

  it('points an organization at the IAM zone, honouring a non-default mount prefix', () => {
    expect(iamManageHref(ZONES, { kind: 'organization', orgId: ORG })).toBe(`/iam/orgs/${ORG}`);
    expect(iamManageHref({ iam: '/identity' }, { kind: 'organization', orgId: ORG })).toBe(`/identity/orgs/${ORG}`);
  });

  it('points a project at its team route in the IAM zone', () => {
    expect(iamManageHref(ZONES, { kind: 'project', orgId: ORG, teamId: TEAM, projectId: PROJECT })).toBe(`/iam/orgs/${ORG}/teams/${TEAM}/projects/${PROJECT}`);
  });

  it('is null with no IAM zone in the map, and for a project whose team is not known', () => {
    expect(iamManageHref({ gateway: '/gateway' }, { kind: 'organization', orgId: ORG })).toBeNull();
    expect(iamManageHref(ZONES, { kind: 'project', orgId: ORG, teamId: null, projectId: PROJECT })).toBeNull();
  });
});
```

Replace `ts/apps/gateway-console/tests/integration/scope-page.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org] — the organization settings page (SMA-636 spec § 4.1, § 4.2). SMA-636
// § 7.4 re-baselines this file: SMA-512's scope route rendered the zone overview here. A denied
// GetOrganization is still why forbidden.tsx is reachable in this zone: PageError calls forbidden()
// for a `forbidden` presentation. next/navigation's forbidden() and notFound() are MOCKED, so a case
// asserts the SPECIFIC call was made (with no argument), not merely that something threw.
//
// This file is `.ts`, not `.tsx`, so the next/link double below uses createElement rather than JSX.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { Code } from '@connectrpc/connect';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, projectPrn, resetDiscoveryForTest, teamPrn } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { PageError } from '../../app/_components/page-error';
import { serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

const { forbiddenMock, notFoundMock } = vi.hoisted(() => ({
  forbiddenMock: vi.fn(() => {
    throw new Error('called forbidden()');
  }),
  notFoundMock: vi.fn(() => {
    throw new Error('called notFound()');
  }),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), forbidden: forbiddenMock, notFound: notFoundMock, usePathname: () => '/orgs' }));
// ZoneLink renders next/link, which needs the App Router's context that a static render does not have.
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => createElement('a', { href }, children),
}));

import OrganizationSettingsPage from '../../app/(console)/orgs/[org]/page';

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  forbiddenMock.mockClear();
  notFoundMock.mockClear();
});

afterEach(() => {
  resetDiscoveryForTest();
});

function render(element: ReactElement): string {
  return renderToStaticMarkup(createElement(ZoneProvider, { zone: 'gateway', zones: { gateway: '/gateway' }, children: element }));
}

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
const SA_A = serviceAccountPrn(IDS.saA);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

function world(overrides: FakeIamHandlers = {}): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme Corp', ...ACTIVE } }),
    'tenancy.listTeams': () => ({ teams: [{ prn: TEAM_A1, orgPrn: ORG_A, slug: 'platform', name: 'Platform Team', ...ACTIVE }] }),
    'tenancy.listProjects': () => ({ projects: [{ prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'gw', name: 'Inference Gateway', ...ACTIVE }] }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [{ prn: SA_A, ownerPrn: ORG_A, name: 'ci-bot', status: 'active' }] }),
    'serviceAccounts.getServiceAccount': () => ({ serviceAccount: { prn: SA_A, ownerPrn: ORG_A, name: 'ci-bot', status: 'active' } }),
    ...overrides,
  };
}

function page(org: string, query: Record<string, string> = {}): Promise<ReactElement> {
  return OrganizationSettingsPage({ params: Promise.resolve({ org }), searchParams: Promise.resolve(query) });
}

describe('the organization settings page', () => {
  it('renders the header, the gateway line, the service accounts and the projects — not the zone overview', async () => {
    env.iam.setHandlers(world());

    const html = render(await page(IDS.orgA));

    expect(html).toContain('data-testid="org-settings"');
    expect(html).toContain('Acme Corp');
    expect(html).toContain('data-testid="gateway-state-line"');
    expect(html).toContain('data-testid="service-accounts"');
    expect(html).toContain('ci-bot');
    expect(html).toContain('data-testid="projects"');
    expect(html).toContain('Platform Team');
    // ZoneLink hands next/link the zone-relative remainder.
    expect(html).toContain(`href="/orgs/${IDS.orgA}/projects/${IDS.projectA1}"`);
    expect(html).not.toContain('data-testid="zone-overview"');
    // The single-zone map of this tier has no IAM zone, so there is no "Manage in IAM" link (§ 4.4).
    expect(html).not.toContain('data-testid="manage-in-iam"');
    expect(env.iam.callsTo('tenancy.getOrganization').at(-1)?.request).toMatchObject({ prn: ORG_A });
  });

  it('loads the account that ?sa= selects', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.callsTo('serviceAccounts.getServiceAccount').length;

    const html = render(await page(IDS.orgA, { sa: IDS.saA }));

    expect(html).toContain('data-testid="sa-panel"');
    const calls = env.iam.callsTo('serviceAccounts.getServiceAccount').slice(before);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.request).toMatchObject({ prn: SA_A });
  });

  it('shows only the section denial, and still renders the page, when ListServiceAccounts is forbidden', async () => {
    env.iam.setHandlers(
      world({
        'serviceAccounts.listServiceAccounts': () => {
          throw denial();
        },
      }),
    );

    const html = render(await page(IDS.orgA));

    expect(html).toContain('data-testid="service-accounts-denied"');
    expect(html).toContain('You cannot view service accounts here.');
    expect(html).toContain('data-testid="projects"');
  });

  it('calls forbidden() when GetOrganization is denied, and the correlation id survives into the error the view would render', async () => {
    const sentId = '0198f2c1-8888-7000-8000-000000000099';
    env.iam.setHandlers(
      world({
        'tenancy.getOrganization': () => {
          throw denial({ code: Code.PermissionDenied, reason: 'forbidden', correlationId: sentId });
        },
      }),
    );

    const element = await page(IDS.orgA);

    expect(element.type).toBe(PageError);
    const { error } = element.props as { error: PaigasusError };
    expect(error.presentation).toBe('forbidden');
    expect(error.correlationId).toBe(sentId);
    await expect(PageError(element.props as { error: PaigasusError })).rejects.toThrow('called forbidden()');
    expect(forbiddenMock).toHaveBeenCalledTimes(1);
    expect(forbiddenMock).toHaveBeenCalledWith();
    expect(notFoundMock).not.toHaveBeenCalled();
  });

  it('calls notFound() for a non-UUID [org] segment, and makes NO IAM call', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.calls.length;

    await expect(page('not-a-uuid')).rejects.toThrow('called notFound()');

    expect(notFoundMock).toHaveBeenCalledTimes(1);
    expect(notFoundMock).toHaveBeenCalledWith();
    expect(env.iam.calls.length).toBe(before);
  });

  it('calls notFound(), not the error view, when IAM answers invalid-input (prn-mismatch)', async () => {
    env.iam.setHandlers(
      world({
        'tenancy.getOrganization': () => {
          throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch' });
        },
      }),
    );

    await expect(page(IDS.orgA)).rejects.toThrow('called notFound()');

    expect(notFoundMock).toHaveBeenCalledTimes(1);
    expect(forbiddenMock).not.toHaveBeenCalled();
  });
});
```

Create `ts/apps/gateway-console/tests/integration/project-page.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org]/projects/[project] — the project settings page (SMA-636 spec § 4.1, § 4.2,
// D5). A failed GetTeam or GetOrganization keeps the page and shows the UUID instead of the name.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, projectPrn, resetDiscoveryForTest, teamPrn } from '@paigasus/console-core';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { PageError } from '../../app/_components/page-error';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

const { notFoundMock } = vi.hoisted(() => ({
  notFoundMock: vi.fn(() => {
    throw new Error('called notFound()');
  }),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), notFound: notFoundMock, usePathname: () => '/orgs' }));
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => createElement('a', { href }, children),
}));

import ProjectSettingsPage from '../../app/(console)/orgs/[org]/projects/[project]/page';

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  notFoundMock.mockClear();
});

afterEach(() => {
  resetDiscoveryForTest();
});

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

function world(overrides: FakeIamHandlers = {}): FakeIamHandlers {
  return {
    'tenancy.getProject': (req) => ({ project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'gw', name: 'Inference Gateway', ...ACTIVE } }),
    'tenancy.getTeam': () => ({ team: { prn: TEAM_A1, orgPrn: ORG_A, slug: 'platform', name: 'Platform Team', ...ACTIVE } }),
    'tenancy.getOrganization': () => ({ organization: { prn: ORG_A, slug: 'acme', name: 'Acme Corp', ...ACTIVE } }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }),
    ...overrides,
  };
}

function render(element: ReactElement): string {
  return renderToStaticMarkup(createElement(ZoneProvider, { zone: 'gateway', zones: { gateway: '/gateway' }, children: element }));
}

function page(project: string = IDS.projectA1): Promise<ReactElement> {
  return ProjectSettingsPage({ params: Promise.resolve({ org: IDS.orgA, project }), searchParams: Promise.resolve({}) });
}

describe('the project settings page', () => {
  it('renders the project, its team, the breadcrumb link to its organization, and a section owned by the project', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.callsTo('serviceAccounts.listServiceAccounts').length;

    const html = render(await page());

    expect(html).toContain('data-testid="project-settings"');
    expect(html).toContain('Inference Gateway');
    expect(html).toContain('Team: Platform Team');
    expect(html).toContain(`<a href="/orgs/${IDS.orgA}">Acme Corp</a>`);
    expect(html).toContain('data-testid="service-accounts"');
    expect(env.iam.callsTo('serviceAccounts.listServiceAccounts').slice(before)[0]?.request).toMatchObject({ ownerPrn: PROJECT_A1 });
  });

  it('shows the UUIDs when GetTeam and GetOrganization are denied, and still renders', async () => {
    env.iam.setHandlers(
      world({
        'tenancy.getTeam': () => {
          throw denial();
        },
        'tenancy.getOrganization': () => {
          throw denial();
        },
      }),
    );

    const html = render(await page());

    expect(html).toContain(`Team: ${IDS.teamA1}`);
    expect(html).toContain(`<a href="/orgs/${IDS.orgA}">${IDS.orgA}</a>`);
  });

  it('returns the page error for a denied project', async () => {
    env.iam.setHandlers(
      world({
        'tenancy.getProject': () => {
          throw denial();
        },
      }),
    );
    expect((await page()).type).toBe(PageError);
  });

  it('calls notFound() for a non-UUID [project] segment, and makes NO IAM call', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.calls.length;

    await expect(page('gateway')).rejects.toThrow('called notFound()');

    expect(env.iam.calls.length).toBe(before);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/nav.test.ts tests/integration/scope-page.test.ts tests/integration/project-page.test.ts
```

Expected: FAIL. `nav.test.ts`: `iamManageHref is not a function`. `scope-page.test.ts`: the old page
renders `zone-overview` and no `org-settings`. `project-page.test.ts`: the page module does not
exist.

- [ ] **Step 3: Add `iamManageHref`**

Append to `ts/apps/gateway-console/lib/nav.ts`:

```ts

/** What a "Manage in IAM" link points at (SMA-636 spec § 4.4). */
export type ManageTarget = { readonly kind: 'organization'; readonly orgId: string } | { readonly kind: 'project'; readonly orgId: string; readonly teamId: string | null; readonly projectId: string };

/**
 * The "Manage in IAM" href (SMA-636 spec § 4.4, D10): the same node in the IAM zone, from the zone
 * map's `iam` entry, in the pattern of buildNavEntries above. Null when the map has no IAM zone
 * (the single-zone e2e tier), and for a project whose team is not known, because the IAM zone's
 * project route carries the team.
 */
export function iamManageHref(zones: ZoneMap, target: ManageTarget): string | null {
  const iamBase = zones['iam'];
  if (iamBase === undefined) return null;
  if (target.kind === 'organization') return `${iamBase}/orgs/${target.orgId}`;
  return target.teamId === null ? null : `${iamBase}/orgs/${target.orgId}/teams/${target.teamId}/projects/${target.projectId}`;
}
```

- [ ] **Step 4: Add the line, the two blocks and the two pages**

Create `ts/apps/gateway-console/app/_components/gateway-state-line.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The gateway's state in one line (SMA-636 spec § 4.1: "the gateway state line, in compact form").
// The settings pages show it; the overview keeps the full view. Server-safe and client-safe.
import type { ReactElement } from 'react';
import type { GatewayView } from './gateway-state';

const LINE: Readonly<Record<GatewayView['state'], string>> = {
  available: 'The gateway is available.',
  degraded: 'The gateway is not available.',
  absent: 'The gateway is not configured.',
};

export function GatewayStateLine({ view }: { readonly view: GatewayView }): ReactElement {
  return (
    <p data-testid="gateway-state-line" data-state={view.state} className="text-muted-foreground text-sm">
      {LINE[view.state]}
    </p>
  );
}
```

Create `ts/apps/gateway-console/app/(console)/service-accounts/block.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section of both settings pages (SMA-636 spec § 4.1, § 4.2, § 4.6). A plain
// async FUNCTION, awaited by its page, not a JSX component: its error branch awaits SectionError,
// and a page must return a fully resolved tree (the reason app/_components/error-tail.tsx records).
//
// A forbidden list shows only this section's denial; the page stays 200. It passes the same five
// Server Actions on both pages: no action takes its owner from the page (plan SPEC DEVIATION 5).
import type { ReactElement } from 'react';
import { SectionError } from '../../_components/section-error';
import { ServiceAccountSection } from '../../_components/service-account-section';
import { allowModelCallsAction, archiveServiceAccountAction, createServiceAccountAction, issueApiKeyAction, revokeApiKeyAction } from './actions';
import type { OwnerKind, SectionView } from './view';

export async function serviceAccountsBlock({ view, ownerKind, path }: { readonly view: SectionView; readonly ownerKind: OwnerKind; readonly path: string }): Promise<ReactElement> {
  let body: ReactElement;
  if (view.kind === 'denied') {
    body = (
      <p data-testid="service-accounts-denied" className="text-muted-foreground text-sm">
        You cannot view service accounts here.
      </p>
    );
  } else if (view.kind === 'error') {
    body = await SectionError({ error: view.error });
  } else {
    body = (
      <ServiceAccountSection
        key={view.ownerPrn}
        ownerKind={ownerKind}
        path={path}
        view={view}
        actions={{ create: createServiceAccountAction, allow: allowModelCallsAction, issue: issueApiKeyAction, revoke: revokeApiKeyAction, archive: archiveServiceAccountAction }}
      />
    );
  }
  return (
    <section aria-labelledby="service-accounts-heading" data-testid="service-accounts" className="flex flex-col gap-3">
      <h2 id="service-accounts-heading" className="text-lg font-semibold">
        Service accounts
      </h2>
      {body}
    </section>
  );
}
```

Create `ts/apps/gateway-console/app/(console)/orgs/[org]/projects-block.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The Projects section of the organization page (SMA-636 spec § 4.1, § 4.2): the organization's
// projects, grouped by team. A team is a grouping label only (D4); a project links to its settings
// page with prefetch={false} (§ 4.1). At most 50 teams, and 50 projects per team, show; a full list
// says "More exist. See the IAM zone." A plain async FUNCTION, awaited by the page, because its
// error branch awaits SectionError.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { EmptyState } from '@paigasus/ui';
import { SectionError } from '../../../_components/section-error';
import { GATEWAY_BASE_PATH } from '../../../../lib/nav';
import type { ProjectsView, TeamGroup } from './load';

const MORE = 'More exist. See the IAM zone.';

function TeamProjects({ orgId, team }: { readonly orgId: string; readonly team: TeamGroup }): ReactElement {
  return (
    <div data-testid="project-group" className="flex flex-col gap-1">
      <h3 className="text-sm font-medium">{team.name}</h3>
      {team.projects.length === 0 ? (
        <p className="text-muted-foreground text-sm">No projects.</p>
      ) : (
        <ul className="flex flex-col gap-1 text-sm">
          {team.projects.map((project) => (
            <li key={project.projectId ?? project.slug}>
              {project.projectId === null ? (
                project.name
              ) : (
                <ZoneLink prefetch={false} href={`${GATEWAY_BASE_PATH}/orgs/${orgId}/projects/${project.projectId}`} className="hover:underline">
                  {project.name}
                </ZoneLink>
              )}
            </li>
          ))}
        </ul>
      )}
      {team.moreProjects ? (
        <p data-testid="more-projects" className="text-muted-foreground text-xs">
          {MORE}
        </p>
      ) : null}
    </div>
  );
}

export async function projectsBlock({ orgId, projects }: { readonly orgId: string; readonly projects: ProjectsView }): Promise<ReactElement> {
  let body: ReactElement;
  if (projects.kind === 'error') {
    body = await SectionError({ error: projects.error });
  } else if (projects.teams.length === 0) {
    body = <EmptyState title="No teams yet" />;
  } else {
    body = (
      <div className="flex flex-col gap-4">
        {projects.teams.map((team) => (
          <TeamProjects key={team.teamId ?? team.name} orgId={orgId} team={team} />
        ))}
        {projects.moreTeams ? (
          <p data-testid="more-teams" className="text-muted-foreground text-xs">
            {MORE}
          </p>
        ) : null}
      </div>
    );
  }
  return (
    <section aria-labelledby="projects-heading" data-testid="projects" className="flex flex-col gap-3">
      <h2 id="projects-heading" className="text-lg font-semibold">
        Projects
      </h2>
      {body}
    </section>
  );
}
```

Replace `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org] — the organization settings (SMA-636 spec § 4.1, § 4.2). It replaces the
// SMA-512 scope route, which changed only the URL and the breadcrumbs.
//
// GetOrganization first (in the loader): a denial is the 403 view through PageError, and nothing
// else runs; this route is still why forbidden.tsx is reachable in this zone. Then the
// service-accounts section and the Projects list. A section failure never fails the page.
//
// Rename and archive of the organization stay in the IAM zone (D10): the page links there when the
// zone map has an IAM zone (§ 4.4).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { isUuid } from '@paigasus/console-core';
import { gatewayView } from '../../../_components/gateway-state';
import { GatewayStateLine } from '../../../_components/gateway-state-line';
import { PageError } from '../../../_components/page-error';
import { getPublicConfig } from '../../../../lib/config';
import { currentSession, discovery, iamClients, mayI } from '../../../../lib/console';
import { GATEWAY_BASE_PATH, iamManageHref } from '../../../../lib/nav';
import { parseOffset } from '../../../../lib/paging';
import { BADGE_LABEL, lifecycleView } from '../../node-status';
import { serviceAccountsBlock } from '../../service-accounts/block';
import { parseAccountParam } from '../../service-accounts/service-account-id';
import { loadOrganizationSettings } from './load';
import { projectsBlock } from './projects-block';

type Props = {
  params: Promise<{ org: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function OrganizationSettingsPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org)) notFound();
  const [session, clients, may] = await Promise.all([currentSession(), iamClients(), mayI()]);
  const probe = discovery();
  const [iam, gateway] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken)]);
  const data = await loadOrganizationSettings(
    { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: may, iam, now: Date.now },
    { org, saOffset: parseOffset(query['saOffset']), keyOffset: parseOffset(query['keyOffset']), sa: parseAccountParam(query['sa']) },
  );
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  const path = `${GATEWAY_BASE_PATH}/orgs/${data.orgId}`;
  const manage = iamManageHref(getPublicConfig().zones, { kind: 'organization', orgId: data.orgId });
  const badge = BADGE_LABEL[lifecycleView(data.organization.lifecycle)];
  return (
    <div className="flex flex-col gap-8 p-6" data-testid="org-settings">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: data.organization.name }]} />
      <header className="flex flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.organization.name}</h1>
          {badge === null ? null : (
            <span data-testid="node-status" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
              {badge}
            </span>
          )}
        </div>
        <p className="text-muted-foreground text-sm">{data.organization.slug}</p>
        {manage === null ? null : (
          <ZoneLink href={manage} data-testid="manage-in-iam" className="text-sm underline">
            Manage in IAM
          </ZoneLink>
        )}
      </header>
      <GatewayStateLine view={gatewayView(gateway)} />
      {await serviceAccountsBlock({ view: data.section, ownerKind: 'organization', path })}
      {await projectsBlock({ orgId: data.orgId, projects: data.projects })}
    </div>
  );
}
```

Create `ts/apps/gateway-console/app/(console)/orgs/[org]/projects/[project]/page.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org]/projects/[project] — the project settings (SMA-636 spec § 4.1, § 4.2, D5: a
// flat route with no team segment). The header names the project, its slug, its team and its
// status; the breadcrumbs are Overview › <org> › <project>. A failed GetTeam or GetOrganization
// shows the UUID instead of the name. A URL that pairs one organization with another
// organization's project answers 403 or 404 and never shows the project (§ 4.1).
//
// Rename and archive of the project stay in the IAM zone (D10).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { isUuid } from '@paigasus/console-core';
import { gatewayView } from '../../../../../_components/gateway-state';
import { GatewayStateLine } from '../../../../../_components/gateway-state-line';
import { PageError } from '../../../../../_components/page-error';
import { getPublicConfig } from '../../../../../../lib/config';
import { currentSession, discovery, iamClients, mayI } from '../../../../../../lib/console';
import { GATEWAY_BASE_PATH, iamManageHref } from '../../../../../../lib/nav';
import { parseOffset } from '../../../../../../lib/paging';
import { BADGE_LABEL, lifecycleView } from '../../../../node-status';
import { serviceAccountsBlock } from '../../../../service-accounts/block';
import { parseAccountParam } from '../../../../service-accounts/service-account-id';
import { loadProjectSettings } from './load';

type Props = {
  params: Promise<{ org: string; project: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function ProjectSettingsPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, project }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(project)) notFound();
  const [session, clients, may] = await Promise.all([currentSession(), iamClients(), mayI()]);
  const probe = discovery();
  const [iam, gateway] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken)]);
  const data = await loadProjectSettings(
    { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: may, iam, now: Date.now },
    { org, project, saOffset: parseOffset(query['saOffset']), keyOffset: parseOffset(query['keyOffset']), sa: parseAccountParam(query['sa']) },
  );
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  const orgPath = `${GATEWAY_BASE_PATH}/orgs/${data.orgId}`;
  const path = `${orgPath}/projects/${data.projectId}`;
  const manage = iamManageHref(getPublicConfig().zones, { kind: 'project', orgId: data.orgId, teamId: data.team.id, projectId: data.projectId });
  const badge = BADGE_LABEL[lifecycleView(data.project.lifecycle)];
  return (
    <div className="flex flex-col gap-8 p-6" data-testid="project-settings">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: data.organizationName ?? data.orgId, href: orgPath }, { label: data.project.name }]} />
      <header className="flex flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.project.name}</h1>
          {badge === null ? null : (
            <span data-testid="node-status" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
              {badge}
            </span>
          )}
        </div>
        <p className="text-muted-foreground text-sm">{data.project.slug}</p>
        <p data-testid="project-team" className="text-sm">
          {`Team: ${data.team.name ?? data.team.id ?? 'unknown'}`}
        </p>
        {manage === null ? null : (
          <ZoneLink href={manage} data-testid="manage-in-iam" className="text-sm underline">
            Manage in IAM
          </ZoneLink>
        )}
      </header>
      <GatewayStateLine view={gatewayView(gateway)} />
      {await serviceAccountsBlock({ view: data.section, ownerKind: 'project', path })}
    </div>
  );
}
```

- [ ] **Step 5: Update the README**

In `ts/apps/gateway-console/README.md`, replace lines 5-6 with:

```markdown
deployment. It has the login, the console shell, the zone overview, and the organization and
project settings pages (SMA-636). Design: `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md`
and `docs/superpowers/specs/2026-09-18-sma-636-gateway-org-project-settings-design.md`.
```

Replace line 85 (the "organization scope route changes the URL …" bullet; it is line 86 after the
edit above) with:

```markdown
- The settings pages list the service accounts that the organization or the project owns, and their API keys (SMA-636). An account that a team owns is not visible in this zone, so an organization admin has no complete list of live keys here.
- Creating a service account makes two IAM calls, not one atomic call. A failed `gateway_user` grant leaves an account that cannot call models until someone uses "Allow model calls".
- An account that got `gateway_user` outside the console, at an ancestor scope, shows "Can call models: Yes". The console cannot show where the grant comes from.
- There is no "Stop model calls" control: `RevokeRole` needs a grant id, and only a platform admin can list another principal's grants. Archive the account or revoke its keys instead.
- After an archive, IAM evicts the account's keys from its API-key cache. How fast every IAM replica stops accepting them depends on IAM's cache configuration, which the console does not check.
- `FormError`, `node-status.ts`, `section-error.tsx` and the two-step confirm button are copies of iam-console's. Nothing gates a divergence.
- The organization page makes one `ListProjects` call per shown team (at most 50, at most 8 in flight). Row R19 counts the calls of one render; nothing measures their latency against a real IAM.
- With JavaScript off, the settings pages are read-only: every mutation control renders after hydration.
```

- [ ] **Step 6: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/nav.test.ts tests/integration/scope-page.test.ts tests/integration/project-page.test.ts
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
pnpm --dir ts exec eslint apps/gateway-console
```

Expected: PASS (nav +3, scope-page 6, project-page 4). Typecheck and lint exit 0.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the organization and project settings pages to the gateway zone (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 17: "Your projects" on the overview

D16.

**Files:**
- Create: `ts/apps/gateway-console/app/_components/your-projects.tsx`
- Modify: `ts/apps/gateway-console/app/(console)/overview/page.tsx:1-20` (whole file)
- Test: `ts/apps/gateway-console/tests/unit/your-projects.test.tsx` (create)
- Test: `ts/apps/gateway-console/tests/integration/overview-page.test.ts` (whole file)

**Interfaces:**
- Consumes: `IamResult`, `MyScopes`, `ScopeEntry` (types) from `@paigasus/console-core`;
  `myScopes` from `lib/console`; `GATEWAY_BASE_PATH` from `lib/nav`.
- Produces: `yourProjectRows(scopes: IamResult<MyScopes>, basePath: string): YourProjectRow[]`
  with `YourProjectRow = { key: string; kind: 'team' | 'project'; label: string; href: string | null }`;
  `YourProjects(props: { scopes: IamResult<MyScopes>; basePath: string }): ReactElement` (test id
  `your-projects`).

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/your-projects.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// "Your projects" on the overview (SMA-636 D16): the TEAM and PROJECT entries of myScopes(), a
// project as a link to its settings page, with no prefetch. An organization entry is the switcher's.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { IamResult, MyScopes } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { YourProjects, yourProjectRows } from '../../app/_components/your-projects';

vi.mock('next/link', () => ({
  default: ({ href, prefetch, children }: { href: string; prefetch?: boolean; children?: ReactNode }) => (
    <a href={href} data-prefetch={String(prefetch)}>
      {children}
    </a>
  ),
}));

const ORG = '0190a100-0000-7000-8000-00000000000a';
const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';
const PROJECT = '0190a1c3-0000-7000-8000-0000000000a1';

const SCOPES: IamResult<MyScopes> = {
  ok: true,
  value: {
    entries: [
      { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG}`, orgId: ORG, label: 'Acme', denied: false },
      { kind: 'team', prn: `prn:pgs:iam::${ORG}:team/${TEAM}`, orgId: ORG, teamId: TEAM, label: 'Platform Team', denied: false },
      { kind: 'project', prn: `prn:pgs:iam::${ORG}:project/${PROJECT}`, orgId: ORG, teamId: TEAM, projectId: PROJECT, label: null, denied: false },
    ],
    hiddenCount: 2,
    grantsListed: true,
  },
};

function render(scopes: IamResult<MyScopes>): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      <YourProjects scopes={scopes} basePath="/gateway" />
    </ZoneProvider>,
  );
}

describe('Your projects', () => {
  it('keeps the team and project entries only, and links a project to its settings page', () => {
    expect(yourProjectRows(SCOPES, '/gateway')).toEqual([
      { key: `prn:pgs:iam::${ORG}:team/${TEAM}`, kind: 'team', label: 'Platform Team', href: null },
      { key: `prn:pgs:iam::${ORG}:project/${PROJECT}`, kind: 'project', label: PROJECT, href: `/gateway/orgs/${ORG}/projects/${PROJECT}` },
    ]);
  });

  it('renders a team as a label and a project as a link that does not prefetch, and says how many are hidden', () => {
    const html = render(SCOPES);
    expect(html).toContain('data-testid="your-projects"');
    expect(html).toContain('Team: Platform Team');
    expect(html).toContain(`href="/orgs/${ORG}/projects/${PROJECT}" data-prefetch="false"`);
    expect(html).not.toContain('Acme');
    expect(html).toContain('2 more scopes are not shown.');
  });

  it('says so when there is no team or project scope, and when the scopes could not be loaded', () => {
    expect(render({ ok: true, value: { entries: [], hiddenCount: 0, grantsListed: true } })).toContain('No team or project scopes');
    const error = { presentation: 'degraded' } as PaigasusError;
    expect(render({ ok: false, error })).toContain('Your teams and projects could not be loaded.');
  });
});
```

Replace `ts/apps/gateway-console/tests/integration/overview-page.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/overview against the REAL discovery() and myScopes(), with a scripted fake gateway (spec
// § 10.3) and a scripted fake IAM (SMA-636 D16). The page is awaited, then the element it returns is
// rendered with renderToStaticMarkup inside a ZoneProvider, because "Your projects" renders ZoneLink.
//
// RECORDED LIMIT (spec § 10.3): the `absent` state of the gateway view is UNREACHABLE at this tier —
// lib/config.ts refuses a PAIGASUS_SERVICES map with no `gateway` entry. That branch is covered only
// by tests/unit/zone-overview.test.tsx.
//
// This file is `.ts`, not `.tsx`, so the next/link double below uses createElement rather than JSX.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, projectPrn, resetDiscoveryForTest, teamPrn } from '@paigasus/console-core';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => createElement('a', { href }, children),
}));

const { default: OverviewPage } = await import('../../app/(console)/overview/page');

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  // Every case starts from a healthy, capable gateway and an IAM with no scripted scope, so a case
  // that forgets to script anything fails loudly rather than inheriting the PREVIOUS case's state.
  env.gateway.setReachable(true);
  env.gateway.setServiceInfo({ service: 'gateway', version: '0.0.0-fake', capabilities: ['gateway.chat.stream'] });
  env.iam.setHandlers({});
});

afterEach(() => {
  resetDiscoveryForTest();
});

function render(element: ReactElement): string {
  return renderToStaticMarkup(createElement(ZoneProvider, { zone: 'gateway', zones: { gateway: '/gateway' }, children: element }));
}

describe('OverviewPage', () => {
  it('reports available with streaming and the version when the descriptor lists gateway.chat.stream', async () => {
    env.gateway.setServiceInfo({ service: 'gateway', version: '1.2.3', capabilities: ['gateway.chat.stream'] });

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="true"');
    expect(html).toContain('1.2.3');
  });

  it('reports available without streaming when the descriptor omits the capability', async () => {
    env.gateway.setServiceInfo({ service: 'gateway', version: '1.2.3', capabilities: [] });

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="false"');
  });

  it('reports degraded with a rendered reason when the gateway answers a bad status', async () => {
    env.gateway.setServiceInfo({ status: 503 });

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
  });

  it('reports degraded with a rendered reason when the gateway is unreachable', async () => {
    env.gateway.setReachable(false);

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
  });

  it('lists the team and project scopes of myScopes() as "Your projects", a project as a link to its page (D16)', async () => {
    const ORG_A = organizationPrn(IDS.orgA);
    const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
    const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
    env.iam.setHandlers({
      'authn.introspect': () => ({
        memberships: [
          { id: '0190a1d4-0000-7000-8000-0000000000c1', principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0', nodePrn: TEAM_A1 },
          { id: '0190a1d4-0000-7000-8000-0000000000c2', principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0', nodePrn: PROJECT_A1 },
        ],
      }),
      'tenancy.getTeam': () => ({ team: { prn: TEAM_A1, orgPrn: ORG_A, slug: 'platform', name: 'Platform Team' } }),
      'tenancy.getProject': () => ({ project: { prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'gw', name: 'Inference Gateway' } }),
    });

    const html = render(await OverviewPage());

    expect(html).toContain('data-testid="your-projects"');
    expect(html).toContain('Team: Platform Team');
    expect(html).toContain(`<a href="/orgs/${IDS.orgA}/projects/${IDS.projectA1}">Inference Gateway</a>`);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/your-projects.test.tsx tests/integration/overview-page.test.ts
```

Expected: FAIL. `your-projects.test.tsx` cannot load its module. The last overview case finds no
`your-projects` in the page.

- [ ] **Step 3: Implement the list and add it to the overview**

Create `ts/apps/gateway-console/app/_components/your-projects.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// "Your projects" on the overview (SMA-636 D16): the TEAM and PROJECT entries of myScopes(), a
// project as a link to its settings page. It reads the SAME myScopes() result as the organization
// switcher — a React cache() — so this list costs no IAM call. A project_admin cannot read the
// organization, so the organization page is a 403 for them; without this list they would reach a
// project page only by typing its URL. There is no team route (D4), so a team is a label.
//
// Server-safe and client-safe: its only runtime import is ZoneLink. Links do not prefetch (§ 4.1).
// At most the 50 scopes myScopes() shows (SCOPE_CAP) appear here.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import type { IamResult, MyScopes, ScopeEntry } from '@paigasus/console-core';
import { EmptyState } from '@paigasus/ui';

export type YourProjectRow = { readonly key: string; readonly kind: 'team' | 'project'; readonly label: string; readonly href: string | null };

export function yourProjectRows(scopes: IamResult<MyScopes>, basePath: string): YourProjectRow[] {
  if (!scopes.ok) return [];
  return scopes.value.entries.flatMap((entry: ScopeEntry): YourProjectRow[] => {
    if (entry.kind === 'team') return [{ key: entry.prn, kind: 'team', label: entry.label ?? entry.teamId, href: null }];
    if (entry.kind === 'project') return [{ key: entry.prn, kind: 'project', label: entry.label ?? entry.projectId, href: `${basePath}/orgs/${entry.orgId}/projects/${entry.projectId}` }];
    return [];
  });
}

function ListBody({ scopes, rows }: { readonly scopes: IamResult<MyScopes>; readonly rows: readonly YourProjectRow[] }): ReactElement {
  if (!scopes.ok) return <p className="text-muted-foreground text-sm">Your teams and projects could not be loaded.</p>;
  if (rows.length === 0) return <EmptyState title="No team or project scopes" />;
  return (
    <ul className="flex flex-col gap-1 text-sm">
      {rows.map((row) => (
        <li key={row.key} data-kind={row.kind}>
          {row.href === null ? (
            <span>{`Team: ${row.label}`}</span>
          ) : (
            <ZoneLink prefetch={false} href={row.href} className="hover:underline">
              {row.label}
            </ZoneLink>
          )}
        </li>
      ))}
    </ul>
  );
}

export function YourProjects({ scopes, basePath }: { readonly scopes: IamResult<MyScopes>; readonly basePath: string }): ReactElement {
  const rows = yourProjectRows(scopes, basePath);
  return (
    <section aria-labelledby="your-projects-heading" data-testid="your-projects" className="flex flex-col gap-2">
      <h2 id="your-projects-heading" className="text-lg font-semibold">
        Your projects
      </h2>
      <ListBody scopes={scopes} rows={rows} />
      {scopes.ok && scopes.value.hiddenCount > 0 ? <p className="text-muted-foreground text-xs">{`${String(scopes.value.hiddenCount)} more scopes are not shown.`}</p> : null}
    </section>
  );
}
```

Replace `ts/apps/gateway-console/app/(console)/overview/page.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/overview (spec § 6, plan D14; SMA-636 D16). currentSession(), discovery() and myScopes()
// are React cache() wrappers built by the one createConsoleRuntime call, and Discovery memoizes
// getServiceState per handle — so this page and the layout above it share ONE session resolution,
// ONE gateway probe and ONE myScopes() walk per request. "Your projects" therefore costs no call.
// That sharing is exactly what a second createConsoleRuntime() call would break, invisibly.
//
// This route is /gateway/overview, NOT /gateway/: (public)/page.tsx already owns the latter, and
// two page.tsx at one path fail the Next build (plan D14).
import type { ReactElement } from 'react';
import { gatewayView } from '../../_components/gateway-state';
import { YourProjects } from '../../_components/your-projects';
import { ZoneOverview } from '../../_components/zone-overview';
import { currentSession, discovery, myScopes } from '../../../lib/console';
import { GATEWAY_BASE_PATH } from '../../../lib/nav';

export default async function OverviewPage(): Promise<ReactElement> {
  const session = await currentSession();
  const [state, scopes] = await Promise.all([discovery().getServiceState('gateway', session.accessToken), myScopes()]);
  return (
    <div className="flex flex-col">
      <ZoneOverview view={gatewayView(state)} scope={null} />
      <div className="px-8 pb-8">
        <YourProjects scopes={scopes} basePath={GATEWAY_BASE_PATH} />
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/your-projects.test.tsx tests/integration/overview-page.test.ts tests/unit/zone-overview.test.tsx
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
pnpm --dir ts exec eslint apps/gateway-console
```

Expected: PASS. Typecheck and lint exit 0.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): list the user's teams and projects on the gateway overview (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 18: The stateful e2e world, the R5 extension, and the R9/R11 re-baselines

§ 7.2 (the world), § 7.4 (R5, R11, `two-zone-session.spec.ts`). After Task 16, R9 and R11 fail:
they wait for `zone-overview` on `/gateway/orgs/<org>`.

**Files:**
- Modify: `ts/apps/gateway-console/tests/e2e/support/world.ts:1-80` (whole file)
- Modify: `ts/apps/gateway-console/tests/e2e/token-leak.spec.ts:1-200` (whole file)
- Modify: `ts/apps/gateway-console/tests/e2e/two-zone-session.spec.ts:75`
- Modify: `ts/apps/gateway-console/tests/e2e/two-zone-runtime.spec.ts:38`

**Interfaces:**
- Consumes: `ApiKeyStatus`, `NodeStatus` from `@paigasus/sdk/iam/types` (Task 1); `denial`,
  `FakeIamHandlers` from `@paigasus/console-core/testing`.
- Produces (`world.ts`): the old exports (`PRINCIPAL_PRN`, `ORG_ID`, `ORG_PRN`, `ORG_NAME`,
  `Descriptor`, `DEFAULT_IAM_DESCRIPTOR` — now with `iam.apikeys` — `DEFAULT_GATEWAY_DESCRIPTOR`,
  `WorldOptions`, `worldHandlers`), plus `TEAM_ID`, `TEAM_PRN`, `TEAM_NAME`, `PROJECT_ID`,
  `PROJECT_PRN`, `PROJECT_NAME`, `SEEDED_SA_ID`, `SEEDED_SA_PRN`, `SEEDED_SA_NAME`,
  `TOKEN_PREFIX = 'pgs_e2e_'`. `WorldOptions` gains `allow?: readonly string[]`,
  `failFirstGrant?: boolean`, `seedServiceAccount?: boolean`, `projectAdmin?: boolean`.

- [ ] **Step 1: Re-baseline R9 and R11, and extend R5 (the failing tests)**

In `ts/apps/gateway-console/tests/e2e/two-zone-session.spec.ts`, replace line 75 with:

```ts
  // SMA-636 § 7.4: /gateway/orgs/<org> is the organization settings page now, not the zone overview.
  await expect(page.getByTestId('org-settings')).toBeVisible();
```

In `ts/apps/gateway-console/tests/e2e/two-zone-runtime.spec.ts`, replace line 38 with:

```ts
  // SMA-636 § 7.4: /gateway/orgs/<org> is the organization settings page now, not the zone overview.
  await expect(page.getByTestId('org-settings')).toBeVisible();
```

Replace `ts/apps/gateway-console/tests/e2e/token-leak.spec.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017: the browser never receives a token. Every response the page receives in a signed-in
// session is collected: HTML documents, RSC payloads (navigations and prefetches) alike, and — since
// SMA-636 gave this zone its first Server Actions — a Server Action result. None may contain the
// access or the refresh token the fake IdP issued. SMA-636 § 7.4 extends this row the way
// iam-console's copy already is: it runs one action (create a service account) and requires the
// action's body to be buffered and scanned.
//
// EVERY RSC BODY AND THE ACTION BODY ARE BUFFERED BY THIS TEST, not read from Playwright afterwards,
// for the measured reason iam-console's copy documents: a streamed answer can arrive with
// `bodySize: -1`, and `response.body()` then rejects or hangs under load. `page.route` fetches those
// responses, reads their text in this process, and fulfils each request with the same bytes, so the
// scan never depends on Playwright retaining a streamed body.
import type { Request, Response } from '@playwright/test';
import { ORG_ID } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

/** The response stream counts as quiet after this long with no new response. */
const QUIESCE_MS = 500;
/** A bound on the quiesce wait, so a page that never goes quiet fails the row rather than hanging it. */
const QUIESCE_TIMEOUT_MS = 10_000;

type Seen = {
  readonly url: string;
  readonly method: string;
  readonly contentType: string;
  /** A POST with Next's `next-action` header: a Server Action call. */
  readonly action: boolean;
  /**
   * Whether HTTP permits this response to carry a body at all. Only HEAD, 204, 205 and 304 are
   * truly bodyless. A 3xx MAY carry one, so it is no longer classified away — see `redirect`.
   */
  readonly expectsBody: boolean;
  /** A 3xx whose body Playwright's Response.body() contract refuses. Exempt POSITIVELY, like prefetchRsc. */
  readonly redirect: boolean;
  /** true only after the body was read. */
  readonly bodyRead: boolean;
  /** true when the body came from the route interceptor, not from `response.body()`. */
  readonly buffered: boolean;
  /** The one class the row may leave unread, when the interceptor could not buffer it. */
  readonly prefetchRsc: boolean;
  readonly body: string;
  /** The headers and the body: what the leak scan searches. */
  readonly text: string;
};

/**
 * The LAST-RESORT class of response whose body this row may leave unread, matched POSITIVELY on
 * the request: `Next-Router-Prefetch: 1` together with the `_rsc=` query the router adds. See
 * iam-console's token-leak.spec.ts for the measurement this mirrors.
 */
async function isPrefetchRsc(response: Response): Promise<boolean> {
  const request = response.request();
  const headers = await request.allHeaders();
  return headers['next-router-prefetch'] === '1' && new URL(request.url()).searchParams.has('_rsc');
}

/** A Server Action call: a POST that carries Next's own action header. */
function isServerAction(request: Request): boolean {
  return request.method() === 'POST' && request.headers()['next-action'] !== undefined;
}

/** An RSC payload fetch: the GET the App Router makes for a client navigation or a prefetch. */
function isRscGet(request: Request): boolean {
  return request.method() === 'GET' && new URL(request.url()).searchParams.has('_rsc');
}

async function capture(response: Response, buffered: ReadonlyMap<Request, string>): Promise<Seen> {
  const request = response.request();
  const headers = await response.allHeaders();
  const requestHeaders = await request.allHeaders();
  const status = response.status();
  // HTTP-correct, not status-class shorthand. Only these four cases genuinely carry no body.
  const expectsBody = request.method() !== 'HEAD' && status !== 204 && status !== 205 && status !== 304;
  const redirect = status >= 300 && status < 400;
  const prefetchRsc = await isPrefetchRsc(response);
  const fromRoute = buffered.get(request);
  let body = fromRoute ?? '';
  let bodyRead = fromRoute !== undefined;
  if (!bodyRead && expectsBody && !prefetchRsc) {
    try {
      body = (await response.body()).toString('utf8');
      bodyRead = true;
    } catch {
      // The read failed. bodyRead stays false, so no vacuity guard counts this response as
      // scanned AND the residue assertion reports it.
    }
  }
  return {
    url: response.url(),
    method: request.method(),
    contentType: headers['content-type'] ?? '',
    action: requestHeaders['next-action'] !== undefined,
    expectsBody,
    redirect,
    bodyRead,
    buffered: fromRoute !== undefined,
    prefetchRsc,
    body,
    text: `${JSON.stringify(headers)}\n${body}`,
  };
}

/** true when the body was read and is not empty, so the leak scan searched it. */
function scanned(response: Seen): boolean {
  return response.bodyRead && response.body.length > 0;
}

test('R5: no response body, header, RSC payload or action result contains a fake token (ADR-0017)', async ({ page, harness }) => {
  // The buffered bodies, keyed by the Request the response carries — the SAME object the route
  // handler saw, so no url or timing match is needed.
  const buffered = new Map<Request, string>();
  // Only the streaming classes are fetched and refilled: a Server Action POST to a settings page
  // and every same-origin RSC GET. Documents and static assets fall through untouched, so the
  // interception cannot change how the page loads.
  await page.route(
    (url) => url.origin === harness.origin && (url.pathname.startsWith('/gateway/orgs/') || url.searchParams.has('_rsc')),
    async (route) => {
      const request = route.request();
      if (!isServerAction(request) && !isRscGet(request)) {
        await route.fallback();
        return;
      }
      try {
        // maxRedirects: 0 — the browser must see a redirect as a redirect.
        const answer = await route.fetch({ maxRedirects: 0 });
        const text = await answer.text();
        buffered.set(request, text);
        const headers = { ...answer.headers() };
        delete headers['content-encoding'];
        delete headers['content-length'];
        await route.fulfill({ status: answer.status(), headers, body: text });
      } catch {
        // The router cancelled the request, or the page closed. Put it back on the ordinary path
        // rather than failing the handler: capture() then records it unread, and the residue
        // assertion reports it unless it is the measured prefetch class.
        await route.fallback().catch(() => undefined);
      }
    },
  );

  const pending: Promise<Seen>[] = [];
  // NAMED, so the listener can be detached before the drain below.
  const onResponse = (response: Response): void => {
    pending.push(capture(response, buffered));
  };
  page.on('response', onResponse);
  const issuedBefore = harness.idp.issued.length;

  const { accessToken, refreshToken } = await signIn(page, harness, '/gateway/overview');
  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await waitForHydration(page);

  // One Server Action (SMA-636 § 7.4): create a service account through the form.
  const create = page.getByTestId('service-accounts').getByRole('form', { name: 'Create service account' });
  await create.getByLabel('Name').fill('leak-check');
  await create.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByTestId('sa-result').getByRole('status')).toContainText('Service account created.');

  // A client-side navigation, so the RSC payload path is exercised, not only full documents.
  const rsc = page.waitForResponse((response) => (response.headers()['content-type'] ?? '').startsWith('text/x-component') && response.request().method() === 'GET');
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Overview', exact: true }).click();
  await rsc;
  await page.waitForURL((url) => url.pathname === '/gateway/overview');

  // Wait for the stream to go quiet BEFORE detaching the listener, so a late payload is scanned
  // rather than dropped. Bounded, so a chatty page cannot hang this row.
  const quiesceDeadline = Date.now() + QUIESCE_TIMEOUT_MS;
  let settledCount = -1;
  while (pending.length !== settledCount && Date.now() < quiesceDeadline) {
    settledCount = pending.length;
    await page.waitForTimeout(QUIESCE_MS);
  }
  const quiesced = pending.length === settledCount;
  page.off('response', onResponse);
  const seen = await Promise.all(pending);

  expect(quiesced, `the response stream did not go quiet within ${String(QUIESCE_TIMEOUT_MS)} ms, so this scan may be missing responses and cannot be trusted`).toBe(true);
  const tokens = [accessToken, refreshToken, ...harness.idp.issued.slice(issuedBefore).flatMap((issued) => [issued.accessToken, issued.refreshToken])];
  expect(tokens.length).toBeGreaterThanOrEqual(2);
  for (const token of tokens) expect(token.length).toBeGreaterThanOrEqual(16);

  const leaks = seen.filter((response) => tokens.some((token) => response.text.includes(token))).map((response) => `${response.method} ${response.url}`);
  expect(leaks).toEqual([]);

  // The RESIDUE: every response that should carry a body, and whose body this row did not read,
  // must belong to the one measured class.
  expect(seen.filter((response) => response.expectsBody && !response.bodyRead && !response.prefetchRsc && !response.redirect).map((response) => `${response.method} ${response.url}`)).toEqual([]);
  console.log(
    `R5 scan: ${String(seen.length)} responses, ${String(seen.filter(scanned).length)} scanned, ${String(seen.filter((response) => response.buffered).length)} buffered, ${String(seen.filter((response) => response.action).length)} action, ${String(seen.filter((response) => response.expectsBody && !response.bodyRead).length)} unread`,
  );

  // Vacuity guards: for each kind of response the row names, at least one non-empty body was READ
  // and scanned.
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'POST' && response.action && scanned(response))).toBe(true);
  // The load regressions stay visible: the action body AND an RSC GET body were scanned FROM THE
  // ROUTE BUFFER, not merely because response.body() happened to work.
  expect(seen.some((response) => response.action && response.method === 'POST' && response.buffered && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'GET' && !response.action && response.contentType.startsWith('text/x-component') && response.buffered && scanned(response))).toBe(true);
});
```

- [ ] **Step 2: Build and run the three rows, and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:build iam-console-ts:build
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test tests/e2e/token-leak.spec.ts --project single-zone
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test --project two-zone
```

Expected: R5 FAILS: the old world has no `serviceAccounts.*` handler, so the create answers
Unimplemented and "Service account created." never shows. R9 and R11 PASS now that they wait for
`org-settings`; they FAILED before Step 1 (confirm this by reading the diff: they waited for
`zone-overview`). The two-zone project needs Docker.

- [ ] **Step 3: Rewrite the world**

Replace `ts/apps/gateway-console/tests/e2e/support/world.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The IAM and gateway state the e2e tier scripts (SMA-512 spec § 10.4; SMA-636 spec § 7.2).
// `setHandlers()` REPLACES the whole map, so worldHandlers() always returns the FULL set.
//
// STATEFUL since SMA-636. One worldHandlers() call is one world: it records the service accounts,
// the keys and the grants that the create, issue, revoke, archive and grant calls make, and every
// later answer of the SAME world reads them. harness.useWorld() starts a new world, so a test never
// sees another test's accounts. IsAuthorized about a SERVICE ACCOUNT answers from the recorded
// gateway_user grants, so the main row cannot show "Can call models: Yes" without a grant.
// IsAuthorized about the signed-in USER answers from an allow list, as in iam-console's world.
//
// The two-zone tier serves the iam-console app from this same world, so the handlers that app's
// organization page needs (listTeams, listMemberships) are here too.
//
// PRNs are literal strings: @paigasus/console-core's prn-tenancy.ts imports server-only, which
// throws under Playwright. NodeStatus and ApiKeyStatus come from @paigasus/sdk's guard-free
// ./iam/types entry.
import { randomUUID } from 'node:crypto';
import { Code, ConnectError } from '@connectrpc/connect';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { ApiKeyStatus, NodeStatus } from '@paigasus/sdk/iam/types';

export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const ORG_NAME = 'Acme Research';
export const TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e2';
export const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
export const TEAM_NAME = 'Platform Team';
export const PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e3';
export const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;
export const PROJECT_NAME = 'Inference Gateway';

/** The account `seedServiceAccount` puts into a world: owned by the organization, active, no key, no grant. */
export const SEEDED_SA_ID = '0190a1e5-0000-7000-8000-0000000000a1';
export const SEEDED_SA_PRN = `prn:pgs:iam:::principal/${SEEDED_SA_ID}`;
export const SEEDED_SA_NAME = 'seeded-bot';

/** Every token this world issues is this prefix and then 32 hex digits. */
export const TOKEN_PREFIX = 'pgs_e2e_';

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_IAM_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.apikeys'] };
export const DEFAULT_GATEWAY_DESCRIPTOR: Descriptor = { service: 'gateway', version: '0.0.0-e2e', capabilities: ['gateway.chat.stream'] };

export type WorldOptions = {
  /** false: a first-time identity with no membership and no grant. Default: true. */
  readonly memberships?: boolean;
  /** What the fake IAM's `GET /v1/service-info` answers. Default: DEFAULT_IAM_DESCRIPTOR. */
  readonly iamDescriptor?: Descriptor;
  /** What the fake gateway's `GET /v1/service-info` answers. Default: DEFAULT_GATEWAY_DESCRIPTOR. */
  readonly gatewayDescriptor?: Descriptor;
  /** Replace single handlers, for example with one that throws denial(). */
  readonly overrides?: FakeIamHandlers;
  /** The actions IsAuthorized allows the SIGNED-IN USER. Default: every action. */
  readonly allow?: readonly string[];
  /** true: the FIRST GrantRole fails with an internal error; later ones succeed (§ 7.2 row 2). */
  readonly failFirstGrant?: boolean;
  /** true: the world starts with SEEDED_SA_*, owned by the organization. */
  readonly seedServiceAccount?: boolean;
  /** true: the user is project_admin of PROJECT only — no membership, and no organization access (§ 7.2 row 5). */
  readonly projectAdmin?: boolean;
};

const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };
const CREATED = { createdAt: { seconds: 1_788_000_000n, nanos: 0 } };

const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME, ...ACTIVE };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME, ...ACTIVE };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME, ...ACTIVE };

type Account = { readonly prn: string; readonly ownerPrn: string; readonly name: string; status: string };
type Key = {
  readonly id: string;
  readonly serviceAccountPrn: string;
  readonly scopePrn: string;
  readonly prefix: string;
  status: ApiKeyStatus;
  readonly expiresAt: { readonly seconds: bigint; readonly nanos: number } | undefined;
};
type Grant = { readonly id: string; readonly principalPrn: string; readonly roleKey: string; readonly scopePrn: string };

const notFound = (): Error => denial({ code: Code.NotFound, reason: 'not-found' });

/** A grant at `scopePrn` covers `resourcePrn` when it is the same node or its organization (Cedar's `resource in ?resource`). */
function covers(scopePrn: string, resourcePrn: string): boolean {
  return scopePrn === resourcePrn || (scopePrn === ORG_PRN && resourcePrn.startsWith(`prn:pgs:iam::${ORG_ID}:`));
}

/** IAM's offset paging: limit 0 is the server default of 50. */
function page<T>(items: readonly T[], request: { readonly limit: number; readonly offset: bigint }): T[] {
  const start = Number(request.offset);
  return items.slice(start, start + (request.limit === 0 ? 50 : request.limit));
}

/** The signed-in user's OWN role grants (myScopes() lists them). */
function userGrants(projectAdmin: boolean, withScopes: boolean) {
  if (projectAdmin) return [{ id: '0190a1d4-0000-7000-8000-0000000000f6', principalPrn: PRINCIPAL_PRN, roleKey: 'project_admin', scopePrn: PROJECT_PRN }];
  if (withScopes) return [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: PROJECT_PRN }];
  return [];
}

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const projectAdmin = options.projectAdmin === true;
  const withScopes = (options.memberships ?? true) && !projectAdmin;
  const allow = options.allow === undefined ? null : new Set(options.allow);
  const accounts: Account[] = options.seedServiceAccount === true ? [{ prn: SEEDED_SA_PRN, ownerPrn: ORG_PRN, name: SEEDED_SA_NAME, status: 'active' }] : [];
  const keys: Key[] = [];
  const grants: Grant[] = [];
  let grantFailuresLeft = options.failFirstGrant === true ? 1 : 0;

  const accountView = (account: Account) => ({ prn: account.prn, ownerPrn: account.ownerPrn, name: account.name, status: account.status, audit: CREATED });
  const keyView = (key: Key) => ({
    id: key.id,
    serviceAccountPrn: key.serviceAccountPrn,
    scopePrn: key.scopePrn,
    prefix: key.prefix,
    status: key.status,
    ...(key.expiresAt === undefined ? {} : { expiresAt: { seconds: key.expiresAt.seconds, nanos: key.expiresAt.nanos } }),
    audit: CREATED,
  });
  const accountAt = (prn: string): Account => {
    const account = accounts.find((candidate) => candidate.prn === prn);
    if (account === undefined) throw notFound();
    return account;
  };
  /** A call a project_admin may not make: it reads the organization or the team (§ 7.2 row 5). */
  const organizationOnly = (): void => {
    if (projectAdmin) throw denial();
  };

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
    'authz.listRoleGrants': () => ({ grants: userGrants(projectAdmin, withScopes) }),
    'authz.isAuthorized': (req) => {
      if (req.principalPrn === PRINCIPAL_PRN) return { allowed: allow === null || allow.has(req.action), determiningPolicies: [], reason: '' };
      // About a service account: only InvokeModel, and only from a recorded gateway_user grant.
      const allowed = req.action === 'InvokeModel' && grants.some((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === 'gateway_user' && covers(grant.scopePrn, req.resourcePrn));
      return { allowed, determiningPolicies: [], reason: '' };
    },
    'authz.grantRole': (req) => {
      if (grantFailuresLeft > 0) {
        grantFailuresLeft -= 1;
        throw new ConnectError('the e2e world fails this grant', Code.Internal);
      }
      // IAM answers a duplicate grant with an internal error, not a conflict (SMA-636 spec § 3.2).
      if (grants.some((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === req.roleKey && grant.scopePrn === req.scopePrn)) {
        throw new ConnectError('duplicate grant', Code.Internal);
      }
      const grant: Grant = { id: randomUUID(), principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn };
      grants.push(grant);
      return { grant };
    },
    'tenancy.getOrganization': (req) => {
      organizationOnly();
      if (req.prn !== ORG_PRN) throw notFound();
      return { organization: ORGANIZATION };
    },
    'tenancy.getTeam': (req) => {
      organizationOnly();
      if (req.prn !== TEAM_PRN) throw notFound();
      return { team: TEAM };
    },
    'tenancy.getProject': (req) => {
      if (req.prn !== PROJECT_PRN) throw notFound();
      return { project: PROJECT };
    },
    'tenancy.listTeams': (req) => {
      organizationOnly();
      return { teams: req.orgPrn === ORG_PRN ? [TEAM] : [] };
    },
    'tenancy.listProjects': (req) => ({ projects: req.teamPrn === TEAM_PRN ? [PROJECT] : [] }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'serviceAccounts.createServiceAccount': (req) => {
      const name = req.name.trim();
      if (accounts.some((account) => account.ownerPrn === req.ownerPrn && account.name === name)) throw denial({ code: Code.AlreadyExists, reason: 'service-account-name-conflict' });
      const account: Account = { prn: `prn:pgs:iam:::principal/${randomUUID()}`, ownerPrn: req.ownerPrn, name, status: 'active' };
      accounts.push(account);
      return { serviceAccount: accountView(account) };
    },
    'serviceAccounts.getServiceAccount': (req) => ({ serviceAccount: accountView(accountAt(req.prn)) }),
    'serviceAccounts.listServiceAccounts': (req) => ({ serviceAccounts: page(accounts.filter((account) => account.ownerPrn === req.ownerPrn), req).map(accountView) }),
    // IAM disables the principal; its key rows stay ACTIVE (spec § 3.1). The console shows them inactive.
    'serviceAccounts.archiveServiceAccount': (req) => {
      accountAt(req.prn).status = 'disabled';
      return {};
    },
    'serviceAccounts.issueApiKey': (req) => {
      accountAt(req.serviceAccountPrn);
      const token = `${TOKEN_PREFIX}${randomUUID().replaceAll('-', '')}`;
      const key: Key = {
        id: randomUUID(),
        serviceAccountPrn: req.serviceAccountPrn,
        scopePrn: req.scopePrn,
        prefix: token.slice(0, 12),
        status: ApiKeyStatus.ACTIVE,
        expiresAt: req.expiresAt === undefined ? undefined : { seconds: req.expiresAt.seconds, nanos: req.expiresAt.nanos },
      };
      keys.push(key);
      return { apiKey: keyView(key), token };
    },
    'serviceAccounts.revokeApiKey': (req) => {
      const key = keys.find((candidate) => candidate.id === req.id);
      if (key === undefined) throw notFound();
      key.status = ApiKeyStatus.REVOKED;
      return {};
    },
    'serviceAccounts.listApiKeys': (req) => ({ apiKeys: page(keys.filter((key) => key.serviceAccountPrn === req.serviceAccountPrn), req).map(keyView) }),
    ...options.overrides,
  };
}
```

- [ ] **Step 4: Run the rows again and confirm they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console run typecheck
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test --project single-zone
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test --project two-zone
```

Expected: typecheck exit 0. Every single-zone row passes (R1–R7, R4b, and R5 with the action
guards). Every two-zone row passes (R8–R12). The build from Step 2 is still current: this step
changed only test files.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console/tests/e2e
git commit -F - <<'EOF'
test(ts): make the gateway e2e world stateful and scan an action in R5 (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 19: The single-zone settings rows R13–R18

§ 7.2 rows 1-6. The new rows are registered first, so the row test fails until each spec exists.

**Files:**
- Modify: `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts:18`
- Create: `ts/apps/gateway-console/tests/e2e/support/response-scan.ts`
- Create: `ts/apps/gateway-console/tests/e2e/service-accounts.spec.ts` (R13, R14, R15, R17, R18)
- Create: `ts/apps/gateway-console/tests/e2e/api-key-token.spec.ts` (R16)

**Interfaces:**
- Consumes: the world of Task 18; `signIn`, `waitForHydration` (support/login); the page test
  ids of Tasks 15-17.
- Produces (`response-scan.ts`): `type Recorded = { url; method; action: boolean; contentType; expectsBody: boolean; bodyRead: boolean; prefetchRsc: boolean; body: string }`;
  `startResponseScan(page: Page, origin: string): Promise<{ stop(): Promise<Recorded[]> }>`.

- [ ] **Step 1: Register the rows (the failing test)**

In `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts`, replace line 18 with:

```ts
const ROWS = ['R1', 'R2', 'R3', 'R4', 'R4b', 'R5', 'R6', 'R7', 'R8', 'R9', 'R10', 'R11', 'R12', 'R13', 'R14', 'R15', 'R16', 'R17', 'R18'];
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/e2e-rows.test.ts
```

Expected: FAIL: R13 to R18 each have 0 tests, not 1.

- [ ] **Step 2: Add the response recorder**

Create `ts/apps/gateway-console/tests/e2e/support/response-scan.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A response recorder for the token-exposure row (SMA-636 spec § 7.2 row 4, R16). It buffers every
// same-origin Server Action POST and every RSC GET through `page.route`, for the measured reason
// token-leak.spec.ts records: a streamed body (`bodySize: -1`) can make `response.body()` reject or
// hang under load. Every other response is read with `response.body()`. A prefetch that the
// interceptor could not buffer is the one class left unread; the caller asserts that the rest of
// the unread set is empty.
import type { Page, Request, Response } from '@playwright/test';

export type Recorded = {
  readonly url: string;
  readonly method: string;
  /** A POST with Next's `next-action` header: a Server Action call. */
  readonly action: boolean;
  readonly contentType: string;
  /** false for HEAD, 204, 205, 304 and a redirect. */
  readonly expectsBody: boolean;
  readonly bodyRead: boolean;
  readonly prefetchRsc: boolean;
  readonly body: string;
};

export type ResponseScan = { stop(): Promise<Recorded[]> };

function isServerAction(request: Request): boolean {
  return request.method() === 'POST' && request.headers()['next-action'] !== undefined;
}

function isRscGet(request: Request): boolean {
  return request.method() === 'GET' && new URL(request.url()).searchParams.has('_rsc');
}

async function record(response: Response, buffered: ReadonlyMap<Request, string>): Promise<Recorded> {
  const request = response.request();
  const requestHeaders = await request.allHeaders();
  const status = response.status();
  const expectsBody = request.method() !== 'HEAD' && status !== 204 && status !== 205 && status !== 304 && (status < 300 || status >= 400);
  const prefetchRsc = requestHeaders['next-router-prefetch'] === '1' && new URL(request.url()).searchParams.has('_rsc');
  const fromRoute = buffered.get(request);
  let body = fromRoute ?? '';
  let bodyRead = fromRoute !== undefined;
  if (!bodyRead && expectsBody && !prefetchRsc) {
    try {
      body = (await response.body()).toString('utf8');
      bodyRead = true;
    } catch {
      // The read failed: bodyRead stays false, and the caller's residue assertion reports it.
    }
  }
  const headers = await response.allHeaders();
  return { url: response.url(), method: request.method(), action: requestHeaders['next-action'] !== undefined, contentType: headers['content-type'] ?? '', expectsBody, bodyRead, prefetchRsc, body };
}

/** Starts recording every response of `page`. `stop()` detaches the listener FIRST, then drains. */
export async function startResponseScan(page: Page, origin: string): Promise<ResponseScan> {
  const buffered = new Map<Request, string>();
  await page.route(
    (url) => url.origin === origin,
    async (route) => {
      const request = route.request();
      if (!isServerAction(request) && !isRscGet(request)) {
        await route.fallback();
        return;
      }
      try {
        const answer = await route.fetch({ maxRedirects: 0 });
        const text = await answer.text();
        buffered.set(request, text);
        const headers = { ...answer.headers() };
        delete headers['content-encoding'];
        delete headers['content-length'];
        await route.fulfill({ status: answer.status(), headers, body: text });
      } catch {
        await route.fallback().catch(() => undefined);
      }
    },
  );
  const pending: Promise<Recorded>[] = [];
  const onResponse = (response: Response): void => {
    pending.push(record(response, buffered));
  };
  page.on('response', onResponse);
  return {
    async stop() {
      page.off('response', onResponse);
      await page.unrouteAll({ behavior: 'ignoreErrors' });
      return Promise.all(pending);
    },
  };
}
```

- [ ] **Step 3: Write the five settings rows**

Create `ts/apps/gateway-console/tests/e2e/service-accounts.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The settings pages in the single-zone tier (SMA-636 spec § 7.2 rows 1, 2, 3, 5 and 6). The world
// is stateful (support/world.ts): an account, a key and a grant that one action makes are what the
// next render reads. Row 4, the token exposure, is api-key-token.spec.ts.
import { Code } from '@connectrpc/connect';
import { denial } from '@paigasus/console-core/testing';
import { signIn, waitForHydration } from './support/login';
import { ORG_ID, PROJECT_ID, PROJECT_NAME, SEEDED_SA_ID, TOKEN_PREFIX } from './support/world';
import { expect, test } from './support/harness';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;
const PROJECT_PATH = `/gateway/orgs/${ORG_ID}/projects/${PROJECT_ID}`;

test('R13: create an account that can call models, issue a key once, revoke it, archive the account (§ 7.2 row 1)', async ({ page, harness }) => {
  await signIn(page, harness, ORG_PATH);
  const section = page.getByTestId('service-accounts');
  const result = section.getByTestId('sa-result');
  // The single-zone map has no IAM zone, so there is no "Manage in IAM" link (§ 4.4).
  await expect(page.getByTestId('manage-in-iam')).toHaveCount(0);

  const create = section.getByRole('form', { name: 'Create service account' });
  await create.getByLabel('Name').fill('ci-bot');
  await create.getByRole('button', { name: 'Create' }).click();
  await expect(result.getByText('Service account created. It can call models.')).toBeVisible();

  await result.getByRole('link', { name: 'Select it' }).click();
  await page.waitForURL((url) => url.searchParams.has('sa'));
  const panel = section.getByTestId('sa-panel');
  // The world answers "yes" only from the gateway_user grant the create made.
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'yes');

  const issue = panel.getByRole('form', { name: 'Issue API key' });
  await issue.getByLabel('Expiry').selectOption('30');
  await issue.getByRole('button', { name: 'Issue key' }).click();
  const shown = section.getByTestId('token-value');
  await expect(shown).toHaveText(new RegExp(`^${TOKEN_PREFIX}[0-9a-f]{32}$`));
  const token = (await shown.textContent()) ?? '';
  expect(page.url()).not.toContain(token);

  await page.reload();
  await waitForHydration(page);
  await expect(section.getByTestId('token-panel')).toHaveCount(0);
  expect(await page.content()).not.toContain(token);
  const key = panel.getByTestId('api-key-row');
  await expect(key).toHaveAttribute('data-status', 'active');

  await key.getByRole('button', { name: 'Revoke', exact: true }).click();
  await key.getByRole('button', { name: 'Confirm revoke' }).click();
  await expect(result.getByText('Key revoked.')).toBeVisible();
  await expect(key).toHaveAttribute('data-status', 'revoked');

  await panel.getByRole('button', { name: 'Archive', exact: true }).click();
  await panel.getByRole('button', { name: 'Confirm archive' }).click();
  await expect(result.getByText('Service account archived.')).toBeVisible();
  await expect(panel.getByTestId('sa-panel-archived')).toHaveText('Archived');
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'archived');
  await expect(key).toHaveAttribute('data-status', 'inactive');
  await expect(key).toContainText('Inactive (account archived)');
});

test('R14: a failed grant leaves the account unable to call models, and "Allow model calls" repairs it (§ 7.2 row 2)', async ({ page, harness }) => {
  harness.useWorld({ failFirstGrant: true });
  await signIn(page, harness, ORG_PATH);
  const section = page.getByTestId('service-accounts');
  const result = section.getByTestId('sa-result');

  const create = section.getByRole('form', { name: 'Create service account' });
  await create.getByLabel('Name').fill('repair-bot');
  await create.getByRole('button', { name: 'Create' }).click();
  await expect(result.getByText('Service account created, but it cannot call models yet.')).toBeVisible();

  await result.getByRole('link', { name: 'Select it' }).click();
  await page.waitForURL((url) => url.searchParams.has('sa'));
  const panel = section.getByTestId('sa-panel');
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'no');

  await panel.getByRole('button', { name: 'Allow model calls' }).click();
  await expect(result.getByText('Model calls allowed.')).toBeVisible();
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'yes');
});

test('R15: a viewer sees no mutation control, and a user who may not list accounts gets the section denial with HTTP 200 (§ 7.2 row 3)', async ({ page, harness }) => {
  harness.useWorld({ allow: [], seedServiceAccount: true });
  await signIn(page, harness, ORG_PATH);
  await page.goto(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));
  await waitForHydration(page);
  const section = page.getByTestId('service-accounts');
  await expect(section.getByTestId('sa-row')).toHaveCount(1);
  await expect(section.getByTestId('sa-panel')).toBeVisible();
  await expect(section.getByRole('form')).toHaveCount(0);
  await expect(section.getByRole('button')).toHaveCount(0);

  harness.useWorld({
    overrides: {
      'serviceAccounts.listServiceAccounts': () => {
        throw denial({ code: Code.PermissionDenied, reason: 'forbidden' });
      },
    },
  });
  const response = await page.goto(harness.url(ORG_PATH));
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('service-accounts-denied')).toHaveText('You cannot view service accounts here.');
  await expect(page.getByTestId('projects')).toBeVisible();
});

test('R17: a project_admin reaches the project page from "Your projects", and the organization page is a 403 for them (§ 7.2 row 5)', async ({ page, harness }) => {
  harness.useWorld({ projectAdmin: true });
  await signIn(page, harness, '/gateway/overview');

  await page.getByTestId('your-projects').getByRole('link', { name: PROJECT_NAME }).click();
  await page.waitForURL((url) => url.pathname === PROJECT_PATH);
  await expect(page.getByTestId('project-settings')).toBeVisible();
  await expect(page.getByTestId('service-accounts')).toBeVisible();

  const response = await page.goto(harness.url(ORG_PATH));
  expect(response?.status()).toBe(403);
  await expect(page.getByTestId('forbidden-view')).toBeVisible();
});

test('R18: without iam.apikeys in the IAM descriptor, no key control shows and no ListApiKeys call is made (§ 7.2 row 6)', async ({ page, harness }) => {
  harness.useWorld({ seedServiceAccount: true, iamDescriptor: { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar'] } });
  await signIn(page, harness, ORG_PATH);
  const before = harness.iam.callsTo('serviceAccounts.listApiKeys').length;

  await page.goto(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));
  await waitForHydration(page);
  const panel = page.getByTestId('sa-panel');
  // Vacuity: the panel renders, and a control that is not a key control still shows.
  await expect(panel.getByRole('button', { name: 'Archive', exact: true })).toBeVisible();
  await expect(panel.getByRole('form', { name: 'Issue API key' })).toHaveCount(0);
  await expect(panel.getByTestId('api-keys')).toHaveCount(0);
  expect(harness.iam.callsTo('serviceAccounts.listApiKeys').length - before).toBe(0);
});
```

Create `ts/apps/gateway-console/tests/e2e/api-key-token.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 7.2 row 4 and § 6 item 1: the plaintext API key token is the only secret of the
// settings screens. It must be in EXACTLY ONE response body — the issue action's response — and in
// no later HTML, RSC or prefetch response, and in no URL. This row depends on every token rule of
// § 5.4: a useActionState would send the token back in the next request body, a document POST
// would put it in an HTML response, and a revalidated render that read it would put it in an RSC.
import { signIn, waitForHydration } from './support/login';
import { startResponseScan, type Recorded } from './support/response-scan';
import { ORG_ID, SEEDED_SA_ID, TOKEN_PREFIX } from './support/world';
import { expect, test } from './support/harness';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;

function read(response: Recorded): boolean {
  return response.bodyRead && response.body.length > 0;
}

test('R16: an issued token is in exactly one response body, the action response, and in none after it (§ 7.2 row 4)', async ({ page, harness }) => {
  harness.useWorld({ seedServiceAccount: true });
  await signIn(page, harness, ORG_PATH);
  await page.goto(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));
  await waitForHydration(page);
  const scan = await startResponseScan(page, harness.origin);

  await page.getByTestId('sa-panel').getByRole('button', { name: 'Issue key' }).click();
  const shown = page.getByTestId('token-value');
  await expect(shown).toHaveText(new RegExp(`^${TOKEN_PREFIX}[0-9a-f]{32}$`));
  const token = (await shown.textContent()) ?? '';
  expect(token.length).toBe(TOKEN_PREFIX.length + 32);

  // Every later response class: a document reload, a client navigation (RSC), a second document,
  // and whatever the router prefetches on the way.
  await page.reload();
  await waitForHydration(page);
  await expect(page.getByTestId('token-panel')).toHaveCount(0);
  await page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('link', { name: 'Overview', exact: true }).click();
  await page.waitForURL((url) => url.pathname === '/gateway/overview');
  await expect(page.getByTestId('zone-overview')).toBeVisible();
  await page.goto(harness.url(ORG_PATH));
  await expect(page.getByTestId('org-settings')).toBeVisible();
  await page.waitForLoadState('networkidle');

  const seen = await scan.stop();
  const carrying = seen.filter((response) => response.body.includes(token));
  expect(carrying.map((response) => `${response.method} ${response.url}`)).toHaveLength(1);
  expect(carrying[0]?.method).toBe('POST');
  expect(carrying[0]?.action).toBe(true);
  expect(seen.filter((response) => response.url.includes(token))).toEqual([]);

  // Vacuity guards: the action body, an HTML body and a non-action RSC body were READ and searched.
  expect(seen.some((response) => response.action && read(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/html') && read(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && read(response))).toBe(true);
  // The residue: a body this row could not read must be the one measured prefetch class.
  expect(seen.filter((response) => response.expectsBody && !response.bodyRead && !response.prefetchRsc).map((response) => `${response.method} ${response.url}`)).toEqual([]);
});
```

- [ ] **Step 4: Run the rows and the row register**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/e2e-rows.test.ts tests/unit/hydration.test.ts
moon run gateway-console-ts:build
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test tests/e2e/service-accounts.spec.ts tests/e2e/api-key-token.spec.ts --project single-zone
```

Expected: `e2e-rows.test.ts` PASSES (each of R13–R18 has exactly one test). `hydration.test.ts`
PASSES: the new specs call no `locator.waitFor(`. The six Playwright rows PASS. If a row fails,
read the Playwright trace (`retain-on-failure`, under the OS temp dir) before any change.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console/tests
git commit -F - <<'EOF'
test(ts): add the gateway settings e2e rows R13 to R18 (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 20: The call count rows R19–R20 and the two-zone row R21

§ 7.3 (D12) and § 7.2 two-zone tier. The expected counts are the § 7.3 formula, written in the
test BEFORE it runs. A difference is a finding to explain in the pull request, never a number to
copy into the test.

**Files:**
- Modify: `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts:3-11`, `:18`
- Create: `ts/apps/gateway-console/tests/e2e/call-count.spec.ts` (R19, R20)
- Create: `ts/apps/gateway-console/tests/e2e/two-zone-manage-link.spec.ts` (R21)
- Modify: `ts/apps/gateway-console/README.md` (the line that starts with `  Each row is one test.`)

**Interfaces:**
- Consumes: the world of Task 18 (`ORG_ID`, `ORG_NAME`, `SEEDED_SA_ID`); `FakeIamCall` from
  `@paigasus/console-core/testing`; the two-zone harness.
- Produces: rows R19, R20 (single-zone project) and R21 (two-zone project, basename `two-zone-*`).

- [ ] **Step 1: Register the rows (the failing test)**

In `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts`, replace line 18 with:

```ts
const ROWS = ['R1', 'R2', 'R3', 'R4', 'R4b', 'R5', 'R6', 'R7', 'R8', 'R9', 'R10', 'R11', 'R12', 'R13', 'R14', 'R15', 'R16', 'R17', 'R18', 'R19', 'R20', 'R21'];
```

and replace lines 3-11 (the header prose) with:

```ts
// Spec § 10.4 has six rows (R1-R6). This zone's e2e tier adds two more cases that are not in the
// spec's table: R4b, a second capability case that pairs with R4, and R7, the 403 control this
// zone's plan added deliberately. Spec § 10.5 contributes five more (R8-R12): the two-zone tier
// (SMA-512 PR4 task 4). SMA-636 § 7.2 and § 7.3 add nine for the settings pages: R13-R18 (the
// single-zone rows), R19-R20 (the per-request call count) and R21 (the two-zone "Manage in IAM"
// link). Each id must have exactly one Playwright test whose title starts with it. A deleted or
// renamed scenario then fails this vitest suite, which runs in gateway-console-ts:test on every PR
// that touches the app, even when the e2e task does not run. Mirrors
// ts/apps/iam-console/tests/unit/e2e-rows.test.ts.
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/e2e-rows.test.ts
```

Expected: FAIL: R19, R20 and R21 have 0 tests.

- [ ] **Step 2: Write the call count rows**

Create `ts/apps/gateway-console/tests/e2e/call-count.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 7.3 (D12): the per-request IAM call count of /gateway/orgs/<org>. The page is
// fetched with a PLAIN request (page.request: the context's cookies, no router, so no prefetch).
// proxy.ts mints one correlation id per request and every IAM call carries it, so the fake's call
// log is grouped by that id, and the ONE group that holds ListServiceAccounts is this render.
//
// The expected counts are the § 7.3 formula, derived from § 4.2 BEFORE the measurement. A
// difference is a finding to explain, not a number to copy in here. Only gRPC calls are counted:
// the discovery probe is HTTP (`http.getServiceInfo`), is not in the formula, and is memoized by
// the descriptor cache, not per request.
//
// LIMIT (§ 7.3): this counts calls against a fake. It does not measure latency against a real IAM.
import type { FakeIamCall } from '@paigasus/console-core/testing';
import { signIn } from './support/login';
import { ORG_ID, SEEDED_SA_ID } from './support/world';
import { expect, test } from './support/harness';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;

/** The default world's scopes (support/world.ts): an organization and a team membership, and a project grant. */
const SCOPES = { organization: 1, team: 1, project: 1 } as const;
/** The default world's teams in the organization (support/world.ts `tenancy.listTeams`). */
const TEAMS = 1;

/** § 7.3's table for S scopes and T teams, with no `sa` parameter. */
function formula(): Record<string, number> {
  return {
    'authn.introspect': 1, // the session principal, memoized per request
    'authz.listRoleGrants': 1, // myScopes(), with iam.authz.cedar present
    'tenancy.getOrganization': 1 + SCOPES.organization, // the page, plus myScopes()'s label of the org scope
    'tenancy.getTeam': SCOPES.team, // myScopes() labels
    'tenancy.getProject': SCOPES.project, // myScopes() labels
    'authz.isAuthorized': 5, // mayI(), five distinct actions
    'serviceAccounts.listServiceAccounts': 1, // the section
    'tenancy.listTeams': 1, // the Projects list
    'tenancy.listProjects': TEAMS, // the Projects list, one per shown team
  };
}

/** The gRPC calls of the ONE request whose group holds ListServiceAccounts, counted by method. */
function countOneRender(calls: readonly FakeIamCall[]): Record<string, number> {
  const groups = new Map<string, FakeIamCall[]>();
  for (const call of calls) {
    if (call.correlationId === null || call.method.startsWith('http.')) continue;
    groups.set(call.correlationId, [...(groups.get(call.correlationId) ?? []), call]);
  }
  const renders = [...groups.values()].filter((group) => group.some((call) => call.method === 'serviceAccounts.listServiceAccounts'));
  expect(renders).toHaveLength(1);
  const counts: Record<string, number> = {};
  for (const call of renders[0] ?? []) counts[call.method] = (counts[call.method] ?? 0) + 1;
  return counts;
}

test('R19: one render of the organization page makes exactly the calls of the § 7.3 formula (D12)', async ({ page, harness }) => {
  await signIn(page, harness, '/gateway/overview');
  const start = harness.iam.calls.length;

  const response = await page.request.get(harness.url(ORG_PATH));

  expect(response.status()).toBe(200);
  expect(countOneRender(harness.iam.calls.slice(start))).toEqual(formula());
});

test('R20: with ?sa=, the same render adds exactly one GetServiceAccount, one IsAuthorized and one ListApiKeys (§ 7.3)', async ({ page, harness }) => {
  harness.useWorld({ seedServiceAccount: true });
  await signIn(page, harness, '/gateway/overview');
  const start = harness.iam.calls.length;

  const response = await page.request.get(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));

  expect(response.status()).toBe(200);
  const base = formula();
  expect(countOneRender(harness.iam.calls.slice(start))).toEqual({
    ...base,
    'authz.isAuthorized': (base['authz.isAuthorized'] ?? 0) + 1, // the model-call state
    'serviceAccounts.getServiceAccount': 1,
    'serviceAccounts.listApiKeys': 1,
  });
});
```

Create `ts/apps/gateway-console/tests/e2e/two-zone-manage-link.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 4.4 and § 7.2 (two-zone tier): the organization page's "Manage in IAM" link comes
// from the zone map's `iam` entry and resolves to the same organization in the IAM zone. It is a
// cross-zone link, so ZoneLink renders it as a plain <a> and the click is a hard navigation. The
// single-zone tier has no IAM zone, and R13 asserts the link is absent there.
import { ORG_ID, ORG_NAME } from './support/world';
import { expect, test } from './support/two-zone-harness';

test('R21: the organization page links to the same organization in the IAM zone (SMA-636 § 4.4)', async ({ page, harness }) => {
  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await expect(page.getByTestId('org-settings')).toBeVisible();

  const link = page.getByTestId('manage-in-iam');
  await expect(link).toHaveAttribute('href', `/iam/orgs/${ORG_ID}`);
  await link.click();

  await page.waitForURL(`${harness.origin}/iam/orgs/${ORG_ID}`);
  await expect(page.getByRole('heading', { level: 1, name: ORG_NAME })).toBeVisible();
});
```

In `ts/apps/gateway-console/README.md`, in the line that starts with `  Each row is one test.`,
replace `(R1–R7, plus R4b and R7's 403 control, then R8–R12)` with
`(R1–R7, plus R4b and R7's 403 control, then R8–R12, then R13–R21 for the settings pages of SMA-636)`.

- [ ] **Step 3: Run the rows**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts --filter @paigasus/gateway-console exec vitest run tests/unit/e2e-rows.test.ts tests/unit/hydration.test.ts
moon run gateway-console-ts:build iam-console-ts:build
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test tests/e2e/call-count.spec.ts --project single-zone
pnpm --dir ts --filter @paigasus/gateway-console exec playwright test tests/e2e/two-zone-manage-link.spec.ts --project two-zone
```

Expected: the two vitest files PASS. R19, R20 and R21 PASS. The two-zone row needs Docker.

If R19 or R20 FAILS on a count, do NOT change the expected value to make it pass. Read the
difference (`toEqual` prints both maps), find the call site that makes the extra or missing call,
and either fix the code or record the finding in the pull request with the reason. Spec § 7.3
requires this.

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts format:write
git add ts/apps/gateway-console
git commit -F - <<'EOF'
test(ts): count the IAM calls of one settings render and add the two-zone link row (SMA-636)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 21: Full verification

No new code. This task proves the branch the way CI does, and records what cannot run locally.

**Files:** none, unless a check fails. A fix is a new commit ("add, don't amend").

- [ ] **Step 1: The per-project tasks of every touched project**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-sdk-ts:typecheck paigasus-console-core-ts:typecheck paigasus-ui-ts:typecheck paigasus-app-shell-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck
moon run paigasus-sdk-ts:test paigasus-console-core-ts:test paigasus-ui-ts:test paigasus-app-shell-ts:test iam-console-ts:test gateway-console-ts:test
moon run ts:lint ts:fmt
```

Expected: every task passes. `gateway-console-ts:test` and `iam-console-ts:test` build the app
first (`~:build`), then run vitest and the tailwind-source guard. `ts:fmt` is the separate
Prettier gate; a red there means `pnpm --dir ts format:write` was skipped before a commit.

- [ ] **Step 2: Both e2e tiers**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e
```

Expected: PASS. Docker must run: the gateway two-zone project starts a Redis container. Every
iam-console edit of Tasks 3-5 also selects the gateway tier (`/ts/apps/iam-console/**/*` is its
input).

- [ ] **Step 3: The full CI target set**

Before a re-run of anything that failed, follow step 0 of CLAUDE.md's "Diagnosing an
unattributed `moon ci` failure": copy the CI report and `.moon/cache/states/<project>/<task>/`
out of the repository. A re-run overwrites the evidence.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
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

Expected: exit 0. KNOWN LOCAL LIMITS of this machine class (CLAUDE.md, SMA-512 correction). They
never apply in CI:

- `repo:affected-smoke` needs system `/bin/bash` 3.2. If it hangs or reds with many "expected rc
  0" self-test failures, re-run it alone through a bash-only shim directory, and never by putting
  `/bin` first on PATH (that downgrades `python3` and breaks `tomllib`):
  ```bash
  export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
  SHIM="$(mktemp -d)"; ln -s /bin/bash "$SHIM/bash"
  PATH="$SHIM:$PATH" moon run repo:affected-smoke --force
  ```
- `repo:ruff-ci` and `repo:next-public-free` need bash 4 or later (`mapfile`). Read their verdict
  from a direct run: `/opt/homebrew/bin/bash ci/ruff/run.sh` and
  `/opt/homebrew/bin/bash ci/next-public/run.sh`.
- `repo:actionlint` has no working local bash. Its verdict comes from CI only.
- A sub-3-second `repo:affected-smoke` abort with a `proto-shim … Permission denied` line is the
  documented infrastructure abort, not a finding. Re-run the task alone.

This branch adds no Rust crate, no `repo:*` gate, no workflow and no new Moon task input, so
none of the registry obligations of CLAUDE.md apply.

- [ ] **Step 4: Record the result**

If every check passed, there is nothing to commit. If a fix was needed, commit it with its own
message (`fix(ts): … (SMA-636)`), then run the failing check again.

---

## Spec coverage

| Spec item | Task(s) |
| -- | -- |
| § 1.1, D1 (the settings are service accounts and keys) | 11, 15, 16 |
| § 1.2, D2 (create also grants `gateway_user`) | 10, 13 |
| D3 (two RPCs, `partial`, repair control) | 10, 11 (`allow` control), 13, 15, 19 (R14) |
| D4 (org and project screens; teams as labels) | 12, 16, 17 |
| D5 (flat project route) | 12, 16 |
| D6 (scope from IAM, never from a form) | 10, 13 (accepted-inputs tests) |
| D7 (expiry choices, server computes the date) | 8, 10, 13, 15 |
| D8 (fail-closed model-call state; archived → no call) | 9, 11 |
| D9 (no "Stop model calls") | 15 (no such control), 16 (README) |
| D10 (rename/archive stay in IAM; "Manage in IAM") | 16, 20 (R21) |
| D11 (pure helpers to console-core, buttons to ui, copies recorded) | 4, 5, 8, 14, 16 (README) |
| D12 (call count per request) | 20 (R19, R20) |
| D13 (`iam.apikeys` and `iam.authz.cedar` gates) | 11, 13, 19 (R18) |
| D14 (keys and model-call state for one selected account) | 11, 20 (R20) |
| D15 (pager, limit+1, IAM order) | 7, 11, 12 |
| D16 ("Your projects" on the overview) | 17, 19 (R17) |
| § 3.4 (serviceAccounts client, IAM_ACTIONS, fake route, ZoneLink prefetch) | 2, 3, 6 |
| § 4.1 (routes, UUID segments, `sa`/`saOffset`/`keyOffset`, `prefetch={false}`, mixed URL) | 7, 8, 12, 15, 16 |
| § 4.2 (both loaders, 8 in flight, section-level 403, view model) | 11, 12 |
| § 4.3 (`modelCallState`) | 9 |
| § 4.4 ("Manage in IAM" href from the zone map) | 16, 20 |
| § 4.5 (where the code lives) | 1-6, SPEC DEVIATIONS 1, 2, 5 |
| § 4.6 (client components, one result region) | 14, 15 |
| § 5.1 (action order, accepted inputs, FormError, relogin link) | 13, 14 |
| § 5.2 (`CreateState`, messages, revalidation table) | 8, 10, 13, 15 |
| § 5.3 (allow; conditions; "may already be allowed") | 10, 11, 13, 15 |
| § 5.4 (issue; `IssueKeyState`; forbidden copy; token rules 1-7) | 8, 10, 13, 14, 15, 19 (R13, R16) |
| § 5.5 (revoke, two-step confirm) | 10, 13, 14, 15 |
| § 5.6 (archive, confirm text, "Archived" badge) | 10, 13, 14, 15 |
| § 5.7 (keys list, status mapping, other-scope note) | 8, 11, 15 |
| § 5.8 (owner not active → read-only, notes) | 8, 11, 15 |
| § 6 (security items 1-5) | 13 (logs), 19 (R16), 10 (D6), 9, 12 |
| § 7.1 (unit and integration tests) | 2-17 |
| § 7.2 (stateful world; rows 1-6; two-zone row) | 18, 19, 20 |
| § 7.3 (call count, formula first) | 20 |
| § 7.4 (re-baselines: R11, `two-zone-session`, scope-page, R5, actions-structure copy, ALL_ACTIONS, moved tests) | 3, 4, 13, 16, 18 |
| § 8 (follow-ups) | not in scope; recorded in the spec |
| § 9 (recorded limits) | 16 (README) |

## Self-review

- **Placeholders:** none. Every code step holds the full code or an exact line replacement.
  Copies (`node-status.ts`, its test, `section-error.tsx`, `next-cache.ts`,
  `actions-structure.test.ts`) are made with `cp` and then given exact replacements, not "similar
  to" instructions.
- **Type consistency:** `SectionView`/`SectionOk`/`SelectedView`/`PanelControls`/`KeysView`
  (Task 8) are the shapes the loader returns (Task 11) and the components read (Task 15).
  `CreateAction`, `IssueKeyAction` and `FormAction` match the action signatures of Task 13, which
  `serviceAccountsBlock` (Task 16) passes. `SettingsParams` is defined in the organization loader
  and imported by the project loader. `scriptedMayI`, `clientsFor` and `callsSince` are defined in
  Task 10 before Tasks 11 and 12 use them. The world exports of Task 18 are the ones Tasks 19 and
  20 import.
- **Risks recorded for the implementer:**
  1. `ts/packages/paigasus-console-core/testing/dev-world.ts` (the `dev:stack` world) has no
     service-account handlers. Under `pnpm --dir ts dev:stack` the settings section shows the
     section error (Unimplemented). This plan does not change it; a follow-up can.
  2. R19/R20 assert exact counts against a fake. A call that the layout makes and the spec formula
     does not list will red them; that is the intended "finding", not a flake.
  3. Every iam-console change (Tasks 3-5) selects the Docker-backed gateway two-zone tier in CI.
  4. The e2e rows R9, R11 and R5 are red between Task 16 and Task 18. Do not push between them.
