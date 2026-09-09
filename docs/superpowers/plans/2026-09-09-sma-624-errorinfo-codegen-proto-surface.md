<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-624 — `google.rpc.ErrorInfo` codegen and the widened `@paigasus/proto` surface

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate the `google.rpc.ErrorInfo` descriptor for TypeScript and widen `@paigasus/proto` so a Connect-ES client can read structured error details and resolve canonical error codes.

**Architecture:** A second, TypeScript-only `buf generate` invocation runs against the already-declared `buf.build/googleapis/googleapis` dep and emits one file into the existing generated tree. A hand-written codec in `@paigasus/proto` derives the kebab wire spellings from the generated enum descriptors — never a table — mirroring the Rust `ErrorReason::{as_wire_reason, from_wire_reason}` including its Unicode-safe validation order. The package's public surface then exposes the error registry, the codec, the `ErrorInfo` schema and the seven IAM service descriptors.

**Tech Stack:** buf 1.70.0, protoc-gen-es v2.13.0, `@bufbuild/protobuf` 2.14.1, TypeScript 6.0.3, vitest 5.0.0, Moon 2.5.3, Node 24.16.0.

**Spec:** `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md` — §§ 5, 9.3, 11.2 (obligation 3), 11.3 (obligations 4, 7, 8). PR **A** of the three-way split in § 14.1.

**Issue:** SMA-624. Blocks SMA-508 (PR B), which blocks SMA-625 (PR C).

## Global Constraints

- **Branch:** `feature/sma-624-contracts-ts-errorinfo-proto-surface`, in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-508`. Run every command from that worktree root. Do **not** `cd` to the main checkout.
- **PATH:** every shell invocation needs `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Also `export PROTO_REPORTER=text` before any command whose stdout you capture from `proto` or a proto-shimmed tool.
- **SPDX:** every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- **Commits:** conventional, with a workspace scope (`feat(ts):`, `build(contracts):`). Subject lowercase after the scope, ≤100 chars. **No body line may begin with `word:`** — commitlint parses it as a footer token and fails `footer-leading-blank`. Never use `--no-verify`.
- **Do not hand-edit** anything under `src/generated/` — it is `buf generate` output and is covered by the codegen-drift gate.
- **TypeScript strictness that will bite:** `ts/tsconfig.base.json` sets `noUncheckedIndexedAccess: true` (so `Schema.value[n]` is `T | undefined`), `verbatimModuleSyntax: true` (so a type must be imported with `import type`), and `exactOptionalPropertyTypes: true`.
- **Import extensions are `.js`** even for TypeScript sources — the repo uses `moduleResolution: bundler` with `import_extension=.js` codegen.
- **Known local flake:** `repo:affected-smoke` and `repo:actionlint` can hang forever on this machine — bash 5.3.15 deadlocks on a `while read` fed by a `<<<` here-string over roughly 512 bytes. A hang is **not** a gate failure. If one hangs, interrupt it and rely on CI for that gate.

---

## File Structure

| File | Responsibility |
|---|---|
| `contracts/buf.gen.googleapis.yaml` | **Create.** TypeScript-only template, `clean: false`, for the googleapis module. |
| `contracts/moon.yml` | **Modify.** `generate` becomes a `script:` running both templates in order; gains the new template in `inputs`. |
| `ci/affected-graph/ci_targets.py` | **Modify.** `CONTRACTS_GENERATE_INPUTS` — a strict-equality pin — gains the new template. |
| `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` | **Generated, committed.** Never hand-edited. |
| `ts/packages/paigasus-proto/src/error_info.test.ts` | **Create.** Asserts the generated descriptor exists and is the right type. |
| `ts/packages/paigasus-proto/src/error.ts` | **Create.** The four codec functions. |
| `ts/packages/paigasus-proto/src/error.test.ts` | **Create.** Rust-parity tests for the codec. |
| `ts/packages/paigasus-proto/src/iam.ts` | **Create.** The `./iam` subpath barrel (see the note under Task 3). |
| `ts/packages/paigasus-proto/src/index.ts` | **Modify.** Root barrel gains the error registry, the codec and `ErrorInfoSchema`. |
| `ts/packages/paigasus-proto/src/index.test.ts` | **Create.** Pins the public surface. |
| `ts/packages/paigasus-proto/package.json` | **Modify.** Adds the `./iam` export entry. |

Tests are **colocated** `*.test.ts` beside their source, matching this package's existing habit (`capability.test.ts`, `audit.test.ts`). The SDK package in PR B uses a `tests/` directory instead; each package keeps its own convention.

---

## Task 1: Generate the `google.rpc.ErrorInfo` descriptor

**Files:**
- Create: `contracts/buf.gen.googleapis.yaml`
- Modify: `contracts/moon.yml` (the `generate` task, lines 7-26)
- Modify: `ci/affected-graph/ci_targets.py:393-400` (`CONTRACTS_GENERATE_INPUTS`)
- Create: `ts/packages/paigasus-proto/src/error_info.test.ts`
- Generated: `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `ErrorInfoSchema: GenMessage<ErrorInfo>` and the type `ErrorInfo` (fields `reason: string`, `domain: string`, `metadata: { [key: string]: string }`), importable from `./generated/google/rpc/error_details_pb.js`. Task 3 re-exports it.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-proto/src/error_info.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { create } from '@bufbuild/protobuf';
import { ErrorInfoSchema } from './generated/google/rpc/error_details_pb.js';

// This file is a fast local signal that the two-invocation codegen ordering in
// contracts/moon.yml has not regressed. It is NOT the control — buf.gen.yaml's
// `clean: true` wipes the shared out: tree, so a reversed order deletes the
// generated module, and what catches that in CI is the codegen-drift step
// (.github/workflows/ci.yml:309-322), which runs unconditionally and reports
// the deletion through `git diff --exit-code`. See spec § 5.1.
describe('generated google.rpc.ErrorInfo', () => {
  it('is generated with its canonical type name', () => {
    expect(ErrorInfoSchema.typeName).toBe('google.rpc.ErrorInfo');
  });

  it('carries the three fields SMA-504 populates', () => {
    const names = ErrorInfoSchema.fields.map((f) => f.name).sort();
    expect(names).toEqual(['domain', 'metadata', 'reason']);
  });

  it('round-trips the (domain, reason, metadata) triple IAM emits', () => {
    const info = create(ErrorInfoSchema, {
      reason: 'slug-conflict',
      domain: 'iam.paigasus.io',
      metadata: { retryable: 'false', correlation_id: 'abc' },
    });
    expect(info.reason).toBe('slug-conflict');
    expect(info.domain).toBe('iam.paigasus.io');
    expect(info.metadata['retryable']).toBe('false');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-proto exec vitest run src/error_info.test.ts
```

Expected: FAIL — `Failed to resolve import "./generated/google/rpc/error_details_pb.js"`.

- [ ] **Step 3: Create the TypeScript-only template**

Create `contracts/buf.gen.googleapis.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# The SECOND of two templates run by contracts:generate (SMA-624). It exists
# because Connect-ES can only read error details through
# `ConnectError.findDetails(desc)`, which requires a generated descriptor —
# there is no schema-free overload (@connectrpc/connect@2.2.0,
# connect-error.d.ts:84-85). This closes ADR-0019 amendment A1.4's open item.
#
# TypeScript ONLY, deliberately. A1.4 records that *referencing* google.rpc
# from a local proto makes `buf generate` exit 0 while emitting Rust and Python
# that point at modules the run never produces. That is avoided here two ways:
# error.proto still imports nothing, and this template runs against a DIFFERENT
# input module (buf.build/googleapis/googleapis, already declared in buf.yaml
# with a buf.lock entry) with only the es plugin.
#
# `clean: false` is REQUIRED and load-bearing. buf.gen.yaml sets `clean: true`
# over the SAME out: tree, so it must run FIRST and this one second. Reversed,
# the main template wipes this output. Nothing pins that ordering — no gate
# reads a task's script: — so the control is the codegen-drift step in ci.yml,
# which is unconditional and reports the deleted file (spec § 5.1).
version: v2
clean: false

plugins:
  # Pinned to the SAME version as buf.gen.yaml's es plugin. If one moves, both
  # move: two protoc-gen-es versions writing into one tree would produce output
  # that neither template alone reproduces, and the drift gate would flap.
  - remote: buf.build/bufbuild/es:v2.13.0
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
      - import_extension=.js
```

- [ ] **Step 4: Make `contracts:generate` run both templates in order**

In `contracts/moon.yml`, replace the `generate` task's `command:` line with a `script:` and add the new input. The task becomes:

```yaml
  generate:
    # TWO invocations, and the ORDER is load-bearing (SMA-624). buf.gen.yaml sets
    # `clean: true`, which wipes ts/packages/paigasus-proto/src/generated before it
    # writes. buf.gen.googleapis.yaml writes google/rpc/ into that SAME tree with
    # `clean: false`. Run them the other way round and the googleapis output is
    # deleted by the main template.
    #
    # `set -euo pipefail` is REQUIRED. Moon does not enable errexit for `script:`
    # blocks and takes the block's status from its LAST command, so without it a
    # failed first `buf generate` followed by a successful second one exits 0 —
    # and the codegen-drift gate would then diff a half-regenerated tree.
    #
    # RECOVERY: if the second invocation fails (a BSR outage is the realistic
    # cause), the tree is left with google/rpc/error_details_pb.ts DELETED,
    # because `clean: true` already ran. Re-run `moon run contracts:generate
    # --force`. Do NOT commit the deletion.
    script: |
      set -euo pipefail
      buf generate
      buf generate --template buf.gen.googleapis.yaml \
        buf.build/googleapis/googleapis \
        --path google/rpc/error_details.proto
    toolchain: 'system'
    # SMA-592. The three REMOTE plugin versions live in buf.gen.yaml, already listed below. The
    # other two generators do not, and without them this task's cache key is a lie:
    #   /.prototools   pins buf ITSELF (1.70.0). buf's own version changes its output.
    #   /py/uv.lock    pins protoc-gen-python_betterproto2 — the `local:` plugin in buf.gen.yaml
    #                  is run through `uv run --project ../py`, so the py workspace lock is what
    #                  selects the compiler version.
    # This matters because ci.yml:249-262's codegen-drift gate DELEGATES its freshness to this
    # task: it runs `moon run contracts:generate` and diffs. On a cache hit buf never runs, and
    # the diff compares the committed output against itself. `.moon/cache` is restored across CI
    # runs (ci.yml:115-121), so that vacuous pass happens in CI, not just locally.
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
      - 'buf.gen.yaml'
      # SMA-624. Without this the second template's version and its `--path`
      # narrowing are outside the cache key, and an edit here serves a cached pass.
      - 'buf.gen.googleapis.yaml'
      - 'buf.lock'
      - '/.prototools'
      - '/py/uv.lock'
```

Leave `lint`, `fmt` and `breaking` untouched.

- [ ] **Step 5: Regenerate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
moon run contracts:generate --force
```

Expected: exit 0. Then confirm exactly one new file and that nothing else moved:

```bash
git status --short -- ts/packages/paigasus-proto/src/generated \
  rs/crates/libs/paigasus-proto/src/generated \
  py/packages/paigasus-proto/src/paigasus_proto/generated
```

Expected: a single untracked `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts`, and **no modifications** to the Rust or Python trees. If the Rust or Python trees changed, stop — the template is not TypeScript-only.

- [ ] **Step 6: Run the test to verify it passes**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/error_info.test.ts
```

Expected: PASS, 3 tests.

- [ ] **Step 7: Prove the ordering is load-bearing (measurement M5)**

Temporarily swap the two `buf generate` lines in `contracts/moon.yml`, then:

```bash
moon run contracts:generate --force
ls ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts
```

Expected: the `ls` fails — the file is gone, proving `clean: true` destroys it. **Restore the correct order by editing the file back** (do not `git checkout --`, which would also revert Step 4's uncommitted work), then re-run `moon run contracts:generate --force` and confirm the file returns.

Record the result in the spec's § 13 table by flipping M5 to **TAKEN**.

- [ ] **Step 8: Update the strict-equality input pin**

In `ci/affected-graph/ci_targets.py`, `CONTRACTS_GENERATE_INPUTS` (around line 393) becomes:

```python
CONTRACTS_GENERATE_INPUTS = (
    "contracts/proto/**/*",
    ".prototools",
    "contracts/buf.gen.googleapis.yaml",
    "contracts/buf.gen.yaml",
    "contracts/buf.lock",
    "contracts/buf.yaml",
    "py/uv.lock",
)
```

The tuple is compared against Moon's *resolved* inputs, which are sorted, so keep the entries in the order shown. If the gate reports a mismatch, take the ordering from its own error output rather than guessing.

- [ ] **Step 9: Run the gate**

```bash
moon run repo:affected-smoke
```

Expected: PASS. If it hangs for more than about 60 seconds, that is the documented local bash here-string deadlock — interrupt it, note it, and let CI run the gate.

- [ ] **Step 10: Run the whole TypeScript package**

```bash
moon run paigasus-proto-ts:test paigasus-proto-ts:typecheck
```

Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add contracts/buf.gen.googleapis.yaml contracts/moon.yml \
        ci/affected-graph/ci_targets.py \
        ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts \
        ts/packages/paigasus-proto/src/error_info.test.ts \
        docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md
git commit -F - <<'MSG'
build(contracts): generate google.rpc.ErrorInfo for TypeScript

Connect-ES reads error details only through findDetails(desc), which
requires a generated descriptor. This adds a second, TypeScript-only buf
template against the already declared googleapis dep, emitting one
self-contained file. Closes ADR-0019 amendment A1.4's open item.

The two invocations run in a load-bearing order. buf.gen.yaml sets
clean true over the shared output tree, so the main template must run
first. Measured by reversing them and watching the googleapis output
disappear. Nothing pins the ordering, since no gate reads a task script,
so the control is the unconditional codegen-drift step in ci.yml.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017Ccgmc1RJB88FqwTgtrXt3
MSG
```

---

## Task 2: The wire-reason and wire-domain codec

**Files:**
- Create: `ts/packages/paigasus-proto/src/error.ts`
- Create: `ts/packages/paigasus-proto/src/error.test.ts`

**Interfaces:**
- Consumes: `ErrorReason`, `ErrorReasonSchema`, `ErrorDomain`, `ErrorDomainSchema` from `./generated/paigasus/common/v1/error_pb.js` (already generated; `ErrorReasonSchema.values` is a `DescEnumValue[]` whose entries carry `.name` — the raw proto name such as `ERROR_REASON_SLUG_CONFLICT` — and `.number`).
- Produces, for Task 3 and for PR C:
  - `asWireReason(reason: ErrorReason): string | undefined`
  - `fromWireReason(reason: string): ErrorReason | undefined`
  - `asWireDomain(domain: ErrorDomain): string | undefined`
  - `fromWireDomain(domain: string): ErrorDomain | undefined`

  All four return `undefined` for the zero sentinel and for anything unresolvable.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-proto/src/error.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { asWireDomain, asWireReason, fromWireDomain, fromWireReason } from './error.js';
import {
  ErrorDomain,
  ErrorReason,
  ErrorReasonSchema,
} from './generated/paigasus/common/v1/error_pb.js';

describe('asWireReason', () => {
  it('spells the registry codes exactly', () => {
    expect(asWireReason(ErrorReason.SLUG_CONFLICT)).toBe('slug-conflict');
    expect(asWireReason(ErrorReason.INVALID_REQUEST_SCHEMA)).toBe('invalid-request-schema');
    expect(asWireReason(ErrorReason.CAPABILITY_DISABLED)).toBe('capability-disabled');
    expect(asWireReason(ErrorReason.INTERNAL)).toBe('internal');
  });

  it('has no wire spelling for the zero sentinel', () => {
    expect(asWireReason(ErrorReason.UNSPECIFIED)).toBeUndefined();
  });

  it('has no wire spelling for a number the enum does not know', () => {
    // A newer service can emit a code this build's generated enum predates.
    expect(asWireReason(99999 as ErrorReason)).toBeUndefined();
  });
});

describe('fromWireReason', () => {
  it('resolves every code this build declares', () => {
    expect(fromWireReason('slug-conflict')).toBe(ErrorReason.SLUG_CONFLICT);
    expect(fromWireReason('authn-unavailable')).toBe(ErrorReason.AUTHN_UNAVAILABLE);
  });

  it('has no value for the zero sentinel spelling', () => {
    expect(fromWireReason('unspecified')).toBeUndefined();
  });

  it('rejects malformed input rather than folding it', () => {
    // Parity with the Rust from_wire_reason_rejects_malformed_input test
    // (rs/crates/libs/paigasus-proto/src/error.rs:287-302). The list is
    // identical, deliberately: a laxer TypeScript parser would accept codes the
    // service can never emit, and would weaken the consumed-side table test in
    // SMA-625.
    //
    // The last two are the reason validation runs BEFORE any case transform,
    // not after. MEASURED on Node 24.16.0: "ınternal".toUpperCase() is exactly
    // "INTERNAL" and "ſlug-conflict".toUpperCase() is "SLUG-CONFLICT", because
    // JavaScript folds U+0131 and U+017F just as Rust's str::to_uppercase does.
    // A deny-list check applied after uppercasing would resolve both to real
    // registry values.
    for (const bad of [
      'slug_conflict',
      'SLUG-CONFLICT',
      'Slug-Conflict',
      '',
      '-slug',
      'slug-',
      'slug--conflict',
      'no-such-code',
      'ınternal',
      'ſlug-conflict',
    ]) {
      expect(fromWireReason(bad), bad).toBeUndefined();
    }
  });
});

describe('the reason codec round-trips the whole registry', () => {
  it('covers every non-sentinel value declared in error.proto', () => {
    const values = ErrorReasonSchema.values.filter((v) => v.name !== 'ERROR_REASON_UNSPECIFIED');

    // Cardinality guard. The Rust mirror asserts 57 at
    // rs/crates/libs/paigasus-proto/src/error.rs:230; the two must agree,
    // because both derive from the same proto.
    expect(values).toHaveLength(57);

    for (const value of values) {
      const reason = value.number as ErrorReason;
      const wire = asWireReason(reason);
      expect(wire, value.name).toBeDefined();
      expect(wire).toMatch(/^[a-z][a-z0-9]*(-[a-z0-9]+)*$/);
      expect(fromWireReason(wire as string)).toBe(reason);
    }
  });
});

describe('the domain codec', () => {
  it('spells both domains with the suffix', () => {
    expect(asWireDomain(ErrorDomain.IAM)).toBe('iam.paigasus.io');
    expect(asWireDomain(ErrorDomain.GATEWAY)).toBe('gateway.paigasus.io');
  });

  it('has no wire spelling for the zero sentinel', () => {
    expect(asWireDomain(ErrorDomain.UNSPECIFIED)).toBeUndefined();
  });

  it('round-trips both domains', () => {
    expect(fromWireDomain('iam.paigasus.io')).toBe(ErrorDomain.IAM);
    expect(fromWireDomain('gateway.paigasus.io')).toBe(ErrorDomain.GATEWAY);
  });

  it('requires the suffix and rejects a malformed label', () => {
    for (const bad of ['iam', 'iam.example.com', 'IAM.paigasus.io', '.paigasus.io', 'ıam.paigasus.io']) {
      expect(fromWireDomain(bad), bad).toBeUndefined();
    }
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/error.test.ts
```

Expected: FAIL — `Failed to resolve import "./error.js"`.

- [ ] **Step 3: Write the implementation**

Create `ts/packages/paigasus-proto/src/error.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import {
  ErrorDomain,
  ErrorDomainSchema,
  ErrorReason,
  ErrorReasonSchema,
} from './generated/paigasus/common/v1/error_pb.js';

const REASON_PREFIX = 'ERROR_REASON_';
const DOMAIN_PREFIX = 'ERROR_DOMAIN_';
const DOMAIN_SUFFIX = '.paigasus.io';
const UNSPECIFIED = 'UNSPECIFIED';

/**
 * The grammar a wire token must already satisfy, as an ASCII ALLOW-list:
 * lowercase letter first, then lowercase letters, digits and single hyphens,
 * no trailing hyphen.
 *
 * This is checked BEFORE any case transform, and that order is the whole point.
 * MEASURED on Node 24.16.0: `"ınternal".toUpperCase()` is `"INTERNAL"` and
 * `"ſlug-conflict".toUpperCase()` is `"SLUG-CONFLICT"` — JavaScript folds
 * U+0131 (dotless i) and U+017F (long s) exactly as Rust's `str::to_uppercase`
 * does. So a deny-list applied to the uppercased string would resolve both of
 * those to real registry values. Mirrors `is_wire_token` in
 * rs/crates/libs/paigasus-proto/src/error.rs:51-64.
 */
const WIRE_TOKEN = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;

/**
 * Reverse lookups, built once from the generated descriptors rather than
 * tabulated. Same reasoning as `capabilityWireKey`: reading the descriptor's
 * full proto `name` — not the TypeScript enum's reverse map — means
 * protobuf-es's prefix-stripping heuristic (`findEnumSharedPrefix`) cannot
 * affect the transform, which keeps exact parity with the Rust side.
 */
const REASON_BY_PROTO_NAME = new Map<string, ErrorReason>(
  ErrorReasonSchema.values.map((v) => [v.name, v.number as ErrorReason]),
);
const DOMAIN_BY_PROTO_NAME = new Map<string, ErrorDomain>(
  ErrorDomainSchema.values.map((v) => [v.name, v.number as ErrorDomain]),
);

/** The kebab wire spelling of a reason, or `undefined` for the zero sentinel. */
export function asWireReason(reason: ErrorReason): string | undefined {
  const name = ErrorReasonSchema.value[reason]?.name;
  if (name === undefined || !name.startsWith(REASON_PREFIX)) {
    return undefined;
  }
  const short = name.slice(REASON_PREFIX.length);
  if (short === UNSPECIFIED) {
    return undefined;
  }
  return short.toLowerCase().replace(/_/g, '-');
}

/**
 * The reason a kebab wire string names, or `undefined` when it is malformed or
 * absent from this build's registry.
 *
 * An unknown-but-well-formed code is not an error: a newer service may emit a
 * code this build predates, and ADR-0019 decision 9 requires the consumer to
 * fall back to a generic presentation rather than throw.
 */
export function fromWireReason(reason: string): ErrorReason | undefined {
  if (!WIRE_TOKEN.test(reason)) {
    return undefined;
  }
  // Safe only because the allow-list above already restricted the input to
  // [a-z0-9-]; on that alphabet toUpperCase is pure ASCII.
  const name = `${REASON_PREFIX}${reason.toUpperCase().replace(/-/g, '_')}`;
  const value = REASON_BY_PROTO_NAME.get(name);
  return value === undefined || value === ErrorReason.UNSPECIFIED ? undefined : value;
}

/** The wire spelling of a domain, or `undefined` for the zero sentinel. */
export function asWireDomain(domain: ErrorDomain): string | undefined {
  const name = ErrorDomainSchema.value[domain]?.name;
  if (name === undefined || !name.startsWith(DOMAIN_PREFIX)) {
    return undefined;
  }
  const short = name.slice(DOMAIN_PREFIX.length);
  if (short === UNSPECIFIED) {
    return undefined;
  }
  return `${short.toLowerCase().replace(/_/g, '-')}${DOMAIN_SUFFIX}`;
}

/** The domain a wire string names, or `undefined` when it is malformed or unknown. */
export function fromWireDomain(domain: string): ErrorDomain | undefined {
  if (!domain.endsWith(DOMAIN_SUFFIX)) {
    return undefined;
  }
  const label = domain.slice(0, domain.length - DOMAIN_SUFFIX.length);
  if (!WIRE_TOKEN.test(label)) {
    return undefined;
  }
  const name = `${DOMAIN_PREFIX}${label.toUpperCase().replace(/-/g, '_')}`;
  const value = DOMAIN_BY_PROTO_NAME.get(name);
  return value === undefined || value === ErrorDomain.UNSPECIFIED ? undefined : value;
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/error.test.ts
```

Expected: PASS.

If the cardinality assertion fails with a number other than 57, do **not** edit the number to match. Check `contracts/proto/paigasus/common/v1/error.proto` and the Rust mirror at `rs/crates/libs/paigasus-proto/src/error.rs:230` first — a disagreement between them is the actual defect.

- [ ] **Step 5: Typecheck**

```bash
moon run paigasus-proto-ts:typecheck
```

Expected: PASS. `noUncheckedIndexedAccess` is why both `as*` functions use `?.` on `Schema.value[…]`; if you removed that, this is where it fails.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-proto/src/error.ts ts/packages/paigasus-proto/src/error.test.ts
git commit -F - <<'MSG'
feat(ts): add the canonical error-code codec to @paigasus/proto

The TypeScript twin of ErrorReason and ErrorDomain's wire transforms,
derived from the generated enum descriptors rather than tabulated, so
there is no second copy of the registry to drift against the proto.

Validation runs before the case transform, as an ASCII allow-list.
Measured on Node 24.16.0, toUpperCase folds U+0131 to I and U+017F to
S, so a deny-list applied after uppercasing would resolve both to real
registry values. The rejection set matches the Rust test exactly.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017Ccgmc1RJB88FqwTgtrXt3
MSG
```

---

## Task 3: Widen the public surface

**Files:**
- Create: `ts/packages/paigasus-proto/src/iam.ts`
- Modify: `ts/packages/paigasus-proto/src/index.ts`
- Modify: `ts/packages/paigasus-proto/package.json` (the `exports` map)
- Create: `ts/packages/paigasus-proto/src/index.test.ts`

**Interfaces:**
- Consumes: Task 1's `ErrorInfoSchema`, Task 2's four codec functions.
- Produces, for PR B and PR C:
  - from `@paigasus/proto`: `ErrorReason`, `ErrorReasonSchema`, `ErrorDomain`, `ErrorDomainSchema`, `ErrorInfoSchema`, the type `ErrorInfo`, and the four codec functions.
  - from `@paigasus/proto/iam`: all seven service descriptors — `TenancyService`, `AuthnService`, `AuthorizationService`, `ServiceAccountService`, `AuditService`, `UserService`, `OutboxService` — and every `iam/v1` message type and schema.

**A deviation from the spec, and why.** Spec § 11.3 obligation 4 says the seven service descriptors join the **root barrel**. That is not possible as written: `iam_pb.ts` exports a **deprecated `ServiceInfo`** message (`iam.proto:22-33`, kept only because buf forbids message deletion) whose name collides with the live `ServiceInfo` the root barrel already re-exports from `common/v1/service_info_pb.js`. A blanket `export *` is therefore a duplicate-export compile error, and enumerating roughly a hundred message names by hand is a maintenance burden that would need editing on every proto change.

A dedicated `./iam` subpath resolves it: the two `ServiceInfo` names live in different modules and never collide. This is **not** the `./generated/*` subpath the spec rules out — that was rejected because it would make the generated file layout public API, and `./iam` is a curated entry backed by a hand-written barrel file, so the layout stays free to change. Update the spec's obligation 4 to match while doing this task.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-proto/src/index.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import {
  ErrorDomain,
  ErrorDomainSchema,
  ErrorInfoSchema,
  ErrorReason,
  ErrorReasonSchema,
  asWireDomain,
  asWireReason,
  fromWireDomain,
  fromWireReason,
} from './index.js';
import {
  AuditService,
  AuthnService,
  AuthorizationService,
  OutboxService,
  ServiceAccountService,
  TenancyService,
  UserService,
} from './iam.js';

// These assertions are the package's public-surface contract. @paigasus/sdk's
// eslint boundary rule permits it to import @paigasus/proto and nothing else in
// the @paigasus/* namespace, so anything the SDK needs must be reachable here.
// Narrowing this surface breaks a consumer that cannot route around it.
describe('the root barrel', () => {
  it('exposes the error registry and its schemas', () => {
    expect(ErrorReason.SLUG_CONFLICT).toBe(1);
    expect(ErrorDomain.IAM).toBe(1);
    expect(ErrorReasonSchema.typeName).toBe('paigasus.common.v1.ErrorReason');
    expect(ErrorDomainSchema.typeName).toBe('paigasus.common.v1.ErrorDomain');
  });

  it('exposes the google.rpc.ErrorInfo schema Connect-ES needs for findDetails', () => {
    expect(ErrorInfoSchema.typeName).toBe('google.rpc.ErrorInfo');
  });

  it('exposes both codec directions', () => {
    expect(asWireReason(ErrorReason.SLUG_CONFLICT)).toBe('slug-conflict');
    expect(fromWireReason('slug-conflict')).toBe(ErrorReason.SLUG_CONFLICT);
    expect(asWireDomain(ErrorDomain.GATEWAY)).toBe('gateway.paigasus.io');
    expect(fromWireDomain('gateway.paigasus.io')).toBe(ErrorDomain.GATEWAY);
  });
});

describe('the ./iam subpath', () => {
  it('exposes all seven IAM services', () => {
    const services = [
      TenancyService,
      AuthnService,
      AuthorizationService,
      ServiceAccountService,
      AuditService,
      UserService,
      OutboxService,
    ];
    expect(services.map((s) => s.typeName)).toEqual([
      'paigasus.iam.v1.TenancyService',
      'paigasus.iam.v1.AuthnService',
      'paigasus.iam.v1.AuthorizationService',
      'paigasus.iam.v1.ServiceAccountService',
      'paigasus.iam.v1.AuditService',
      'paigasus.iam.v1.UserService',
      'paigasus.iam.v1.OutboxService',
    ]);
  });

  it('is a separate module from the root barrel, which is what avoids the ServiceInfo collision', async () => {
    // iam.proto:22-33 keeps a DEPRECATED ServiceInfo message that buf forbids
    // deleting. The live one is paigasus.common.v1.ServiceInfo, re-exported
    // from the root. BOTH generated modules export a runtime `ServiceInfoSchema`
    // AND a `ServiceInfo` type, so re-exporting both from one module is a
    // duplicate-export error. They must never meet.
    //
    // Asserted on the SCHEMA, not the message type: `ServiceInfo` is a type and
    // is erased at runtime, so an `in` check against the namespace object would
    // read false and prove nothing.
    const iam = await import('./iam.js');
    const root = await import('./index.js');
    expect(iam.ServiceInfoSchema.typeName).toBe('paigasus.iam.v1.ServiceInfo');
    expect(root.ServiceInfoSchema.typeName).toBe('paigasus.common.v1.ServiceInfo');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/index.test.ts
```

Expected: FAIL — `Failed to resolve import "./iam.js"`.

- [ ] **Step 3: Create the `./iam` barrel**

Create `ts/packages/paigasus-proto/src/iam.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The iam/v1 surface, as its OWN entry point rather than part of the root barrel.
//
// The reason is a name collision, not taste. iam.proto:22-33 keeps a DEPRECATED
// `ServiceInfo` message — dead, served by nothing, retained only because buf
// forbids deleting a message — and the root barrel already re-exports the LIVE
// `paigasus.common.v1.ServiceInfo`. Re-exporting both from one module is a
// duplicate-export error, and hand-enumerating every other iam/v1 name to dodge
// it would need editing on every proto change.
//
// This is a curated entry backed by this file, NOT a `./generated/*` passthrough.
// That distinction matters: a passthrough would make the generated file layout
// public API, so moving a generated directory would become a breaking change for
// consumers. Here the layout stays free to move behind this barrel.
export * from './generated/paigasus/iam/v1/iam_pb.js';
```

- [ ] **Step 4: Widen the root barrel**

Replace `ts/packages/paigasus-proto/src/index.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0

export type { Auditable } from './audit.js';
export { ActorSchema } from './generated/paigasus/common/v1/actor_pb.js';
export type { Actor } from './generated/paigasus/common/v1/actor_pb.js';
export { AuditMetadataSchema } from './generated/paigasus/common/v1/audit_pb.js';
export type { AuditMetadata } from './generated/paigasus/common/v1/audit_pb.js';
export { capabilityWireKey } from './capability.js';
export { Capability, ServiceInfoSchema, ServiceInfoService } from './generated/paigasus/common/v1/service_info_pb.js';
export type { ServiceInfo } from './generated/paigasus/common/v1/service_info_pb.js';

// The canonical error registry (ADR-0019) and its codec. ErrorReason and
// ErrorDomain are REGISTRIES, never wire types — no proto field has either
// type, and the wire carries the kebab strings the codec produces.
export { ErrorDomain, ErrorDomainSchema, ErrorReason, ErrorReasonSchema } from './generated/paigasus/common/v1/error_pb.js';
export { asWireDomain, asWireReason, fromWireDomain, fromWireReason } from './error.js';

// google.rpc.ErrorInfo, generated by the second buf template (SMA-624). This is
// what ConnectError.findDetails(desc) needs; there is no schema-free overload.
export { ErrorInfoSchema } from './generated/google/rpc/error_details_pb.js';
export type { ErrorInfo } from './generated/google/rpc/error_details_pb.js';
```

- [ ] **Step 5: Add the `./iam` export entry**

In `ts/packages/paigasus-proto/package.json`, the `exports` map becomes:

```json
  "exports": {
    ".": "./src/index.ts",
    "./iam": "./src/iam.ts"
  },
```

- [ ] **Step 6: Run the test to verify it passes**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/index.test.ts
```

Expected: PASS.

- [ ] **Step 7: Run the package's full suite and typecheck**

```bash
moon run paigasus-proto-ts:test paigasus-proto-ts:typecheck paigasus-proto-ts:build
```

Expected: PASS.

- [ ] **Step 8: Run the workspace lint and format gates**

```bash
moon run ts:lint ts:fmt
```

Expected: PASS. `src/generated/**` is excluded from both (`ts/eslint.config.js:14`, `ts/.prettierignore`), so the new generated file needs no formatting. If `ts:fmt` reports the hand-written files, run `pnpm -C ts exec prettier --write src` on them and re-run.

- [ ] **Step 9: Update the spec's obligation 4 and § 13**

In `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md`, § 11.3 obligation 4: record that the seven service descriptors ship through a `./iam` subpath rather than the root barrel, and why (the deprecated `ServiceInfo` collision). Confirm the sentence ruling out `./generated/*` still stands, because it does and for a different reason.

Flip M5 to **TAKEN** in § 13 if Task 1 Step 7 has not already.

- [ ] **Step 10: Commit**

```bash
git add ts/packages/paigasus-proto/src/index.ts ts/packages/paigasus-proto/src/iam.ts \
        ts/packages/paigasus-proto/src/index.test.ts \
        ts/packages/paigasus-proto/package.json \
        docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md
git commit -F - <<'MSG'
feat(ts): widen the @paigasus/proto public surface for the SDK

Exposes the error registry, both codec directions and the ErrorInfo
schema from the root barrel, and the seven IAM service descriptors from
a new iam subpath.

The subpath is forced by a name collision rather than chosen. iam.proto
keeps a deprecated ServiceInfo message that buf will not let us delete,
and the root barrel already re-exports the live one from common. It is
a curated barrel file, not a generated passthrough, so the generated
layout stays free to move.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017Ccgmc1RJB88FqwTgtrXt3
MSG
```

---

## Final verification

- [ ] **Run the affected graph like CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
moon ci :build :test :lint :fmt :typecheck :breaking :affected-smoke :ruff-ci --base origin/main --include-relations
```

Expected: PASS. A sub-3-second `affected-smoke` abort is the documented infrastructure flake — capture the output before re-running, and grep it for `proto-shim`.

- [ ] **Confirm the codegen is reproducible**

```bash
moon run contracts:generate --force
git status --short -- ts/packages/paigasus-proto/src/generated
```

Expected: empty. A non-empty result means the committed output does not match what the templates produce, which is exactly what the CI drift step would catch.

## Acceptance criteria mapping

| SMA-624 AC | Task / step |
|---|---|
| 1 — both trees generate, re-running is a no-op | Task 1 Steps 5-6; Final verification |
| 2 — the ordering is load-bearing, proven by reversal | Task 1 Step 7 (M5) |
| 3 — allow-list before the case transform, ten Rust-parity rejections | Task 2 Steps 1, 3 |
| 4 — `ErrorInfoSchema.typeName` asserted | Task 1 Step 1 |
| 5 — both directions round-trip over all 57 reasons | Task 2 Step 1 |

## What this plan deliberately does not do

- **No `@paigasus/sdk` changes.** The package stays `export {};`. Its `moon.yml` inputs, the `contracts->proto` expected-set edit and the new `proto->sdk` affected-graph case all belong to PR B (SMA-508), because they only make sense once the SDK depends on this package.
- **No script pin on `contracts:generate`'s ordering.** No gate reads a task's `script:`, so adding one means a new registry obligation. Spec § 5.1 records this as a stated residual.
- **No catalog entries.** `@connectrpc/*` arrives in PR B.
