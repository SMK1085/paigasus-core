<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-625 — `@paigasus/sdk` error mapping and the chat client — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `./errors` and `./chat` entry points to `@paigasus/sdk` — a total error model that maps five wire inputs to one plain `PaigasusError`, and an OpenAI-compatible chat client whose stream is a true passthrough.

**Architecture:** Three small table modules under `src/errors/` feed one `mapError` function. Every guarded entry file sits at `src/` root, because the merged `tests/server-guard.test.ts` compares the guard import as a literal string. `src/chat.ts` uses `fetch`, never a Connect `Transport`, so it shares nothing with the transport cache. The stream is returned as the identical `ReadableStream` object the platform produced.

**Tech Stack:** TypeScript 6.0.3, vitest 5.0.0, Node 24, `@connectrpc/connect` 2.2.0, `@bufbuild/protobuf` 2.14.1, `@paigasus/proto` (workspace).

**Spec:** `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md`, Revision 3, §§ 8, 9, 10, and § 11.2 obligation 9.

**Issue:** SMA-625. **Branch:** `feature/sma-625-ts-sdk-error-mapping-and-chat-client`. **Worktree:** `.claude/worktrees/sma-625`.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Run every command from the worktree, never from the main checkout.
- Prefix each shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. The Bash tool's PATH lacks the proto-managed CLIs.
- Conventional commits with a workspace scope: `feat(ts): …`. Do not put a `#NNN` line or a `token: value` line in the commit BODY — commitlint reads it as a footer and fails.
- Do not bypass the git hooks with `--no-verify`.
- **`message` is never an input to a branch.** No function under `src/errors/` may read `PaigasusError.message` to decide anything. This is SMA-625's AC 1.
- **`PaigasusError` is a plain data object.** Never a class, never an `Error` subclass. `mapError` returns it and never throws it. React's server-to-client serializer rejects class instances.
- **Every guarded entry file lives at `src/` root.** `tests/server-guard.test.ts:19` pins `GUARD_IMPORT = "import './server-guard.js';"` and compares it with `toBe` at `:52`. A guarded entry one directory deep needs `'../server-guard.js'` and fails.
- **A `package.json` `exports` entry and the file it points at land in the SAME commit.** `tests/server-guard.test.ts` iterates that map, so an entry without a file reds the suite.
- The root barrel `src/index.ts` names each re-export **explicitly**. Never `export *` for two modules that could share a name — ES semantics drop an ambiguous star-exported name silently rather than erroring.
- `@paigasus/proto`'s codec returns `undefined` for an unresolved value. `PaigasusError` uses `null`. Convert at the boundary; do not let `undefined` leak into the object.

## File Structure

| File | Responsibility | Entry point |
|---|---|---|
| `src/errors/types.ts` | `Presentation`, `TransportCause`, `TransportInfo`, `PaigasusError`; re-exports `ErrorReason`/`ErrorDomain` as VALUES | `./errors/types` — **unguarded** |
| `src/errors/transport-status.ts` | The three total status tables | internal |
| `src/errors/presentation.ts` | The 57-key reason override `Record` | internal |
| `src/errors/map-error.ts` | `mapError` over its five-arm input union | internal |
| `src/errors.ts` | The `./errors` entry barrel | `./errors` — guarded |
| `src/chat.ts` | The chat client and the terminal-frame parser | `./chat` — guarded |
| `src/index.ts` | Root barrel — MODIFY | `.` — guarded |
| `package.json` | Three new `exports` entries — MODIFY | — |
| `moon.yml` | `chat.rs` added to `test.inputs` — MODIFY | — |
| `ci/affected-graph/run.sh` | The `gateway->sdk` control case — MODIFY | — |

Tests live in `tests/`, one file per source module, matching PR B's layout.

---

### Task 1: The client-safe type surface

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/types.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (add the `./errors/types` export)
- Test: `ts/packages/paigasus-sdk/tests/errors-types.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `Presentation`, `TransportCause`, `TransportInfo`, `PaigasusError` (types); `ErrorReason`, `ErrorDomain` (values, re-exported).

**Why the enums are re-exported as values.** The merged eslint boundary `paigasus/boundaries/apps` bans `@paigasus/proto` and `@paigasus/proto/**` from `apps/**`, and `ts/packages/paigasus-next-config/src/eslint.mjs:28-31` records that type imports are banned alongside value imports. Without this re-export an app receives `reason: 906` and has no legal way to write `ErrorReason.INVALID_REQUEST_SCHEMA` — which is the package's whole purpose (spec § 9.6).

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/errors-types.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';

import { ErrorDomain, ErrorReason } from '../src/errors/types.js';

describe('the client-safe type surface', () => {
  // Spec § 9.6. An app may not import @paigasus/proto, so if these are not VALUES here, no
  // consumer can name a reason at all and the package fails its stated purpose.
  it('re-exports ErrorReason as a runtime value', () => {
    expect(ErrorReason.INVALID_REQUEST_SCHEMA).toBe(906);
    expect(ErrorReason.CAPABILITY_DISABLED).toBe(904);
    expect(ErrorReason.UPSTREAM_ERROR).toBe(307);
  });

  it('re-exports ErrorDomain as a runtime value', () => {
    expect(ErrorDomain.IAM).toBe(1);
    expect(ErrorDomain.GATEWAY).toBe(2);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/errors-types.test.ts
```

Expected: FAIL — cannot resolve `../src/errors/types.js`.

- [ ] **Step 3: Write the implementation**

Create `ts/packages/paigasus-sdk/src/errors/types.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The guard-free, client-safe surface (spec § 6.3, § 9.6). This file carries NO
// `import './server-guard.js'` and must not gain one: `tests/server-guard.test.ts` lists
// './errors/types' in UNGUARDED_ENTRIES and asserts the guard is absent.
//
// `Code` is imported as a TYPE only. Under `verbatimModuleSyntax` (ts/tsconfig.base.json:9) an
// `import type` emits nothing, so naming it here puts no runtime dependency on
// @connectrpc/connect into a client bundle.
import type { Code } from '@connectrpc/connect';
// Imported as TYPES for the field declarations below, and separately RE-EXPORTED as values.
// A re-export does not create a local binding, so both lines are needed and they do not conflict.
import type { ErrorDomain, ErrorReason } from '@paigasus/proto';

// The two registry enums, re-exported as VALUES. An app cannot import @paigasus/proto — the
// eslint boundary `paigasus/boundaries/apps` bans it, type imports included — so without this
// line a consumer receives `reason: 906` and cannot write the name. Two small frozen enum
// objects in the client bundle is the deliberate cost (spec § 9.6).
export { ErrorDomain, ErrorReason } from '@paigasus/proto';

/** What a consumer's error boundary switches on. Never the message, never the raw status. */
export type Presentation = 'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'invalid-input' | 'conflict' | 'disabled' | 'generic';

/** Why a request produced no response at all. */
export type TransportCause = 'timeout' | 'network' | 'aborted';

/**
 * The raw transport outcome, carried for LOGGING only. A consumer branches on `presentation`.
 *
 * `codeName` sits beside `code` so a reader can render the name without a VALUE import of
 * @connectrpc/connect, which is a server-side dependency (spec § 9.1).
 */
export type TransportInfo =
  | { readonly kind: 'grpc'; readonly code: Code; readonly codeName: string }
  | { readonly kind: 'http'; readonly status: number }
  | { readonly kind: 'transport'; readonly cause: TransportCause };

/**
 * One shape for every failure the SDK can report.
 *
 * It is a PLAIN OBJECT, never an `Error` subclass, because React's server-to-client serializer
 * rejects class instances and this object crosses that boundary as a prop (spec § 6.3, § 9.1).
 *
 * `message` is for display and logging and is NEVER an input to a branch (AC 1). It holds no
 * `ConnectError` and no `Headers`, which is what AC 3 requires.
 */
export interface PaigasusError {
  readonly presentation: Presentation;
  /** `null` when the wire's domain did not resolve; `rawDomain` still holds what it said. */
  readonly domain: ErrorDomain | null;
  /** `null` when the wire's reason did not resolve; `rawReason` still holds what it said. */
  readonly reason: ErrorReason | null;
  readonly rawReason: string | null;
  readonly rawDomain: string | null;
  readonly message: string;
  readonly correlationId: string | null;
  readonly requestId: string | null;
  /** Tri-state. `null` is the wire's "unknown"; collapsing it to `false` would assert what the service declined to assert (ADR-0019 decision 7). */
  readonly retryable: boolean | null;
  readonly metadata: Readonly<Record<string, string>>;
  readonly transport: TransportInfo;
}
```

- [ ] **Step 4: Add the export entry**

In `ts/packages/paigasus-sdk/package.json`, change the `exports` map to:

```json
  "exports": {
    ".": "./src/index.ts",
    "./iam": "./src/iam.ts",
    "./errors/types": "./src/errors/types.ts"
  },
```

- [ ] **Step 5: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS. `tests/server-guard.test.ts` must also pass — it now sees the third entry and asserts it carries no guard.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/types.ts ts/packages/paigasus-sdk/tests/errors-types.test.ts ts/packages/paigasus-sdk/package.json
git commit -m "feat(ts): add the client-safe error type surface to @paigasus/sdk"
```

---

### Task 2: The three transport-status tables

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/transport-status.ts`
- Test: `ts/packages/paigasus-sdk/tests/transport-status.test.ts`

**Interfaces:**
- Consumes: `Presentation`, `TransportCause` from Task 1.
- Produces: `presentationForGrpcCode(code: Code): Presentation`, `presentationForHttpStatus(status: number): Presentation`, `presentationForTransportCause(cause: TransportCause): Presentation`.

**The trap this task exists to avoid.** MEASURED on `@connectrpc/connect` 2.2.0 under Node 24.18.1: `Code` is a non-const numeric enum, so `Object.values(Code)` yields **32** entries — 16 numeric (1 to 16) and 16 string names. A totality test written the obvious way iterates the strings too and asserts nothing about them. Filter to numbers.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/transport-status.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { Code } from '@connectrpc/connect';
import { describe, expect, it } from 'vitest';

import { presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from '../src/errors/transport-status.js';
import type { Presentation } from '../src/errors/types.js';

const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'invalid-input', 'conflict', 'disabled', 'generic'];

describe('the gRPC status table', () => {
  // TOTALITY is the assertion that carries this suite. A transcription of the table into the
  // test only proves that someone transcribed it. MEASURED: Object.values(Code) yields 32
  // entries — 16 numeric and 16 string names — so the numeric filter is required, not tidiness.
  it('resolves every numeric Code member', () => {
    const numeric = Object.values(Code).filter((v): v is Code => typeof v === 'number');
    expect(numeric).toHaveLength(16);
    for (const code of numeric) {
      expect(PRESENTATIONS).toContain(presentationForGrpcCode(code));
    }
  });

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
    [Code.Unimplemented, 'disabled'],
    [Code.ResourceExhausted, 'degraded'],
  ] as const)('maps code %i to %s', (code, expected) => {
    expect(presentationForGrpcCode(code)).toBe(expected);
  });

  it('falls through to generic', () => {
    expect(presentationForGrpcCode(Code.Internal)).toBe('generic');
  });
});

describe('the HTTP status table', () => {
  it.each([
    [401, 'relogin'],
    [403, 'forbidden'],
    [404, 'not-found'],
    [408, 'degraded'],
    [429, 'degraded'],
    [502, 'degraded'],
    [503, 'degraded'],
    [504, 'degraded'],
    [400, 'invalid-input'],
    [413, 'invalid-input'],
    [415, 'invalid-input'],
    [422, 'invalid-input'],
    [409, 'conflict'],
    [501, 'disabled'],
  ] as const)('maps %i to %s', (status, expected) => {
    expect(presentationForHttpStatus(status)).toBe(expected);
  });

  // 200 has no row on purpose. The terminal SSE frame arrives with status 200 because the head
  // was already committed, and its `degraded` presentation comes from the reason OVERRIDE table
  // in Task 3, never from here (spec § 9.4).
  it.each([200, 418, 500, 599])('falls through to generic for %i', (status) => {
    expect(presentationForHttpStatus(status)).toBe('generic');
  });
});

describe('the transport-cause table', () => {
  it.each([
    ['timeout', 'degraded'],
    ['network', 'degraded'],
    // A caller abort is not a service fault and must not render as one.
    ['aborted', 'generic'],
  ] as const)('maps %s to %s', (cause, expected) => {
    expect(presentationForTransportCause(cause)).toBe(expected);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/transport-status.test.ts
```

Expected: FAIL — cannot resolve `../src/errors/transport-status.js`.

- [ ] **Step 3: Write the implementation**

Create `ts/packages/paigasus-sdk/src/errors/transport-status.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The two total transport-status tables plus the transport-cause table (spec § 9.2). Internal:
// reached through ./errors, so it carries no guard of its own.
import { Code } from '@connectrpc/connect';

import type { Presentation, TransportCause } from './types.js';

/**
 * gRPC `Code` -> `Presentation`. Total by falling through to `generic`.
 *
 * `ResourceExhausted` sits under `degraded` rather than getting its own state: a gateway proxying
 * OpenAI produces it routinely, and it means "try later", which is what `degraded` renders.
 * `retryable` carries the finer signal. This row was reviewed and kept deliberately (spec § 9.2).
 */
const GRPC_TABLE: ReadonlyMap<Code, Presentation> = new Map([
  [Code.Unauthenticated, 'relogin'],
  [Code.PermissionDenied, 'forbidden'],
  [Code.NotFound, 'not-found'],
  [Code.Unavailable, 'degraded'],
  [Code.DeadlineExceeded, 'degraded'],
  [Code.ResourceExhausted, 'degraded'],
  [Code.InvalidArgument, 'invalid-input'],
  [Code.AlreadyExists, 'conflict'],
  [Code.FailedPrecondition, 'conflict'],
  [Code.Aborted, 'conflict'],
  [Code.Unimplemented, 'disabled'],
]);

/**
 * HTTP status -> `Presentation`. Total by falling through to `generic`.
 *
 * There is deliberately NO 200 row. A terminal SSE error frame carries status 200, and its
 * `degraded` presentation comes from the reason override table, not from here (spec § 9.4).
 */
const HTTP_TABLE: ReadonlyMap<number, Presentation> = new Map([
  [401, 'relogin'],
  [403, 'forbidden'],
  [404, 'not-found'],
  [408, 'degraded'],
  [429, 'degraded'],
  [502, 'degraded'],
  [503, 'degraded'],
  [504, 'degraded'],
  [400, 'invalid-input'],
  [413, 'invalid-input'],
  [415, 'invalid-input'],
  [422, 'invalid-input'],
  [409, 'conflict'],
  [501, 'disabled'],
]);

const CAUSE_TABLE: Readonly<Record<TransportCause, Presentation>> = {
  timeout: 'degraded',
  network: 'degraded',
  // A caller abort is not a service fault. Rendering it as `degraded` would blame the service
  // for the caller's own decision.
  aborted: 'generic',
};

export function presentationForGrpcCode(code: Code): Presentation {
  return GRPC_TABLE.get(code) ?? 'generic';
}

export function presentationForHttpStatus(status: number): Presentation {
  return HTTP_TABLE.get(status) ?? 'generic';
}

export function presentationForTransportCause(cause: TransportCause): Presentation {
  return CAUSE_TABLE[cause];
}

/** The `Code` member name, for logging. `Code[code]` is the reverse map a numeric enum carries. */
export function grpcCodeName(code: Code): string {
  return (Code[code] as string | undefined) ?? String(code);
}
```

- [ ] **Step 4: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS, including the 16-member totality assertion.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/transport-status.ts ts/packages/paigasus-sdk/tests/transport-status.test.ts
git commit -m "feat(ts): add the transport-status tables to @paigasus/sdk"
```

---

### Task 3: The reason override table — SMA-625's AC 2

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/presentation.ts`
- Test: `ts/packages/paigasus-sdk/tests/presentation.test.ts`

**Interfaces:**
- Consumes: `Presentation`, `ErrorReason` from Task 1.
- Produces: `PRESENTATION` (the total `Record`), `presentationOverride(reason: ErrorReason | null): Presentation | null`.

**Two mechanisms, both required (spec § 9.4).** The total `Record` makes a missing reason a **compile** error — MEASURED on tsc 6.0.3, omitting a member yields `TS2741` naming it. The descriptor-driven test makes it a **test** failure. The type alone can be switched off by a refactor to `Partial<…>`, and the test notices. The test alone runs later. Keep both.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/presentation.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { ErrorReason, ErrorReasonSchema, asWireReason, fromWireReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { PRESENTATION, presentationOverride } from '../src/errors/presentation.js';

describe('AC 2 — the override table is driven off the registry', () => {
  // The registry's own values, not a hand-written list. An unmapped code fails HERE rather than
  // rendering an empty toast in a console months later.
  const reasons = ErrorReasonSchema.values.filter((v) => v.number !== 0);

  it('sees the whole registry', () => {
    expect(reasons).toHaveLength(57);
  });

  it.each(reasons.map((v) => [v.name, v.number] as const))('%s has an entry and round-trips', (_name, number) => {
    const reason = number as ErrorReason;
    const wire = asWireReason(reason);
    expect(wire).toBeDefined();
    expect(fromWireReason(wire as string)).toBe(reason);
    expect(PRESENTATION).toHaveProperty(String(number));
  });
});

describe('the three overrides that change or pin an answer', () => {
  // Spec § 9.4. UPSTREAM_ERROR is the one where the default is visibly WRONG: its arm sets
  // status 200, the HTTP table has no 200 row, so without this entry the mid-stream failure the
  // parser exists to catch would render as `generic`. chat.rs:59-62 documents the opposite.
  it('maps UPSTREAM_ERROR to degraded', () => {
    expect(presentationOverride(ErrorReason.UPSTREAM_ERROR)).toBe('degraded');
  });

  it.each([ErrorReason.UPSTREAM_UNAVAILABLE, ErrorReason.UPSTREAM_TIMEOUT])('pins %i to degraded', (reason) => {
    expect(presentationOverride(reason)).toBe('degraded');
  });

  // IAM answers 422 and the gateway answers 400 for this identical wire code
  // (error.proto:239-247). Both already reach `invalid-input` from the status table, so this
  // entry PINS that agreement: neither status can later split the presentation.
  it('pins INVALID_REQUEST_SCHEMA to invalid-input', () => {
    expect(presentationOverride(ErrorReason.INVALID_REQUEST_SCHEMA)).toBe('invalid-input');
  });

  // gRPC Unimplemented with no HTTP form at all (convert.rs:96-104). Unimplemented reads as
  // "this build cannot do that"; the product meaning is "this deployment turned it off".
  it('maps CAPABILITY_DISABLED to disabled', () => {
    expect(presentationOverride(ErrorReason.CAPABILITY_DISABLED)).toBe('disabled');
  });
});

describe('every other reason defers to the transport', () => {
  it('returns null for a from-transport reason', () => {
    expect(presentationOverride(ErrorReason.SLUG_CONFLICT)).toBeNull();
  });

  it('returns null for an unmapped reason', () => {
    expect(presentationOverride(null)).toBeNull();
  });

  // The sentinel is not a real reason and is not in the table. It must not throw.
  it('returns null for the zero sentinel', () => {
    expect(presentationOverride(ErrorReason.UNSPECIFIED)).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/presentation.test.ts
```

Expected: FAIL — cannot resolve `../src/errors/presentation.js`.

- [ ] **Step 3: Generate the 57 table rows**

Do not hand-type 57 keys. Derive them from the generated enum, so the table cannot disagree with the registry:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-625
python3 - <<'PY'
import re
src = open('ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts').read()
block = re.search(r'export enum ErrorReason \{(.*?)\n\}', src, re.S).group(1)
rows = re.findall(r'^  ([A-Z][A-Z0-9_]*) = (\d+),', block, re.M)
rows = [(n, v) for n, v in rows if v != '0']
print(f'// {len(rows)} rows')
for name, _ in rows:
    print(f"  [ErrorReason.{name}]: 'from-transport',")
PY
```

Expected: a header line reading `// 57 rows`, then 57 lines. If the count is not 57, stop — the registry changed and the spec's § 9.4 needs re-reading before you continue.

- [ ] **Step 4: Write the implementation**

Create `ts/packages/paigasus-sdk/src/errors/presentation.ts`. Paste the 57 generated rows into the `PRESENTATION` literal, then edit the four override rows to their stated values:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The reason -> presentation OVERRIDE table (spec § 9.4). Internal: reached through ./errors.
//
// Two mechanisms guard it and BOTH are required. This total `Record` makes a missing reason a
// COMPILE error — MEASURED on tsc 6.0.3, omitting a member yields "TS2741: Property
// '[ErrorReason.INTERNAL]' is missing". `tests/presentation.test.ts` iterates
// ErrorReasonSchema.values and makes it a TEST failure. The type alone can be switched off by a
// refactor to `Partial<Record<…>>`, which the test notices; the test alone runs later.
//
// The `Exclude` is load-bearing: UNSPECIFIED is the zero sentinel, the test skips it, and
// demanding an entry for it would make the table 58 keys rather than 57.
import { ErrorReason } from '@paigasus/proto';

import type { Presentation } from './types.js';

/** `'from-transport'` means "take the transport status table's answer", not "no opinion". */
export type PresentationEntry = Presentation | 'from-transport';

export const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, PresentationEntry> = {
  // ---- paste the 57 generated rows here, then apply the four edits below ----
  //
  // [ErrorReason.UPSTREAM_UNAVAILABLE]: 'degraded',
  // [ErrorReason.UPSTREAM_TIMEOUT]: 'degraded',
  // [ErrorReason.UPSTREAM_ERROR]: 'degraded',
  // [ErrorReason.CAPABILITY_DISABLED]: 'disabled',
  // [ErrorReason.INVALID_REQUEST_SCHEMA]: 'invalid-input',
  //
  // Every OTHER row keeps 'from-transport', so all 57 are a reviewed decision, not a default.
};

/**
 * The presentation this reason overrides the transport status with, or `null` to defer.
 *
 * Returns `null` for an unmapped reason and for the zero sentinel, so a caller can always fall
 * back to the transport table without a special case.
 */
export function presentationOverride(reason: ErrorReason | null): Presentation | null {
  if (reason === null || reason === ErrorReason.UNSPECIFIED) return null;
  const entry = PRESENTATION[reason as Exclude<ErrorReason, ErrorReason.UNSPECIFIED>] as PresentationEntry | undefined;
  if (entry === undefined || entry === 'from-transport') return null;
  return entry;
}
```

- [ ] **Step 5: Prove the compile-time half actually bites**

Delete one row from the literal, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: `TS2741`, naming the member you deleted. Restore the row afterwards. **Record the exact error text in the commit body** — it is the evidence that the compile-time mechanism works, and nothing else in the suite can prove it.

- [ ] **Step 6: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS, with 57 parameterized cases in the registry suite.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/presentation.ts ts/packages/paigasus-sdk/tests/presentation.test.ts
git commit -m "feat(ts): add the reason presentation override table to @paigasus/sdk"
```

---

### Task 4: `mapError` and the `./errors` entry

**Files:**
- Create: `ts/packages/paigasus-sdk/src/errors/map-error.ts`
- Create: `ts/packages/paigasus-sdk/src/errors.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (add the `./errors` export)
- Test: `ts/packages/paigasus-sdk/tests/map-error.test.ts`

**Interfaces:**
- Consumes: Tasks 1, 2, 3.
- Produces: `ErrorInput`, `mapError(input: ErrorInput): PaigasusError`.

**Arms 2 and 3 are ONE code path.** The spec describes them separately because their provenance differs — IAM's `{error:{code,message}}` and the gateway's `{error:{message,type,param,code}}`. As data they are the same envelope plus an optional `param`, so one reader handles both. The tests keep them apart, as § 10 requires.

**Three facts that cost a measurement each.**
- The correlation id has **two spellings**. The `ErrorInfo` metadata key is `correlation_id` (`convert.rs:70`); the response header is `paigasus-correlation-id` (`correlation.rs:31`). Read the metadata key first, then the header.
- The gateway forwards OpenAI's own envelope **verbatim** (`chat.rs:113-119`), so `code` and `param` are **nullable** and `code` may be an underscore token outside the registry. `fromWireReason` rejects it, which is the correct degradation.
- IAM's HTTP body carries **no** `field` key. `field` reaches `ErrorInfo.metadata` on the gRPC path only (`convert.rs:129-130`).

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/map-error.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { toBinary } from '@bufbuild/protobuf';
import { Code, ConnectError } from '@connectrpc/connect';
import { ErrorDomain, ErrorInfoSchema, ErrorReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { mapError } from '../src/errors/map-error.js';

/**
 * Build a ConnectError carrying an ErrorInfo detail in the INCOMING WIRE SHAPE.
 *
 * This matters. A detail supplied as `{ desc, value }` takes findDetails's OUTGOING branch — a
 * plain create() — and proves nothing about decoding a `grpc-status-details-bin` trailer of the
 * kind tonic_types::ErrorDetails::with_error_info produces (convert.rs:79). The `{ type, value }`
 * form takes the fromBinary branch, which is the path under test (spec § 10).
 */
function connectErrorWithDetail(message: string, code: Code, metadata: Record<string, string>, reason = 'slug-conflict', domain = 'iam'): ConnectError {
  const bytes = toBinary(ErrorInfoSchema, { $typeName: 'google.rpc.ErrorInfo', reason, domain, metadata });
  return new ConnectError(message, code, undefined, [{ type: 'google.rpc.ErrorInfo', value: bytes }]);
}

describe('arm 1 — a ConnectError carrying an ErrorInfo detail', () => {
  it('round-trips the pair, lifts the three keys, and keeps the rest', () => {
    const err = connectErrorWithDetail('conflict', Code.AlreadyExists, {
      retryable: 'false',
      correlation_id: 'corr-1',
      request_id: 'req-1',
      capability: 'chat_stream',
      field: 'slug',
    });

    const result = mapError({ kind: 'grpc', error: err });

    expect(result.reason).toBe(ErrorReason.SLUG_CONFLICT);
    expect(result.domain).toBe(ErrorDomain.IAM);
    expect(result.rawReason).toBe('slug-conflict');
    expect(result.rawDomain).toBe('iam');
    expect(result.correlationId).toBe('corr-1');
    expect(result.requestId).toBe('req-1');
    expect(result.retryable).toBe(false);
    expect(result.presentation).toBe('conflict');
    // The three lifted keys are REMOVED, so a caller cannot branch on a raw duplicate that
    // disagrees with the parsed field.
    expect(result.metadata).toEqual({ capability: 'chat_stream', field: 'slug' });
    expect(result.transport).toEqual({ kind: 'grpc', code: Code.AlreadyExists, codeName: 'AlreadyExists' });
  });

  it('treats an unknown retryable as null rather than false', () => {
    const err = connectErrorWithDetail('x', Code.Internal, { retryable: 'unknown' });
    expect(mapError({ kind: 'grpc', error: err }).retryable).toBeNull();
  });

  it('keeps rawReason and correlationId when the reason is unknown', () => {
    const err = connectErrorWithDetail('x', Code.NotFound, { correlation_id: 'corr-2' }, 'no-such-code', 'iam');
    const result = mapError({ kind: 'grpc', error: err });
    expect(result.reason).toBeNull();
    expect(result.rawReason).toBe('no-such-code');
    expect(result.correlationId).toBe('corr-2');
    expect(result.presentation).toBe('not-found');
  });

  it('keeps rawDomain when the domain is unknown', () => {
    const err = connectErrorWithDetail('x', Code.Internal, {}, 'slug-conflict', 'billing');
    const result = mapError({ kind: 'grpc', error: err });
    expect(result.domain).toBeNull();
    expect(result.rawDomain).toBe('billing');
  });
});

describe('arm 1b — a ConnectError with NO ErrorInfo detail', () => {
  // Reachable, not hypothetical: a network failure, a proxy or a connection reset raises a
  // ConnectError carrying a status and no detail. Without this branch the SDK THROWS while
  // mapping an error, turning a recoverable upstream failure into an unhandled BFF exception.
  it('falls back to the status table and throws nothing', () => {
    const result = mapError({ kind: 'grpc', error: new ConnectError('connection reset', Code.Unavailable) });
    expect(result.reason).toBeNull();
    expect(result.domain).toBeNull();
    expect(result.rawReason).toBeNull();
    expect(result.rawDomain).toBeNull();
    expect(result.retryable).toBeNull();
    expect(result.metadata).toEqual({});
    expect(result.presentation).toBe('degraded');
    expect(result.message).toContain('connection reset');
  });

  it('still reads a correlation id from the header union', () => {
    const headers = new Headers({ 'paigasus-correlation-id': 'corr-3' });
    const result = mapError({ kind: 'grpc', error: new ConnectError('x', Code.Internal, headers) });
    expect(result.correlationId).toBe('corr-3');
  });
});

describe("arm 2 — IAM's HTTP envelope", () => {
  it('reads the correlation id and retryable from the HEADERS, not the body', () => {
    const result = mapError({
      kind: 'http',
      status: 422,
      headers: new Headers({ 'paigasus-correlation-id': 'corr-4', 'paigasus-request-id': 'req-4', 'paigasus-retryable': 'true' }),
      body: { error: { code: 'invalid-request-schema', message: 'bad shape' } },
    });

    expect(result.correlationId).toBe('corr-4');
    expect(result.requestId).toBe('req-4');
    expect(result.retryable).toBe(true);
    expect(result.reason).toBe(ErrorReason.INVALID_REQUEST_SCHEMA);
    expect(result.presentation).toBe('invalid-input');
    // IAM's HTTP body has no metadata map at all — `field` is gRPC-only (convert.rs:129-130).
    expect(result.metadata).toEqual({});
    expect(result.transport).toEqual({ kind: 'http', status: 422 });
  });
});

describe("arm 3 — the gateway's OpenAI envelope", () => {
  it('carries param into metadata and pins the 400/422 divergence', () => {
    const result = mapError({
      kind: 'http',
      status: 400,
      headers: new Headers({ 'paigasus-retryable': 'false' }),
      body: { error: { message: 'bad shape', type: 'invalid_request_error', param: 'model', code: 'invalid-request-schema' } },
    });

    expect(result.reason).toBe(ErrorReason.INVALID_REQUEST_SCHEMA);
    // The SAME code as arm 2's 422 resolves to the SAME presentation. That agreement is what the
    // override entry pins (error.proto:239-247).
    expect(result.presentation).toBe('invalid-input');
    expect(result.metadata).toEqual({ param: 'model' });
  });

  // The gateway forwards a non-2xx upstream response VERBATIM (chat.rs:113-119), so this body is
  // OpenAI's own vocabulary, not the registry's.
  it('degrades an upstream passthrough with an underscore code', () => {
    const result = mapError({
      kind: 'http',
      status: 429,
      headers: new Headers(),
      body: { error: { message: 'You exceeded your current quota', type: 'insufficient_quota', param: null, code: 'insufficient_quota' } },
    });

    expect(result.reason).toBeNull();
    expect(result.rawReason).toBe('insufficient_quota');
    expect(result.presentation).toBe('degraded');
    expect(result.retryable).toBeNull();
    // A null param must not enter metadata as the string "null".
    expect(result.metadata).toEqual({});
  });

  it('handles a null code', () => {
    const result = mapError({ kind: 'http', status: 500, headers: new Headers(), body: { error: { message: 'boom', code: null, param: null } } });
    expect(result.reason).toBeNull();
    expect(result.rawReason).toBeNull();
    expect(result.presentation).toBe('generic');
  });
});

describe('mapError is total over a malformed body', () => {
  // mapError runs in the BFF on the FAILURE path. A throw here turns a recoverable upstream
  // failure into an unhandled exception — the same failure mode arm 1b exists to prevent.
  it.each([
    ['an HTML body', '<html><body>502 Bad Gateway</body></html>'],
    ['an empty string', ''],
    ['undefined', undefined],
    ['a JSON body with no error key', { detail: 'nope' }],
    ['a JSON null', null],
  ])('does not throw on %s', (_label, body) => {
    const result = mapError({ kind: 'http', status: 502, headers: new Headers(), body });
    expect(result.reason).toBeNull();
    expect(result.rawReason).toBeNull();
    expect(result.presentation).toBe('degraded');
    expect(result.message).not.toBe('');
  });
});

describe('arm 5 — a transport failure with no response at all', () => {
  it.each([
    ['timeout', 'degraded'],
    ['network', 'degraded'],
    ['aborted', 'generic'],
  ] as const)('maps %s to %s', (cause, expected) => {
    const result = mapError({ kind: 'transport', cause, message: 'no response' });
    expect(result.presentation).toBe(expected);
    expect(result.transport).toEqual({ kind: 'transport', cause });
    expect(result.reason).toBeNull();
    expect(result.retryable).toBeNull();
  });
});

describe('AC 1 — the message is never an input to a branch', () => {
  it('maps one wire error with three messages to three identical objects modulo message', () => {
    const results = ['first', 'second', 'third'].map((m) => mapError({ kind: 'grpc', error: connectErrorWithDetail(m, Code.NotFound, { retryable: 'false' }) }));

    const stripped = results.map(({ message, ...rest }) => rest);
    expect(stripped[0]).toEqual(stripped[1]);
    expect(stripped[1]).toEqual(stripped[2]);
    expect(results.map((r) => r.message)).toEqual(['first', 'second', 'third']);
  });
});

describe('AC 3 — nothing raw reaches the browser', () => {
  it('returns a PLAIN object, never an Error subclass', () => {
    const result = mapError({ kind: 'grpc', error: new ConnectError('x', Code.Internal) });
    expect(Object.getPrototypeOf(result)).toBe(Object.prototype);
    expect(result).not.toBeInstanceOf(Error);
  });

  it('carries no ConnectError and no Headers', () => {
    const headers = new Headers({ 'paigasus-correlation-id': 'corr-5' });
    const result = mapError({ kind: 'grpc', error: new ConnectError('x', Code.Internal, headers) });
    for (const value of Object.values(result)) {
      expect(value).not.toBeInstanceOf(Headers);
      expect(value).not.toBeInstanceOf(ConnectError);
    }
    // The whole object must survive the RSC boundary, which is what AC 3 turns on.
    expect(() => structuredClone(result)).not.toThrow();
  });

  it.each([
    [Code.Unauthenticated, 'relogin'],
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unavailable, 'degraded'],
  ] as const)('maps %i to %s', (code, expected) => {
    expect(mapError({ kind: 'grpc', error: new ConnectError('x', code) }).presentation).toBe(expected);
  });

  it.each([
    [401, 'relogin'],
    [403, 'forbidden'],
    [404, 'not-found'],
    [503, 'degraded'],
  ] as const)('maps HTTP %i to %s', (status, expected) => {
    expect(mapError({ kind: 'http', status, headers: new Headers(), body: {} }).presentation).toBe(expected);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/map-error.test.ts
```

Expected: FAIL — cannot resolve `../src/errors/map-error.js`.

- [ ] **Step 3: Write the implementation**

Create `ts/packages/paigasus-sdk/src/errors/map-error.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The five-arm normalizer (spec § 9.5). Internal: reached through ./errors.
//
// It branches on (domain, reason) and on the transport status. It NEVER reads `message` to decide
// anything — that is AC 1, and it is satisfied structurally by there being no such read here.
import { ConnectError } from '@connectrpc/connect';
// `type ErrorReason` inline rather than a second import statement: verbatimModuleSyntax is on, so
// a type in a value import must be marked, and one statement per module keeps the lint quiet.
import { type ErrorReason, ErrorInfoSchema, fromWireDomain, fromWireReason } from '@paigasus/proto';

import { presentationOverride } from './presentation.js';
import { grpcCodeName, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from './transport-status.js';
import type { PaigasusError, Presentation, TransportCause, TransportInfo } from './types.js';

/** The three keys lifted out of ErrorInfo.metadata into their own typed fields. */
const LIFTED_KEYS = ['retryable', 'correlation_id', 'request_id'] as const;

const CORRELATION_HEADER = 'paigasus-correlation-id';
const REQUEST_ID_HEADER = 'paigasus-request-id';
const RETRYABLE_HEADER = 'paigasus-retryable';

export type ErrorInput =
  | { readonly kind: 'grpc'; readonly error: ConnectError }
  /**
   * Both HTTP envelopes. IAM sends `{error:{code,message}}` and the gateway sends
   * `{error:{message,type,param,code}}`; as DATA they are one envelope plus an optional `param`,
   * so one reader handles both. `body` is `unknown` because it may be an unparsed non-JSON body.
   */
  | { readonly kind: 'http'; readonly status: number; readonly headers: Headers; readonly body: unknown }
  | { readonly kind: 'terminal-frame'; readonly body: unknown }
  | { readonly kind: 'transport'; readonly cause: TransportCause; readonly message: string };

/** The wire's tri-state. `unknown`, an absent value, and anything unrecognized all mean `null`. */
function parseRetryable(value: string | null | undefined): boolean | null {
  if (value === 'true') return true;
  if (value === 'false') return false;
  return null;
}

/** `undefined` is what the codec returns; `null` is what PaigasusError carries. */
function orNull<T>(value: T | undefined): T | null {
  return value ?? null;
}

/** Read `error.code` / `error.message` / `error.param` out of a body that may be anything at all. */
function readEnvelope(body: unknown): { code: string | null; message: string | null; param: string | null } {
  if (typeof body !== 'object' || body === null) return { code: null, message: null, param: null };
  const error = (body as { error?: unknown }).error;
  if (typeof error !== 'object' || error === null) return { code: null, message: null, param: null };
  const e = error as { code?: unknown; message?: unknown; param?: unknown };
  return {
    code: typeof e.code === 'string' ? e.code : null,
    message: typeof e.message === 'string' ? e.message : null,
    // A null param must not become the string "null" in metadata.
    param: typeof e.param === 'string' ? e.param : null,
  };
}

/** The presentation an override selects, else the transport table's answer. */
function resolve(reason: ErrorReason | undefined, fallback: Presentation): Presentation {
  return presentationOverride(orNull(reason)) ?? fallback;
}

export function mapError(input: ErrorInput): PaigasusError {
  switch (input.kind) {
    case 'grpc':
      return mapConnect(input.error);
    case 'http':
      return mapHttp(input.status, input.headers, input.body);
    case 'terminal-frame':
      // The head was already committed, so the status cannot change: it is 200, and the HTTP
      // table has no 200 row. `upstream-error`'s OVERRIDE entry is what makes this `degraded`
      // rather than `generic` (spec § 9.4).
      return mapHttp(200, new Headers(), input.body);
    case 'transport':
      return {
        presentation: presentationForTransportCause(input.cause),
        domain: null,
        reason: null,
        rawReason: null,
        rawDomain: null,
        message: input.message,
        correlationId: null,
        requestId: null,
        retryable: null,
        metadata: {},
        transport: { kind: 'transport', cause: input.cause },
      };
  }
}

function mapConnect(err: ConnectError): PaigasusError {
  const transport: TransportInfo = { kind: 'grpc', code: err.code, codeName: grpcCodeName(err.code) };
  const fallback = presentationForGrpcCode(err.code);
  // findDetails returns an EMPTY ARRAY with no signal when the error carries no detail — a
  // network failure, a proxy, or a connection reset. Branch BEFORE reading [0], or the SDK
  // throws while mapping an error (spec § 9.5 arm 1).
  const detail = err.findDetails(ErrorInfoSchema)[0];

  if (detail === undefined) {
    return {
      presentation: fallback,
      domain: null,
      reason: null,
      rawReason: null,
      rawDomain: null,
      message: err.message,
      // err.metadata is a union of response headers and trailers, so it may still carry the id.
      correlationId: err.metadata.get(CORRELATION_HEADER),
      requestId: err.metadata.get(REQUEST_ID_HEADER),
      retryable: null,
      metadata: {},
      transport,
    };
  }

  const metadata: Record<string, string> = { ...detail.metadata };
  for (const key of LIFTED_KEYS) delete metadata[key];

  const reason = fromWireReason(detail.reason);
  return {
    presentation: resolve(reason, fallback),
    domain: orNull(fromWireDomain(detail.domain)),
    reason: orNull(reason),
    // Always what the wire SAID, whether or not it resolved.
    rawReason: detail.reason === '' ? null : detail.reason,
    rawDomain: detail.domain === '' ? null : detail.domain,
    message: err.message,
    // The metadata KEY is `correlation_id` (convert.rs:70); the HEADER is
    // `paigasus-correlation-id` (correlation.rs:31). Two spellings, both read, metadata first.
    correlationId: detail.metadata.correlation_id ?? err.metadata.get(CORRELATION_HEADER),
    requestId: detail.metadata.request_id ?? err.metadata.get(REQUEST_ID_HEADER),
    retryable: parseRetryable(detail.metadata.retryable),
    metadata,
    transport,
  };
}

function mapHttp(status: number, headers: Headers, body: unknown): PaigasusError {
  const { code, message, param } = readEnvelope(body);
  const reason = code === null ? undefined : fromWireReason(code);
  const fallback = presentationForHttpStatus(status);

  return {
    presentation: resolve(reason, fallback),
    // Neither HTTP envelope carries a domain. Both are single-service bodies, so there is nothing
    // to read and nothing to preserve.
    domain: null,
    reason: orNull(reason),
    rawReason: code,
    rawDomain: null,
    // Never empty: a malformed or non-JSON body still yields something a user can report.
    message: message ?? `HTTP ${status}`,
    correlationId: headers.get(CORRELATION_HEADER),
    requestId: headers.get(REQUEST_ID_HEADER),
    retryable: parseRetryable(headers.get(RETRYABLE_HEADER)),
    metadata: param === null ? {} : { param },
    transport: { kind: 'http', status },
  };
}
```

- [ ] **Step 4: Create the guarded entry barrel**

Create `ts/packages/paigasus-sdk/src/errors.ts`. It sits at `src/` ROOT, not under `src/errors/`, because `tests/server-guard.test.ts` compares the guard import as a literal string:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The `./errors` entry. It is GUARDED and lives at src/ root: tests/server-guard.test.ts:19 pins
// the literal "import './server-guard.js';" and compares it with toBe at :52, so an entry one
// directory deep would need '../server-guard.js' and fail.
//
// mapError runs in the BFF. A client component receives an already-mapped PaigasusError as a
// serializable prop and imports its TYPES from './errors/types' instead (spec § 6.3).
import './server-guard.js';

export { mapError } from './errors/map-error.js';
export type { ErrorInput } from './errors/map-error.js';
export { PRESENTATION, presentationOverride } from './errors/presentation.js';
export type { PresentationEntry } from './errors/presentation.js';
export { grpcCodeName, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from './errors/transport-status.js';
export { ErrorDomain, ErrorReason } from './errors/types.js';
export type { PaigasusError, Presentation, TransportCause, TransportInfo } from './errors/types.js';
```

- [ ] **Step 5: Add the export entry**

In `ts/packages/paigasus-sdk/package.json`, the `exports` map becomes:

```json
  "exports": {
    ".": "./src/index.ts",
    "./iam": "./src/iam.ts",
    "./errors": "./src/errors.ts",
    "./errors/types": "./src/errors/types.ts"
  },
```

- [ ] **Step 6: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS. `tests/server-guard.test.ts` now checks three guarded entries and one unguarded one.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/src/errors/map-error.ts ts/packages/paigasus-sdk/src/errors.ts ts/packages/paigasus-sdk/tests/map-error.test.ts ts/packages/paigasus-sdk/package.json
git commit -m "feat(ts): add mapError and the ./errors entry to @paigasus/sdk"
```

---

### Task 5: The terminal-frame parser and its drift check

**Files:**
- Create: `ts/packages/paigasus-sdk/src/chat.ts` (the parser half; Task 6 adds the client)
- Modify: `ts/packages/paigasus-sdk/package.json` (add the `./chat` export)
- Modify: `ts/packages/paigasus-sdk/moon.yml` (add `chat.rs` to `test.inputs`)
- Test: `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts`

**Interfaces:**
- Consumes: `mapError` from Task 4.
- Produces: `createTerminalFrameParser(): { push(chunk: Uint8Array): PaigasusError[] }`.

**Why the parser is stateful and takes bytes.** A `ReadableStream` chunk is not guaranteed to hold one complete SSE record. A record ends at a blank line and a chunk boundary can fall anywhere, so a parser over one bare chunk is lossy **exactly on the split-frame case** — the terminal error split across two chunks, which is the case it exists to catch. It takes `Uint8Array` because a per-chunk `TextDecoder().decode()` splits a multi-byte character on the same boundary; the parser owns a decoder and calls `decode(chunk, { stream: true })`.

**Why `push` returns an array.** Two frames can arrive in one chunk. A `PaigasusError | null` return cannot express that, and § 10 tests it.

**The drift check, and why it needs a Moon input.** Without `chat.rs` among `paigasus-sdk-ts:test`'s inputs, editing `TERMINAL_SSE_ERROR` selects no SDK task and Moon serves a cached PASS — on the one PR the check exists to catch. That is the same vacuity M11 measured for AC 2. **Key on the constant NAME, never on line 63:** any edit above it moves the constant.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ErrorReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { createTerminalFrameParser } from '../src/chat.js';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const CHAT_RS = resolve(REPO_ROOT, 'rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs');

/**
 * Read TERMINAL_SSE_ERROR out of the gateway's Rust source, BY CONSTANT NAME.
 *
 * Keyed on the name and never on a line number: any edit above the constant moves it, and a
 * line-keyed fixture would then compare the wrong line and could pass while the frame changed.
 *
 * `chat.rs` is declared in paigasus-sdk-ts:test's `inputs` (moon.yml) so that editing the constant
 * SELECTS this suite. Without that declaration Moon serves a cached PASS on exactly the PR that
 * changes the frame (spec § 8.4, § 11.2 obligation 9).
 */
function terminalFrameFromRust(): string {
  const src = readFileSync(CHAT_RS, 'utf8');
  const match = /const TERMINAL_SSE_ERROR: &str = "((?:[^"\\]|\\.)*)";/.exec(src);
  if (match === null) throw new Error(`TERMINAL_SSE_ERROR not found in ${CHAT_RS} — the gateway renamed or removed it`);
  // Decode the Rust string literal's escapes. Backslash LAST would double-process the others.
  return match[1].replace(/\\(.)/g, (_all, ch: string) => (ch === 'n' ? '\n' : ch === 't' ? '\t' : ch === 'r' ? '\r' : ch));
}

const FRAME = terminalFrameFromRust();
const encode = (s: string): Uint8Array => new TextEncoder().encode(s);

describe('the terminal frame is read from the gateway, not hand-built', () => {
  // Without this the test would assert against an object it wrote itself, and could not fail
  // when the gateway changes the frame — which is the one drift it exists to absorb.
  it('is a well-formed SSE data record carrying the registry code', () => {
    expect(FRAME.startsWith('data: ')).toBe(true);
    expect(FRAME.endsWith('\n\n')).toBe(true);
    expect(FRAME).toContain('"code":"upstream-error"');
  });
});

describe('createTerminalFrameParser', () => {
  it('parses one whole frame in one chunk', () => {
    const parser = createTerminalFrameParser();
    const out = parser.push(encode(FRAME));
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe(ErrorReason.UPSTREAM_ERROR);
    expect(out[0].rawReason).toBe('upstream-error');
    // Status 200: the head was already committed. The `degraded` presentation comes from the
    // OVERRIDE table, because the HTTP table has no 200 row (spec § 9.4).
    expect(out[0].presentation).toBe('degraded');
    expect(out[0].transport).toEqual({ kind: 'http', status: 200 });
    // chat.rs:56-62 — the frame deliberately carries no retryable signal.
    expect(out[0].retryable).toBeNull();
  });

  // THE case the parser exists for. A chunk boundary can fall anywhere, and a parser over one
  // bare chunk misses the terminal error precisely here.
  it('parses a frame split across two chunks', () => {
    const parser = createTerminalFrameParser();
    const split = Math.floor(FRAME.length / 2);
    expect(parser.push(encode(FRAME.slice(0, split)))).toEqual([]);
    const out = parser.push(encode(FRAME.slice(split)));
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe(ErrorReason.UPSTREAM_ERROR);
  });

  it('returns both frames when two arrive in one chunk', () => {
    const parser = createTerminalFrameParser();
    const out = parser.push(encode(FRAME + FRAME));
    expect(out).toHaveLength(2);
  });

  it('holds a partial trailing record and returns nothing', () => {
    const parser = createTerminalFrameParser();
    expect(parser.push(encode('data: {"error":{"code":"upstream-error"'))).toEqual([]);
  });

  it('ignores ordinary data records', () => {
    const parser = createTerminalFrameParser();
    expect(parser.push(encode('data: {"choices":[{"delta":{"content":"hi"}}]}\n\n'))).toEqual([]);
    expect(parser.push(encode('data: [DONE]\n\n'))).toEqual([]);
  });

  // A per-chunk TextDecoder().decode() splits a multi-byte character on the same boundary the
  // record parser has to survive, so the parser owns a streaming decoder.
  it('survives a multi-byte character split across a chunk boundary', () => {
    const parser = createTerminalFrameParser();
    const bytes = encode('data: {"choices":[{"delta":{"content":"é"}}]}\n\n' + FRAME);
    const cut = 40;
    expect(parser.push(bytes.slice(0, cut))).toEqual([]);
    const out = parser.push(bytes.slice(cut));
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe(ErrorReason.UPSTREAM_ERROR);
  });

  it('accepts CRLF record delimiters', () => {
    const parser = createTerminalFrameParser();
    const crlf = FRAME.replace(/\n\n$/, '\r\n\r\n');
    expect(parser.push(encode(crlf))).toHaveLength(1);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/terminal-frame.test.ts
```

Expected: FAIL — cannot resolve `../src/chat.js`.

- [ ] **Step 3: Write the implementation**

Create `ts/packages/paigasus-sdk/src/chat.ts` with the parser half only. Task 6 appends the client to this same file:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The `./chat` entry (spec § 8). GUARDED and at src/ root, like every other guarded entry.
import './server-guard.js';

import { mapError } from './errors/map-error.js';
import type { PaigasusError } from './errors/types.js';

/** The registry code the gateway puts in its one terminal SSE frame (chat.rs:63). */
const TERMINAL_CODE = 'upstream-error';

/**
 * A stateful, incremental SSE parser that reports terminal error frames.
 *
 * The SDK NEVER drives this itself, because scanning the stream means buffering it, and buffering
 * would undo the unbuffered forwarding the gateway goes to trouble to preserve (chat.rs:194-220).
 * It is EXPORTED so a caller consuming its own stream can drive it (spec § 8.4).
 *
 * It holds only the trailing partial record and the trailing partial character, so it does not
 * reintroduce the buffering the passthrough avoids.
 */
export function createTerminalFrameParser(): { push(chunk: Uint8Array): PaigasusError[] } {
  // A streaming decoder, not a per-chunk one: a multi-byte character can straddle a chunk
  // boundary exactly as a record can.
  const decoder = new TextDecoder('utf-8');
  let buffer = '';

  return {
    push(chunk: Uint8Array): PaigasusError[] {
      buffer += decoder.decode(chunk, { stream: true });

      const found: PaigasusError[] = [];
      // An SSE record ends at a blank line. The gateway emits "\n\n" (chat.rs:63); "\r\n\r\n" is
      // legal SSE and is not produced here, but accepting it costs one alternation and rejecting
      // it would be a silent miss.
      const delimiter = /\r?\n\r?\n/;
      for (;;) {
        const match = delimiter.exec(buffer);
        if (match === null) break;
        const record = buffer.slice(0, match.index);
        buffer = buffer.slice(match.index + match[0].length);

        const error = terminalErrorFrom(record);
        if (error !== null) found.push(error);
      }
      return found;
    },
  };
}

/** A `PaigasusError` when this record is the terminal error frame, else `null`. */
function terminalErrorFrom(record: string): PaigasusError | null {
  const data = record
    .split(/\r?\n/)
    .filter((line) => line.startsWith('data:'))
    .map((line) => line.slice('data:'.length).trim())
    .join('');
  if (data === '' || data === '[DONE]') return null;

  let body: unknown;
  try {
    body = JSON.parse(data);
  } catch {
    // A stream carries ordinary content chunks too, and a caller may feed us anything. An
    // unparseable record is not an error to report; it is simply not the terminal frame.
    return null;
  }

  const code = (body as { error?: { code?: unknown } } | null)?.error?.code;
  if (code !== TERMINAL_CODE) return null;
  return mapError({ kind: 'terminal-frame', body });
}
```

- [ ] **Step 4: Add the export entry and the Moon input**

In `ts/packages/paigasus-sdk/package.json`:

```json
  "exports": {
    ".": "./src/index.ts",
    "./chat": "./src/chat.ts",
    "./iam": "./src/iam.ts",
    "./errors": "./src/errors.ts",
    "./errors/types": "./src/errors/types.ts"
  },
```

In `ts/packages/paigasus-sdk/moon.yml`, the `test` task's `inputs` becomes:

```yaml
  test:
    # vitest.config.ts matches neither src/**/* nor tests/**/*, so without this line an edit to the
    # resolution conditions serves a cached PASS — and those conditions are the only thing keeping
    # `server-only`'s unconditional throw from failing the whole suite.
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - 'vitest.config.ts'
      # SMA-625, spec § 8.4. tests/terminal-frame.test.ts reads TERMINAL_SSE_ERROR out of this
      # Rust file by constant name. Without this input, editing that constant selects NO
      # paigasus-sdk-ts task and Moon serves a cached PASS on exactly the PR that changes the
      # frame — the same vacuity M11 measured for the error registry. The control on this line is
      # `run_task_case_ci "gateway->sdk"` in ci/affected-graph/run.sh.
      - '/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs'
```

- [ ] **Step 5: Prove the drift check actually bites**

Edit the constant in `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` — change `upstream-error` to `upstream-broken` — then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/terminal-frame.test.ts
```

Expected: FAIL. Then `git checkout -- rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` and re-run to confirm PASS. **Record both results in the commit body.** A drift check that cannot fail is the failure mode this task exists to avoid.

- [ ] **Step 6: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/src/chat.ts ts/packages/paigasus-sdk/tests/terminal-frame.test.ts ts/packages/paigasus-sdk/package.json ts/packages/paigasus-sdk/moon.yml
git commit -m "feat(ts): add the terminal SSE frame parser to @paigasus/sdk"
```

---

### Task 6: The chat client

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/chat.ts` (append the client)
- Test: `ts/packages/paigasus-sdk/tests/chat.test.ts`

**Interfaces:**
- Consumes: `mapError` from Task 4, the parser from Task 5.
- Produces: `ChatClientOptions`, `ChatResult`, `ChatClient`, `createChatClient(options: ChatClientOptions, auth: { bearer: string }): ChatClient`.

**Four decisions, each stated because an implementer would otherwise guess (spec § 8.1, § 8.5).**
- **`auth` is required and is NOT the `Auth` union.** No path through `chat_completions` accepts an anonymous caller — `chat.rs:5-6` records that `require_iam_auth` ran, and `MissingBearer` answers 401 (`error.rs:122-128`). An anonymous arm would be a legal value with no legal use.
- **No `Transport`, no cache.** The client calls `fetch`. § 7.1's cache-key rule does not reach it.
- **`fetch` is an optional field, read PER CALL.** A module-load capture makes `vi.stubGlobal` useless, and three test rows need a stub.
- **The pre-header deadline must NOT be `AbortSignal.timeout`.** That signal keeps running after the headers arrive and aborts the streaming body at 10 s — truncating every completion longer than ten seconds, which is the normal case for the product. Use a manual `AbortController` and `clearTimeout` the moment the `fetch` promise settles.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/chat.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, vi } from 'vitest';

import { createChatClient } from '../src/chat.js';

const BASE = 'https://gateway.example.test';

/**
 * A stub `fetch` that models undici's abort behaviour faithfully.
 *
 * The body stream ERRORS when the signal aborts. That is what makes the "deadline does not
 * outlive the head" test discriminating: a build using AbortSignal.timeout keeps its signal live
 * after the headers arrive, so this stub's stream errors and the test fails. A stub that ignored
 * the signal would pass under both the correct and the broken build.
 */
function streamingFetch(chunks: readonly { afterMs: number; text: string }[]): typeof globalThis.fetch {
  return vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
    const signal = init?.signal ?? undefined;
    const timers: NodeJS.Timeout[] = [];
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        for (const { afterMs, text } of chunks) {
          timers.push(setTimeout(() => controller.enqueue(new TextEncoder().encode(text)), afterMs));
        }
        timers.push(setTimeout(() => controller.close(), Math.max(...chunks.map((c) => c.afterMs)) + 10));
        signal?.addEventListener('abort', () => {
          for (const t of timers) clearTimeout(t);
          controller.error(signal.reason);
        });
      },
    });
    return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
  }) as unknown as typeof globalThis.fetch;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('the chat client sends a credential', () => {
  it('attaches the bearer and posts to /v1/chat/completions', async () => {
    const fetchImpl = vi.fn(async () => new Response(JSON.stringify({ id: 'x' }), { status: 200, headers: { 'content-type': 'application/json' } }));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'TOKEN' });

    await client.completions({ model: 'gpt-4o', messages: [] });

    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(`${BASE}/v1/chat/completions`);
    expect(init.method).toBe('POST');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer TOKEN');
    expect(new Headers(init.headers).get('content-type')).toBe('application/json');
  });

  it('refuses an empty bearer locally rather than sending "Bearer "', () => {
    expect(() => createChatClient({ baseUrl: BASE }, { bearer: '   ' })).toThrow(/empty or whitespace-only bearer/);
  });
});

describe('the two response shapes', () => {
  it('returns kind json for a 2xx JSON response', async () => {
    const fetchImpl = vi.fn(async () => new Response(JSON.stringify({ id: 'chat-1' }), { status: 200, headers: { 'content-type': 'application/json' } }));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result).toEqual({ kind: 'json', status: 200, body: { id: 'chat-1' } });
  });

  // AC 4. The SDK does not read, buffer, decode or re-encode the stream.
  it('returns the IDENTICAL ReadableStream object for a 2xx SSE response', async () => {
    const body = new ReadableStream<Uint8Array>({ start: (c) => c.close() });
    const response = new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
    const fetchImpl = vi.fn(async () => response);
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');
    expect(result.body).toBe(response.body);
  });

  // chat.rs:139-141 — a stream:true request that fails BEFORE the head is committed answers as
  // plain JSON, not SSE. The caller must branch on content-type, not on its own stream flag.
  it('maps a non-2xx JSON answer to a stream:true request', async () => {
    const fetchImpl = vi.fn(
      async () =>
        new Response(JSON.stringify({ error: { message: 'no', type: 'invalid_request_error', param: null, code: 'streaming-disabled' } }), {
          status: 501,
          headers: { 'content-type': 'application/json', 'paigasus-correlation-id': 'corr-9' },
        }),
    );
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.presentation).toBe('disabled');
    expect(result.error.correlationId).toBe('corr-9');
  });
});

describe('the client never throws a mapped error', () => {
  it('maps a rejected fetch to the transport arm', async () => {
    const fetchImpl = vi.fn(async () => {
      throw new TypeError('fetch failed');
    });
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'network' });
    expect(result.error.presentation).toBe('degraded');
  });

  it('reports a caller abort as aborted, not as a service fault', async () => {
    const controller = new AbortController();
    const fetchImpl = vi.fn(async (_u: unknown, init?: RequestInit) => {
      init?.signal?.throwIfAborted();
      throw new DOMException('aborted', 'AbortError');
    });
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });
    controller.abort();

    const result = await client.completions({}, { signal: controller.signal });
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'aborted' });
    expect(result.error.presentation).toBe('generic');
  });
});

describe('the pre-header deadline', () => {
  it('fires when the headers never arrive', async () => {
    const fetchImpl = vi.fn(async (_u: unknown, init?: RequestInit) => {
      return await new Promise<Response>((_res, rej) => {
        init?.signal?.addEventListener('abort', () => rej(init.signal?.reason));
      });
    });
    const client = createChatClient({ baseUrl: BASE, headerTimeoutMs: 40, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'timeout' });
  });

  // THE row that separates the correct build from the broken one. Under
  // AbortSignal.any([caller, AbortSignal.timeout(N)]) the signal stays live after the headers
  // arrive, so the stub's stream errors at N and this test fails. Under a manual
  // AbortController cleared when fetch settles, the chunk arrives.
  it('does NOT abort the body after the headers arrive', async () => {
    const fetchImpl = streamingFetch([{ afterMs: 120, text: 'data: {"choices":[]}\n\n' }]);
    const client = createChatClient({ baseUrl: BASE, headerTimeoutMs: 40, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');

    const chunks: string[] = [];
    const decoder = new TextDecoder();
    for await (const chunk of result.body as unknown as AsyncIterable<Uint8Array>) {
      chunks.push(decoder.decode(chunk));
    }
    expect(chunks.join('')).toContain('"choices"');
  });
});

describe('cancellation', () => {
  // The SDK returns the platform's OWN stream, so propagation to a socket is undici's behaviour,
  // not this package's. What the SDK controls is that the object is the platform's — which is
  // what makes propagation possible — and that is asserted here and in the identity row above.
  // End-to-end propagation to a socket is NOT tested; see the plan's stated gaps.
  it('cancelling the returned stream reaches the source', async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start: (c) => c.enqueue(new TextEncoder().encode('data: {}\n\n')),
      cancel: () => {
        cancelled = true;
      },
    });
    const fetchImpl = vi.fn(async () => new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl as unknown as typeof globalThis.fetch }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    if (result.kind !== 'stream') throw new Error('unreachable');
    await result.body.cancel();
    expect(cancelled).toBe(true);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/chat.test.ts
```

Expected: FAIL — `createChatClient` is not exported.

- [ ] **Step 3: Append the client to `src/chat.ts`**

Add these imports at the top of `ts/packages/paigasus-sdk/src/chat.ts`, beside the existing ones:

```ts
import type { ErrorInput } from './errors/map-error.js';
import type { TransportCause } from './errors/types.js';
```

Then append:

```ts
/** The default bound on the wait for response HEADERS. Matches the gRPC transport's 10 s (§ 7.2). */
export const DEFAULT_HEADER_TIMEOUT_MS = 10_000;

export interface ChatClientOptions {
  readonly baseUrl: string;
  /**
   * Bounds the wait for response HEADERS only. After the head is committed no wall-clock timeout
   * applies: a long chat completion is a correct slow response, not a stalled one.
   */
  readonly headerTimeoutMs?: number;
  /**
   * The injection seam, read PER CALL. A module-load capture would make `vi.stubGlobal` useless
   * and would freeze whatever dispatcher a host had installed at import time (spec § 8.1).
   */
  readonly fetch?: typeof globalThis.fetch;
}

/**
 * The client never throws a mapped error. A caller branches on `kind`, and on `content-type`
 * rather than on its own `stream` flag — a `stream: true` request that fails before the head is
 * committed answers as plain JSON, not SSE (chat.rs:139-141).
 */
export type ChatResult =
  | { readonly kind: 'json'; readonly status: number; readonly body: unknown }
  | { readonly kind: 'stream'; readonly body: ReadableStream<Uint8Array> }
  | { readonly kind: 'error'; readonly error: PaigasusError };

export interface ChatClient {
  completions(request: unknown, options?: { readonly signal?: AbortSignal }): Promise<ChatResult>;
}

/**
 * The gateway's `POST /v1/chat/completions` — the one hand-written HTTP surface (spec § 4.1).
 *
 * `auth` is REQUIRED and is deliberately NOT the `Auth` union `./iam` uses. No path through the
 * gateway's `chat_completions` accepts an anonymous caller: `require_iam_auth` has already run
 * (chat.rs:5-6) and `MissingBearer` answers 401 (error.rs:122-128). An `{ anonymous: true }` arm
 * here would be a legal value with no legal use, so the narrower type makes it a compile error.
 *
 * This client uses `fetch` and builds no `Transport`, so it shares nothing with the transport
 * cache and § 7.1's cache-key rule does not reach it.
 */
export function createChatClient(options: ChatClientOptions, auth: { readonly bearer: string }): ChatClient {
  // Refused here, where the token is BOUND, rather than sent as `Authorization: Bearer `. A
  // caller reading an unset environment variable gets a clear local error naming the cause —
  // the same rule src/transport.ts applies on the gRPC side.
  if (auth.bearer.trim() === '') {
    throw new Error('@paigasus/sdk: refusing to send an empty or whitespace-only bearer token to the chat endpoint. This usually means an unset environment variable.');
  }

  const headerTimeoutMs = options.headerTimeoutMs ?? DEFAULT_HEADER_TIMEOUT_MS;
  const url = `${options.baseUrl.replace(/\/+$/, '')}/v1/chat/completions`;

  return {
    async completions(request, callOptions): Promise<ChatResult> {
      const fetchImpl = options.fetch ?? globalThis.fetch;
      const callerSignal = callOptions?.signal;

      // NOT AbortSignal.timeout. That signal keeps running after the headers arrive and aborts
      // the streaming body at the deadline, truncating every completion longer than the window —
      // the normal case for this product. A manual controller whose timer is cleared the moment
      // the fetch promise settles bounds the HEAD only (spec § 8.5).
      const controller = new AbortController();
      let timedOut = false;
      const timer = setTimeout(() => {
        timedOut = true;
        controller.abort(new DOMException('chat header timeout', 'TimeoutError'));
      }, headerTimeoutMs);

      let response: Response;
      try {
        response = await fetchImpl(url, {
          method: 'POST',
          headers: { authorization: `Bearer ${auth.bearer}`, 'content-type': 'application/json' },
          body: JSON.stringify(request),
          signal: callerSignal === undefined ? controller.signal : AbortSignal.any([controller.signal, callerSignal]),
        });
      } catch (cause) {
        return { kind: 'error', error: mapError(transportInput(cause, timedOut, callerSignal)) };
      } finally {
        // The BODY STREAM outlives this scope deliberately. Clearing the timer here is what stops
        // the pre-header deadline from ever reaching it.
        clearTimeout(timer);
      }

      if (!response.ok) {
        // The SDK maps it, so the caller needs no error knowledge of its own. The body may be
        // gateway-generated OR an upstream OpenAI envelope forwarded verbatim (chat.rs:113-119),
        // and it may not be JSON at all — mapError is total over both.
        return { kind: 'error', error: mapError({ kind: 'http', status: response.status, headers: response.headers, body: await readBody(response) }) };
      }

      const contentType = response.headers.get('content-type') ?? '';
      if (contentType.includes('text/event-stream')) {
        if (response.body === null) {
          return { kind: 'error', error: mapError({ kind: 'transport', cause: 'network', message: 'the gateway answered text/event-stream with no body' }) };
        }
        // AC 4: the IDENTICAL object. Not read, not buffered, not decoded, not re-encoded.
        // Cancelling it reaches the upstream connection because it IS the platform's stream.
        return { kind: 'stream', body: response.body };
      }

      return { kind: 'json', status: response.status, body: await readBody(response) };
    },
  };
}

/** Parse when we can, hand back the raw text when we cannot. `mapError` handles both. */
async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function transportInput(cause: unknown, timedOut: boolean, callerSignal: AbortSignal | undefined): ErrorInput {
  const message = cause instanceof Error ? cause.message : String(cause);
  // Order matters: our own timer wins over the generic abort classification, because a timeout
  // ABORTS and would otherwise read as a caller abort.
  const kind: TransportCause = timedOut ? 'timeout' : callerSignal?.aborted === true ? 'aborted' : 'network';
  return { kind: 'transport', cause: kind, message };
}
```

- [ ] **Step 4: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS.

- [ ] **Step 5: Prove the deadline row actually bites**

Temporarily replace the signal expression with the broken form:

```ts
signal: AbortSignal.any([AbortSignal.timeout(headerTimeoutMs), ...(callerSignal === undefined ? [] : [callerSignal])]),
```

and delete the `clearTimeout`. Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/chat.test.ts
```

Expected: the row `does NOT abort the body after the headers arrive` FAILS. Restore the correct code and re-run to confirm PASS. **Record both results in the commit body.** Without this the row passes under both builds and proves nothing.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-sdk/src/chat.ts ts/packages/paigasus-sdk/tests/chat.test.ts
git commit -m "feat(ts): add the OpenAI-compatible chat client to @paigasus/sdk"
```

---

### Task 7: The `gateway->sdk` affected-graph case — obligation 9

**Files:**
- Modify: `ci/affected-graph/run.sh` (add the case beside `proto->sdk`)

**Interfaces:**
- Consumes: Task 5's `moon.yml` input line.
- Produces: nothing the TypeScript uses. This is the CONTROL on that input line.

**Why this task exists.** Task 5's Moon input is the only thing that makes the terminal-frame drift check real. Nothing else in the repo notices if a future edit drops it: `repo:input-liveness` scans `repo:*` tasks only, and it proves DECLARED inputs are live, never that NEEDED ones are declared. This case is the control, exactly as `proto->sdk` is the control on the `@paigasus/proto` input (spec § 11.2).

**MEASURE the expected set. Do not transcribe it.** A `chat.rs` edit also selects the gateway crate's own Rust tasks, so this set is NOT a copy of `proto->sdk`'s.

- [ ] **Step 1: Measure the expected set**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-625
grep -n 'run_task_case_ci "proto-iam->sdk"' -B4 ci/affected-graph/run.sh
```

Read `_assert_task_case_impl` in that file and reproduce its `moon query tasks --affected` traversal exactly, against the anchor `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs`.

**Parse the JSON; never grep it.** `moon query tasks --affected` emits each selected task's `deps[]`, and every dep entry carries a `"target"` key of its own — so `grep -o '"target": "[^"]*"'` counts SCHEDULED upstreams as SELECTIONS. Take one target per `tasks[project][task]`.

Confirm `paigasus-sdk-ts:test` is in the result. If it is not, Task 5's `moon.yml` input did not land — fix that before continuing, because the case would otherwise be baselined against the broken state.

- [ ] **Step 2: Add the case**

Insert into `ci/affected-graph/run.sh`, directly after the `proto-iam->sdk` case, substituting the measured set:

```bash
  # SMA-625, spec § 8.4 and § 11.2 obligation 9 — a gateway chat.rs edit must select the SDK's
  # test. This is the ONLY control on the '/rs/.../chat.rs' entry in paigasus-sdk-ts:test's
  # `inputs`. tests/terminal-frame.test.ts reads TERMINAL_SSE_ERROR out of that Rust file by
  # constant name; remove the input and editing the constant selects no paigasus-sdk-ts task at
  # all, so Moon serves a cached PASS on exactly the PR that changes the frame — the same vacuity
  # M11 measured for the error registry, and nothing else in the repo notices.
  # The expected set is MEASURED with the same traversal _assert_task_case_impl uses, not copied
  # from the cases above: a chat.rs edit also selects the gateway crate's own Rust tasks.
  # Strict equality: re-baseline deliberately when the set legitimately changes.
  run_task_case_ci "gateway->sdk" "rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs" \
    "<THE MEASURED SET>"
```

- [ ] **Step 3: Run the gate**

macOS bash 5.3.15 deadlocks on a `while read` fed by a here-string over roughly 512 bytes, which hangs this suite locally while CI stays green. Use the system bash:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-625
/bin/bash ci/affected-graph/run.sh
```

Expected: `PASS gateway->sdk`, and every pre-existing case still passing. A hang is the bash bug, not a gate failure — re-run under `/bin/bash`.

- [ ] **Step 4: Prove the case actually bites**

Remove the `chat.rs` line from `ts/packages/paigasus-sdk/moon.yml`, re-run the gate, and confirm `gateway->sdk` FAILS. Restore the line and confirm it passes again. **Record both results in the commit body.**

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "test(ci): add the gateway-to-sdk affected-graph control case"
```

---

### Task 8: The root barrel, and verification against the full graph

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/index.ts`
- Test: `ts/packages/paigasus-sdk/tests/index-barrel.test.ts`

**Interfaces:**
- Consumes: every earlier task.
- Produces: the `.` entry's complete surface.

**Why the barrel names each re-export.** `src/iam.ts`, `src/chat.ts` and `src/errors.ts` could each grow a name the others already have. Under the ES semantics TypeScript follows, a star-exported ambiguous name is **excluded silently** rather than erroring — the exact failure `ts/packages/paigasus-proto/src/iam.ts:5-14` documents for `ServiceInfo`, and the reason PR A split that package's surface.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/index-barrel.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';

import * as barrel from '../src/index.js';

describe('the root barrel serves every entry surface', () => {
  it.each(['createChatClient', 'createTerminalFrameParser', 'mapError', 'createIamClient', 'disposeTransports', 'ErrorReason', 'ErrorDomain', 'presentationForGrpcCode', 'presentationForHttpStatus'])(
    'exports %s',
    (name) => {
      expect(barrel).toHaveProperty(name);
      expect(barrel[name as keyof typeof barrel]).toBeDefined();
    },
  );

  // An ambiguous star-exported name is dropped SILENTLY under ES semantics, so a value that is
  // present here can still vanish when a second module grows the same name. Naming each
  // re-export explicitly is what prevents it; this test is the reminder, not the mechanism.
  it('exposes ErrorReason as the registry enum, not a shadow', () => {
    expect(barrel.ErrorReason.UPSTREAM_ERROR).toBe(307);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/index-barrel.test.ts
```

Expected: FAIL — the chat and error names are absent.

- [ ] **Step 3: Rewrite the barrel**

Replace `ts/packages/paigasus-sdk/src/index.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. It re-exports every subpath so `@paigasus/sdk` serves a consumer that wants
// everything; the subpaths exist so a caller needing one surface does not pull the rest into its
// module graph (spec § 6.1).
//
// Each name is listed EXPLICITLY. `export *` from two modules that share a name drops the
// ambiguous name SILENTLY under the ES semantics TypeScript follows — no error, no warning — the
// exact failure ts/packages/paigasus-proto/src/iam.ts:5-14 documents for `ServiceInfo`.
import './server-guard.js';

export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService, bindAuth, createIamClient, disposeTransports } from './iam.js';
export type { Auth, TransportOptions } from './iam.js';

export { DEFAULT_HEADER_TIMEOUT_MS, createChatClient, createTerminalFrameParser } from './chat.js';
export type { ChatClient, ChatClientOptions, ChatResult } from './chat.js';

export { ErrorDomain, ErrorReason, PRESENTATION, grpcCodeName, mapError, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause, presentationOverride } from './errors.js';
export type { ErrorInput, PaigasusError, Presentation, PresentationEntry, TransportCause, TransportInfo } from './errors.js';
```

- [ ] **Step 4: Run the package's own gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec vitest run && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: PASS. Every suite, including the three PR B shipped.

- [ ] **Step 5: Run the whole-tree TypeScript gates**

`ts:fmt` is its own whole-tree Prettier gate, decoupled from lint and tsc. Run it after touching any `ts` file, or CI reds on formatting alone:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-625/ts
pnpm exec prettier --write . && pnpm exec eslint .
```

Expected: Prettier rewrites nothing outside this PR's files, and eslint is clean.

- [ ] **Step 6: Run the affected graph the way CI does**

Per-project tasks do NOT run the repo-level gates. Run the full command from CLAUDE.md's marker block, with `--base origin/main`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-625
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :test-e2e --base origin/main --include-relations
```

**Reading a failure.** Do not re-run before capturing evidence — a passing re-run rewrites `stdout.log`, truncates `stderr.log`, rewrites `lastRun.json` and flips the `ciReport.json` row to `passed`. Follow the procedure between the `moon-diagnosis` markers in `CLAUDE.md`. Two known non-defects:

- A `repo:release-parity*` task aborting at rc=2 inside an agent session is the proto NDJSON trap, not a red. It is INCONCLUSIVE.
- A sub-3s `repo:affected-smoke` failure under a concurrent `moon ci` is the known intermittent abort. Capture the full output and grep it for `proto-shim` BEFORE re-running.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/src/index.ts ts/packages/paigasus-sdk/tests/index-barrel.test.ts
git commit -m "feat(ts): re-export the chat and error surfaces from the @paigasus/sdk root barrel"
```

---

## What this plan deliberately does NOT cover

Stated so a reviewer does not read the suite as proving more than it does.

- **No live-service tier.** The SDK has no server to talk to in CI. Everything is tested through interceptors and a stub `fetch`. This proves the SDK *forms* correct requests and *interprets* correct responses. It does not prove IAM or the gateway accepts them. That is SMA-509/SMA-510's integration surface.
- **AC 1 of SMA-508 is not proven by a build.** No test here runs a client-side Next build. The `server-only` boundary is enforced structurally.
- **Cancellation never reaches a socket in any test.** The SDK returns the platform's own stream, so propagation belongs to undici. Only an integration tier could prove it.
- **No idle timeout.** Removed from the SDK's contract at the spec gate. Once the head is committed, a stalled stream is the caller's responsibility, and their `AbortSignal` is the lever.
- **SMA-627 is out of scope.** Whether an anonymous gRPC client may forward a caller-supplied `authorization` header is a separate decision on `src/transport.ts`.

## Spec coverage check

| Spec section | Task |
|---|---|
| § 8.1 the client surface | 6 |
| § 8.2 two response shapes, who maps | 6 |
| § 8.3 the passthrough | 6 |
| § 8.4 the parser and its drift check | 5, 7 |
| § 8.5 deadlines and cancellation | 6 |
| § 9.1 the one shape | 1 |
| § 9.2 the three status tables | 2 |
| § 9.3 the codec | landed in PR A |
| § 9.4 the override table | 3 |
| § 9.5 the five arms | 4 |
| § 9.6 a consumer can name a reason | 1 |
| § 10 the test tiers | 1-8 |
| § 11.1 the SDK's task inputs | 5 |
| § 11.2 obligation 9 | 7 |
| § 12.1 AC 1 | 4 (message-independence, degradation) |
| § 12.1 AC 2 | 3 (the Record and the descriptor test) |
| § 12.1 AC 3 | 4 (the four codes, the plain object, no raw types) |
| § 12.1 AC 4 | 6 (stream identity) |
