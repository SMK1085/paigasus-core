# SMA-640: Connection-nominated headers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The three in-process HTTP proxies in `ts/` remove every field that a `Connection` header names (except `content-length`), and also `proxy-authenticate` and `proxy-authorization`, in both directions, through one shared helper.

**Architecture:** A new pure module `ts/packages/paigasus-console-core/testing/hop-by-hop.ts` exports `forwardableHeaders()`. The TLS terminator imports it with a relative path. The counting forwarder and `ts/tooling/dev-stack.ts` import it from `@paigasus/console-core/testing`. The three local copies of `HOP_BY_HOP` and `forwardable()` are deleted. Behaviour tests pin the terminator and the counting forwarder. A text-pin test pins the `dev-stack.ts` call sites.

**Tech Stack:** TypeScript (strict, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `verbatimModuleSyntax`), Node 24 `node:http` / `node:https` / `node:tls` / `node:net`, vitest, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-10-01-sma-640-connection-nominated-headers-design.md` (APPROVED by Sven on 2026-10-01, Q1-Q5 answered). Read the spec before you start a task.

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-640-connection-nominated-headers`, on branch `feature/sma-640-connection-nominated-headers`. Start every Bash command with `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" &&`.
- Conventional commits with a workspace scope (`feat(ts): …`, `test(ts): …`). End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset`, or `git checkout -- <file>`. Never `amend` then `reset HEAD~1`.
- The fixed set is exactly: `connection, keep-alive, proxy-authenticate, proxy-authorization, proxy-connection, te, trailer, transfer-encoding, upgrade` (spec §4.2).
- `NEVER_NOMINATED` is exactly `{'content-length'}` (decision D6). `host` is NOT protected (decision D3).
- The exported name is `forwardableHeaders` (decision D4). Do not export the set.
- The helper is pure: it does not log, does not throw, and does not change its input (spec §4.2).
- Do not edit `ts/apps/iam-console/tests/integration/doubles/tls-terminator.test.ts` (spec §6.2).
- Mutation runs are authorized. Make the mutation with the Edit tool. Restore it with the Edit tool (the exact reverse edit). Confirm with `git diff`. Never commit a mutation. A mutation must compile (`tsc` must pass), or it proves nothing.
- Do not install host software. Do not leave background jobs running.
- After you touch any ts file, run `ts:fmt` (Prettier) and the `typecheck` targets.

## Review Focus

These five input classes are implied by the spec, but no spec test exercises them. Each line has a test in the named task.

1. **A `GET` with a body and `Connection: Content-Length` through a proxy.** A person expects the body to reach the upstream intact (D6 at the call site, not only in the helper). Without D6, Node writes the body with no framing and the upstream reads it as a second request (spec §6.5). Test: Task 4, "keeps a nominated content-length, so a GET body keeps its framing".
2. **Two separate `Connection` header lines on the wire.** A person expects the fields that both lines name to be removed. Node joins the lines with `", "` (spec §6.6), and only a raw socket can send two lines. Test: Task 4, "removes the fields that two separate Connection lines nominate".
3. **A Firefox-style WebSocket handshake, `Connection: keep-alive, Upgrade`.** A person expects the tunnel still to open, and the upstream to see `Connection: Upgrade`. Test: Task 3, "still tunnels a handshake whose Connection is keep-alive, Upgrade".
4. **A nomination of `host` or `x-forwarded-host` at the terminator.** A person expects the terminator still to send its own `Host` and `X-Forwarded-Host` (spec D3: "no effect in the terminator"). A refactor that filters the merged object, not `req.headers`, would drop them. Test: Task 2, "keeps the terminator's own host and x-forwarded-host when the client nominates them".
5. **A response with several `set-cookie` values and a nominating `Connection`.** A person expects every cookie to arrive. The helper copies arrays as is (spec §6.1 row 7), and the terminator path must keep them too. Test: Task 2, response-direction case (the upstream sends two `set-cookie` values, and the test asserts both).

---

## File map

| File | Responsibility | Task |
|---|---|---|
| `ts/packages/paigasus-console-core/testing/hop-by-hop.ts` (new) | The fixed set, `NEVER_NOMINATED`, and `forwardableHeaders()` | 1 |
| `ts/packages/paigasus-console-core/testing/index.ts` | Export `forwardableHeaders`; subpath comment | 1 |
| `ts/packages/paigasus-console-core/package.json` | `_comment_exports` text | 1 |
| `ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts` (new) | Table tests over the helper (spec §6.1) | 1 |
| `ts/packages/paigasus-console-core/testing/tls-terminator.ts` | Use the helper in `forward()` (Task 2) and in `tunnel()` plus the 101 (Task 3) | 2, 3 |
| `ts/packages/paigasus-console-core/tests/unit/terminator-hop-by-hop.test.ts` (new) | Terminator normal-path behaviour tests (spec §6.2) | 2 |
| `ts/packages/paigasus-console-core/tests/unit/terminator-upgrade.test.ts` | Upgrade request, 101 response and Firefox cases | 3 |
| `ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts` | Use the helper | 4 |
| `ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts` | Forwarder behaviour tests (spec §6.3) | 4 |
| `ts/tooling/dev-stack.ts` | Use the helper; fix the `ts/moon.yml` citation | 5 |
| `ts/packages/paigasus-console-core/tests/unit/dev-stack-hop-by-hop-wiring.test.ts` (new) | Text pin (spec §6.4) | 5 |
| `ts/packages/paigasus-console-core/moon.yml` | `test` input `/ts/tooling/dev-stack.ts` | 5 |

Task 6 is the whole-branch verification: AC4 grep, the mutation battery, the affected set, the three Docker tiers, and the full `ci-targets` command.

## Decisions taken while writing this plan (unattended)

- **P1. AC4 grep excludes `tests/`.** Spec §6.1 row 1 must name each of the nine fixed names, so `hop-by-hop.test.ts` spells `'proxy-connection'`. Spec AC4 permits this: "If a test file must spell that literal, change the grep to exclude `tests/` and record why." The AC4 command in Task 6 adds `--exclude-dir=tests`. Record this in the PR body.
- **P2. Token trim uses `String.prototype.trim()`.** The spec says "remove spaces and tabs at both ends". `trim()` removes a superset (all Unicode white space). A header value cannot hold CR or LF, so the difference has no effect on a real request. M3 still has a real target: remove the `.trim()` call.
- **P3. The text pin strips comment lines before it matches.** A `//` or ` * ` line that mentions `forwardableHeaders(req.headers)` must not satisfy the pin (memory: "Re-run a mutation battery whole", "a fixture inside a comment went inert").
- **P4. One extra integration row per Review Focus line** (above). The spec does not forbid them. They are additive.

---

### Task 1: The shared helper `forwardableHeaders`

**Files:**
- Create: `ts/packages/paigasus-console-core/testing/hop-by-hop.ts`
- Create: `ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts`
- Modify: `ts/packages/paigasus-console-core/testing/index.ts:3-11` (comment) and append one export line
- Modify: `ts/packages/paigasus-console-core/package.json:3` (`_comment_exports`)

**Interfaces:**
- Consumes: nothing.
- Produces: `export function forwardableHeaders(headers: IncomingHttpHeaders): IncomingHttpHeaders` from `ts/packages/paigasus-console-core/testing/hop-by-hop.ts`, re-exported from `@paigasus/console-core/testing` (`testing/index.ts`). Tasks 2-5 import it.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// forwardableHeaders (SMA-640 spec § 6.1). Rows 3, 4 and 9 vary case and white space on purpose:
// a fixture that uses only lower case and no spaces inherits the implementer's assumption, and a
// missing case fold or trim then passes every row.
//
// The fixed list below spells 'proxy-connection'. That is why the AC4 grep in the plan excludes
// tests/ (plan decision P1).
import type { IncomingHttpHeaders } from 'node:http';
import { describe, expect, it } from 'vitest';
import { forwardableHeaders } from '../../testing/index';

const FIXED = ['connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade'] as const;

describe('forwardableHeaders', () => {
  // Row 1. One named row per fixed name, so a deletion from the set reds a row that says which.
  it.each(FIXED)('removes the fixed hop-by-hop field %s', (name) => {
    const out = forwardableHeaders({ [name]: 'v', 'x-kept': '1' });
    expect(out).not.toHaveProperty(name);
    expect(out['x-kept']).toBe('1');
  });

  // Row 2.
  it('removes a field that Connection nominates and keeps the others', () => {
    const out = forwardableHeaders({ connection: 'x-internal', 'x-internal': '1', 'x-other': '2' });
    expect(out).not.toHaveProperty('x-internal');
    expect(out['x-other']).toBe('2');
  });

  // Row 3.
  it('folds case and strips spaces and tabs around each nominated token', () => {
    const out = forwardableHeaders({ connection: ' X-Internal ,\tX-Other ', 'x-internal': '1', 'x-other': '2', 'x-kept': '3' });
    expect(out).not.toHaveProperty('x-internal');
    expect(out).not.toHaveProperty('x-other');
    expect(out['x-kept']).toBe('3');
  });

  // Row 4. Node joins repeated Connection lines with ", " and keeps the sender's case and spaces
  // (spec § 6.6).
  it('reads every token of joined Connection lines', () => {
    const out = forwardableHeaders({ connection: 'x-a, X-B , close', 'x-a': '1', 'x-b': '2', 'x-kept': '3' });
    expect(out).not.toHaveProperty('x-a');
    expect(out).not.toHaveProperty('x-b');
    expect(out['x-kept']).toBe('3');
  });

  // Row 5. @types/node types `connection` as `string | undefined`, so an array needs a cast. The
  // helper accepts one as defence in depth, for a caller that builds headers by hand.
  it('reads a Connection value that is an array', () => {
    const headers = { connection: ['x-a', 'X-B'], 'x-a': '1', 'x-b': '2', 'x-kept': '3' } as unknown as IncomingHttpHeaders;
    const out = forwardableHeaders(headers);
    expect(out).not.toHaveProperty('x-a');
    expect(out).not.toHaveProperty('x-b');
    expect(out['x-kept']).toBe('3');
  });

  // Row 6. NOT a mutation target: no header name is empty, so no change to token handling can make
  // this row fail. It checks only that the helper does not throw and keeps every other field.
  it('does not throw on empty tokens and keeps every other field', () => {
    const out = forwardableHeaders({ connection: ' , ,', 'x-a': '1', host: 'h' });
    expect(out).toEqual({ 'x-a': '1', host: 'h' });
  });

  // Row 7.
  it('drops undefined values and passes a set-cookie array through unchanged', () => {
    const cookies = ['a=1; Path=/', 'b=2; Path=/'];
    const out = forwardableHeaders({ 'x-gone': undefined, 'set-cookie': cookies });
    expect(out).not.toHaveProperty('x-gone');
    expect(out['set-cookie']).toEqual(cookies);
  });

  // Row 8.
  it('does not change the input object', () => {
    const input: IncomingHttpHeaders = { connection: 'x-a', 'x-a': '1', 'keep-alive': 'timeout=5' };
    const before = structuredClone(input);
    forwardableHeaders(input);
    expect(input).toEqual(before);
  });

  // Row 9, decision D6. In Node the outgoing headers object IS the request framing, so removing a
  // nominated content-length would let a GET body reach the upstream as a second request (spec § 6.5).
  it('keeps content-length when Connection nominates it', () => {
    expect(forwardableHeaders({ connection: 'content-length', 'content-length': '3' })['content-length']).toBe('3');
  });

  it('keeps content-length and still removes the other tokens when the nomination varies case', () => {
    const out = forwardableHeaders({ connection: 'Content-Length, x-a', 'content-length': '3', 'x-a': '1' });
    expect(out['content-length']).toBe('3');
    expect(out).not.toHaveProperty('x-a');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/hop-by-hop.test.ts`
Expected: FAIL. The file does not load: `forwardableHeaders` is not exported from `../../testing/index` (a `SyntaxError`/"does not provide an export named" or "is not a function").

- [ ] **Step 3: Write the helper**

Create `ts/packages/paigasus-console-core/testing/hop-by-hop.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The hop-by-hop rule that every in-process proxy in ts/ shares (SMA-640): the TLS terminator in
// this directory, gateway-console's counting forwarder, and ts/tooling/dev-stack.ts's default-zone
// proxy. It exists ONCE so that a later change cannot reach only one of them. SMA-640 replaced
// three copies of a fixed name list; the copies did not remove the fields that a `Connection`
// header names.
import type { IncomingHttpHeaders } from 'node:http';

/**
 * RFC 9110 § 7.6.1 hop-by-hop fields, plus proxy-authenticate / proxy-authorization. RFC 9110
 * § 11.7 does not call the last two hop-by-hop (a proxy MAY relay credentials); RFC 2616
 * § 13.5.1 did. These proxies never relay proxy credentials, so they are removed by policy. Do not
 * remove them from this set on the argument that RFC 9110 does not list them.
 */
const HOP_BY_HOP: ReadonlySet<string> = new Set(['connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade']);

/**
 * Names that a Connection nomination never removes (SMA-640 decision D6). A deliberate deviation
 * from RFC 9110 § 7.6.1: in Node the outgoing headers object IS the message framing, so removing
 * a nominated content-length from a GET makes Node write the body with no framing, and the
 * upstream reads it as a second, pipelined request (measured, SMA-640 spec § 6.5).
 */
const NEVER_NOMINATED: ReadonlySet<string> = new Set(['content-length']);

/**
 * The headers a proxy may forward: every entry except the fixed hop-by-hop fields, the fields that
 * the `Connection` header names (RFC 9110 § 7.6.1), and `undefined` values. Use it for the request
 * AND the response. Pure and total: it never throws (both proxy files call it from event callbacks,
 * where a throw kills the process), it never logs, and it returns a new object.
 */
export function forwardableHeaders(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  // Node joins repeated Connection lines into one string with ", ". The array branch is defence in
  // depth for a caller that builds headers by hand; @types/node does not type one.
  const connection = headers.connection as string | readonly string[] | undefined;
  const values: readonly string[] = typeof connection === 'string' ? [connection] : (connection ?? []);
  const nominated = new Set<string>();
  for (const value of values) {
    for (const token of value.split(',')) {
      // Node keeps the sender's case and spaces, so both steps are needed. An empty token needs no
      // special case: no header name is empty, so it removes nothing.
      const name = token.trim().toLowerCase();
      if (!NEVER_NOMINATED.has(name)) nominated.add(name);
    }
  }
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    // Node lower-cases header names in req.headers and res.headers, so the comparison is exact.
    if (value === undefined || HOP_BY_HOP.has(name) || nominated.has(name)) continue;
    out[name] = value;
  }
  return out;
}
```

If `ts:lint` later reports `@typescript-eslint/no-unnecessary-type-assertion` on the `as` line, do not delete the array branch. Change the line to `const connection: unknown = headers.connection;` and the next line to `const values: readonly string[] = typeof connection === 'string' ? [connection] : Array.isArray(connection) ? connection.filter((v): v is string => typeof v === 'string') : [];`.

- [ ] **Step 4: Export it and update the two comments**

In `ts/packages/paigasus-console-core/testing/index.ts`, replace lines 3-5:

```ts
// The testing surface. OUTSIDE src/ and deliberately NOT server-only guarded: vitest and Playwright
// harnesses import it outside a Next server, and so does `ts/tooling/dev-stack.ts` (SMA-641), a
// developer-facing command rather than a test — `dev-env.ts` and `dev-world.ts` exist only for it.
```

with:

```ts
// The testing surface. OUTSIDE src/ and deliberately NOT server-only guarded: vitest and Playwright
// harnesses import it outside a Next server, and so does `ts/tooling/dev-stack.ts` (SMA-641), a
// developer-facing command rather than a test — `dev-env.ts` and `dev-world.ts` exist only for it.
// It also holds `hop-by-hop.ts`, the one hop-by-hop rule that every in-process proxy in ts/ shares:
// the TLS terminator here, gateway-console's counting forwarder, and dev-stack.ts (SMA-640).
```

Insert one line directly after the `export { startFakeIdp, type FakeIdp } from './fake-idp';` line, so the exports stay in module-path order:

```ts
export { forwardableHeaders } from './hop-by-hop';
```

In `ts/packages/paigasus-console-core/package.json`, change the end of the `_comment_exports` value from:

```
…and so does ts/tooling/dev-stack.ts (SMA-641), a developer-facing command rather than a test."
```

to:

```
…and so does ts/tooling/dev-stack.ts (SMA-641), a developer-facing command rather than a test. ./testing also holds forwardableHeaders, the one hop-by-hop rule that the TLS terminator, gateway-console's counting forwarder and dev-stack.ts share (SMA-640)."
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/hop-by-hop.test.ts`
Expected: PASS, 18 tests (9 + 9).

- [ ] **Step 6: Prove the helper rows bite (M1, M2, M3, M4, M5, M12)**

Run each mutation alone with the Edit tool, run the Step 5 command, record the red rows, then make the exact reverse edit and confirm `git diff -- ts/packages/paigasus-console-core/testing/hop-by-hop.ts` shows only the Task 1 content (no mutation).

| # | Edit in `hop-by-hop.ts` | Expected red rows |
|---|---|---|
| M1 | Replace `if (!NEVER_NOMINATED.has(name)) nominated.add(name);` with `void name;` | rows 2, 3, 4, 5, and row 9b (`x-a`) |
| M2 | Replace `token.trim().toLowerCase()` with `token.trim()` | rows 3, 4 |
| M3 | Replace `token.trim().toLowerCase()` with `token.toLowerCase()` | rows 3, 4 |
| M4 | Delete `'proxy-authorization', ` from `HOP_BY_HOP` | row 1 `proxy-authorization` |
| M5 | Delete `'proxy-authenticate', ` from `HOP_BY_HOP` | row 1 `proxy-authenticate` |
| M12 | Replace `if (!NEVER_NOMINATED.has(name)) nominated.add(name);` with `nominated.add(name);` (the base tsconfig has no `noUnusedLocals`, so the unused constant still compiles) | rows 9a and 9b |

Run `pnpm --dir ts exec tsc -p packages/paigasus-console-core/tsconfig.json --noEmit` once per mutation to prove it compiles. Write each result (mutation, red row names) into the task report. Tasks 2-4 re-run M1, M4, M5 and M12 against the integration rows; Task 6 runs the whole battery again.

- [ ] **Step 7: Format, type-check, lint the package**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:fmt paigasus-console-core-ts:typecheck ts:lint`
Expected: all pass. If `ts:fmt` fails, run `pnpm --dir ts exec prettier --write <the files of this task>` and re-run.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-640-connection-nominated-headers
git add ts/packages/paigasus-console-core/testing/hop-by-hop.ts ts/packages/paigasus-console-core/testing/index.ts ts/packages/paigasus-console-core/package.json ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts
git commit -m "feat(ts): shared forwardableHeaders removes Connection-nominated fields (SMA-640)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The terminator's `forward()` uses the helper

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/tls-terminator.ts:13` (import), `:19-33` (delete the local copy), `:103`, `:106`
- Create: `ts/packages/paigasus-console-core/tests/unit/terminator-hop-by-hop.test.ts`

**Interfaces:**
- Consumes: `forwardableHeaders(headers: IncomingHttpHeaders): IncomingHttpHeaders` from `./hop-by-hop` (Task 1).
- Produces: nothing new. `startTlsTerminator` keeps its signature.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/unit/terminator-hop-by-hop.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-640 spec § 6.2: the terminator's forward() removes the fields that a `Connection` header
// nominates (RFC 9110 § 7.6.1) and the two proxy-auth fields, in both directions. The upgrade path
// is in terminator-upgrade.test.ts.
//
// This file has its OWN echo upstream. The iam-console terminator test's upstream echoes a fixed
// field set and checks it with toEqual, so it cannot carry these cases (spec § 6.2).
import { createServer, type IncomingHttpHeaders, type Server } from 'node:http';
import { request as httpsRequest } from 'node:https';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { startTlsTerminator, testTls } from '../../testing/index';

const tls = testTls();
let upstream: Server;
let terminator: { origin: string; close(): Promise<void> };

type Reply = { status: number; headers: IncomingHttpHeaders; body: string };

function send(path: string, headers: Record<string, string>): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const req = httpsRequest(new URL(path, terminator.origin), { headers, ca: tls.cert }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    req.end();
  });
}

beforeAll(async () => {
  upstream = createServer((req, res) => {
    if (req.url === '/nominating-response') {
      // A node:http server sends a caller-set Connection value verbatim (measured, spec § 6.6).
      res.setHeader('set-cookie', ['a=1; Path=/', 'b=2; Path=/']);
      res.writeHead(200, { connection: 'X-Resp', 'x-resp': '1', 'x-resp-kept': '1', 'proxy-authenticate': 'Basic realm="x"', 'content-type': 'text/plain' });
      res.end('ok');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(req.headers));
  });
  await new Promise<void>((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  const { port } = upstream.address() as AddressInfo;
  terminator = await startTlsTerminator({ tls, target: `http://127.0.0.1:${String(port)}` });
});

afterAll(async () => {
  await terminator.close();
  await new Promise<void>((resolve) => {
    upstream.closeAllConnections();
    upstream.close(() => resolve());
  });
});

describe('the terminator removes Connection-nominated fields', () => {
  it('in the request direction, and removes proxy-authorization', async () => {
    const reply = await send('/echo', { connection: 'x-internal', 'x-internal': '1', 'x-kept': '1', 'proxy-authorization': 'Basic eA==' });
    const seen = JSON.parse(reply.body) as Record<string, string>;
    expect(seen['x-kept']).toBe('1');
    expect(seen).not.toHaveProperty('x-internal');
    expect(seen).not.toHaveProperty('proxy-authorization');
  });

  it('in the response direction, removes proxy-authenticate, and keeps every set-cookie value', async () => {
    const reply = await send('/nominating-response', {});
    expect(reply.status).toBe(200);
    expect(reply.headers['x-resp-kept']).toBe('1');
    expect(reply.headers).not.toHaveProperty('x-resp');
    expect(reply.headers).not.toHaveProperty('proxy-authenticate');
    expect(reply.headers['set-cookie']).toEqual(['a=1; Path=/', 'b=2; Path=/']);
  });

  // Review Focus 4, decision D3. forward() sets host and x-forwarded-* AFTER the filtered spread,
  // so a nomination of them has no effect here. A refactor that filters the merged object would
  // drop them and reds this row.
  it("keeps the terminator's own host and x-forwarded-host when the client nominates them", async () => {
    const reply = await send('/echo', { connection: 'Host, X-Forwarded-Host' });
    const seen = JSON.parse(reply.body) as Record<string, string>;
    const publicHost = new URL(terminator.origin).host;
    expect(seen.host).toBe(publicHost);
    expect(seen['x-forwarded-host']).toBe(publicHost);
    expect(seen['x-forwarded-proto']).toBe('https');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/terminator-hop-by-hop.test.ts`
Expected: FAIL on the request case (`x-internal` and `proxy-authorization` present) and on the response case (`x-resp` and `proxy-authenticate` present). The D3 case passes already; that is correct (it pins existing behaviour).

- [ ] **Step 3: Wire the helper into `tls-terminator.ts`**

Change line 13 from:

```ts
import { request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
```

to:

```ts
import { request as httpRequest, type IncomingMessage, type ServerResponse } from 'node:http';
```

Add after line 17 (`import type { TlsMaterial } from './tls';`):

```ts
import { forwardableHeaders } from './hop-by-hop';
```

Delete lines 19-33 (the `HOP_BY_HOP` doc comment, the `HOP_BY_HOP` constant, and the whole `forwardable()` function) and put in their place:

```ts
// Hop-by-hop filtering, in both directions and on the upgrade path, is `forwardableHeaders()` in
// ./hop-by-hop.ts. It is the one copy of the rule: gateway-console's counting forwarder and
// ts/tooling/dev-stack.ts import the same function from `@paigasus/console-core/testing` (SMA-640).
```

In `forward()`, change line 103:

```ts
        headers: { ...forwardable(req.headers), host, 'x-forwarded-proto': 'https', 'x-forwarded-host': host, 'x-forwarded-port': host.split(':')[1] ?? '443' },
```

to:

```ts
        headers: { ...forwardableHeaders(req.headers), host, 'x-forwarded-proto': 'https', 'x-forwarded-host': host, 'x-forwarded-port': host.split(':')[1] ?? '443' },
```

and line 106:

```ts
        res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
```

to:

```ts
        res.writeHead(upstreamRes.statusCode ?? 502, forwardableHeaders(upstreamRes.headers));
```

In `tunnel()`, change the one remaining `...forwardable(req.headers),` to `...forwardableHeaders(req.headers),` and change its comment `// forwardable() strips these two` to `// forwardableHeaders() strips these two`. (Task 3 adds the tests for `tunnel()`. This rename only keeps the file compiling.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/terminator-hop-by-hop.test.ts tests/unit/terminator-upgrade.test.ts tests/unit/hop-by-hop.test.ts`
Expected: PASS (all files).

- [ ] **Step 5: Prove the call sites bite (M6, M7, and M1/M4/M5 on the integration rows)**

One at a time, with the Edit tool; run the Step 4 command; record; make the reverse edit; confirm with `git diff`.

| # | Edit | Expected red |
|---|---|---|
| M6 | Line in `forward()`: `...forwardableHeaders(req.headers),` → `...req.headers,` | request case |
| M7 | `forwardableHeaders(upstreamRes.headers)` in `forward()` → `upstreamRes.headers` | response case |
| M1 | As in Task 1 (in `hop-by-hop.ts`) | request and response cases |
| M4 | As in Task 1 | request case |
| M5 | As in Task 1 | response case |

`res.writeHead(status, upstreamRes.headers)` compiles (`OutgoingHttpHeaders` accepts `IncomingHttpHeaders`). Check each mutation with `pnpm --dir ts exec tsc -p packages/paigasus-console-core/tsconfig.json --noEmit`.

- [ ] **Step 6: Format, type-check, lint**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:fmt paigasus-console-core-ts:typecheck ts:lint`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-640-connection-nominated-headers
git add ts/packages/paigasus-console-core/testing/tls-terminator.ts ts/packages/paigasus-console-core/tests/unit/terminator-hop-by-hop.test.ts
git commit -m "fix(ts): the TLS terminator removes Connection-nominated fields in forward() (SMA-640)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The terminator's upgrade path and its hand-written 101

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/tls-terminator.ts` (`tunnel()`: the request headers, and the `lines` loop of the 101, today lines 159-165)
- Modify: `ts/packages/paigasus-console-core/tests/unit/terminator-upgrade.test.ts` (fixture options plus three cases)

**Interfaces:**
- Consumes: `forwardableHeaders` (Task 1); the import in `tls-terminator.ts` from Task 2.
- Produces: nothing new.

- [ ] **Step 1: Extend the two fixtures**

In `terminator-upgrade.test.ts`, change the first import line to:

```ts
import { createServer, type IncomingHttpHeaders } from 'node:http';
```

Replace the `upgradeUpstream` signature and its `server.on('upgrade', …)` start, so that it records each upgrade request's headers and can send a chosen `Connection` value and extra lines in the 101. Replace:

```ts
async function upgradeUpstream(opts: { trailer?: string } = {}): Promise<{ url: string; seen: string[] }> {
  const seen: string[] = [];
```

with:

```ts
async function upgradeUpstream(opts: { trailer?: string; connection?: string; extraLines?: readonly string[] } = {}): Promise<{ url: string; seen: string[]; requests: IncomingHttpHeaders[] }> {
  const seen: string[] = [];
  const requests: IncomingHttpHeaders[] = [];
```

Directly after the line `  server.on('upgrade', (req, socket: Duplex, head: Buffer) => {` (keep that line; `req` is inferred as `IncomingMessage`), insert:

```ts
    requests.push(req.headers);
```

Replace the 101 write:

```ts
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: test-accept\r\n\r\n${opts.trailer ?? ''}`);
```

with:

```ts
    const extra = (opts.extraLines ?? []).map((line) => `${line}\r\n`).join('');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: ${opts.connection ?? 'Upgrade'}\r\n${extra}sec-websocket-accept: test-accept\r\n\r\n${opts.trailer ?? ''}`);
```

Change the return line `return { url: \`http://127.0.0.1:${String(port)}\`, seen };` to:

```ts
  return { url: `http://127.0.0.1:${String(port)}`, seen, requests };
```

Change the `handshake` signature and the request write. Replace:

```ts
function handshake(origin: string, path: string, body = ''): Promise<{ head: string; socket: Duplex }> {
```

with:

```ts
function handshake(origin: string, path: string, body = '', connection = 'Upgrade', extraLines: readonly string[] = []): Promise<{ head: string; socket: Duplex }> {
```

and replace the `socket.write(` line inside it with:

```ts
      const extra = extraLines.map((line) => `${line}\r\n`).join('');
      socket.write(`GET ${path} HTTP/1.1\r\nhost: ${url.host}\r\nconnection: ${connection}\r\nupgrade: websocket\r\n${extra}sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n${body}`);
```

Add one sentence to the `handshake` doc comment: `\`connection\` and \`extraLines\` let a test send a nominating Connection value (SMA-640).`

- [ ] **Step 2: Write the three failing cases**

Append inside the `describe('the terminator tunnels a WebSocket upgrade', …)` block, after the last `it`:

```ts
  // SMA-640 spec § 6.2: the handshake request removes a nominated field, and still carries
  // Connection: Upgrade to the upstream.
  it('removes a field that the handshake Connection nominates', async () => {
    const back = await upgradeUpstream();
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(() => terminator.close());

    const { head, socket } = await handshake(terminator.origin, '/iam/_next/hmr', '', 'Upgrade, X-Foo', ['x-foo: 1', 'x-kept: 1']);
    expect(head).toContain('101');
    expect(back.requests).toHaveLength(1);
    const seen = back.requests[0] ?? {};
    expect(seen).not.toHaveProperty('x-foo');
    expect(seen['x-kept']).toBe('1');
    expect(seen.connection).toBe('Upgrade');
    expect(seen.upgrade).toBe('websocket');
    socket.destroy();
  });

  // SMA-640 spec § 6.2: the hand-written 101 removes a nominated field, and still carries
  // Connection: Upgrade, Upgrade and Sec-WebSocket-Accept to the client.
  it('removes a field that the upstream 101 Connection nominates', async () => {
    const back = await upgradeUpstream({ connection: 'Upgrade, X-Up', extraLines: ['x-up: 1'] });
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(() => terminator.close());

    const { head, socket } = await handshake(terminator.origin, '/iam/_next/hmr');
    const lines = head.split('\r\n\r\n')[0]?.toLowerCase().split('\r\n') ?? [];
    expect(lines[0]).toContain('101');
    expect(lines.some((line) => line.startsWith('x-up:'))).toBe(false);
    expect(lines).toContain('connection: upgrade');
    expect(lines).toContain('upgrade: websocket');
    expect(lines).toContain('sec-websocket-accept: test-accept');
    socket.destroy();
  });

  // Review Focus 3. Firefox sends `Connection: keep-alive, Upgrade`. Both tokens are in the fixed
  // set, and tunnel() puts Connection and Upgrade back, so the tunnel still opens.
  it('still tunnels a handshake whose Connection is keep-alive, Upgrade', async () => {
    const back = await upgradeUpstream();
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(() => terminator.close());

    const { head, socket } = await handshake(terminator.origin, '/iam/_next/hmr', '', 'keep-alive, Upgrade');
    expect(head).toContain('101 Switching Protocols');
    expect(back.requests[0]?.connection).toBe('Upgrade');
    const echoed = new Promise<string>((resolve) => socket.once('data', (chunk: Buffer) => resolve(chunk.toString('utf8'))));
    socket.write('ping');
    await expect(echoed).resolves.toEqual('echo:ping');
    socket.destroy();
  });
```

- [ ] **Step 3: Run the tests to verify the right ones fail**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/terminator-upgrade.test.ts`
Expected: the five existing cases PASS. "removes a field that the handshake Connection nominates" PASSES already, because Task 2 Step 3 already changed `tunnel()` to `forwardableHeaders(req.headers)`. "removes a field that the upstream 101 Connection nominates" FAILS (`x-up: 1` is in the 101, and `connection: upgrade, x-up` is not the line `connection: upgrade`). The Firefox case PASSES (it pins existing behaviour).

If the 101 case fails for a different reason (for example Node's client does not emit `'upgrade'` for a 101 whose Connection is `Upgrade, X-Up`, so the handshake rejects with "closed with no response"), stop. Record the measurement and report it as a blocker: the spec assumes that the upgrade event fires.

To prove the request case is not vacuous, run M8 now: in `tunnel()` change `...forwardableHeaders(req.headers),` to `...req.headers,`, run the command, expect "removes a field that the handshake Connection nominates" to FAIL (`x-foo` present), then make the reverse edit and confirm with `git diff`.

- [ ] **Step 4: Filter the hand-written 101**

In `tls-terminator.ts`, in `tunnel()`'s `upstream.on('upgrade', …)` handler, replace:

```ts
      const lines = [`HTTP/1.1 ${String(upstreamRes.statusCode ?? 101)} ${upstreamRes.statusMessage ?? 'Switching Protocols'}`];
      for (const [name, value] of Object.entries(upstreamRes.headers)) {
```

with:

```ts
      const lines = [`HTTP/1.1 ${String(upstreamRes.statusCode ?? 101)} ${upstreamRes.statusMessage ?? 'Switching Protocols'}`];
      // The same rule as the request above (SMA-640): filter, then put Connection and Upgrade
      // back, because the handshake response needs both. Sec-WebSocket-Accept and the other
      // handshake fields are not hop-by-hop, so they still reach the browser.
      const responseHeaders = { ...forwardableHeaders(upstreamRes.headers), connection: 'Upgrade', upgrade: upstreamRes.headers.upgrade ?? 'websocket' };
      for (const [name, value] of Object.entries(responseHeaders)) {
```

Leave the rest of the loop (`if (value === undefined) continue;` and the `Array.isArray` line) unchanged.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/terminator-upgrade.test.ts tests/unit/terminator-hop-by-hop.test.ts`
Expected: PASS, all cases (8 in `terminator-upgrade.test.ts`).

- [ ] **Step 6: Prove the 101 filter bites (M11)**

With the Edit tool, change `{ ...forwardableHeaders(upstreamRes.headers), connection: 'Upgrade', …` to `{ ...upstreamRes.headers, connection: 'Upgrade', …`. Check it compiles (`pnpm --dir ts exec tsc -p packages/paigasus-console-core/tsconfig.json --noEmit`). Run the Step 5 command. Expected: "removes a field that the upstream 101 Connection nominates" FAILS (`x-up: 1` present). Make the reverse edit and confirm with `git diff`.

- [ ] **Step 7: Format, type-check, lint**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:fmt paigasus-console-core-ts:typecheck ts:lint`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-640-connection-nominated-headers
git add ts/packages/paigasus-console-core/testing/tls-terminator.ts ts/packages/paigasus-console-core/tests/unit/terminator-upgrade.test.ts
git commit -m "fix(ts): the terminator's upgrade path and 101 remove Connection-nominated fields (SMA-640)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The counting forwarder uses the helper

**Files:**
- Modify: `ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts:23-35` (imports, delete the local copy), `:51`, `:66`
- Modify: `ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts` (imports; four cases)

**Interfaces:**
- Consumes: `forwardableHeaders` from `@paigasus/console-core/testing` (Task 1). `gateway-console` already depends on `@paigasus/console-core` (`package.json:19`), and `two-zone-harness.ts:34` already imports this subpath, so no new dependency edge appears.
- Produces: nothing new. `startCountingForwarder` keeps its signature.

- [ ] **Step 1: Write the failing tests**

In `counting-forwarder.test.ts`, change the import line 7 to:

```ts
import { Agent, createServer, request as httpRequest, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { connect as netConnect, type AddressInfo } from 'node:net';
```

and delete the old line 8 (`import type { AddressInfo } from 'node:net';`).

Append inside the `describe('the counting forwarder', …)` block, after the last `it`:

```ts
  // SMA-640 spec § 6.3. Mixed case on purpose: a fixture in lower case only inherits the
  // implementer's assumption.
  it('removes Connection-nominated fields and proxy-authorization from the request', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });

    const res = await get(`${forwarder.url}/x`, { headers: { connection: 'X-Internal, keep-alive', 'x-internal': '1', 'x-kept': '1', 'proxy-authorization': 'Basic eA==' } });
    const body = JSON.parse(res.body) as { headers: Record<string, string> };
    expect(body.headers['x-kept']).toBe('1');
    expect(body.headers).not.toHaveProperty('x-internal');
    expect(body.headers).not.toHaveProperty('proxy-authorization');
  });

  it('removes Connection-nominated fields and proxy-authenticate from the response', async () => {
    upstream = await startEcho((_req, res) => {
      res.writeHead(200, { connection: 'X-Resp', 'x-resp': '1', 'x-resp-kept': '1', 'proxy-authenticate': 'Basic realm="x"', 'content-type': 'text/plain' });
      res.end('ok');
    });
    forwarder = await startCountingForwarder({ target: upstream.url });

    const res = await get(`${forwarder.url}/x`);
    expect(res.status).toBe(200);
    expect(res.headers['x-resp-kept']).toBe('1');
    expect(res.headers).not.toHaveProperty('x-resp');
    expect(res.headers).not.toHaveProperty('proxy-authenticate');
  });

  // Review Focus 1, decision D6, at the call site. A GET is not chunked by default, so a removed
  // content-length makes Node write the body with no framing; the upstream then reads an empty GET
  // and a garbage second request (measured, spec § 6.5).
  it('keeps a nominated content-length, so a GET body keeps its framing', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });

    const res = await get(`${forwarder.url}/x`, { method: 'GET', headers: { connection: 'Content-Length' }, body: 'abc' });
    const body = JSON.parse(res.body) as { method: string; body: string };
    expect(body.method).toBe('GET');
    expect(body.body).toBe('abc');
  });

  // Review Focus 2. Only a raw socket sends two separate Connection lines; Node joins them with
  // ", " (spec § 6.6), and the helper must read every token of the joined value.
  it('removes the fields that two separate Connection lines nominate', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });
    const { hostname, port } = new URL(forwarder.url);

    const raw = await new Promise<string>((resolve, reject) => {
      const socket = netConnect({ host: hostname, port: Number(port) }, () => {
        socket.write(`GET /x HTTP/1.1\r\nhost: ${hostname}:${port}\r\nConnection: x-a\r\nConnection: X-B , close\r\nx-a: 1\r\nx-b: 2\r\nx-kept: 3\r\n\r\n`);
      });
      const chunks: Buffer[] = [];
      socket.on('data', (chunk: Buffer) => chunks.push(chunk));
      // `close` in the request makes the forwarder end the connection after the response.
      socket.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
      socket.on('error', reject);
    });
    // The forwarder answers this `close` request with a chunked body, so take the one JSON object
    // between the first `{` and the last `}` rather than everything after the header block.
    const body = JSON.parse(raw.slice(raw.indexOf('{'), raw.lastIndexOf('}') + 1)) as { headers: Record<string, string> };
    expect(body.headers['x-kept']).toBe('3');
    expect(body.headers).not.toHaveProperty('x-a');
    expect(body.headers).not.toHaveProperty('x-b');
  });
```

Note on the raw case: the echo upstream calls `writeHead` and then `end(data)`, so its body is chunked, and the forwarder relays it chunked. The body is one JSON object, and a chunk-size line holds no `{` or `}`, so the slice from the first `{` to the last `}` is exact when the object arrives in one chunk (it is small). If the parse still fails, print `raw` and record it before you change the extraction.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/apps/gateway-console exec vitest run tests/integration/doubles/counting-forwarder.test.ts`
Expected: the request case, the response case and the two-lines case FAIL (the fields are still present). The content-length case PASSES today (no nomination step exists yet, so nothing removes `content-length`); it pins D6 after Step 3.

If this command needs the built standalone server (the package's vitest `setupFiles`), and fails before any test runs, use `moon run gateway-console-ts:test` instead for every run in this task.

- [ ] **Step 3: Wire the helper into `counting-forwarder.ts`**

Change line 23:

```ts
import { createServer, request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
```

to:

```ts
import { createServer, request as httpRequest, type IncomingMessage, type ServerResponse } from 'node:http';
```

Add after line 24 (`import type { AddressInfo } from 'node:net';`):

```ts
import { forwardableHeaders } from '@paigasus/console-core/testing';
```

Delete lines 26-35 (the `HOP_BY_HOP` doc comment, the constant, and `forwardable()`), and put in their place:

```ts
// Hop-by-hop filtering in both directions is `forwardableHeaders()`, the one copy of the rule that
// the TLS terminator and ts/tooling/dev-stack.ts also use (SMA-640). It removes the fixed fields
// and every field that the `Connection` header names.
```

Change line 51's `headers: forwardable(req.headers)` to `headers: forwardableHeaders(req.headers)`, and line 66's `forwardable(upstreamRes.headers)` to `forwardableHeaders(upstreamRes.headers)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run the Step 2 command.
Expected: PASS, every case in the file (the six existing cases and the four new ones).

- [ ] **Step 5: Prove the call sites bite (M9, M10, M12 at the call site)**

One at a time, with the Edit tool; check it compiles (`pnpm --dir ts exec tsc -p apps/gateway-console/tsconfig.json --noEmit`; if that tsconfig does not include `tests/e2e/support`, use `moon run gateway-console-ts:typecheck`); run the Step 2 command; record; reverse; confirm with `git diff`.

| # | Edit | Expected red |
|---|---|---|
| M9 | line 51: `headers: forwardableHeaders(req.headers)` → `headers: req.headers` | request case, two-lines case |
| M10 | line 66: `forwardableHeaders(upstreamRes.headers)` → `upstreamRes.headers` | response case |
| M12 | As in Task 1 (in `hop-by-hop.ts`) | content-length case |

If M12 does not red the content-length case (for example Node answers the client before the garbage second request), record the observed bytes and keep the case: Task 1 row 9 still pins M12. Do not weaken the assertion.

- [ ] **Step 6: Format, type-check, lint**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:fmt gateway-console-ts:typecheck ts:lint`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-640-connection-nominated-headers
git add ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts
git commit -m "fix(ts): the counting forwarder removes Connection-nominated fields (SMA-640)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `dev-stack.ts` uses the helper, with a text pin

**Files:**
- Modify: `ts/tooling/dev-stack.ts:27` (citation), `:32` (import), `:39` (import), `:74-89` (delete the copy), `:138-139` (calls)
- Create: `ts/packages/paigasus-console-core/tests/unit/dev-stack-hop-by-hop-wiring.test.ts`
- Modify: `ts/packages/paigasus-console-core/moon.yml` (`test` inputs, after the SMA-661 entry at line 96)

**Interfaces:**
- Consumes: `forwardableHeaders` from `@paigasus/console-core/testing` (Task 1).
- Produces: nothing new.

- [ ] **Step 1: Write the failing text pin**

Create `ts/packages/paigasus-console-core/tests/unit/dev-stack-hop-by-hop-wiring.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-640 spec § 6.4. ts/tooling/dev-stack.ts cannot run in a test (Docker plus two `next dev`
// servers; its own header says so), so this file pins its default-zone proxy's call sites as TEXT:
// both directions go through the shared forwardableHeaders(), and no local copy of the rule comes
// back. moon.yml lists /ts/tooling/dev-stack.ts as an input of this package's `test` task, so an
// edit to dev-stack.ts re-runs this file rather than serving a cached pass.
//
// Comment lines are removed before any match, so a comment that names a call cannot satisfy the
// pin (plan decision P3).
//
// RESIDUAL (spec § 10): a text pin cannot prove runtime behaviour. A change that keeps these
// strings but adds a second, unfiltered httpRequest reds nothing here.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// tests/unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five `../`, the same
// depth as tests/unit/bulk-replay-ceiling.test.ts.
const REPO_ROOT = new URL('../../../../../', import.meta.url);
const DEV_STACK = 'ts/tooling/dev-stack.ts';

const source = readFileSync(fileURLToPath(new URL(DEV_STACK, REPO_ROOT)), 'utf8');
const code = source
  .split('\n')
  .filter((line) => !/^\s*(\/\/|\/\*|\*)/.test(line))
  .join('\n');

function count(needle: string): number {
  return code.split(needle).length - 1;
}

describe('dev-stack.ts uses the shared hop-by-hop rule', () => {
  it('reads a non-empty file', () => {
    expect(code.length).toBeGreaterThan(1000);
  });

  it('imports forwardableHeaders from @paigasus/console-core/testing', () => {
    expect(code).toMatch(/^import \{[^}]*\bforwardableHeaders\b[^}]*\} from '@paigasus\/console-core\/testing';$/m);
  });

  it('filters the request headers exactly once', () => {
    expect(count('forwardableHeaders(req.headers)')).toBe(1);
  });

  it('filters the response headers exactly once', () => {
    expect(count('forwardableHeaders(upstreamRes.headers)')).toBe(1);
  });

  it('keeps no local copy of the rule', () => {
    expect(code).not.toContain('HOP_BY_HOP');
    expect(code).not.toContain('function forwardable(');
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/dev-stack-hop-by-hop-wiring.test.ts`
Expected: FAIL on the import, the two counts (0), and "keeps no local copy". "reads a non-empty file" PASSES.

- [ ] **Step 3: Edit `dev-stack.ts`**

Line 27: change `` never the root project (`ts/moon.yml:28-30`). Typed ESLint rules report lint problems, `` to `` never the root project (`ts/moon.yml:31-33`). Typed ESLint rules report lint problems, ``. (Spec re-check: the "No build/typecheck/test here" block starts at `ts/moon.yml:31` on `8fff8997`. Confirm with `sed -n 31,33p ts/moon.yml` before you edit.)

Line 32: remove `type IncomingHttpHeaders, ` so the line reads:

```ts
import { createServer, request as httpRequest, type IncomingMessage, type ServerResponse } from 'node:http';
```

Line 39: add `forwardableHeaders` to the existing import:

```ts
import { buildDevEnv, DEV_GATEWAY_DESCRIPTOR, DEV_IAM_DESCRIPTOR, devWorld, forwardableHeaders, startFakeGateway, startFakeIam, startFakeIdp, startTlsTerminator, testTls } from '@paigasus/console-core/testing';
```

Delete lines 74-89 completely (the "Hop-by-hop headers … Copied from tls-terminator.ts" doc comment, `const HOP_BY_HOP …`, the blank line, and the whole `function forwardable(…) { … }`), including the blank line after the function, so one blank line stays between `log()` and the `preflight()` doc comment.

Lines 138-139 (they move up by 16 lines after the delete; find them by text):

```ts
    const upstream = httpRequest({ hostname: '127.0.0.1', port: IAM_PORT, method: req.method, path: req.url, headers: forwardable(req.headers) }, (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
```

become:

```ts
    // Hop-by-hop filtering in both directions is the shared rule in @paigasus/console-core/testing
    // (SMA-640). tests/unit/dev-stack-hop-by-hop-wiring.test.ts in that package pins these two calls.
    const upstream = httpRequest({ hostname: '127.0.0.1', port: IAM_PORT, method: req.method, path: req.url, headers: forwardableHeaders(req.headers) }, (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, forwardableHeaders(upstreamRes.headers));
```

The new comment names the call only in prose without the `(req.headers)` text, so the pin's comment filter is not what keeps the count at 1. Check: `grep -c 'forwardableHeaders(req.headers)' ts/tooling/dev-stack.ts` prints `1`.

- [ ] **Step 4: Add the Moon input**

In `ts/packages/paigasus-console-core/moon.yml`, in the `test` task `inputs`, insert after the line `      - '/rs/crates/libs/paigasus-iam-core/src/dead_letter.rs'` (line 96):

```yaml
      # SMA-640 spec § 6.4. tests/unit/dev-stack-hop-by-hop-wiring.test.ts reads ts/tooling/dev-stack.ts
      # as TEXT, to pin its default-zone proxy's two forwardableHeaders() calls. Without this input,
      # an edit to dev-stack.ts selects no paigasus-console-core-ts task, and Moon serves a cached
      # PASS on exactly the commit that reverts a call site to unfiltered headers.
      - '/ts/tooling/dev-stack.ts'
```

- [ ] **Step 5: Run the pin and the type check that reaches `dev-stack.ts`**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/dev-stack-hop-by-hop-wiring.test.ts && pnpm --dir ts exec tsc -p tooling/tsconfig.json --noEmit`
Expected: the pin PASSES (5 tests). `tsc` exits 0 with no output. No CI task type-checks `dev-stack.ts` (spec AC6), so this local `tsc` is the only type gate for it.

- [ ] **Step 6: Prove the input resolves and the pin bites (M13, M14)**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && moon task paigasus-console-core-ts:test --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print([f for f in d.get("inputFiles", []) if "dev-stack" in f])'`
Expected: a list that holds `ts/tooling/dev-stack.ts` (a missing `inputFiles` key is a failure, per memory "Moon task inputs affect without a project edge"). If the JSON shape differs, print the keys and find the resolved input file list; record what you used.

Then, one at a time, with the Edit tool in `dev-stack.ts`; run `pnpm --dir ts exec tsc -p tooling/tsconfig.json --noEmit` (must pass) and the pin; record; reverse; confirm with `git diff`:

| # | Edit | Expected red |
|---|---|---|
| M13 | `headers: forwardableHeaders(req.headers)` → `headers: req.headers` | "filters the request headers exactly once" |
| M14 | `forwardableHeaders(upstreamRes.headers)` → `upstreamRes.headers` | "filters the response headers exactly once" |

- [ ] **Step 7: Format, lint**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:fmt ts:lint paigasus-console-core-ts:typecheck`
Expected: all pass. `ts:lint` covers `tooling/**/*`.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-640-connection-nominated-headers
git add ts/tooling/dev-stack.ts ts/packages/paigasus-console-core/tests/unit/dev-stack-hop-by-hop-wiring.test.ts ts/packages/paigasus-console-core/moon.yml
git commit -m "fix(ts): dev-stack's default-zone proxy uses the shared hop-by-hop rule (SMA-640)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Whole-branch verification

**Files:** none changed, unless a check fails. A fix goes in a new commit, never an amend.

**Interfaces:**
- Consumes: the commits of Tasks 1-5.
- Produces: the evidence for the PR body (AC4 output, the mutation table with results, the affected set, the e2e and `ci-targets` results).

- [ ] **Step 1: AC4, the set exists once**

Run: `cd <worktree> && grep -rln "'proxy-connection'" ts --include='*.ts' --exclude-dir=node_modules --exclude-dir=.next --exclude-dir=tests`
Expected: exactly one line, `ts/packages/paigasus-console-core/testing/hop-by-hop.ts`. Without `--exclude-dir=tests` the command also lists `ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts`, which must spell the name (plan decision P1). Record both outputs for the PR body.

- [ ] **Step 2: Run the package test suites**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-console-core-ts:test iam-console-ts:test gateway-console-ts:test`
Expected: all pass. `iam-console-ts:test` includes the unchanged `tests/integration/doubles/tls-terminator.test.ts`, which must stay green.

- [ ] **Step 3: Type checks, format, lint**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck ts:fmt ts:lint && pnpm --dir ts exec tsc -p tooling/tsconfig.json --noEmit`
Expected: all pass, and `tsc` exits 0.

- [ ] **Step 4: The whole mutation battery (spec §6.7)**

Run every mutation again, one at a time, after the last fix (memory: "Re-run a mutation battery whole"). For each: Edit, `tsc` on the owning project (must pass), run the test command, record red rows, reverse with Edit, `git diff --stat` must be empty before the next one.

| # | File | Edit | Test command | Expected red |
|---|---|---|---|---|
| M1 | `testing/hop-by-hop.ts` | `if (!NEVER_NOMINATED.has(name)) nominated.add(name);` → `void name;` | A + B | rows 2-5, 9b; terminator request + response; forwarder request + response + two-lines |
| M2 | `testing/hop-by-hop.ts` | `token.trim().toLowerCase()` → `token.trim()` | A | rows 3, 4 |
| M3 | `testing/hop-by-hop.ts` | `token.trim().toLowerCase()` → `token.toLowerCase()` | A | rows 3, 4 |
| M4 | `testing/hop-by-hop.ts` | delete `'proxy-authorization', ` | A + B | row 1; terminator request; forwarder request |
| M5 | `testing/hop-by-hop.ts` | delete `'proxy-authenticate', ` | A + B | row 1; terminator response; forwarder response |
| M6 | `testing/tls-terminator.ts` | `forward()`: `...forwardableHeaders(req.headers),` → `...req.headers,` | A | terminator request |
| M7 | `testing/tls-terminator.ts` | `forward()`: `forwardableHeaders(upstreamRes.headers)` → `upstreamRes.headers` | A | terminator response |
| M8 | `testing/tls-terminator.ts` | `tunnel()`: `...forwardableHeaders(req.headers),` → `...req.headers,` | A | upgrade request case |
| M9 | `counting-forwarder.ts` | `headers: forwardableHeaders(req.headers)` → `headers: req.headers` | B | forwarder request, two-lines |
| M10 | `counting-forwarder.ts` | `forwardableHeaders(upstreamRes.headers)` → `upstreamRes.headers` | B | forwarder response |
| M11 | `testing/tls-terminator.ts` | 101: `...forwardableHeaders(upstreamRes.headers),` → `...upstreamRes.headers,` | A | upgrade response case |
| M12 | `testing/hop-by-hop.ts` | `if (!NEVER_NOMINATED.has(name)) nominated.add(name);` → `nominated.add(name);` | A + B | rows 9a, 9b; forwarder content-length |
| M13 | `ts/tooling/dev-stack.ts` | `headers: forwardableHeaders(req.headers)` → `headers: req.headers` | A | pin: request once |
| M14 | `ts/tooling/dev-stack.ts` | `forwardableHeaders(upstreamRes.headers)` → `upstreamRes.headers` | A | pin: response once |

Test command A: `pnpm --dir ts/packages/paigasus-console-core exec vitest run tests/unit/hop-by-hop.test.ts tests/unit/terminator-hop-by-hop.test.ts tests/unit/terminator-upgrade.test.ts tests/unit/dev-stack-hop-by-hop-wiring.test.ts`
Test command B: `pnpm --dir ts/apps/gateway-console exec vitest run tests/integration/doubles/counting-forwarder.test.ts` (or `moon run gateway-console-ts:test`, per Task 4 Step 2).

If the permission system refuses a mutation run, restore the file, write the exact manual steps for that mutation under a "Mutation proof pending" heading for the PR body, and continue. Save the results table in the scratchpad for the PR body. Do not write a report file into the repo.

- [ ] **Step 5: The affected set**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && git diff --name-only origin/main...HEAD | moon query tasks --affected | python3 -c 'import json,sys; d=json.load(sys.stdin); print(sorted(f"{p}:{t}" for p,ts in d["tasks"].items() for t in ts))'`
Expected (one target per `tasks[project][task]`; never grep `"target"`): the list holds at least `paigasus-console-core-ts:test`, `paigasus-console-core-ts:typecheck`, `paigasus-console-core-ts:build`, `paigasus-console-core-ts:test-e2e`, `iam-console-ts:test`, `iam-console-ts:test-e2e`, `gateway-console-ts:test`, `gateway-console-ts:test-e2e`, `ts:lint`, `ts:fmt`. If the JSON shape differs, print its top-level keys, adapt the parse, and record the command you used. Record the list for the PR body.

- [ ] **Step 6: The three Docker-backed e2e tiers**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && moon run paigasus-console-core-ts:test-e2e iam-console-ts:test-e2e gateway-console-ts:test-e2e`
Expected: all green with no test change (spec §7: browsers send only `keep-alive` or `Upgrade` tokens, and the Next upstreams nominate no other fields). Run it in the foreground with a timeout of 600000 ms; if it needs longer, run the three targets one at a time. Known flakes that pass on a re-run (memory): `ERR_NETWORK_CHANGED`, `Port 24678 is already in use`, R2 `discovery.probe_failed`, and "React never hydrated" under IAM load. Before any re-run, capture `.moon/cache/ciReport.json` and the task's `.moon/cache/states/<project>/test-e2e/` to the scratchpad (root `CLAUDE.md` Step 0). <!-- moon-diagnosis:ok -->

- [ ] **Step 7: The full `ci-targets` command**

Run (foreground, timeout 600000 ms; re-run with the same command if it times out, Moon caches finished tasks):

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :moon-diagnosis-exec :test-e2e --base origin/main --include-relations
```

Before you run it, compare this target list with the one between the `ci-targets` markers in the root `CLAUDE.md`; if they differ, use the `CLAUDE.md` list. Expected: green, or only a red that the root `CLAUDE.md` "This development Mac only" section names as a bash-version or small-pipe artifact. For each such red, re-run that gate directly with the right bash (`/opt/homebrew/bin/bash ci/<gate>/run.sh` for the bash-4+ gates; `/bin/bash` for `repo:affected-smoke`), or in a Linux container if the host is in the 512-byte-pipe state (memory "Small pipe: run gates in a Linux container"), and record that verdict. No `repo:*` gate is expected to change: this branch changes only `ts/` test and tooling code. For any other red, follow the root `CLAUDE.md` diagnosis procedure (Step 0 first) and fix in a new commit.

- [ ] **Step 8: Confirm a clean tree**

Run: `cd <worktree> && git status --short && git log --oneline origin/main..HEAD`
Expected: no modified tracked files (no mutation left behind), and six commits: the spec, this plan, and Tasks 1-5 (plus any fix commits).

---

## Self-review against the spec

- G1 / AC1 (both directions, normal and upgrade paths, the 101): Tasks 2, 3, 4, 5. G2 / AC2 (proxy-auth): Task 1 row 1, Task 2 and Task 4 cases. G3 / AC4 (one definition): Tasks 1-5, Task 6 Step 1. G4 (behaviour tests, text pin): Tasks 2-5. G5 / AC7 (content-length): Task 1 row 9, Task 4 content-length case. AC3 (upgrade still works): Task 3 Steps 3 and 5. AC5 (mutations): Task 6 Step 4. AC6 (fmt, lint, typecheck, tooling tsc, ci-targets): Task 6 Steps 3 and 7.
- Spec §4.3 comments: `testing/index.ts` and `package.json` (Task 1), the terminator doc comment (Task 2), the `dev-stack.ts` copy comment and the `ts/moon.yml` citation (Task 5).
- Spec §6.4 moon input: Task 5 Step 4, proven in Step 6.
- Names: `forwardableHeaders` everywhere; `HOP_BY_HOP` and `NEVER_NOMINATED` only inside `hop-by-hop.ts`.
