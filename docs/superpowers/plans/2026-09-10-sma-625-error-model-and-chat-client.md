# SMA-625 — `@paigasus/sdk` error model and chat client: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the error model (`mapError`, two transport-status tables, a total presentation override table) and the OpenAI-compatible chat client to `@paigasus/sdk`, closing SMA-625's four acceptance criteria.

**Architecture:** Five new modules under `ts/packages/paigasus-sdk/src/`, reached through three new `exports` entries. `errors/types.ts` is a guard-free, client-safe type surface; everything else is server-only. Presentation is derived in two layers — a total map from transport status, then a total override keyed on `ErrorReason` — so a new registry reason is a compile error rather than an empty toast. The chat client is a thin `fetch` wrapper that returns the upstream `ReadableStream` untouched.

**Tech Stack:** TypeScript 6.0.3 (`verbatimModuleSyntax`), vitest 5.0.0, `@connectrpc/connect` 2.2.0, `@paigasus/proto` (workspace), Node 24 global `fetch`. Moon 2.5.3 for task selection.

**Spec:** `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md` §§ 8, 9, 10, 11.3a (approved 2026-09-10, revision 2)

## Global Constraints

- **Working directory** is the worktree root, `.claude/worktrees/sma-508`. Branch: `feature/sma-625-ts-paigasussdk-error-mapping-and-the-openai-compatible-chat`.
- **PATH**: prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. The Bash tool's PATH lacks the proto-managed CLIs.
- **Every source file opens with** `// SPDX-License-Identifier: Apache-2.0`.
- **Every relative import carries a `.js` extension**, even from a `.ts` file. `moduleResolution` is `bundler`, but the repo writes ESM specifiers.
- **A guarded entry's FIRST import statement is the server guard.** Not the first line — the SPDX header and a comment block come first. `tests/server-guard.test.ts` enforces this off `package.json`'s `exports` map.
- **`./errors/types` is the ONE guard-free entry.** It is already pre-registered in that test's `UNGUARDED_ENTRIES`.
- **Type-only imports use `import type`.** `verbatimModuleSyntax: true` means `import type` emits nothing and a value import of a type is a compile error. This is what keeps `chat.ts` from pulling `@connectrpc/connect-node`'s HTTP/2 stack in through `Auth`.
- **Moon task `inputs` are APPENDED, never `options.merge: replace`.** Replacing drops the four inherited inputs.
- **No commit may leave the `exports` map naming a file that does not exist.** Each entry lands in the same commit as its file.
- **Conventional commits, scope `ts`** (or `ci` for the affected-graph task). Allowed scopes: `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. Footer `Refs SMA-625`. Never `--no-verify`.
- **This package adds no dependency.** Everything needed is already in `package.json`.
- **Run ESLint before every commit**, from `ts/`:
  `./node_modules/.bin/eslint packages/paigasus-sdk --max-warnings 0` — expected exit 0.
  `ts:lint` is a CI gate, and `tsc` does not stand in for it. Two rules bite this package, both
  invisible to vitest and to `tsc`, and both MEASURED here:
  - `@typescript-eslint/no-unused-vars` — the repo sets **no** `varsIgnorePattern`, so a leading
    underscore does **not** exempt an unused **variable** (`const { message: _drop, ...rest }`
    is an error). Unused **parameters** are fine under the default `args: 'after-used'`, which
    is why `(_name, number) => …` passes while `_drop` does not.
  - `@typescript-eslint/no-unnecessary-type-assertion` — `number as ErrorReason` is redundant,
    because a numeric enum accepts a plain `number`. Write `const reason = number;`.
    Note this does **not** extend to `expect(x).toBeDefined()`: that carries no narrowing
    signature in the installed vitest, so a following `x!` is **necessary** and removing it
    breaks `tsc`. Do not generalize the rule from one case to the other.

## Two decisions this plan takes

**D1 — `mapError` is one function over a discriminated union, not four exported arms.** § 9.5 describes four inputs to one function. A single `mapError(input: MapErrorInput)` makes the four arms exhaustive against the union, so a fifth wire shape added later is a compile error at the switch. Four separately exported functions would let one be forgotten.

**D2 — The 57-entry override table is written under compiler guidance, not transcribed from this plan.** `Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, Entry>` yields `TS2741` **naming the missing member** (spec M8). So the procedure is: write the three load-bearing entries, compile, add each named member as `'from-transport'`, repeat until clean. Transcribing 57 identifiers into a plan document invites a typo that the compiler catches anyway, and the compile loop is deterministic and verifiable. Step 3 of Task 3 states this as the actual instruction.

## File Structure

**Created**

| File | Responsibility |
|---|---|
| `src/errors/types.ts` | `PaigasusError`, `Presentation`, `ErrorTransport`. Types only, no guard, no runtime import. |
| `src/errors/transport-tables.ts` | § 9.2's two total maps: gRPC `Code` → `Presentation`, HTTP status → `Presentation`. |
| `src/errors/presentation.ts` | § 9.4's total `Record` over the 57 reasons, plus `presentationFor`. |
| `src/errors/map-error.ts` | § 9.5's four arms. The `./errors` entry point. |
| `src/errors/terminal-frame.ts` | § 8.5's stateful SSE parser. |
| `src/chat.ts` | § 8's chat client. The `./chat` entry point. |
| `tests/fixtures/terminal-frame.ts` | The frame pinned verbatim from `chat.rs:63`. |
| `tests/errors-tables.test.ts`, `tests/errors-presentation.test.ts`, `tests/map-error.test.ts`, `tests/terminal-frame.test.ts`, `tests/chat.test.ts` | One suite per module. |

**Modified**

| File | Change |
|---|---|
| `package.json` | Three `exports` entries. |
| `tests/server-guard.test.ts` | Compute each entry's expected guard specifier from its own directory. |
| `moon.yml` | `test` gains the `chat.rs` input. |
| `ci/affected-graph/run.sh` | One `run_task_case_ci "chat-rs->sdk"` case. |

---

### Task 1: The client-safe type surface

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/types.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (add `"./errors/types"`)
- Test: covered by the existing `tests/server-guard.test.ts` (UNGUARDED partition)

**Interfaces:**
- Consumes: `ErrorReason`, `ErrorDomain` from `@paigasus/proto`; `Code` from `@connectrpc/connect`.
- Produces: `type Presentation`, `type ErrorTransport`, `interface PaigasusError`.

- [ ] **Step 1: Write the module**

`src/errors/types.ts` — note there is **no** `import './server-guard.js'` here, deliberately.

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The client-safe surface (spec § 6.3). This file carries NO server guard, and that is the one
// deliberate exception in the package: it holds only types, and `verbatimModuleSyntax: true`
// (ts/tsconfig.base.json:9) means an `import type` emits nothing at all. A 'use client' error
// boundary can therefore name PaigasusError and switch on Presentation with no runtime import and
// no server-only evaluation.
//
// `tests/server-guard.test.ts` asserts this file does NOT import the guard — the assertion is
// pre-registered in its UNGUARDED_ENTRIES set, so removing the exception is a test failure.
import type { Code } from '@connectrpc/connect';
import type { ErrorDomain, ErrorReason } from '@paigasus/proto';

/**
 * What screen this error should produce.
 *
 * Nine values. `rate-limited` is separate from `degraded` deliberately: "you are being
 * rate-limited, try again shortly" and "the service is unwell" are different screens, and lumping
 * them costs the first the only copy that helps a user act (spec § 9.2).
 */
export type Presentation = 'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'rate-limited' | 'invalid-input' | 'conflict' | 'disabled' | 'generic';

/**
 * The raw transport status, carried for logging only.
 *
 * A number or a `Code` — NEVER a `ConnectError` and never `Headers`. That is what makes AC 4's
 * "raw gRPC statuses never reach the browser" hold when the whole object is serialized to a client
 * component as a prop.
 */
export type ErrorTransport = { readonly kind: 'grpc'; readonly code: Code } | { readonly kind: 'http'; readonly status: number };

/**
 * One shape for every failure this SDK can surface (spec § 9.1).
 *
 * Every optional field is `null`, never `undefined`. The codec in `@paigasus/proto` returns
 * `undefined`, so each call site normalizes with `?? null`: this object crosses the server/client
 * boundary as a prop, and `null` survives that uniformly while `undefined` does not.
 */
export interface PaigasusError {
  readonly presentation: Presentation;
  /**
   * Resolved only on the gRPC arm. IAM's HTTP envelope is `{error:{code,message}}` and the
   * gateway's is `{message,type,param,code}` — neither has a domain field — so this is
   * structurally `null` for arms 2, 3 and 4 (spec § 9.1).
   */
  readonly domain: ErrorDomain | null;
  /** `null` means the wire's code did not resolve against the registry. `rawReason` still holds it. */
  readonly reason: ErrorReason | null;
  readonly rawReason: string | null;
  readonly rawDomain: string | null;
  /** Human-readable, for display and logging. NEVER an input to a branch (AC 2). */
  readonly message: string;
  readonly correlationId: string | null;
  readonly requestId: string | null;
  /** Tri-state. `null` is the wire's "unknown" — never collapse it to `false` (ADR-0019 decision 7). */
  readonly retryable: boolean | null;
  /** The ErrorInfo map minus the three lifted keys (`retryable`, `correlation_id`, `request_id`). */
  readonly metadata: Readonly<Record<string, string>>;
  readonly transport: ErrorTransport;
}
```

- [ ] **Step 2: Add the exports entry**

In `package.json`, extend the `exports` map to exactly:

```json
  "exports": {
    ".": "./src/index.ts",
    "./iam": "./src/iam.ts",
    "./errors/types": "./src/errors/types.ts"
  },
```

- [ ] **Step 3: Run the guard suite to verify the new entry is picked up**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/server-guard.test.ts`

Expected: PASS, and the output now names a new case, `entry ./errors/types deliberately carries no guard`. If that case is absent the exports edit did not land.

- [ ] **Step 4: Typecheck**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/tsc -p tsconfig.json --noEmit`

Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/types.ts ts/packages/paigasus-sdk/package.json
git commit -m "feat(ts): add the client-safe PaigasusError type surface

The one entry point in @paigasus/sdk that carries no server guard. Under
verbatimModuleSyntax an import type emits nothing, so a 'use client' error
boundary can name these types with no runtime import.

Refs SMA-625"
```

---

### Task 2: The two transport-status tables

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/transport-tables.ts`
- Test: `ts/packages/paigasus-sdk/tests/errors-tables.test.ts`

**Interfaces:**
- Consumes: `Presentation` from `./types.js`; `Code` from `@connectrpc/connect`.
- Produces: `presentationForGrpcCode(code: Code): Presentation`, `presentationForHttpStatus(status: number): Presentation`.

- [ ] **Step 1: Write the failing test**

`tests/errors-tables.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { Code } from '@connectrpc/connect';
import { describe, expect, it } from 'vitest';

import { presentationForGrpcCode, presentationForHttpStatus } from '../src/errors/transport-tables.js';

describe('the gRPC status table (spec § 9.2)', () => {
  it.each([
    [Code.Unauthenticated, 'relogin'],
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unavailable, 'degraded'],
    [Code.DeadlineExceeded, 'degraded'],
    [Code.InvalidArgument, 'invalid-input'],
    [Code.AlreadyExists, 'conflict'],
    [Code.FailedPrecondition, 'conflict'],
    [Code.Aborted, 'conflict'],
    [Code.ResourceExhausted, 'rate-limited'],
  ] as const)('maps %s to %s', (code, expected) => {
    expect(presentationForGrpcCode(code)).toBe(expected);
  });

  // The row that makes § 9.4's override table load-bearing. Revision 1 mapped Unimplemented to
  // `disabled`, which made the CAPABILITY_DISABLED entry a no-op — the table's own justification,
  // defeated by the table above it. If this assertion is ever "fixed" to `disabled`, read § 9.2
  // before changing it.
  it('maps Unimplemented to generic, NOT disabled', () => {
    expect(presentationForGrpcCode(Code.Unimplemented)).toBe('generic');
  });

  it('falls through to generic', () => {
    expect(presentationForGrpcCode(Code.Internal)).toBe('generic');
    expect(presentationForGrpcCode(Code.Canceled)).toBe('generic');
    expect(presentationForGrpcCode(Code.Unknown)).toBe('generic');
  });
});

describe('the HTTP status table (spec § 9.2)', () => {
  it.each([
    [401, 'relogin'],
    [403, 'forbidden'],
    [404, 'not-found'],
    [408, 'degraded'],
    [409, 'conflict'],
    [413, 'invalid-input'],
    [415, 'invalid-input'],
    [422, 'invalid-input'],
    [400, 'invalid-input'],
    [502, 'degraded'],
    [503, 'degraded'],
    [504, 'degraded'],
  ] as const)('maps %i to %s', (status, expected) => {
    expect(presentationForHttpStatus(status)).toBe(expected);
  });

  // 429 is NOT degraded. Every 429 the SDK can currently see is OpenAI's quota arriving through
  // the gateway's passthrough (spec M13: nothing in this repo emits 429 itself).
  it('maps 429 to rate-limited, NOT degraded', () => {
    expect(presentationForHttpStatus(429)).toBe('rate-limited');
  });

  it('falls through to generic', () => {
    expect(presentationForHttpStatus(500)).toBe('generic');
    expect(presentationForHttpStatus(418)).toBe('generic');
    expect(presentationForHttpStatus(200)).toBe('generic');
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/errors-tables.test.ts`

Expected: FAIL — `Failed to resolve import "../src/errors/transport-tables.js"`.

- [ ] **Step 3: Write the implementation**

`src/errors/transport-tables.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { Code } from '@connectrpc/connect';

import type { Presentation } from './types.js';

/**
 * gRPC Code -> Presentation (spec § 9.2). Total: everything unlisted is `generic`.
 *
 * `Unimplemented` is deliberately ABSENT, so it falls through to `generic`. It means "this build
 * cannot do that"; "this deployment turned that capability off" is a different screen, and
 * § 9.4's CAPABILITY_DISABLED entry is what selects it. Mapping it to `disabled` here would make
 * that entry a no-op.
 *
 * `ResourceExhausted` is listed although nothing emits it today (spec M13). The table enumerates
 * `Code`, and this is the right answer if IAM ever adds a quota — it is not evidence that
 * something produces it.
 */
const GRPC: ReadonlyMap<Code, Presentation> = new Map([
  [Code.Unauthenticated, 'relogin'],
  [Code.PermissionDenied, 'forbidden'],
  [Code.NotFound, 'not-found'],
  [Code.Unavailable, 'degraded'],
  [Code.DeadlineExceeded, 'degraded'],
  [Code.InvalidArgument, 'invalid-input'],
  [Code.AlreadyExists, 'conflict'],
  [Code.FailedPrecondition, 'conflict'],
  [Code.Aborted, 'conflict'],
  [Code.ResourceExhausted, 'rate-limited'],
]);

/**
 * HTTP status -> Presentation (spec § 9.2). Total: everything unlisted is `generic`.
 *
 * 429 is `rate-limited`, not `degraded`: a quota exhaustion and a sick service want different
 * copy. 504 stays `degraded` — a timeout is a service problem, not a quota one.
 *
 * 413 sits with 400 under `invalid-input` because a too-large request is caller-fixable: the
 * caller must send less, the same corrective action a 400 asks for.
 */
const HTTP: ReadonlyMap<number, Presentation> = new Map([
  [400, 'invalid-input'],
  [401, 'relogin'],
  [403, 'forbidden'],
  [404, 'not-found'],
  [408, 'degraded'],
  [409, 'conflict'],
  [413, 'invalid-input'],
  [415, 'invalid-input'],
  [422, 'invalid-input'],
  [429, 'rate-limited'],
  [502, 'degraded'],
  [503, 'degraded'],
  [504, 'degraded'],
]);

export function presentationForGrpcCode(code: Code): Presentation {
  return GRPC.get(code) ?? 'generic';
}

export function presentationForHttpStatus(status: number): Presentation {
  return HTTP.get(status) ?? 'generic';
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/errors-tables.test.ts`

Expected: PASS, 5 test cases (two `it.each` blocks plus three singles).

- [ ] **Step 5: Typecheck and commit**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/tsc -p tsconfig.json --noEmit` — expected exit 0.

```bash
git add ts/packages/paigasus-sdk/src/errors/transport-tables.ts ts/packages/paigasus-sdk/tests/errors-tables.test.ts
git commit -m "feat(ts): add the two transport-status presentation tables

Both are total, falling through to generic. Two rows carry reasoning that
outlives this commit: Unimplemented falls through rather than mapping to
disabled, which is what lets the CAPABILITY_DISABLED override change an
answer; and 429 is rate-limited rather than degraded.

Refs SMA-625"
```

---

### Task 3: The presentation override table

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/presentation.ts`
- Test: `ts/packages/paigasus-sdk/tests/errors-presentation.test.ts`

**Interfaces:**
- Consumes: `Presentation` from `./types.js`; `ErrorReason`, `ErrorReasonSchema` from `@paigasus/proto`.
- Produces: `type PresentationEntry = Presentation | 'from-transport'`, `presentationFor(reason: ErrorReason): PresentationEntry`.

- [ ] **Step 1: Write the failing test**

`tests/errors-presentation.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { ErrorReason, ErrorReasonSchema, asWireReason, fromWireReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { presentationFor } from '../src/errors/presentation.js';

const REAL_REASONS = ErrorReasonSchema.values.filter((v) => v.name !== 'ERROR_REASON_UNSPECIFIED');

describe('AC 3 — the override table is total over the registry', () => {
  // The registry is the source of truth, not a number in this file. If a reason is added to
  // error.proto and regenerated, this count moves and the loop below covers the new member.
  it('sees the whole registry', () => {
    expect(REAL_REASONS).toHaveLength(57);
  });

  // This is AC 3 verbatim: driven off the descriptor, so an unmapped code fails a test rather
  // than rendering an empty toast. It is the SECOND mechanism — the total Record type is the
  // first, and it alone can be switched off by a refactor to Partial<...>, which this notices.
  it.each(REAL_REASONS.map((v) => [v.name, v.number] as const))('%s has an entry and round-trips', (_name, number) => {
    const reason = number as ErrorReason;

    // Guarded, NOT `fromWireReason(asWireReason(reason))`: asWireReason returns
    // `string | undefined` and fromWireReason takes `string`, so the nested form does not
    // compile (spec § 9.4).
    const wire = asWireReason(reason);
    expect(wire).toBeDefined();
    expect(fromWireReason(wire!)).toBe(reason);

    expect(presentationFor(reason)).toBeDefined();
  });
});

describe('the entries that change an answer (spec § 9.4)', () => {
  // Load-bearing: capability-disabled arrives only as gRPC Unimplemented, which § 9.2 maps to
  // `generic`. This entry is what turns it into the "switched off" screen.
  it('lifts CAPABILITY_DISABLED to disabled', () => {
    expect(presentationFor(ErrorReason.CAPABILITY_DISABLED)).toBe('disabled');
  });

  // Load-bearing: IAM maps principal-inactive to PermissionDenied/403, so § 9.2 alone would show
  // "you do not have permission" for an account that exists and is switched off.
  it('lifts PRINCIPAL_INACTIVE to disabled', () => {
    expect(presentationFor(ErrorReason.PRINCIPAL_INACTIVE)).toBe('disabled');
  });

  // Its two neighbours keep `forbidden` — they are provisioning states an admin resolves. This is
  // a considered split, not an oversight, so it is asserted.
  it('leaves the two provisioning neighbours on the transport', () => {
    expect(presentationFor(ErrorReason.IDENTITY_NOT_PROVISIONED)).toBe('from-transport');
    expect(presentationFor(ErrorReason.PROVISIONING_FAILED)).toBe('from-transport');
  });

  // A PIN, not a change: 422 (IAM) and 400 (gateway) both already resolve to `invalid-input`.
  // The entry exists so a future status change cannot silently split one code's presentation
  // across two services (error.proto:239-247).
  it('pins INVALID_REQUEST_SCHEMA to invalid-input', () => {
    expect(presentationFor(ErrorReason.INVALID_REQUEST_SCHEMA)).toBe('invalid-input');
  });
});

describe('the sentinel', () => {
  // The Record excludes UNSPECIFIED, but fromWireReason's RETURN TYPE includes it, and no
  // narrowing expresses "not UNSPECIFIED". presentationFor is the one place that handles it, so
  // call sites can pass any ErrorReason.
  it('resolves UNSPECIFIED to from-transport rather than throwing', () => {
    expect(presentationFor(ErrorReason.UNSPECIFIED)).toBe('from-transport');
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/errors-presentation.test.ts`

Expected: FAIL — `Failed to resolve import "../src/errors/presentation.js"`.

- [ ] **Step 3: Write the implementation, driven by the compiler**

Create `src/errors/presentation.ts` with the header, the type, the three load-bearing entries, and `presentationFor`:

```ts
// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { ErrorReason } from '@paigasus/proto';

import type { Presentation } from './types.js';

export type PresentationEntry = Presentation | 'from-transport';

/**
 * reason -> presentation, for the reasons whose product meaning differs from what their transport
 * status alone conveys (spec § 9.4).
 *
 * TOTAL over the 57 real reasons by TYPE. Omitting a member is `TS2741`, and the error NAMES the
 * missing one (spec M8) — which is how this table was written and how it must be extended. The
 * type alone is not enough, because a refactor to `Partial<...>` would switch it off silently;
 * `tests/errors-presentation.test.ts` iterates the descriptor and notices. Both are kept.
 *
 * `'from-transport'` is not a default that happened — it is a reviewed decision recorded for each
 * of the 54 reasons that take it.
 */
const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, PresentationEntry> = {
  // Reaches a client only as gRPC Unimplemented, which § 9.2 maps to `generic`. The capability
  // name rides in metadata["capability"], so the screen can name what is switched off.
  [ErrorReason.CAPABILITY_DISABLED]: 'disabled',
  // PermissionDenied/403 on the wire, but the account exists and is deactivated — a different
  // screen from "you do not have permission".
  [ErrorReason.PRINCIPAL_INACTIVE]: 'disabled',
  // A PIN. IAM answers 422 and the gateway 400 for this one code; both already resolve to
  // invalid-input, and this entry stops a future status change from splitting them.
  [ErrorReason.INVALID_REQUEST_SCHEMA]: 'invalid-input',
};

/**
 * The lookup, as a function rather than a bare index.
 *
 * `fromWireReason` returns `ErrorReason | undefined` — a type that INCLUDES UNSPECIFIED even
 * though the implementation excludes it at runtime — and no narrowing expresses "not UNSPECIFIED".
 * Indexing PRESENTATION directly at a call site therefore does not compile. This is the one place
 * that handles the sentinel (spec § 9.4).
 */
export function presentationFor(reason: ErrorReason): PresentationEntry {
  return reason === ErrorReason.UNSPECIFIED ? 'from-transport' : PRESENTATION[reason];
}
```

Now run the compiler and let it name every missing member:

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/tsc -p tsconfig.json --noEmit`

Expected on the first run: `TS2741: Property '[ErrorReason.SLUG_CONFLICT]' is missing in type ...`. Add that member with the value `'from-transport'`, keeping the entries grouped by the registry's three numbering bands (1–39, 300–308, 900–908) with a blank line between bands. Re-run. Repeat until exit 0. **Do not invent member names** — take each one from the compiler message. This loop terminates after the 54 non-override members are present.

- [ ] **Step 4: Run the test to verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/errors-presentation.test.ts`

Expected: PASS, with 57 cases from the `it.each` plus 6 singles.

- [ ] **Step 5: Prove the table bites**

A totality mechanism that cannot fail is decoration. Verify both halves:

```bash
# 1. The TYPE half: delete one non-override entry, expect TS2741 naming it.
# 2. The TEST half: change `Record<...>` to `Partial<Record<...>>` and delete the same entry.
#    MEASURED during implementation: `Partial` ALONE does not compile — it trips TS2322 on
#    presentationFor's return type, because the indexed access is now `PresentationEntry |
#    undefined`. That is the type system resisting harder than expected, not a gap. To reach
#    the state this half guards against, silence that error the way a developer under time
#    pressure would — add a `!` to the indexed access — then expect tsc to pass at exit 0
#    while vitest FAILS on the descriptor-driven case for the deleted member.
```

Restore the file afterwards **by deleting the line you added or re-adding the line you removed** — never with `git checkout --`, which would also discard the uncommitted work in this task.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/presentation.ts ts/packages/paigasus-sdk/tests/errors-presentation.test.ts
git commit -m "feat(ts): add the total reason-to-presentation override table (AC 3)

Two mechanisms, both required. The Record type makes a missing reason a
compile error naming it; the descriptor-driven test notices if that type is
ever weakened to Partial. Three entries are not from-transport: two lift a
reason to a different screen, and one pins an agreement between two services
that answer different statuses for the same code.

Refs SMA-625"
```

---

### Task 4: `mapError` — arms 1, 2 and 3

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/map-error.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (add `"./errors"`), `ts/packages/paigasus-sdk/tests/server-guard.test.ts`
- Test: `ts/packages/paigasus-sdk/tests/map-error.test.ts`

**Interfaces:**
- Consumes: `presentationFor` (Task 3), `presentationForGrpcCode`/`presentationForHttpStatus` (Task 2), `PaigasusError` (Task 1); `ConnectError`, `Code` from `@connectrpc/connect`; `ErrorInfoSchema`, `fromWireReason`, `fromWireDomain` from `@paigasus/proto`.
- Produces:
  ```ts
  type MapErrorInput =
    | { kind: 'grpc'; error: ConnectError }
    | { kind: 'iam-http'; status: number; headers: Headers; body: unknown }
    | { kind: 'gateway-http'; status: number; headers: Headers; body: unknown }
    | { kind: 'terminal-frame'; status: number; body: unknown; ids: FrameIds };
  interface FrameIds { correlationId: string | null; requestId: string | null }
  function mapError(input: MapErrorInput): PaigasusError;
  ```

- [ ] **Step 1: Fix the server-guard test first**

This must land before `src/errors/map-error.ts` is named in `exports`, or the suite reds. Replace the `GUARD_IMPORT` constant and the two `it.each` blocks in `tests/server-guard.test.ts`.

Add `relative` and `sep` to the existing `node:path` import:

```ts
import { dirname, relative, resolve, sep } from 'node:path';
```

Replace the `GUARD_IMPORT` constant with:

```ts
// The guard's expected specifier depends on where the entry FILE sits, not on a single literal.
// `src/index.ts` needs './server-guard.js'; `src/errors/map-error.ts` needs '../server-guard.js'.
// Spec § 6.2 layer 3 always said this test "resolves the path relative to each entry" — the
// original literal did not, and a nested entry could not satisfy it (SMA-625).
const GUARD_MODULE = 'src/server-guard.js';

function expectedGuardImport(target: string): string {
  const fromDir = dirname(resolve(PKG_ROOT, target));
  const specifier = relative(fromDir, resolve(PKG_ROOT, GUARD_MODULE)).split(sep).join('/');
  return `import '${specifier.startsWith('.') ? specifier : `./${specifier}`}';`;
}
```

Replace the two `it.each` bodies with:

```ts
  it.each(entries.filter(([name]) => !UNGUARDED_ENTRIES.has(name)))('entry %s imports the guard as its first import statement', (_name, target) => {
    const source = readFileSync(resolve(PKG_ROOT, target), 'utf8');
    expect(firstImportStatement(source)).toBe(expectedGuardImport(target));
  });

  it.each(entries.filter(([name]) => UNGUARDED_ENTRIES.has(name)))('entry %s deliberately carries no guard', (_name, target) => {
    const source = readFileSync(resolve(PKG_ROOT, target), 'utf8');
    expect(source).not.toContain(expectedGuardImport(target));
  });
```

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/server-guard.test.ts`

Expected: PASS — the two existing entries still resolve to `'./server-guard.js'`, so nothing changes for them yet.

- [ ] **Step 2: Write the failing test**

`tests/map-error.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { Code, ConnectError } from '@connectrpc/connect';
import { create } from '@bufbuild/protobuf';
import { ErrorDomain, ErrorInfoSchema, ErrorReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { mapError } from '../src/errors/map-error.js';

/** A ConnectError carrying a real ErrorInfo detail, built the way the wire builds one. */
function grpcError(overrides: { code?: Code; reason?: string; domain?: string; metadata?: Record<string, string>; message?: string } = {}): ConnectError {
  const info = create(ErrorInfoSchema, {
    reason: overrides.reason ?? 'slug-conflict',
    domain: overrides.domain ?? 'iam.paigasus.io',
    metadata: overrides.metadata ?? { retryable: 'false', correlation_id: 'corr-1', request_id: 'req-1' },
  });
  return new ConnectError(overrides.message ?? 'the slug is taken', overrides.code ?? Code.AlreadyExists, undefined, [{ desc: ErrorInfoSchema, value: info }]);
}

function headers(init: Record<string, string> = {}): Headers {
  return new Headers({ 'paigasus-correlation-id': 'corr-1', 'paigasus-request-id': 'req-1', ...init });
}

describe('arm 1 — a ConnectError with an ErrorInfo detail', () => {
  it('resolves the pair, lifts the three keys, and keeps the rest of metadata', () => {
    const mapped = mapError({
      kind: 'grpc',
      error: grpcError({ metadata: { retryable: 'true', correlation_id: 'corr-1', request_id: 'req-1', field: 'slug' } }),
    });

    expect(mapped.reason).toBe(ErrorReason.SLUG_CONFLICT);
    expect(mapped.domain).toBe(ErrorDomain.IAM);
    expect(mapped.rawReason).toBe('slug-conflict');
    expect(mapped.rawDomain).toBe('iam.paigasus.io');
    expect(mapped.retryable).toBe(true);
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.requestId).toBe('req-1');
    expect(mapped.presentation).toBe('conflict');
    // The three lifted keys are gone; everything else survives.
    expect(mapped.metadata).toEqual({ field: 'slug' });
  });

  it('uses rawMessage, so no [code] prefix leaks into a UI string', () => {
    // ConnectError prefixes `message` with the status: "[already_exists] the slug is taken".
    const mapped = mapError({ kind: 'grpc', error: grpcError({ message: 'the slug is taken' }) });
    expect(mapped.message).toBe('the slug is taken');
    expect(mapped.message).not.toContain('[');
  });

  it('carries the transport code and nothing that could leak Headers', () => {
    const mapped = mapError({ kind: 'grpc', error: grpcError() });
    expect(mapped.transport).toEqual({ kind: 'grpc', code: Code.AlreadyExists });
    // AC 4: no ConnectError, no Headers anywhere in the object.
    expect(JSON.stringify(mapped)).not.toContain('ConnectError');
  });

  it('reads a tri-state retryable, and maps an unexpected value to null', () => {
    const unknown = mapError({ kind: 'grpc', error: grpcError({ metadata: { retryable: 'unknown' } }) });
    expect(unknown.retryable).toBeNull();
    const nonsense = mapError({ kind: 'grpc', error: grpcError({ metadata: { retryable: 'perhaps' } }) });
    expect(nonsense.retryable).toBeNull();
  });
});

describe('arm 1 — a ConnectError with NO detail', () => {
  // Reachable: a network failure, a proxy, or a connection reset carries a status and no
  // ErrorInfo. Without this branch the SDK throws WHILE MAPPING an error.
  it('falls back to the status table with a message-only object', () => {
    const mapped = mapError({ kind: 'grpc', error: new ConnectError('connection reset', Code.Unavailable) });

    expect(mapped.reason).toBeNull();
    expect(mapped.domain).toBeNull();
    expect(mapped.rawReason).toBeNull();
    expect(mapped.rawDomain).toBeNull();
    expect(mapped.retryable).toBeNull();
    expect(mapped.requestId).toBeNull();
    expect(mapped.metadata).toEqual({});
    expect(mapped.presentation).toBe('degraded');
    expect(mapped.message).toBe('connection reset');
  });

  it('still reads a correlation id from the error metadata', () => {
    // ConnectError.metadata is "a union of response headers and trailers", so it can carry the id
    // even when no ErrorInfo detail rode along.
    const err = new ConnectError('connection reset', Code.Unavailable, { 'paigasus-correlation-id': 'corr-9' });
    expect(mapError({ kind: 'grpc', error: err }).correlationId).toBe('corr-9');
  });
});

describe('arm 2 — an IAM HTTP response', () => {
  it('reads the ids and retryable from HEADERS, not the body', () => {
    const mapped = mapError({
      kind: 'iam-http',
      status: 409,
      headers: headers({ 'paigasus-retryable': 'false' }),
      body: { error: { code: 'slug-conflict', message: 'the slug is taken' } },
    });

    expect(mapped.reason).toBe(ErrorReason.SLUG_CONFLICT);
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.requestId).toBe('req-1');
    expect(mapped.retryable).toBe(false);
    expect(mapped.presentation).toBe('conflict');
    expect(mapped.transport).toEqual({ kind: 'http', status: 409 });
    // IAM's envelope has no domain field at all.
    expect(mapped.domain).toBeNull();
    expect(mapped.rawDomain).toBeNull();
  });

  it('treats a missing retryable header as unknown, not false', () => {
    const mapped = mapError({ kind: 'iam-http', status: 409, headers: headers(), body: { error: { code: 'slug-conflict', message: 'x' } } });
    expect(mapped.retryable).toBeNull();
  });
});

describe('arm 3 — the gateway, its own envelope', () => {
  it('maps a registry code and omits a null param', () => {
    const mapped = mapError({
      kind: 'gateway-http',
      status: 400,
      headers: headers({ 'paigasus-retryable': 'false' }),
      body: { error: { message: 'streaming is disabled', type: 'invalid_request_error', param: null, code: 'streaming-disabled' } },
    });

    expect(mapped.reason).toBe(ErrorReason.STREAMING_DISABLED);
    expect(mapped.presentation).toBe('invalid-input');
    // A JSON null must be OMITTED, never stringified to "null".
    expect(mapped.metadata).toEqual({});
  });

  it('carries a present param into metadata', () => {
    const mapped = mapError({
      kind: 'gateway-http',
      status: 400,
      headers: headers(),
      body: { error: { message: 'streaming is disabled', type: 'invalid_request_error', param: 'stream', code: 'streaming-disabled' } },
    });
    expect(mapped.metadata).toEqual({ param: 'stream' });
  });
});

describe('arm 3 — the gateway, an upstream OpenAI passthrough', () => {
  // THE ORDINARY PATH, not an edge case. The gateway forwards a non-2xx upstream body verbatim
  // (chat.rs:19-20), so `code` is OpenAI's vocabulary. This is AC 2's degradation requirement
  // doing its job on the SDK's most-used error surface.
  it('degrades an OpenAI code to reason null while keeping rawReason', () => {
    const mapped = mapError({
      kind: 'gateway-http',
      status: 429,
      headers: headers(),
      body: { error: { message: 'You exceeded your current quota', type: 'insufficient_quota', param: null, code: 'insufficient_quota' } },
    });

    expect(mapped.reason).toBeNull();
    expect(mapped.rawReason).toBe('insufficient_quota');
    expect(mapped.presentation).toBe('rate-limited');
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.message).toBe('You exceeded your current quota');
  });

  // The content-type is FORCED to application/json (chat.rs:118, :141) regardless of what the
  // upstream sent, so an HTML error page arrives labelled JSON. Mapping must not throw.
  it('tolerates a body that is not an object at all', () => {
    const mapped = mapError({ kind: 'gateway-http', status: 502, headers: headers(), body: '<html>502 Bad Gateway</html>' });
    expect(mapped.reason).toBeNull();
    expect(mapped.rawReason).toBeNull();
    expect(mapped.presentation).toBe('degraded');
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.message).not.toBe('');
  });

  it('tolerates a JSON body with no error object', () => {
    const mapped = mapError({ kind: 'gateway-http', status: 500, headers: headers(), body: { detail: 'something else entirely' } });
    expect(mapped.reason).toBeNull();
    expect(mapped.presentation).toBe('generic');
    expect(mapped.message).not.toBe('');
  });
});

describe('AC 2 — the branch never reads message text', () => {
  it('maps one wire error with three messages to three identical objects modulo message', () => {
    const mapped = ['the slug is taken', '', 'ERROR: Slug conflict (retry?)'].map((message) => mapError({ kind: 'grpc', error: grpcError({ message }) }));

    const withoutMessage = mapped.map(({ message: _drop, ...rest }) => rest);
    expect(withoutMessage[1]).toEqual(withoutMessage[0]);
    expect(withoutMessage[2]).toEqual(withoutMessage[0]);
    // ...and the messages really did differ, so the assertion above is not vacuous.
    expect(new Set(mapped.map((m) => m.message)).size).toBe(3);
  });
});

describe('AC 4 — the four codes name four states', () => {
  it.each([
    [Code.Unauthenticated, 'relogin'],
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unavailable, 'degraded'],
  ] as const)('%s yields %s', (code, expected) => {
    expect(mapError({ kind: 'grpc', error: new ConnectError('x', code) }).presentation).toBe(expected);
  });
});

describe('AC 2 — an unknown domain degrades without losing the wire value', () => {
  it('keeps rawDomain when the domain does not resolve', () => {
    const mapped = mapError({ kind: 'grpc', error: grpcError({ domain: 'billing.paigasus.io' }) });
    expect(mapped.domain).toBeNull();
    expect(mapped.rawDomain).toBe('billing.paigasus.io');
  });
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/map-error.test.ts`

Expected: FAIL — `Failed to resolve import "../src/errors/map-error.js"`.

- [ ] **Step 4: Write the implementation**

`src/errors/map-error.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { ConnectError } from '@connectrpc/connect';
import type { Code } from '@connectrpc/connect';
import { ErrorInfoSchema, fromWireDomain, fromWireReason } from '@paigasus/proto';
import type { ErrorDomain, ErrorReason } from '@paigasus/proto';

import { presentationFor } from './presentation.js';
import { presentationForGrpcCode, presentationForHttpStatus } from './transport-tables.js';
import type { PaigasusError, Presentation } from './types.js';

const CORRELATION_ID_HEADER = 'paigasus-correlation-id';
const REQUEST_ID_HEADER = 'paigasus-request-id';
const RETRYABLE_HEADER = 'paigasus-retryable';

/** The keys lifted out of the ErrorInfo map into typed fields, so no raw duplicate can disagree. */
const LIFTED_KEYS = new Set(['retryable', 'correlation_id', 'request_id']);

export interface FrameIds {
  readonly correlationId: string | null;
  readonly requestId: string | null;
}

/**
 * The four wire shapes this SDK can meet (spec § 9.5).
 *
 * One discriminated union rather than four exported functions: the switch below is exhaustive
 * against it, so a fifth shape is a compile error rather than a forgotten arm.
 */
export type MapErrorInput =
  | { readonly kind: 'grpc'; readonly error: ConnectError }
  | { readonly kind: 'iam-http'; readonly status: number; readonly headers: Headers; readonly body: unknown }
  | { readonly kind: 'gateway-http'; readonly status: number; readonly headers: Headers; readonly body: unknown }
  | { readonly kind: 'terminal-frame'; readonly status: number; readonly body: unknown; readonly ids: FrameIds };

/**
 * The wire's retryable is exactly "true" | "false" | "unknown" (correlation.rs:59-65). ANYTHING
 * else — including an absent header — is `null`, never `false`: collapsing it would assert a
 * non-retryability the service declined to assert (ADR-0019 decision 7).
 */
function parseRetryable(raw: string | null | undefined): boolean | null {
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  return null;
}

/** Resolve a wire code, keeping the raw value whether or not it resolves. */
function resolveReason(raw: string | null): { reason: ErrorReason | null; rawReason: string | null } {
  // An empty code is not a wire value — it is an absent one, so rawReason is null, never ''.
  if (raw === null || raw === '') return { reason: null, rawReason: null };
  return { reason: fromWireReason(raw) ?? null, rawReason: raw };
}

function resolveDomain(raw: string | null): { domain: ErrorDomain | null; rawDomain: string | null } {
  if (raw === null || raw === '') return { domain: null, rawDomain: null };
  return { domain: fromWireDomain(raw) ?? null, rawDomain: raw };
}

/** An override wins; `'from-transport'` defers to the status-derived value. */
function applyOverride(reason: ErrorReason | null, fromTransport: Presentation): Presentation {
  if (reason === null) return fromTransport;
  const entry = presentationFor(reason);
  return entry === 'from-transport' ? fromTransport : entry;
}

/** Read `{error:{...}}` defensively — the body may be a string, null, or an unrelated object. */
function errorObject(body: unknown): Record<string, unknown> | null {
  if (typeof body !== 'object' || body === null) return null;
  const candidate = (body as { error?: unknown }).error;
  if (typeof candidate !== 'object' || candidate === null) return null;
  return candidate as Record<string, unknown>;
}

function stringField(source: Record<string, unknown> | null, key: string): string | null {
  const value = source?.[key];
  return typeof value === 'string' ? value : null;
}

/**
 * Map any wire failure onto one `PaigasusError`.
 *
 * Branches on `(domain, reason)` and the transport status ONLY. No arm reads `message` — AC 2 is
 * satisfied structurally, not by convention.
 */
export function mapError(input: MapErrorInput): PaigasusError {
  switch (input.kind) {
    case 'grpc':
      return mapConnect(input.error);
    case 'iam-http':
      return mapHttpEnvelope(input.status, input.headers, input.body, 'iam');
    case 'gateway-http':
      return mapHttpEnvelope(input.status, input.headers, input.body, 'gateway');
    case 'terminal-frame':
      return mapTerminalFrame(input.status, input.body, input.ids);
  }
}

function mapConnect(error: ConnectError): PaigasusError {
  const fromTransport = presentationForGrpcCode(error.code);
  // findDetails returns an ARRAY — `[0]` is undefined whenever no detail rode along, which is a
  // reachable path (a network failure, a proxy, a connection reset). Without this branch the SDK
  // throws while mapping an error, turning a recoverable failure into an unhandled exception.
  const info = error.findDetails(ErrorInfoSchema)[0];
  // Even with no detail, `metadata` is a union of response headers and trailers and may carry it.
  const headerCorrelation = error.metadata.get(CORRELATION_ID_HEADER);

  if (info === undefined) {
    return {
      presentation: fromTransport,
      domain: null,
      reason: null,
      rawReason: null,
      rawDomain: null,
      // rawMessage, not message: ConnectError prefixes the latter with "[code] ".
      message: error.rawMessage,
      correlationId: headerCorrelation ?? null,
      requestId: error.metadata.get(REQUEST_ID_HEADER) ?? null,
      retryable: null,
      metadata: {},
      transport: { kind: 'grpc', code: error.code },
    };
  }

  const { reason, rawReason } = resolveReason(info.reason === '' ? null : info.reason);
  const { domain, rawDomain } = resolveDomain(info.domain === '' ? null : info.domain);
  const metadata: Record<string, string> = {};
  for (const [key, value] of Object.entries(info.metadata)) {
    if (!LIFTED_KEYS.has(key)) metadata[key] = value;
  }

  return {
    presentation: applyOverride(reason, fromTransport),
    domain,
    reason,
    rawReason,
    rawDomain,
    message: error.rawMessage,
    // The ids are OMITTED from ErrorInfo.metadata outside a request scope (convert.rs:69-72), so
    // the header fallback is not decoration.
    correlationId: info.metadata['correlation_id'] ?? headerCorrelation ?? null,
    requestId: info.metadata['request_id'] ?? error.metadata.get(REQUEST_ID_HEADER) ?? null,
    retryable: parseRetryable(info.metadata['retryable']),
    metadata,
    transport: { kind: 'grpc', code: error.code },
  };
}

/**
 * Arms 2 and 3. One body, because IAM's `{error:{code,message}}` is the gateway's envelope minus
 * two fields, and both carry the same three response headers.
 *
 * The gateway arm must tolerate far more than the IAM arm: a non-2xx chat body is usually the
 * UPSTREAM's, forwarded verbatim under a forced application/json (chat.rs:19-20). So the body may
 * be a string, may carry OpenAI's underscored vocabulary, or may have no `error` object at all.
 * None of those throws; each degrades to the status table with the correlation id preserved.
 */
function mapHttpEnvelope(status: number, headers: Headers, body: unknown, source: 'iam' | 'gateway'): PaigasusError {
  const fromTransport = presentationForHttpStatus(status);
  const errorObj = errorObject(body);
  const { reason, rawReason } = resolveReason(stringField(errorObj, 'code'));

  const metadata: Record<string, string> = {};
  if (source === 'gateway') {
    // `param` is Option<String>: a JSON null is OMITTED, never stringified to "null".
    const param = stringField(errorObj, 'param');
    if (param !== null) metadata['param'] = param;
  }

  return {
    presentation: applyOverride(reason, fromTransport),
    // Neither HTTP envelope has a domain field, structurally (spec § 9.1).
    domain: null,
    reason,
    rawReason,
    rawDomain: null,
    message: stringField(errorObj, 'message') ?? `HTTP ${String(status)}`,
    correlationId: headers.get(CORRELATION_ID_HEADER),
    requestId: headers.get(REQUEST_ID_HEADER),
    retryable: parseRetryable(headers.get(RETRYABLE_HEADER)),
    metadata,
    transport: { kind: 'http', status },
  };
}

function mapTerminalFrame(status: number, body: unknown, ids: FrameIds): PaigasusError {
  const errorObj = errorObject(body);
  const { reason, rawReason } = resolveReason(stringField(errorObj, 'code'));

  return {
    presentation: applyOverride(reason, presentationForHttpStatus(status)),
    domain: null,
    reason,
    rawReason,
    rawDomain: null,
    message: stringField(errorObj, 'message') ?? 'upstream stream error',
    correlationId: ids.correlationId,
    requestId: ids.requestId,
    // The frame deliberately carries no retryable signal: the 200 head is already committed, so no
    // header can change (chat.rs:56-62).
    retryable: null,
    metadata: {},
    // The COMMITTED 2xx status — chat.rs:128 branches on is_success(), so 200 is usual, not the
    // only reachable value.
    transport: { kind: 'http', status },
  };
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/map-error.test.ts`

Expected: PASS.

Note `presentationForHttpStatus(200)` is `generic`, so the terminal-frame arm's presentation comes from the override table when `upstream-error` resolves, and `generic` otherwise. Task 6 asserts this.

- [ ] **Step 6: Add the `./errors` entry and run the whole suite**

Extend `package.json`'s `exports` to exactly these **four** entries:

```json
  "exports": {
    ".": "./src/index.ts",
    "./iam": "./src/iam.ts",
    "./errors": "./src/errors/map-error.ts",
    "./errors/types": "./src/errors/types.ts"
  },
```

**`./chat` is deliberately absent here** — `src/chat.ts` does not exist until Task 5, and no commit may leave the map naming a missing file (Global Constraints). Task 5 adds the fifth entry in the same commit as the file.

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run && ../../node_modules/.bin/tsc -p tsconfig.json --noEmit`

Expected: PASS and exit 0. The guard suite now checks `./errors` and resolves its expected specifier to `'../server-guard.js'` — which is what Step 1 made possible.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/map-error.ts ts/packages/paigasus-sdk/tests/map-error.test.ts \
        ts/packages/paigasus-sdk/tests/server-guard.test.ts ts/packages/paigasus-sdk/package.json
git commit -m "feat(ts): map four wire shapes onto one PaigasusError (AC 2, AC 4)

One function over a discriminated union, so a fifth wire shape is a compile
error rather than a forgotten arm. Two paths are easy to get wrong and are
tested directly: a ConnectError carrying no ErrorInfo detail, which is what a
proxy or a reset produces; and a gateway body that is really the upstream's,
forwarded verbatim, so it may carry OpenAI's vocabulary or not be JSON at all.
Neither throws while mapping.

The server-guard test now computes each entry's expected guard specifier from
that entry's own directory. It pinned a literal that a nested entry could not
satisfy, which spec section 6.2 never intended.

Refs SMA-625"
```

---

### Task 5: The chat client

**Files:**
- Create: `ts/packages/paigasus-sdk/src/chat.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (add `"./chat"`)
- Test: `ts/packages/paigasus-sdk/tests/chat.test.ts`

**Interfaces:**
- Consumes: `mapError` (Task 4); `Auth` from `./transport.js` (**type-only** — a value import would drag `@connectrpc/connect-node`'s HTTP/2 stack into every `./chat` consumer).
- Produces: `ChatCompletionRequest`, `ChatOptions`, `ChatCompletionResult`, `chatCompletion`.

- [ ] **Step 1: Write the failing test**

`tests/chat.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, vi } from 'vitest';

import { chatCompletion, PaigasusHttpError } from '../src/chat.js';

const OPTIONS = { baseUrl: 'https://gateway.test', auth: { bearer: 'tok' } } as const;
const REQUEST = { model: 'gpt-4o', messages: [{ role: 'user', content: 'hi' }] };

function respond(body: BodyInit | null, init: ResponseInit): Response {
  return new Response(body, { ...init, headers: { 'paigasus-correlation-id': 'corr-1', 'paigasus-request-id': 'req-1', ...init.headers } });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the non-streaming path', () => {
  it('returns kind json with the ids read from the response headers', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => respond(JSON.stringify({ id: 'chatcmpl-1' }), { status: 200, headers: { 'content-type': 'application/json' } })),
    );

    const result = await chatCompletion(REQUEST, OPTIONS);

    expect(result.kind).toBe('json');
    expect(result.status).toBe(200);
    // Both variants carry the ids, so a caller logging a SUCCESS has something to log.
    expect(result.correlationId).toBe('corr-1');
    expect(result.requestId).toBe('req-1');
    if (result.kind !== 'json') throw new Error('unreachable');
    expect(result.body).toEqual({ id: 'chatcmpl-1' });
  });

  it('sends the bearer and the body', async () => {
    const fetchSpy = vi.fn(async () => respond('{}', { status: 200, headers: { 'content-type': 'application/json' } }));
    vi.stubGlobal('fetch', fetchSpy);

    await chatCompletion(REQUEST, OPTIONS);

    const [url, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('https://gateway.test/v1/chat/completions');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer tok');
    expect(JSON.parse(init.body as string)).toEqual(REQUEST);
  });

  it('sends no authorization header for an anonymous caller', async () => {
    const fetchSpy = vi.fn(async () => respond('{}', { status: 200, headers: { 'content-type': 'application/json' } }));
    vi.stubGlobal('fetch', fetchSpy);

    await chatCompletion(REQUEST, { baseUrl: 'https://gateway.test', auth: { anonymous: true } });

    const [, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(new Headers(init.headers).has('authorization')).toBe(false);
  });
});

describe('the streaming path — AC 4', () => {
  it('returns the IDENTICAL ReadableStream object', async () => {
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('data: {}\n\n'));
        controller.close();
      },
    });
    const response = respond(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
    vi.stubGlobal('fetch', vi.fn(async () => response));

    const result = await chatCompletion({ ...REQUEST, stream: true }, OPTIONS);

    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');
    // Identity, not equivalence. The SDK must not read, buffer, decode or re-encode.
    expect(result.body).toBe(response.body);
    expect(result.correlationId).toBe('corr-1');
  });

  // chat.rs:138-141 — a `stream:true` request whose upstream answered non-2xx comes back as JSON,
  // not SSE. A client that trusts its own flag misreads this.
  it('branches on content-type, not on the caller stream flag', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => respond(JSON.stringify({ ok: true }), { status: 200, headers: { 'content-type': 'application/json' } })),
    );

    const result = await chatCompletion({ ...REQUEST, stream: true }, OPTIONS);
    expect(result.kind).toBe('json');
  });
});

describe('a non-2xx throws a PaigasusError', () => {
  it('maps the gateway envelope', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        respond(JSON.stringify({ error: { message: 'streaming is disabled', type: 'invalid_request_error', param: 'stream', code: 'streaming-disabled' } }), {
          status: 400,
          headers: { 'content-type': 'application/json', 'paigasus-retryable': 'false' },
        }),
      ),
    );

    const error = (await chatCompletion(REQUEST, OPTIONS).catch((e: unknown) => e)) as PaigasusHttpError;
    // Pins the contract (spec § 8.2): the thrown value is a real Error, not the plain PaigasusError.
    expect(error).toBeInstanceOf(PaigasusHttpError);
    expect(error.error.presentation).toBe('invalid-input');
    expect(error.error.correlationId).toBe('corr-1');
    expect(error.error.metadata).toEqual({ param: 'stream' });
  });

  it('maps an upstream 429 to rate-limited without resolving OpenAI vocabulary', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        respond(JSON.stringify({ error: { message: 'You exceeded your current quota', type: 'insufficient_quota', param: null, code: 'insufficient_quota' } }), {
          status: 429,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    );

    const error = (await chatCompletion(REQUEST, OPTIONS).catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error.error.presentation).toBe('rate-limited');
    expect(error.error.reason).toBeNull();
    expect(error.error.rawReason).toBe('insufficient_quota');
  });

  it('does not throw while mapping a body that is not JSON', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => respond('<html>502 Bad Gateway</html>', { status: 502, headers: { 'content-type': 'application/json' } })),
    );

    const error = (await chatCompletion(REQUEST, OPTIONS).catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error.error.presentation).toBe('degraded');
    expect(error.error.correlationId).toBe('corr-1');
  });
});

describe('deadlines', () => {
  it('rejects when the response head does not arrive in time', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init: RequestInit) => {
        return await new Promise<Response>((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => {
            reject(new DOMException('aborted', 'AbortError'));
          });
        });
      }),
    );

    const promise = chatCompletion(REQUEST, { ...OPTIONS, timeoutMs: 10 });
    const error = (await promise.catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error.error.presentation).toBe('degraded');
  });

  // The deadline timer must be CLEARED once the head lands, or the same signal would abort the
  // body mid-stream. This is the proof.
  it('does not abort a slow body after the head has arrived', async () => {
    let signal: AbortSignal | undefined;
    const body = new ReadableStream<Uint8Array>({ start() {} });
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init: RequestInit) => {
        signal = init.signal ?? undefined;
        return respond(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
      }),
    );

    const result = await chatCompletion({ ...REQUEST, stream: true }, { ...OPTIONS, timeoutMs: 10 });
    expect(result.kind).toBe('stream');
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(signal?.aborted).toBe(false);
  });
});

describe('cancellation', () => {
  // SCOPED to forwarding: a stub fetch cannot prove a socket closed. See spec § 8.4.
  it('forwards a cancel on the returned stream to the upstream body', async () => {
    const cancel = vi.fn(async () => {});
    const body = new ReadableStream<Uint8Array>({ start() {}, cancel });
    vi.stubGlobal('fetch', vi.fn(async () => respond(body, { status: 200, headers: { 'content-type': 'text/event-stream' } })));

    const result = await chatCompletion({ ...REQUEST, stream: true }, OPTIONS);
    if (result.kind !== 'stream') throw new Error('unreachable');
    await result.body.cancel();

    expect(cancel).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/chat.test.ts`

Expected: FAIL — `Failed to resolve import "../src/chat.js"`.

- [ ] **Step 3: Write the implementation**

`src/chat.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import './server-guard.js';

import { mapError } from './errors/map-error.js';
// TYPE-ONLY, deliberately. `transport.ts` imports @connectrpc/connect-node at module scope; a
// value import here would drag the whole HTTP/2 stack into every `./chat` consumer, defeating the
// reason § 6.1 gives for having subpaths at all. Under verbatimModuleSyntax this emits nothing.
import type { Auth } from './transport.js';

const CORRELATION_ID_HEADER = 'paigasus-correlation-id';
const REQUEST_ID_HEADER = 'paigasus-request-id';

/** Matches the gRPC transport's default (spec § 8.4), so the two surfaces agree. */
export const DEFAULT_CHAT_TIMEOUT_MS = 10_000;

/**
 * The gateway requires `model` and `messages` (dto.rs:22-34); a body missing either is rendered as
 * a 400 `invalid-request-schema`. Requiring them here turns a round-trip into a compile error.
 *
 * `messages` is `unknown[]` on purpose: the gateway does not model message content, and pinning
 * OpenAI's message union here would make every upstream addition a breaking change in this
 * package. The index signature mirrors the gateway's `#[serde(flatten)]` passthrough.
 */
export interface ChatCompletionRequest {
  model: string;
  messages: unknown[];
  stream?: boolean;
  [key: string]: unknown;
}

export interface ChatOptions {
  readonly baseUrl: string;
  readonly auth: Auth;
  /** Bounds the wait for the RESPONSE HEAD only. Defaults to `DEFAULT_CHAT_TIMEOUT_MS`. */
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
}

interface ChatResultBase {
  readonly status: number;
  readonly correlationId: string | null;
  readonly requestId: string | null;
}

export type ChatCompletionResult = (ChatResultBase & { readonly kind: 'json'; readonly body: unknown }) | (ChatResultBase & { readonly kind: 'stream'; readonly body: ReadableStream<Uint8Array> });

/** A body may be labelled application/json and not be JSON — the gateway forces the header. */
async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/**
 * `POST /v1/chat/completions` on the gateway.
 *
 * Returns on 2xx and THROWS a `PaigasusError` otherwise — one rule, not two. A third
 * `{ kind: 'error' }` variant would let a caller ignore a failure by not checking a discriminant.
 */
export async function chatCompletion(request: ChatCompletionRequest, options: ChatOptions): Promise<ChatCompletionResult> {
  const headers = new Headers({ 'content-type': 'application/json' });
  if ('bearer' in options.auth) headers.set('authorization', `Bearer ${options.auth.bearer}`);

  // A pre-header deadline, merged with the caller's signal. The timer is cleared the moment the
  // head lands: one signal held past that point would abort the BODY too, which is why a single
  // AbortSignal cannot serve as both a header deadline and a live-stream lifeline (spec § 8.4).
  const deadline = new AbortController();
  const timer = setTimeout(() => {
    deadline.abort(new DOMException('the response head did not arrive before the deadline', 'TimeoutError'));
  }, options.timeoutMs ?? DEFAULT_CHAT_TIMEOUT_MS);
  const signal = options.signal === undefined ? deadline.signal : AbortSignal.any([options.signal, deadline.signal]);

  let response: Response;
  try {
    response = await fetch(`${options.baseUrl}/v1/chat/completions`, { method: 'POST', headers, body: JSON.stringify(request), signal });
  } catch (cause) {
    // A transport failure or an expired deadline. 504 is the honest status: nothing came back.
    throw mapError({ kind: 'gateway-http', status: 504, headers: new Headers(), body: { error: { message: cause instanceof Error ? cause.message : 'the chat request failed' } } });
  } finally {
    clearTimeout(timer);
  }

  const correlationId = response.headers.get(CORRELATION_ID_HEADER);
  const requestId = response.headers.get(REQUEST_ID_HEADER);

  if (!response.ok) {
    throw mapError({ kind: 'gateway-http', status: response.status, headers: response.headers, body: await readBody(response) });
  }

  // Branch on what the gateway ACTUALLY sent, never on `request.stream`: a `stream:true` request
  // whose upstream answered non-2xx comes back as JSON (chat.rs:138-141).
  const isStream = response.headers.get('content-type')?.includes('text/event-stream') === true;

  if (isStream && response.body !== null) {
    // AC 4: the identical object. No pipeThrough, no reader, no decode. Cancellation propagates
    // to the upstream connection because this IS the upstream body.
    return { kind: 'stream', status: response.status, body: response.body, correlationId, requestId };
  }

  return { kind: 'json', status: response.status, body: await readBody(response), correlationId, requestId };
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/chat.test.ts`

Expected: PASS.

- [ ] **Step 5: Add the `./chat` entry, run everything, commit**

Add `"./chat": "./src/chat.ts"` to `package.json`'s `exports`, keeping the five-entry map from Task 4 Step 6.

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run && ../../node_modules/.bin/tsc -p tsconfig.json --noEmit`

Expected: PASS and exit 0. The guard suite now covers all five entries.

```bash
git add ts/packages/paigasus-sdk/src/chat.ts ts/packages/paigasus-sdk/tests/chat.test.ts ts/packages/paigasus-sdk/package.json
git commit -m "feat(ts): add the OpenAI-compatible chat client (AC 4)

Streaming is a true passthrough: the caller receives the identical
ReadableStream, so the SDK never reads, buffers, decodes or re-encodes, and
cancellation reaches the upstream connection for free. There is deliberately no
idle timeout, because detecting idleness requires an observer in the byte path.

The client branches on the response content-type rather than the caller's own
stream flag, because a stream:true request whose upstream answered non-2xx
comes back as JSON. A non-2xx throws a PaigasusError.

Refs SMA-625"
```

---

### Task 6: The terminal-frame parser, its fixture and its drift check

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/terminal-frame.ts`, `ts/packages/paigasus-sdk/tests/fixtures/terminal-frame.ts`
- Test: `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts`

**Interfaces:**
- Consumes: `mapError`, `FrameIds` (Task 4).
- Produces: `createTerminalFrameParser(ids: FrameIds, committedStatus: number): { push(chunk: Uint8Array | string): PaigasusError | null }`, and `TERMINAL_SSE_ERROR` from the fixture. `committedStatus` was made a required second parameter mid-implementation, because the byte-level parser cannot see the gateway's real 2xx status and the caller — the only thing that constructs a parser — already has it.

- [ ] **Step 1: Write the fixture, pinned verbatim**

`tests/fixtures/terminal-frame.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The terminal SSE error frame, copied VERBATIM from the Rust constant TERMINAL_SSE_ERROR at
// rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:63.
//
// Without this fixture the parser's test would hand-build the object it expects and could not fail
// when the gateway's frame changes — which is the one drift it exists to absorb. The drift check
// in tests/terminal-frame.test.ts reads the Rust file and asserts the two still agree, and
// moon.yml lists that file among paigasus-sdk-ts:test's inputs so the check RUNS on the PR that
// changes it.
export const TERMINAL_SSE_ERROR = 'data: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n';

/** Where the Rust constant lives, relative to the repository root. */
export const CHAT_RS = 'rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs';
```

- [ ] **Step 2: Write the failing test**

`tests/terminal-frame.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { createTerminalFrameParser } from '../src/errors/terminal-frame.js';
import { CHAT_RS, TERMINAL_SSE_ERROR } from './fixtures/terminal-frame.js';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const IDS = { correlationId: 'corr-1', requestId: 'req-1' };
const encode = (s: string): Uint8Array => new TextEncoder().encode(s);

describe('the fixture still matches the Rust constant', () => {
  it('finds the frame verbatim in chat.rs', () => {
    const source = readFileSync(resolve(REPO_ROOT, CHAT_RS), 'utf8');
    // The Rust literal escapes its quotes and newlines; reconstruct what it denotes.
    const literal = /const TERMINAL_SSE_ERROR: &str = "(.*)";/.exec(source)?.[1];
    expect(literal).toBeDefined();
    const denoted = literal!.replaceAll('\\"', '"').replaceAll('\\n', '\n');
    expect(denoted).toBe(TERMINAL_SSE_ERROR);
  });
});

describe('the parser', () => {
  it('returns the mapped error for the pinned frame', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    const error = parser.push(encode(TERMINAL_SSE_ERROR));

    expect(error).not.toBeNull();
    expect(error!.rawReason).toBe('upstream-error');
    expect(error!.message).toBe('upstream stream error');
    expect(error!.correlationId).toBe('corr-1');
    expect(error!.requestId).toBe('req-1');
    // No header can change after the head is committed, so the frame asserts nothing here.
    expect(error!.retryable).toBeNull();
  });

  it('finds a frame split across two chunks', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    const half = Math.floor(TERMINAL_SSE_ERROR.length / 2);
    expect(parser.push(encode(TERMINAL_SSE_ERROR.slice(0, half)))).toBeNull();
    expect(parser.push(encode(TERMINAL_SSE_ERROR.slice(half)))).not.toBeNull();
  });

  it('finds the terminal frame after a data frame in the same chunk', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    const error = parser.push(encode(`data: {"id":"chatcmpl-1"}\n\n${TERMINAL_SSE_ERROR}`));
    expect(error).not.toBeNull();
    expect(error!.rawReason).toBe('upstream-error');
  });

  it('returns null for ordinary data frames', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    expect(parser.push(encode('data: {"id":"a"}\n\ndata: [DONE]\n\n'))).toBeNull();
  });

  it('returns null for a partial trailing record that never completes', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    expect(parser.push(encode('data: {"error":{"code":"upstream-'))).toBeNull();
  });

  it('survives a multi-byte character split across a chunk boundary', () => {
    // A fresh TextDecoder per chunk would corrupt this. The parser owns ONE streaming decoder.
    const frame = 'data: {"error":{"message":"café ☕","type":"api_error","param":null,"code":"upstream-error"}}\n\n';
    const bytes = encode(frame);
    const cut = frame.indexOf('café') + 4; // lands inside the two-byte é
    const parser = createTerminalFrameParser(IDS, 200);
    expect(parser.push(bytes.slice(0, cut))).toBeNull();
    const error = parser.push(bytes.slice(cut));
    expect(error).not.toBeNull();
    expect(error!.message).toBe('café ☕');
  });

  it.each([['\n\n'], ['\r\n\r\n'], ['\r\r'], ['\n\r\n'], ['\r\n\n'], ['\n\r']])('accepts the %j record delimiter', (delimiter) => {
    const parser = createTerminalFrameParser(IDS, 200);
    const frame = TERMINAL_SSE_ERROR.replace('\n\n', delimiter);
    expect(parser.push(encode(frame))).not.toBeNull();
  });

  it('returns null forever after the first terminal error', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    expect(parser.push(encode(TERMINAL_SSE_ERROR))).not.toBeNull();
    expect(parser.push(encode(TERMINAL_SSE_ERROR))).toBeNull();
  });

  it('drops the buffer rather than growing without bound', () => {
    // An upstream that never sends a blank line must not exhaust memory. Dropping is correct: the
    // parser is a best-effort observer, never a reason to fail a stream still delivering data.
    const parser = createTerminalFrameParser(IDS, 200);
    expect(parser.push(encode('data: '.padEnd(70_000, 'x')))).toBeNull();
    expect(parser.push(encode(TERMINAL_SSE_ERROR))).not.toBeNull();
  });

  it('accepts a string chunk from a caller that already decoded', () => {
    const parser = createTerminalFrameParser(IDS, 200);
    expect(parser.push(TERMINAL_SSE_ERROR)).not.toBeNull();
  });

  // committedStatus was added as a required second parameter mid-implementation (see Interfaces
  // above): chat.rs:128 branches on is_success(), so a non-200 2xx is a reachable committed status.
  it('carries a non-200 committed status onto the mapped error', () => {
    const parser = createTerminalFrameParser(IDS, 201);
    const error = parser.push(encode(TERMINAL_SSE_ERROR));

    expect(error).not.toBeNull();
    expect(error!.transport).toEqual({ kind: 'http', status: 201 });
  });
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/terminal-frame.test.ts`

Expected: FAIL — `Failed to resolve import "../src/errors/terminal-frame.js"`.

- [ ] **Step 4: Write the implementation**

`src/errors/terminal-frame.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { mapError } from './map-error.js';
import type { FrameIds } from './map-error.js';
import type { PaigasusError } from './types.js';

/** Beyond this, a stream that never sends a blank line would grow the buffer without bound. */
const MAX_BUFFER_BYTES = 64 * 1024;

/**
 * A record ends at a blank line — any TWO consecutive line terminators, each of which may be
 * CRLF, LF or CR. chat.rs:63 emits `\n\n`, but upstream chunks pass through this gateway
 * verbatim, so the grammar is what this follows, not that one producer's habit.
 *
 * This single regex replaces an earlier three-string `DELIMITERS` list, changed mid-implementation
 * once the list was seen to miss the three mixed forms (`\n\r\n`, `\r\n\n`, `\n\r`). The alternation
 * puts `\r\n` first so a CRLF is consumed whole rather than as a bare CR — which is what makes
 * `\r\n\r\n` match as one four-character delimiter instead of two two-character ones.
 */
const RECORD_DELIMITER = /(?:\r\n|\r|\n){2}/;

function firstDelimiter(buffer: string): { index: number; length: number } | null {
  const match = RECORD_DELIMITER.exec(buffer);
  if (match === null) return null;
  return { index: match.index, length: match[0].length };
}

/**
 * A stateful, incremental parser over an SSE stream, looking for the gateway's ONE terminal error
 * frame (spec § 8.5).
 *
 * Exported because the SDK never reads the stream itself — scanning would mean buffering, which
 * defeats the passthrough AC 4 requires. A caller consuming its own stream drives this with the
 * chunks it is already reading. The parser holds only the trailing partial record, so it does not
 * reintroduce that buffering.
 *
 * A chunk boundary can fall anywhere, so a parser over one chunk would miss the terminal frame in
 * exactly the split case it exists to catch — hence the state.
 *
 * `committedStatus` is the gateway's real 2xx response status, added as a required second
 * parameter mid-implementation: the parser itself sees only bytes and cannot know it, so
 * `chatCompletion` returns it on `ChatCompletionResult.status` and the caller — the only thing
 * that constructs a parser — always has it. chat.rs:128 branches on `is_success()`, so 200 is the
 * usual value but not the only reachable one (spec § 9.5 arm 4).
 */
export function createTerminalFrameParser(ids: FrameIds, committedStatus: number): { push(chunk: Uint8Array | string): PaigasusError | null } {
  // ONE decoder for the parser's lifetime. A fresh TextDecoder per chunk corrupts a multi-byte
  // character split across a chunk boundary — the same defect class as a split record, one level
  // down.
  const decoder = new TextDecoder('utf-8');
  let buffer = '';
  let done = false;

  return {
    push(chunk: Uint8Array | string): PaigasusError | null {
      if (done) return null;

      buffer += typeof chunk === 'string' ? chunk : decoder.decode(chunk, { stream: true });

      let found: PaigasusError | null = null;
      for (;;) {
        const delimiter = firstDelimiter(buffer);
        if (delimiter === null) break;
        const record = buffer.slice(0, delimiter.index);
        buffer = buffer.slice(delimiter.index + delimiter.length);

        const error = parseRecord(record, ids, committedStatus);
        if (error !== null) {
          found = error;
          done = true;
          buffer = '';
          break;
        }
      }

      // Best-effort: drop an unbounded partial rather than fail a stream still delivering data.
      if (buffer.length > MAX_BUFFER_BYTES) buffer = '';
      return found;
    },
  };
}

/** An SSE record is an error only if its `data:` payload is an OpenAI envelope carrying a code. */
function parseRecord(record: string, ids: FrameIds, committedStatus: number): PaigasusError | null {
  const data = record
    .split(/\r\n|\n|\r/)
    .filter((line) => line.startsWith('data:'))
    .map((line) => line.slice('data:'.length).trim())
    .join('\n');
  if (data === '' || data === '[DONE]') return null;

  let body: unknown;
  try {
    body = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as { error?: unknown }).error;
  if (typeof error !== 'object' || error === null) return null;
  if (typeof (error as { code?: unknown }).code !== 'string') return null;

  return mapError({ kind: 'terminal-frame', status: committedStatus, body, ids });
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run tests/terminal-frame.test.ts`

Expected: PASS.

- [ ] **Step 6: Prove the drift check bites**

Change one character inside the fixture's `TERMINAL_SSE_ERROR` string (e.g. `upstream-error` to `upstream-errorX`) and re-run.

Expected: the `finds the frame verbatim in chat.rs` case FAILS. Restore by reverting your single-character edit — **not** with `git checkout --`, which would discard this task's uncommitted work.

- [ ] **Step 7: Export the parser and commit**

Add to `src/chat.ts`, at the end of the file:

```ts
// Re-exported from `./chat` rather than `./errors`, for two reasons. It would be a CYCLE on
// `./errors` — terminal-frame.ts already imports map-error.ts, so map-error.ts re-exporting it
// closes the loop. And a caller reaches for this while consuming a chat stream, which is what
// spec § 8.5 describes it as being for.
export { createTerminalFrameParser } from './errors/terminal-frame.js';
export type { FrameIds } from './errors/map-error.js';
```

Run the full suite and typecheck: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-sdk && ../../node_modules/.bin/vitest run && ../../node_modules/.bin/tsc -p tsconfig.json --noEmit`

Expected: PASS and exit 0.

```bash
git add ts/packages/paigasus-sdk/src/errors/terminal-frame.ts ts/packages/paigasus-sdk/src/chat.ts \
        ts/packages/paigasus-sdk/tests/terminal-frame.test.ts ts/packages/paigasus-sdk/tests/fixtures/terminal-frame.ts
git commit -m "feat(ts): add the terminal SSE frame parser and pin its fixture to chat.rs

The SDK never scans the stream, because scanning means buffering, so this is
exported for a caller consuming its own stream. It is stateful because an SSE
record can split across a chunk boundary, which is exactly the case a terminal
error hits, and it owns one streaming TextDecoder because a multi-byte
character can split the same way.

The fixture is copied verbatim from the Rust constant and a test reads chat.rs
to assert the two still agree. Task 7 adds the Moon input that makes that
assertion run on the PR that changes the constant.

Refs SMA-625"
```

---

### Task 7: Make the drift check reachable

**Files:**
- Modify: `ts/packages/paigasus-sdk/moon.yml`, `ci/affected-graph/run.sh`

**Interfaces:** none — this task changes task selection, not code.

This is the task that makes Task 6's drift check real. Until it lands, editing `chat.rs:63` selects no TypeScript task and the fixture assertion never runs on the PR that breaks it — the identical defect spec § 11.1 measured and fixed for AC 3.

- [ ] **Step 1: Measure the CURRENT selection, before changing anything**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
touch rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs
moon query tasks --affected
```

Record whether any `paigasus-sdk-ts` task appears. Expected: **none**. This is the measurement that justifies the change; if a `paigasus-sdk-ts` task already appears, stop and re-read § 8.6 before proceeding.

Note: `moon query tasks --affected` emits each selected task's `deps[]`, and every dep entry carries its own `"target"` key — so `grep -o '"target": "[^"]*"'` counts SCHEDULED upstreams as SELECTIONS. Parse the JSON and take one target per `tasks[project][task]`.

- [ ] **Step 2: Add the input**

In `ts/packages/paigasus-sdk/moon.yml`, extend the `test` task's `inputs` (APPEND — the file's own comment explains why `options.merge: replace` would drop the four inherited inputs):

```yaml
  test:
    # vitest.config.ts matches neither src/**/* nor tests/**/*, so without this line an edit to the
    # resolution conditions serves a cached PASS — and those conditions are the only thing keeping
    # `server-only`'s unconditional throw from failing the whole suite.
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - 'vitest.config.ts'
      # SMA-625 — tests/fixtures/terminal-frame.ts pins the gateway's terminal SSE frame verbatim,
      # and tests/terminal-frame.test.ts reads THIS file to assert the two still agree. Without
      # this input, editing the Rust constant selects no TypeScript task and the drift check never
      # runs on the one PR it exists to catch — the same defect § 11.1 fixed for AC 3.
      # `ci/affected-graph/run.sh`'s "chat-rs->sdk" case is the only control on this line.
      - '/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs'
```

- [ ] **Step 3: Re-measure and DERIVE the expected set**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
touch rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs
moon query tasks --affected
```

`paigasus-sdk-ts:test` must now appear. Write down the **complete** selected set, sorted, comma-joined. **Do not invent it and do not copy the `proto->sdk` case's set** — a Rust edit selects Rust tasks the TypeScript cases never see.

- [ ] **Step 4: Add the affected-graph case**

In `ci/affected-graph/run.sh`, directly after the `proto-iam->sdk` case, insert:

```bash
  # SMA-625 — a gateway chat.rs edit must select the SDK's test.
  # This is the ONLY control on the `/rs/.../chat.rs` input in ts/packages/paigasus-sdk/moon.yml.
  # tests/fixtures/terminal-frame.ts pins the gateway's terminal SSE frame VERBATIM and
  # tests/terminal-frame.test.ts reads chat.rs to assert the two agree — so without that input the
  # drift check is inert against exactly the change it exists to absorb, and nothing else notices
  # (`repo:input-liveness` scans `repo:*` tasks only, and proves DECLARED inputs are live, never
  # that NEEDED ones are declared). MEASURED before the input existed: the same edit selected no
  # paigasus-sdk-ts task at all.
  # ONE anchor suffices here, unlike the `proto->sdk` pair above: that pair's two anchors prove a
  # GLOB's width, and this input is a literal path with no width to prove.
  # Strict equality: re-baseline deliberately when the set legitimately changes.
  run_task_case_ci "chat-rs->sdk" "rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs" \
    "<MEASURED SET FROM STEP 3>"
```

Replace `<MEASURED SET FROM STEP 3>` with the set you measured. Do not invent it.

- [ ] **Step 5: Run the affected-graph suite**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/affected-graph/run.sh`

Use `/bin/bash` explicitly: this machine's bash 5.3.15 deadlocks forever on a `while read` fed by a `<<<` here-string over ~512 bytes, which hangs this suite locally while CI stays green. macOS's system bash 3.2 runs it clean. A hang is that bug, not a gate failure.

Expected: `PASS chat-rs->sdk`, suite rc 0.

- [ ] **Step 6: Prove the case bites**

Remove the `/rs/.../chat.rs` line from `moon.yml`, re-run the suite, and confirm `chat-rs->sdk` FAILS. Restore the line by re-adding it.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/moon.yml ci/affected-graph/run.sh
git commit -m "ci: select the SDK test suite when the gateway chat frame changes

The terminal-frame fixture pins a Rust constant verbatim, but task inputs are
the only thing conferring affectedness in Moon, and no rs/** path was among the
SDK test task's inputs. So the drift check could not run on the PR that breaks
it. The new affected-graph case is the only control on that input line.

Refs SMA-625"
```

---

### Task 8: Full-graph verification

**Files:** none, unless a gate reds.

- [ ] **Step 1: Run the full CI graph**

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

Two known environment effects, neither a defect in this change:

- The three `repo:release-parity*` gates abort **INCONCLUSIVE at rc=2** inside an agent session, because `proto` emits NDJSON on stdout when it detects one. An inconclusive abort is not a pass — read the message.
- A sub-3s `repo:affected-smoke` failure under a concurrent `moon ci` is a known intermittent abort. **Capture the full task output before re-running**, because a passing re-run overwrites `stdout.log`, truncates `stderr.log` and flips the `ciReport.json` row. Grep the captured output for `proto-shim`: if present, the failure is not about the affected graph.

- [ ] **Step 2: Diagnose any failure non-destructively**

Before re-running anything, copy `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` outside the repo. Then:

```bash
jq '.actions[] | select(.status=="failed")
    | {label, error,
       exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
   .moon/cache/ciReport.json
```

There is no action-level `exitCode` key; the real one is in `operations[]` on the `task-execution` entry. Read `stdout.log` and `stderr.log` for the why, and compare the action's `finishedAt` against `lastRun.json`'s `lastRunTime` before trusting them — if they disagree, the logs belong to a different run.

<!-- moon-diagnosis:ok -->

- [ ] **Step 3: Verify the diff matches this plan**

```bash
git diff origin/main --stat
```

Expected, exactly these files:

```
ci/affected-graph/run.sh
docs/superpowers/plans/2026-09-10-sma-625-error-model-and-chat-client.md
docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md
ts/packages/paigasus-sdk/moon.yml
ts/packages/paigasus-sdk/package.json
ts/packages/paigasus-sdk/src/chat.ts
ts/packages/paigasus-sdk/src/errors/map-error.ts
ts/packages/paigasus-sdk/src/errors/presentation.ts
ts/packages/paigasus-sdk/src/errors/terminal-frame.ts
ts/packages/paigasus-sdk/src/errors/transport-tables.ts
ts/packages/paigasus-sdk/src/errors/types.ts
ts/packages/paigasus-sdk/tests/chat.test.ts
ts/packages/paigasus-sdk/tests/errors-presentation.test.ts
ts/packages/paigasus-sdk/tests/errors-tables.test.ts
ts/packages/paigasus-sdk/tests/fixtures/terminal-frame.ts
ts/packages/paigasus-sdk/tests/map-error.test.ts
ts/packages/paigasus-sdk/tests/server-guard.test.ts
ts/packages/paigasus-sdk/tests/terminal-frame.test.ts
```

Anything else is unintended. `ts/pnpm-lock.yaml` in particular must NOT appear — this change adds no dependency.

- [ ] **Step 3a: Lint the package**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts && ./node_modules/.bin/eslint packages/paigasus-sdk --max-warnings 0`

Expected: exit 0, no output. This is a separate gate from `tsc` — see Global Constraints for the two rules this package trips that neither vitest nor `tsc` can see.

- [ ] **Step 4: Grep for debug residue**

```bash
git diff origin/main -- ts/packages/paigasus-sdk | grep -nE '^\+.*(console\.(log|debug)|\.only\(|\.skip\(|TODO|FIXME|XXX)' || echo "clean"
```

Expected: `clean`.

- [ ] **Step 5: Commit the plan**

```bash
git add docs/superpowers/plans/2026-09-10-sma-625-error-model-and-chat-client.md
git commit -m "docs(ts): add the SMA-625 implementation plan

Refs SMA-625"
```

---

## Acceptance criteria mapping

| AC | Task | How it is proven |
|---|---|---|
| 1 — branch on `(domain, reason)` only, never message text; unknown reason → generic fallback plus correlation id | 4 | `map-error.test.ts`'s "AC 2 — the branch never reads message text": one wire error with three messages yields three objects identical modulo `message`, plus a non-vacuity assertion that the messages really differed. Degradation covered by the OpenAI-passthrough and unknown-domain cases. |
| 2 — a table test driven off the registry; two mechanisms, both required | 3 | `presentation.ts`'s total `Record` (compile error naming the missing member) **and** `errors-presentation.test.ts` iterating `ErrorReasonSchema.values`. Task 3 Step 5 proves both halves bite. Made reachable by the `proto->sdk` input PR B shipped. |
| 3 — the four codes → four states; raw gRPC statuses never reach the browser | 4 | `map-error.test.ts`'s "AC 4 — the four codes name four states", plus the assertion that `transport` carries a plain `Code` and the serialized object holds no `ConnectError`. `PaigasusError` has no `Headers` field by construction. |
| 4 — chat streaming is a `ReadableStream` passthrough | 5 | `chat.test.ts`'s "returns the IDENTICAL ReadableStream object" — identity, not equivalence. Reinforced by the absence of any `pipeThrough` or reader in `chat.ts`. |

## What this plan deliberately does not do

- **No idle timeout on a chat stream.** Resolved at the spec gate: it cannot coexist with AC 4's passthrough. Stall detection belongs to the caller that owns the reader (spec § 8.3).
- **No live-service tier.** Transport behaviour is tested through a stub `fetch`. This suite proves the SDK forms correct requests and interprets correct responses; it does not prove the gateway accepts them. That is SMA-509/SMA-510's integration surface — and it is why the cancellation row is scoped to forwarding.
- **No client-bundle build fixture for AC 1.** Structural enforcement only, as § 6.2 states. A failing-build fixture needs a console consuming the SDK, which is SMA-510.
- **No new `repo:*` gate.** The drift check rides `paigasus-sdk-ts:test`, so it avoids the five-to-seven registration obligations a new gate carries. § 11's "this package adds no `repo:*` gate" stays true.
- **No retry or backoff policy.** `retryable` is carried, never acted on.
- **No second `Presentation` value for a Paigasus quota.** Nothing emits 429 today (spec M13). When a Paigasus quota exists it will need a registry reason, and § 9.4's override table is the mechanism for its distinct copy.
- **No resolution of SMA-627** (whether an anonymous client may forward a caller-supplied `Authorization` header). `ChatOptions.auth` takes the same `Auth` union the IAM clients take; that issue governs any change to it.
