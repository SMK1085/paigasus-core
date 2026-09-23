# SMA-635 Gateway Interactive-User Auth and Console Playground Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A console user calls the gateway chat surface with the user's own OIDC bearer, and the gateway console gets a streaming playground page that the top test tier proves against the current gateway binary.

**Architecture:** `require_iam_auth` keeps its API-key leg and gets a second OIDC leg, which resolves one organization (from a `paigasus-org` header, or inferred) with a pure domain function over the kernel `Prn` API and runs the D9 self-query against that organization. The console adds a Next route handler (`POST /gateway/api/chat`) that relays the gateway's SSE stream unchanged and injects one `paigasus-error` event on a failure, and a client `Playground` component that reads the stream. A third Playwright project runs the real `paigasus-gateway` binary, a fake IAM and a mock OpenAI server.

**Tech Stack:** Rust (axum, tonic, tonic-types, tracing, `paigasus-kernel`), protobuf + buf, TypeScript (Next.js 16.3.4, React 19, zod 4, vitest 5, Playwright 1.63), Moon 2.5.3, proto 0.61.1.

**Spec:** `docs/superpowers/specs/2026-09-23-sma-635-gateway-user-auth-design.md` (revision 2).

---

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth`, on branch `feature/sma-635-gateway-user-auth`. Never touch the main checkout.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). A `'use client'` file puts the SPDX line first and `'use client';` after the header comment.
- Rust crates stay on edition 2024 and rust-version 1.95 (inherited from the workspace). Warnings are errors: never stage dead code.
- Conventional commits. The scope is one of `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`, `repo`, `claude`, `workspace`. The subject is lowercase.
- No line in a commit BODY may be `#NNN` or `token: value` shaped (commitlint `footer-leading-blank`). The trailer goes in its own `-m` paragraph: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Add, do not amend. Every task makes a new commit. Never run `git commit --amend`, never `--no-verify`.
- Never run a command in the background. Stay in the foreground until the command ends.
- Rust checks, from `rs/`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` (the Moon `lint` form; it also lints test code), `cargo nextest run -p paigasus-gateway`.
- TypeScript checks use Moon targets, never `pnpm --filter <pkg> test` (a silent no-op when a package has no `test` script). `:typecheck` is a separate target from `:test`. `ts:fmt` (Prettier) is a separate whole-tree gate: fix with `pnpm -C ts exec prettier --write <paths>`.
- For a fast red/green loop on one vitest file, use `pnpm -C ts/apps/gateway-console exec vitest run <file>` (or the package's own directory). The Moon target is still the task's final check.
- Error codes: `invalid-org-header` = 309 and `org-required` = 310, both HTTP 400, both `param: "paigasus-org"`. Rust names `ErrorReason::InvalidOrgHeader` and `ErrorReason::OrgRequired`. TypeScript names `ErrorReason.INVALID_ORG_HEADER` and `ErrorReason.ORG_REQUIRED`.
- The org header is `paigasus-org`. The correlation header is `paigasus-correlation-id`.
- The canonical org PRN is `prn:pgs:iam:::organization/<uuid>` (lower case), built with `Prn::build("iam", "", None, "organization", uuid)`.
- The 401 code stays `invalid-api-key` for a rejected user bearer. Its message becomes `Invalid credential.`.
- The console's gateway header timeout is `35_000` ms (`CHAT_HEADER_TIMEOUT_MS`). The console's body limit is `1_048_576` bytes (`GATEWAY_DEFAULT_MAX_REQUEST_BYTES`).
- Metric operation labels stay as the CODE spells them: `introspect` (API-key leg), `introspect_token` (OIDC leg), `authorize` (self-query). The spec's table uses other names (`introspect_api_key`, `is_authorized`); the code wins, because the Grafana panel reads the code's names.
- A registry code string (for example `"org-required"`) may appear in Rust only in `src/adapters/http/error.rs` (an `emits` file) and in test modules. `src/domain.rs` must not contain one, or `repo:error-code-single-site` reds.
- No new or edited file may name the Moon CI report file by its file name (`repo:actionlint` check 12). Say "the Moon CI report" instead.
- `gateway-console-ts:test-e2e` needs Docker (its `two-zone` project starts Redis).
- A fresh worktree has no installed dependencies. Task 1 step 1 provisions it. Without it the `commit-msg` hook fails with `commitlint not found`.
- The spec header says a Notion ADR (D1 to D5, D10) must be Accepted before implementation starts. The controller confirms this before Task 2 is dispatched. Task 1 is a measurement and may run first.

## Review Focus

Five failure modes the spec implies that no test would exercise without this list. Most likely first. Each line names the test that pins it and the task that owns that test.

1. A `paigasus-org` value with an obs-text byte (0x80-0xFF) is a valid `HeaderValue` but fails `to_str()`. It must give `400 invalid-org-header`, never a 500 or a panic. Pinned by `an_invalid_org_header_is_400_before_any_authorization` case 4 (Task 5).
2. An upper-case org UUID, in the header or in a PRN from IAM, must give the lower-case canonical PRN, and two spellings of one org count as one org. Pinned by the `resolve_org` table rows `one_uppercase` and `absent_same_org_two_spellings` (Task 2) and by `an_uppercase_org_header_queries_the_lowercase_org_prn` (Task 5).
3. A chunked request body with no `content-length` that is larger than the limit must give 413 before any gateway call. Pinned by `answers 413 for a chunked body over the limit with no content-length` (Task 10).
4. A browser cancel must reach the gateway stream through the response body, also when `request.signal` does not fire. Pinned by `a cancel of the response body cancels the gateway stream` (Task 10).
5. An inherited `RUST_LOG` or `GATEWAY_*` variable in the e2e gateway child can remove the `chat completion proxied` line or reconfigure the gateway. Pinned by `tests/unit/gateway-env.test.ts` (Task 13).

## Differences between the spec and the code (verified 2026-09-23)

- `moon.yml` of `paigasus-gateway-rs` ALREADY lists `paigasus-kernel-rs` in `dependsOn` and the kernel paths in `fileGroups.upstreams`. Task 2 changes only the comment. It removes the `ALLOW_NO_CARGO_BACKING` entry (`ci/affected-graph/cargo_moon_parity.py:74-82`).
- `uuid` is at `Cargo.toml:92` under `[dev-dependencies]`, as the spec says.
- Metric labels: see Global Constraints. `require_iam_auth` today records `("introspect", "ok")` for a non-active key (`auth.rs:64-67`); Task 5 changes that row to `denied`.
- `tests/metrics.rs` already answers `introspect_token` in `AllowedIam` (`:78-88`). Its `UnusedIam` panics on purpose and is never driven. `tests/service_info.rs` panics only in scenarios that must not reach the call. Only `tests/chat_proxy.rs:102-103` needs a real answer. The comment in `tests/service_info.rs:274-276` becomes false and is corrected.
- `chat.rs:253-262` (`the_terminal_sse_frame_carries_a_registered_code`) calls `strip_prefix("data: ")` on the frame, and `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts:44` asserts `FRAME.startsWith('data: ')`. Both break with the `\n\n` prefix. The spec named only `chat.rs:236-248`. Task 6 updates both.
- The registry count anchors are in three places, not one: `rs/crates/libs/paigasus-proto/src/error.rs:230`, `ts/packages/paigasus-proto/src/error.test.ts:60` and `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`. All move from 57 to 59 (Task 3).
- The console has no runtime setting for the gateway's `max_request_bytes`. Task 10 uses a constant equal to the gateway default (`config.rs:190`, 1 MiB). The gateway still enforces its own limit.
- The spec's `result.body.pipeThrough(transform)` cannot inject an event when the SOURCE stream errors: a `TransformStream` is errored with its source. Task 10 uses a pull-based `ReadableStream` over the SDK body reader. Its `cancel()` cancels the reader, so a browser cancel still reaches the SDK stream.
- The spec's 400 for "any other shape" is split the way the gateway splits it: an unparseable body is `invalid-request-body`, a parsed body of the wrong shape is `invalid-request-schema`.
- The e2e world's `authn.introspect` handler answers `PRINCIPAL_PRN` (`tests/e2e/support/world.ts:24,133-149`), so e2e row R25 asserts the principal `PRINCIPAL_PRN`, not `iam.principalPrnFor(token)`.
- The fake gateway serves no chat route by design (`fake-gateway.ts:3-5`), so the SSE token-leak scan cannot live in the `single-zone` project's `token-leak.spec.ts`. It is row R28 in `playground-token-leak.spec.ts`, in the `playground` project.
- There is no gateway README (`rs/crates/services/paigasus-gateway/` has none). The D3 consequence goes into the `auth.rs` module doc (Task 5) and the SDK `org` doc (Task 8). The ADR carries it too.
- `ci/affected-graph/run.sh` holds six task cases whose strict-equality sets gain `gateway-console-ts:test-e2e` when the e2e task keys on the gateway's sources (Task 12). The spec did not name them.
- `gateway-console-ts:test-e2e`'s `playground` rows need `signIn` to accept the playground harness; `tests/e2e/support/login.ts:19` takes the full single-zone `Harness`. Task 13 narrows the parameter type to `Pick<Harness, 'idp' | 'url'>`.

## File Structure

| File | Create / modify | Responsibility |
|---|---|---|
| `docs/superpowers/specs/2026-09-23-sma-635-gateway-user-auth-design.md` | modify (append §12) | Task 1 measurement results |
| `rs/crates/services/paigasus-gateway/Cargo.toml` | modify | add `paigasus-kernel`, move `uuid` (Task 2); add dev-dep `tracing-subscriber` (Task 4) |
| `rs/crates/services/paigasus-gateway/moon.yml` | modify | kernel edge comment; new task `e2e-bin` |
| `ci/affected-graph/cargo_moon_parity.py` | modify | remove the gateway `ALLOW_NO_CARGO_BACKING` entry |
| `rs/crates/services/paigasus-gateway/src/domain.rs` | modify | `CallerContext`, `Credential`, `OrgHeader`, `OrgResolutionError`, `resolve_org` |
| `contracts/proto/paigasus/common/v1/error.proto` | modify | reasons 309, 310 |
| generated bindings (`rs/.../paigasus-proto/src/generated`, `ts/packages/paigasus-proto/src/generated`, `py/packages/paigasus-proto/src/paigasus_proto/generated`) | regenerate | new enum values |
| `rs/crates/libs/paigasus-proto/src/error.rs` | modify | `EXPECTED_REASONS`, count 59 |
| `ts/packages/paigasus-proto/src/error.test.ts` | modify | count 59 |
| `ts/packages/paigasus-sdk/src/errors/presentation.ts` | modify | two `PRESENTATION` entries |
| `ts/packages/paigasus-sdk/tests/presentation.test.ts` | modify | count 59 |
| `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` | modify | two `GatewayError` variants, message, `param` doc |
| `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` | modify | widened `require_iam_auth`, `authz_error`, docs, tests |
| `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` | modify | log line, `TERMINAL_SSE_ERROR` prefix, tests |
| `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs` | modify | `introspect_token` doc |
| `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` | modify | real token leg, OIDC rows, log rows |
| `rs/crates/services/paigasus-gateway/tests/metrics.rs` | modify | OIDC metrics row |
| `rs/crates/services/paigasus-gateway/tests/service_info.rs` | modify | comment |
| `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts` | modify | frame starts with a blank line |
| `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs` | modify | two table rows |
| `ts/packages/paigasus-sdk/src/chat.ts` | modify | `ChatCallOptions` (`org`, `correlationId`) |
| `ts/packages/paigasus-sdk/tests/chat.test.ts` | modify | header rows |
| `ts/apps/gateway-console/lib/chat-stream.ts` | create | the browser's SSE parser |
| `ts/apps/gateway-console/lib/chat-route.ts` | create | the route handler factory |
| `ts/apps/gateway-console/app/api/chat/route.ts` | create | wires the real dependencies |
| `ts/apps/gateway-console/proxy.ts` | modify | `/api/chat` public path |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts` | modify | extract `loadOrganizationHead` |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx` | modify | "Playground" link |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/playground/page.tsx` | create | the playground page |
| `ts/apps/gateway-console/app/_components/playground-notice.ts` | create | the composer rule |
| `ts/apps/gateway-console/app/_components/playground.tsx` | create | the client component |
| `ts/apps/gateway-console/README.md` | modify | limits, D10, tiers, rows |
| `ts/packages/paigasus-console-core/testing/fake-iam.ts` | modify | `authn.introspectApiKey` default and divergence list |
| `ts/packages/paigasus-console-core/tests/integration/introspect-api-key-default.test.ts` | create | pins the new default |
| `ts/apps/gateway-console/moon.yml` | modify | `test-e2e` deps and inputs |
| `ci/affected-graph/run.sh` | modify | six re-baselined task cases |
| `ts/apps/gateway-console/playwright.config.ts` | modify | the `playground` project |
| `ts/apps/gateway-console/tests/e2e/support/{paths,login}.ts` | modify | `REPO_ROOT`; `signIn` parameter type |
| `ts/apps/gateway-console/tests/e2e/support/{gateway-env,gateway-process,mock-openai,playground-harness}.ts` | create | the playground stack (`gateway-env.ts` is import-free, for unit tests) |
| `ts/apps/gateway-console/tests/e2e/playground-{stream,authz,token-leak}.spec.ts` | create | rows R22 to R28 |
| `ts/apps/gateway-console/tests/e2e/capabilities.spec.ts` | modify | row R29 |
| `ts/apps/gateway-console/tests/unit/*.test.ts(x)` | create / modify | unit tests named in each task |

---

### Task 1: Provision the worktree and measure the four open facts (spec §7.3)

**Files:**
- Modify: `docs/superpowers/specs/2026-09-23-sma-635-gateway-user-auth-design.md` (append section 12 at the end of the file, after line 520)
- Throwaway (never committed): `$SCRATCH/probe-tonic.test.ts`, `$SCRATCH/probe-route.ts`, `$SCRATCH/probe-client.mjs`

**Interfaces:**
- Consumes: the current `paigasus-gateway` binary (built from this branch, which has no code change yet); `startFakeIam`, `denial` from `@paigasus/console-core/testing`; the gateway-console standalone build.
- Produces: spec section 12 with four verdicts. Every later task depends on four `pass` verdicts.

**STOP rule.** If ANY probe below fails its criterion, stop. Do not start Task 2. Report the failed probe, its raw output and the numbers to the controller. The design changes in that case.

In every step, `SCRATCH` is the session scratchpad directory. Set it once: `export SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/<session>/scratchpad` (the controller gives the exact path).

- [ ] **Step 1: Provision the worktree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git branch --show-current
proto install
pnpm -C ts install
(cd py && uv sync)
(cd rs && cargo fetch --locked)
```

Expected: `feature/sma-635-gateway-user-auth`, then four commands that exit 0.

- [ ] **Step 2: Build the gateway binary and measure the build time (probe M4)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
/usr/bin/time -p cargo build --locked --bin paigasus-gateway
/usr/bin/time -p cargo build --locked --bin paigasus-gateway
touch crates/services/paigasus-gateway/src/lib.rs
/usr/bin/time -p cargo build --locked --bin paigasus-gateway
touch crates/libs/paigasus-proto/src/lib.rs
/usr/bin/time -p cargo build --locked --bin paigasus-gateway
ls -l target/debug/paigasus-gateway
```

Record the four `real` values: cold, no change, gateway source touched, proto source touched.

Pass criterion M4: the no-change build is at most 10 s `real`, and the gateway-touch build is at most 120 s `real`. A fail means `paigasus-gateway-rs:e2e-bin` (uncached) is too expensive for every e2e run: STOP.

- [ ] **Step 3: Write the tonic probe (probe M1)**

Create `$SCRATCH/probe-tonic.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
// THROWAWAY (SMA-635 Task 1, probe M1). Never committed.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import path from 'node:path';
import { Code } from '@connectrpc/connect';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { afterEach, describe, expect, it } from 'vitest';

const REPO = path.resolve(process.cwd(), '../../..');
const BIN = path.join(REPO, 'rs/target/debug/paigasus-gateway');

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address();
      if (address === null || typeof address === 'string') return reject(new Error('no port'));
      probe.close(() => resolve(address.port));
    });
  });
}

let child: ChildProcess | null = null;
let fake: FakeIam | null = null;
let output = '';

async function startGateway(iamUrl: string): Promise<string> {
  const port = await freePort();
  child = spawn(BIN, [], {
    cwd: path.dirname(BIN),
    stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      PATH: process.env['PATH'] ?? '',
      GATEWAY_HTTP_ADDR: `127.0.0.1:${String(port)}`,
      GATEWAY_LOG_LEVEL: 'info',
      GATEWAY_IAM__GRPC_ADDR: iamUrl,
      GATEWAY_IAM__TLS__MODE: 'loopback_insecure',
      GATEWAY_UPSTREAM__OPENAI__BASE_URL: 'http://127.0.0.1:1',
      GATEWAY_UPSTREAM__OPENAI__API_KEY: 'sk-probe',
      GATEWAY_METRICS__ENABLED: 'false',
    },
  });
  child.stdout?.on('data', (c: Buffer) => (output += c.toString('utf8')));
  child.stderr?.on('data', (c: Buffer) => (output += c.toString('utf8')));
  const url = `http://127.0.0.1:${String(port)}`;
  for (let i = 0; i < 200; i += 1) {
    try {
      if ((await fetch(`${url}/healthz`)).ok) return url;
    } catch {
      // not listening yet
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`the gateway did not start\n${output}`);
}

afterEach(async () => {
  child?.kill('SIGTERM');
  await fake?.close();
});

describe('M1: tonic against the connect-node h2c fake IAM', () => {
  it('P1a: a PermissionDenied carrying ErrorInfo identity-not-provisioned is decoded (discovery answers 200)', async () => {
    fake = await startFakeIam();
    const url = await startGateway(fake.grpcUrl);
    const res = await fetch(`${url}/v1/service-info`, { headers: { authorization: 'Bearer probe-unprovisioned' } });
    console.log('P1a status', res.status, 'calls', fake.calls.map((c) => c.method).join(','));
    expect(res.status).toBe(200);
    expect(fake.callsTo('authn.introspectApiKey')).toHaveLength(1);
    expect(fake.callsTo('authn.introspect')).toHaveLength(1);
  });

  it('P1b: a PermissionDenied carrying ErrorInfo principal-inactive is rejected (discovery answers 401)', async () => {
    fake = await startFakeIam({
      handlers: {
        'authn.introspectApiKey': () => {
          throw denial({ code: Code.Unauthenticated, reason: 'invalid-token' });
        },
        'authn.introspect': () => {
          throw denial({ reason: 'principal-inactive' });
        },
      },
    });
    fake.provisioned.add('probe-inactive');
    const url = await startGateway(fake.grpcUrl);
    const res = await fetch(`${url}/v1/service-info`, { headers: { authorization: 'Bearer probe-inactive' } });
    console.log('P1b status', res.status);
    expect(res.status).toBe(401);
  });
});
```

- [ ] **Step 4: Run the tonic probe**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
cp "$SCRATCH/probe-tonic.test.ts" ts/apps/gateway-console/tests/unit/zz-sma635-probe-tonic.test.ts
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/zz-sma635-probe-tonic.test.ts
rm ts/apps/gateway-console/tests/unit/zz-sma635-probe-tonic.test.ts
git status --porcelain
```

Pass criterion M1: both tests pass (`P1a status 200`, `P1b status 401`), and `git status --porcelain` prints nothing. P1a passes only when tonic decodes `ErrorInfo` from `grpc-status-details-bin` (the gateway accepts exactly that reason, `auth.rs:205-208`); P1b is the control that the decoded reason discriminates. A fail: STOP.

- [ ] **Step 5: Write the Next route probe (probes M2 and M3)**

Create `$SCRATCH/probe-route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
// THROWAWAY (SMA-635 Task 1, probes M2 and M3). Never committed.
export const dynamic = 'force-dynamic';

export function POST(request: Request): Response {
  const started = Date.now();
  request.signal.addEventListener('abort', () => console.log(`SMA635-PROBE abort ${String(Date.now() - started)}`));
  const encoder = new TextEncoder();
  let n = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      const tick = (): void => {
        n += 1;
        controller.enqueue(encoder.encode(`data: {"n":${String(n)},"pad":"${'x'.repeat(64)}"}\n\n`));
        if (n < 4) timer = setTimeout(tick, 1000);
        else controller.close();
      };
      tick();
    },
    cancel() {
      if (timer !== undefined) clearTimeout(timer);
      console.log(`SMA635-PROBE cancel ${String(Date.now() - started)}`);
    },
  });
  const noTransform = new URL(request.url).searchParams.get('nt') !== '0';
  return new Response(body, {
    headers: { 'content-type': 'text/event-stream', 'cache-control': noTransform ? 'no-cache, no-transform' : 'no-cache', 'x-accel-buffering': 'no' },
  });
}
```

Create `$SCRATCH/probe-client.mjs`:

```js
// THROWAWAY (SMA-635 Task 1). Usage: node probe-client.mjs <port> <stream|abort|compress-off>
import http from 'node:http';

const [port, mode] = process.argv.slice(2);
const req = http.request(
  {
    host: '127.0.0.1',
    port: Number(port),
    method: 'POST',
    path: `/gateway/api/sma635-probe${mode === 'compress-off' ? '?nt=0' : ''}`,
    headers: { cookie: '__Host-pgs_sid=probe', 'accept-encoding': 'gzip, deflate, br', 'content-type': 'application/json' },
  },
  (res) => {
    const t0 = Date.now();
    console.log('status', res.statusCode, 'content-encoding', res.headers['content-encoding'] ?? '(none)');
    res.on('data', (chunk) => console.log('chunk', Date.now() - t0, chunk.length));
    res.on('end', () => console.log('end', Date.now() - t0));
    res.on('error', () => console.log('response error', Date.now() - t0));
    if (mode === 'abort') {
      setTimeout(() => {
        console.log('client-abort', Date.now() - t0);
        req.destroy();
      }, 1500);
    }
  },
);
req.on('error', () => undefined);
req.end('{}');
```

- [ ] **Step 6: Build the console with the probe route and run the probes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
mkdir -p ts/apps/gateway-console/app/api/sma635-probe
cp "$SCRATCH/probe-route.ts" ts/apps/gateway-console/app/api/sma635-probe/route.ts
moon run gateway-console-ts:build --force
PORT=4817 HOSTNAME=127.0.0.1 NODE_ENV=production node ts/apps/gateway-console/.next/standalone/apps/gateway-console/server.js > "$SCRATCH/probe-server.log" 2>&1 &
PROBE_PID=$!
for i in $(seq 1 100); do curl -s -o /dev/null http://127.0.0.1:4817/gateway/healthz && break; sleep 0.2; done
node "$SCRATCH/probe-client.mjs" 4817 stream
node "$SCRATCH/probe-client.mjs" 4817 compress-off
node "$SCRATCH/probe-client.mjs" 4817 abort
sleep 3
kill "$PROBE_PID"
grep SMA635-PROBE "$SCRATCH/probe-server.log"
rm -r ts/apps/gateway-console/app/api/sma635-probe
git status --porcelain
```

This step starts the server with `&` and kills it in the same step. That is a child of the step's own shell, not a background tool run. The `curl` loop only waits for readiness; `/gateway/healthz` may answer 500 without the full `PAIGASUS_*` environment, and the loop ends on any HTTP answer.

Pass criterion M3 (`stream` run): `content-encoding (none)`, at least four `chunk` lines, and each of chunks 2, 3 and 4 arrives at least 700 ms after the one before it. Record the `compress-off` run too (information only, no criterion).

Pass criterion M2 (`abort` run): the server log holds `SMA635-PROBE abort <t>` or `SMA635-PROBE cancel <t>` with `t` at most 2500. Record which one fired. `abort` answers the spec's question "yes"; `cancel` alone answers "no, but the body cancel path works", which the design already uses as its second path (spec §6.2). Neither line: STOP.

If the server does not start, or the probe route answers 5xx before any chunk, that is a defect of the probe, not a verdict: start the server once more with the environment `tests/e2e/support/harness.ts:166-183` builds, and repeat the three client runs. Only a measured result counts against the criteria.

After the step, `git status --porcelain` must print nothing. Run `moon run gateway-console-ts:build --force` once more, so no later task reads a build that holds the probe route.

- [ ] **Step 7: Append the results to the spec**

Append this section to the end of the spec. Put the measured values into the cells.

```markdown
## 12. Measurements (plan Task 1, 2026-09-23)

Measured on the development Mac, on the branch before any code change.

| # | Question (§7.3) | Measured | Verdict |
|---|---|---|---|
| M1 | tonic against the connect-node h2c fake IAM, with `ErrorInfo` in `grpc-status-details-bin` | P1a: HTTP <status>, calls <methods>; P1b: HTTP <status> | pass or fail |
| M2 | Next 16.3.4 standalone: `request.signal` abort on client disconnect | `abort` at <ms> ms / `cancel` at <ms> ms / neither | pass or fail, and which path fired |
| M3 | `cache-control: no-transform` against Next compression on a route-handler stream | content-encoding <value>; chunk arrival <ms list>; control without no-transform: <value> | pass or fail |
| M4 | build time of `cargo build --locked --bin paigasus-gateway` | cold <s> s, no change <s> s, gateway touched <s> s, proto touched <s> s | pass or fail |

If M2 fired only `cancel`, the route handler's body cancel (§6.2) is the path that stops the upstream.
```

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add docs/superpowers/specs/2026-09-23-sma-635-gateway-user-auth-design.md
git commit -m "docs(docs): record the sma-635 measurements in the spec" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

If any verdict is `fail`, commit the section anyway and then STOP (see the STOP rule).

---

### Task 2: `resolve_org` over the kernel `Prn` API

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`[dependencies]` block, lines 18-80; `[dev-dependencies]` lines 90-92)
- Modify: `rs/crates/services/paigasus-gateway/moon.yml:11-16` (the comment above the kernel edge)
- Modify: `ci/affected-graph/cargo_moon_parity.py:72-82`
- Modify: `rs/crates/services/paigasus-gateway/src/domain.rs` (add below line 20)
- Modify: `rs/Cargo.lock` (cargo updates it)

**Interfaces:**
- Consumes: `paigasus_kernel::Prn` (`parse`, `build`, `canonical`, `service`, `org`, `resource_type`, `resource_id`); `uuid::Uuid::try_parse`.
- Produces:
  - `pub enum OrgHeader<'a> { Absent, One(&'a str), Many }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub enum OrgResolutionError { InvalidOrgHeader, OrgRequired }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub fn resolve_org(header: OrgHeader<'_>, node_prns: &[&str]) -> Result<paigasus_kernel::Prn, OrgResolutionError>`

- [ ] **Step 1: Write the failing table test**

Append to `rs/crates/services/paigasus-gateway/src/domain.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0190a100-0000-7000-8000-0000000000a1";
    const A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
    const A_PRN_UPPER: &str = "prn:pgs:iam:::organization/0190A100-0000-7000-8000-0000000000A1";
    const B_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000b2";
    const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";
    const PROJECT_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:project/0190a1c3-0000-7000-8000-0000000000d4";
    const PROJECT_IN_B: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000b2:project/0190a1c3-0000-7000-8000-0000000000d5";
    const ROOT_GRANT: &str = "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000";
    const OTHER_SERVICE_ORG: &str = "prn:pgs:gateway:::organization/0190a100-0000-7000-8000-0000000000a1";

    fn ok(prn: &str) -> Result<String, OrgResolutionError> {
        Ok(prn.to_owned())
    }

    /// SMA-635 spec §4.2: every `OrgHeader` arm, a Root grant, another service, a PRN that does
    /// not parse, and duplicates of one org. Review Focus 2: an upper-case UUID gives the
    /// lower-case canonical PRN, and two spellings of one org count as ONE org.
    #[test]
    fn resolve_org_table() {
        let upper = A.to_uppercase();
        let cases: Vec<(&str, OrgHeader<'_>, Vec<&str>, Result<String, OrgResolutionError>)> = vec![
            ("one_valid", OrgHeader::One(A), vec![], ok(A_PRN)),
            ("one_uppercase", OrgHeader::One(&upper), vec![], ok(A_PRN)),
            ("one_ignores_grants", OrgHeader::One(A), vec![B_PRN], ok(A_PRN)),
            ("one_simple_form", OrgHeader::One("0190a1000000700080000000000000a1"), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("one_braced", OrgHeader::One("{0190a100-0000-7000-8000-0000000000a1}"), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("one_urn", OrgHeader::One("urn:uuid:0190a100-0000-7000-8000-0000000000a1"), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("one_not_a_uuid", OrgHeader::One("acme"), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("one_empty", OrgHeader::One(""), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("many", OrgHeader::Many, vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("absent_zero_orgs", OrgHeader::Absent, vec![], Err(OrgResolutionError::OrgRequired)),
            ("absent_one_org", OrgHeader::Absent, vec![A_PRN], ok(A_PRN)),
            ("absent_org_team_project_one_org", OrgHeader::Absent, vec![A_PRN, TEAM_IN_A, PROJECT_IN_A], ok(A_PRN)),
            ("absent_team_only", OrgHeader::Absent, vec![TEAM_IN_A], ok(A_PRN)),
            ("absent_two_orgs", OrgHeader::Absent, vec![A_PRN, B_PRN], Err(OrgResolutionError::OrgRequired)),
            ("absent_two_orgs_via_project", OrgHeader::Absent, vec![A_PRN, PROJECT_IN_B], Err(OrgResolutionError::OrgRequired)),
            ("absent_root_grant_ignored", OrgHeader::Absent, vec![ROOT_GRANT, A_PRN], ok(A_PRN)),
            ("absent_root_grant_only", OrgHeader::Absent, vec![ROOT_GRANT], Err(OrgResolutionError::OrgRequired)),
            ("absent_other_service_ignored", OrgHeader::Absent, vec![OTHER_SERVICE_ORG], Err(OrgResolutionError::OrgRequired)),
            ("absent_unparseable_ignored", OrgHeader::Absent, vec!["not-a-prn", A_PRN], ok(A_PRN)),
            ("absent_same_org_two_spellings", OrgHeader::Absent, vec![A_PRN, A_PRN_UPPER], ok(A_PRN)),
            ("absent_duplicate_org", OrgHeader::Absent, vec![A_PRN, A_PRN, TEAM_IN_A], ok(A_PRN)),
        ];
        for (name, header, prns, want) in cases {
            let got = resolve_org(header, &prns).map(|prn| prn.canonical());
            assert_eq!(got, want, "{name}");
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo nextest run -p paigasus-gateway resolve_org_table
```

Expected: a compile error, `cannot find type OrgHeader` and `cannot find function resolve_org`.

- [ ] **Step 3: Add the dependencies**

In `rs/crates/services/paigasus-gateway/Cargo.toml`, add at the end of `[dependencies]` (after the `paigasus-service-info` line):

```toml
# `domain::resolve_org` (SMA-635) parses and builds org PRNs with the kernel `Prn` API. NOT
# paigasus-iam-core: that crate pulls cedar-policy and more into the gateway.
paigasus-kernel = { workspace = true }
# `domain::resolve_org` reads the `paigasus-org` header as a UUID (SMA-635). Moved from
# [dev-dependencies]; the correlation test in `adapters::iam::client` still uses it.
uuid = { workspace = true }
```

Delete the three `uuid` lines from `[dev-dependencies]` (the two comment lines above `uuid = { workspace = true }` and that line).

- [ ] **Step 4: Write the minimal implementation**

In `rs/crates/services/paigasus-gateway/src/domain.rs`, add below the `CallerContext` struct (after line 20), before the test module:

```rust
use paigasus_kernel::Prn;
use uuid::Uuid;

/// What the `paigasus-org` request header held (SMA-635 D2). The adapter builds it; the domain
/// function below takes no transport type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgHeader<'a> {
    /// No `paigasus-org` header.
    Absent,
    /// Exactly one header, whose value is visible ASCII.
    One(&'a str),
    /// Two or more headers.
    Many,
}

/// Why no organization could be resolved. The HTTP adapter maps each case to a 400 with
/// `param: "paigasus-org"` (`adapters::http::error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgResolutionError {
    /// The header is not ONE UUID in the kernel's 36-character hyphenated form, or it repeats.
    InvalidOrgHeader,
    /// No header, and the user reaches zero or several organizations (D3).
    OrgRequired,
}

const IAM_SERVICE: &str = "iam";
const ORGANIZATION: &str = "organization";

/// Resolve the ONE organization an OIDC caller acts in (SMA-635 spec §4.2).
///
/// - `One(value)`: `value` must be a UUID in the 36-character hyphenated form (the kernel rule,
///   `resource_name.rs:101-107`). The header is NOT checked against `node_prns`: IAM decides that
///   in the self-query.
/// - `Many`: always an error.
/// - `Absent`: every PRN in `node_prns` (memberships and role-grant scopes) names at most one org:
///   the resource id of an `organization`, or the org slot of a `team` or `project`. A PRN that
///   does not parse, of another service, or of another type (a Root grant) is ignored. Exactly
///   one distinct org is the answer.
///
/// The answer is built with the kernel builder, so its canonical form is lower case
/// (`prn:pgs:iam:::organization/<uuid>`) whatever case the input used.
pub fn resolve_org(header: OrgHeader<'_>, node_prns: &[&str]) -> Result<Prn, OrgResolutionError> {
    match header {
        OrgHeader::Many => Err(OrgResolutionError::InvalidOrgHeader),
        OrgHeader::One(value) => parse_org_uuid(value).map(org_prn).ok_or(OrgResolutionError::InvalidOrgHeader),
        OrgHeader::Absent => {
            let mut found: Option<Uuid> = None;
            for raw in node_prns {
                let Some(org) = org_of(raw) else { continue };
                match found {
                    None => found = Some(org),
                    Some(existing) if existing == org => {}
                    Some(_) => return Err(OrgResolutionError::OrgRequired),
                }
            }
            found.map(org_prn).ok_or(OrgResolutionError::OrgRequired)
        }
    }
}

/// The kernel's UUID rule: exactly 36 characters, and `Uuid::try_parse` accepts it. The length
/// check rejects the simple (32), braced (38) and `urn:uuid:` (45) forms that `try_parse` would
/// otherwise accept.
fn parse_org_uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    Uuid::try_parse(value).ok()
}

fn org_prn(id: Uuid) -> Prn {
    Prn::build(IAM_SERVICE, "", None, ORGANIZATION, id).expect("static organization PRN parts are valid")
}

/// The org a tenancy PRN belongs to, or `None` when the PRN names no org.
fn org_of(raw: &str) -> Option<Uuid> {
    let prn = Prn::parse(raw).ok()?;
    if prn.service() != IAM_SERVICE {
        return None;
    }
    match prn.resource_type() {
        ORGANIZATION => Some(prn.resource_id()),
        "team" | "project" => prn.org(),
        _ => None,
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo nextest run -p paigasus-gateway resolve_org_table
```

Expected: `1 passed`.

- [ ] **Step 6: Update the Moon edge comment and the parity allowlist**

In `rs/crates/services/paigasus-gateway/moon.yml`, replace lines 11-15 (the six-line comment above `- 'paigasus-kernel-rs'`) with:

```yaml
  # A `{ workspace = true }` Cargo dependency since SMA-635 (`domain::resolve_org` uses the kernel
  # `Prn` API), so this hand-written edge is REQUIRED (ci/CLAUDE.md: Moon does not resolve
  # `workspace = true`). It also feeds `fileGroups.upstreams` below.
```

In `ci/affected-graph/cargo_moon_parity.py`, replace lines 72-82 with:

```python
# (consumer, upstream) -> why this hand-declared Moon edge has no Cargo backing.
# An allowlisted edge is a RECORDED DECISION, not a silent exemption: the reason string is required.
# EMPTY since SMA-635: the one entry, ("paigasus-gateway-rs", "paigasus-kernel-rs"), is now backed
# by a real Cargo dependency (the gateway's `domain::resolve_org`).
ALLOW_NO_CARGO_BACKING: dict[tuple[str, str], str] = {}
```

- [ ] **Step 7: Run the crate checks and the affected-graph suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-gateway
cd ..
/bin/bash ci/affected-graph/run.sh
```

Expected: fmt and clippy clean; all gateway tests pass; the affected-graph suite ends with its pass line. It runs under system `/bin/bash` 3.2 on purpose (root CLAUDE.md, "This development Mac only"). If the parity check reports a row about the removed allowlist entry (for example a self-test that expects one entry), read the row: the fix is in the self-test fixture, not a restored entry. If `cargo_moon_parity.py` fails on its type annotation under the repo's Python, drop the annotation and keep `ALLOW_NO_CARGO_BACKING = {}`.

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add rs/crates/services/paigasus-gateway/Cargo.toml rs/crates/services/paigasus-gateway/moon.yml rs/crates/services/paigasus-gateway/src/domain.rs rs/Cargo.lock ci/affected-graph/cargo_moon_parity.py
git commit -m "feat(rs): resolve the caller's organization with the kernel prn api" -m "SMA-635 spec section 4.2. The gateway now depends on paigasus-kernel, so the allowlisted Moon edge is backed by Cargo." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Error codes 309 and 310

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto:209` (insert after `ERROR_REASON_STREAMING_DISABLED = 308;`)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `ts/packages/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/src/paigasus_proto/generated/**`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:141` (doc), `:208` (list), `:230` (count)
- Modify: `ts/packages/paigasus-proto/src/error.test.ts:57-60`
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts:11-12` (comment), `:69` (entries)
- Modify: `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (`:32-38`, `:47-98`, `:106-113`, `:120-177`, `:183-189`, `:129`, tests `:220-233`, `:271-274`)

**Interfaces:**
- Consumes: `OrgResolutionError` (Task 2).
- Produces: `GatewayError::InvalidOrgHeader`, `GatewayError::OrgRequired` (both 400, `param: Some("paigasus-org")`, `Retryable::No`); `impl From<OrgResolutionError> for GatewayError`; the registry values 309 and 310 in all three languages.

- [ ] **Step 1: Write the failing tests**

In `rs/crates/libs/paigasus-proto/src/error.rs`, in `EXPECTED_REASONS`, after `"streaming-disabled",` (line 208) add:

```rust
        "invalid-org-header",
        "org-required",
```

and change line 230 to:

```rust
        assert_eq!(actual.len(), 59, "the registry should hold 59 reasons");
```

and in the doc comment at line 141 change `57` to `59`.

In `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs`, in `each_case_maps_to_its_bound_status` (after line 232) add:

```rust
        assert_eq!(GatewayError::InvalidOrgHeader.into_response().status(), StatusCode::BAD_REQUEST);
        assert_eq!(GatewayError::OrgRequired.into_response().status(), StatusCode::BAD_REQUEST);
```

and add this test at the end of the test module:

```rust
    /// SMA-635 spec §4.5: both org codes name the HEADER in `param` — a deviation from OpenAI,
    /// which uses `param` for a body field. The resolution errors map one to one.
    #[tokio::test]
    async fn the_org_codes_name_the_header_in_param() {
        for (err, code) in [(GatewayError::InvalidOrgHeader, "invalid-org-header"), (GatewayError::OrgRequired, "org-required")] {
            let body = body_json(err.into_response()).await;
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["param"], "paigasus-org");
            assert_eq!(body["error"]["type"], "invalid_request_error");
            assert_eq!(err.retryable(), Retryable::No);
        }
        assert_eq!(GatewayError::from(crate::domain::OrgResolutionError::InvalidOrgHeader), GatewayError::InvalidOrgHeader);
        assert_eq!(GatewayError::from(crate::domain::OrgResolutionError::OrgRequired), GatewayError::OrgRequired);
    }

    /// SMA-635 spec §4.1: the code stays `invalid-api-key` (a compatibility contract), and the
    /// message no longer says "API key", because a user bearer is rejected with it too.
    #[tokio::test]
    async fn the_invalid_credential_message_names_a_credential() {
        let body = body_json(GatewayError::InvalidCredential.into_response()).await;
        assert_eq!(body["error"]["code"], "invalid-api-key");
        assert_eq!(body["error"]["message"], "Invalid credential.");
    }
```

In `ts/packages/paigasus-proto/src/error.test.ts`, change lines 57-60 to:

```ts
    // Cardinality guard. The Rust mirror asserts 59 at
    // rs/crates/libs/paigasus-proto/src/error.rs:230; the two must agree,
    // because both derive from the same proto.
    expect(values).toHaveLength(59);
```

In `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`, change `57` to `59`.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo nextest run -p paigasus-proto the_registry_contains_exactly_the_expected_reasons
cargo nextest run -p paigasus-gateway the_org_codes_name_the_header_in_param
```

Expected: the proto test fails with `declared in the test but not in the registry: ["invalid-org-header", "org-required"]`; the gateway test fails to compile (`no variant named InvalidOrgHeader`).

- [ ] **Step 3: Add the reasons to the proto and regenerate**

In `contracts/proto/paigasus/common/v1/error.proto`, after line 209 (`ERROR_REASON_STREAMING_DISABLED = 308;`) insert:

```proto
  // "invalid-org-header" — the paigasus-org header is not ONE organization
  // UUID in the 36-character form (400, param "paigasus-org"). `param` names a
  // HEADER here, a deviation from OpenAI, which uses it for a body field.
  ERROR_REASON_INVALID_ORG_HEADER = 309;
  // "org-required" — no paigasus-org header, and the user reaches zero or
  // several organizations (400, param "paigasus-org").
  ERROR_REASON_ORG_REQUIRED = 310;
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
(cd contracts && buf format -w)
moon run contracts:generate
git status --porcelain
```

Expected: `git status` shows `error.proto` and the generated error files for Rust, TypeScript and Python, and NO deleted file. If `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` shows as deleted (a BSR rate limit), run `git checkout -- ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` and run `moon run contracts:generate` again.

- [ ] **Step 4: Add the SDK presentation entries**

In `ts/packages/paigasus-sdk/src/errors/presentation.ts`, after line 69 (`[ErrorReason.STREAMING_DISABLED]: 'from-transport',`) add:

```ts
  // SMA-635. Both answer 400, which the transport table already presents as invalid input, as it
  // does for the other gateway 400 codes.
  [ErrorReason.INVALID_ORG_HEADER]: 'from-transport',
  [ErrorReason.ORG_REQUIRED]: 'from-transport',
```

and change the comment at lines 11-12 to: `// demanding an entry for it would make the table 60 keys rather than 59.`

- [ ] **Step 5: Add the gateway variants**

In `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs`:

Replace the `ErrorBody` doc (lines 32-38) with:

```rust
/// The body of the OpenAI error envelope. `param` names what is at fault when something is:
/// `StreamingDisabled` names the request field `stream` (SMA-505 D9), and `InvalidOrgHeader` and
/// `OrgRequired` name the request HEADER `paigasus-org` (SMA-635). Naming a header is a
/// deliberate deviation from OpenAI, which uses `param` only for a body field. Every other case
/// leaves it `null`. `code` is a stable machine-readable diagnostic drawn from the canonical
/// registry (`common/v1/error.proto`, SMA-504) — every [`GatewayError`] case emits one, so `code`
/// is no longer `null` in practice; the field stays `Option` because [`ErrorEnvelope`] has no
/// other case that omits it today. `r#type` serializes as `"type"`.
```

Add two variants at the end of the enum (after `StreamingDisabled`, line 97):

```rust
    /// The `paigasus-org` header is not ONE organization UUID in the 36-character form → 400,
    /// `param: "paigasus-org"` (SMA-635).
    InvalidOrgHeader,
    /// An OIDC caller sent no `paigasus-org` header, and reaches zero or several organizations →
    /// 400, `param: "paigasus-org"` (SMA-635 D3).
    OrgRequired,
```

After the `From<OpenAiError>` impl (after line 113) add:

```rust
/// Map a failed organization resolution (SMA-635 spec §4.2) to its 400.
impl From<crate::domain::OrgResolutionError> for GatewayError {
    fn from(err: crate::domain::OrgResolutionError) -> Self {
        match err {
            crate::domain::OrgResolutionError::InvalidOrgHeader => GatewayError::InvalidOrgHeader,
            crate::domain::OrgResolutionError::OrgRequired => GatewayError::OrgRequired,
        }
    }
}
```

Change line 129 to:

```rust
            GatewayError::InvalidCredential => (StatusCode::UNAUTHORIZED, "invalid_request_error", Some("invalid-api-key"), None, "Invalid credential."),
```

In `parts()`, after the `StreamingDisabled` arm (after line 175) add:

```rust
            GatewayError::InvalidOrgHeader => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("invalid-org-header"),
                Some("paigasus-org"),
                "The paigasus-org header must hold exactly one organization UUID in the 36-character form.",
            ),
            GatewayError::OrgRequired => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("org-required"),
                Some("paigasus-org"),
                "Send the paigasus-org header with the organization UUID: the gateway cannot choose one organization for this user.",
            ),
```

In `retryable()` (line 187) append `| Self::InvalidOrgHeader | Self::OrgRequired` to the `Retryable::No` arm.

In `body_is_the_openai_envelope_shape` (lines 271-274), change the assertion message to: `"InvalidCredential names nothing at fault, so param is null (StreamingDisabled, InvalidOrgHeader and OrgRequired set it)"`.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-proto
cargo nextest run -p paigasus-gateway
cd ..
moon run paigasus-proto-ts:test paigasus-sdk-ts:test paigasus-sdk-ts:typecheck repo:error-code-single-site contracts:lint contracts:fmt
(cd contracts && buf breaking --against "$(git rev-parse --git-common-dir)#branch=main")
moon run ts:fmt
```

Expected: all pass. `every_gateway_code_is_declared_in_the_canonical_registry` iterates the new variants and passes. `buf breaking` passes (adding enum values is not breaking); it names the common git dir because a worktree's `.git` is a file. Run `moon run py:test` too if the Python generated file changed shape; expected: pass.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git status --porcelain
git add contracts/proto/paigasus/common/v1/error.proto rs/crates/libs/paigasus-proto ts/packages/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-sdk/src/errors/presentation.ts ts/packages/paigasus-sdk/tests/presentation.test.ts rs/crates/services/paigasus-gateway/src/adapters/http/error.rs
git commit -m "feat(contracts): add the invalid-org-header and org-required reasons" -m "SMA-635 spec section 4.5. Reasons 309 and 310, both 400 with param paigasus-org. The rejected-credential message now says credential." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `CallerContext` with `Credential`, and the request log line

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`[dev-dependencies]`, append)
- Modify: `rs/crates/services/paigasus-gateway/src/domain.rs:1-20`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:41` (import), `:122` (construction), tests `:522-526` (probe)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:49` (import), `:157-170` (log)
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (new test, helper)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub enum Credential { ApiKey { key_id: String }, Oidc }` (derives `Debug, Clone, PartialEq, Eq`)
  - `pub struct CallerContext { pub principal_prn: String, pub scope_prn: String, pub credential: Credential }`
  - The log line `chat completion proxied` with fields `model`, `stream`, `status`, `latency_ms`, `principal`, `scope`, `auth` (`"api_key"` or `"oidc"`), and `key_id` only for an API key.

- [ ] **Step 1: Write the failing test**

Add at the end of `[dev-dependencies]` in `rs/crates/services/paigasus-gateway/Cargo.toml`:

```toml
# Captures log lines in the auth and chat tests (SMA-635): one `paigasus-org ignored` warning, and
# the `auth`/`scope` fields of `chat completion proxied`.
tracing-subscriber = { workspace = true }
```

In `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs`, add after the imports:

```rust
// ---- log capture (SMA-635) ---------------------------------------------------------------------

/// A shared byte buffer that `tracing_subscriber` writes into. `#[tokio::test]` runs on one
/// thread, and `oneshot` drives the handler on it, so a thread-local default subscriber sees
/// every line the handler logs.
#[derive(Clone, Default)]
struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(tracing::Level::INFO).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}
```

and this test after `egress_never_forwards_caller_credentials`:

```rust
/// SMA-635 spec §4.3: the request log names the credential kind and the scope. An API key
/// keeps its `key_id`; the prompt is still never logged.
#[tokio::test]
async fn the_request_log_names_the_credential_and_the_scope() {
    let (logs, _guard) = capture_logs();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = logs.text();
    let line = text.lines().find(|l| l.contains("chat completion proxied")).expect("one request log line");
    assert!(line.contains("api_key"), "auth=api_key: {line}");
    assert!(line.contains(CALLER_SCOPE), "scope: {line}");
    assert!(line.contains(CALLER_KEY_ID), "key_id for an API key: {line}");
    assert!(!line.contains("\"hi\""), "the prompt is never logged: {line}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo nextest run -p paigasus-gateway the_request_log_names_the_credential_and_the_scope
```

Expected: FAIL at `auth=api_key` (the line has no `auth` field yet).

- [ ] **Step 3: Write the minimal implementation**

Replace `rs/crates/services/paigasus-gateway/src/domain.rs:1-20` with:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The authenticated caller identity a request carries after the auth middleware validates its
//! bearer credential, and the organization resolution the OIDC path uses (SMA-635). Consumed by
//! the chat handler to log request metadata (never the prompt/response body or any other PII).

/// Which credential authenticated the request. `Oidc`, not `User`: an OIDC bearer can belong to a
/// machine client, and IAM uses the same name (`paigasus-iam` `authn.rs:230`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// An IAM API key. `key_id` is the key's identifier, never the secret — safe to log.
    ApiKey { key_id: String },
    /// An OIDC access token (a console user, SMA-635).
    Oidc,
}

/// The caller identity resolved from a request's bearer credential. Carried through the request
/// extensions so the handler never re-authenticates.
#[derive(Debug, Clone)]
pub struct CallerContext {
    /// The authenticated principal's PRN: a service account for a key, a user for an OIDC token.
    pub principal_prn: String,
    /// The scope the request is authorized against: the key's own `scope_prn` for an API key; the
    /// resolved organization PRN for an OIDC token.
    pub scope_prn: String,
    /// Which credential authenticated the request.
    pub credential: Credential,
}
```

In `auth.rs:41` change the import to `use crate::domain::{CallerContext, Credential};`. Change line 122 to:

```rust
    req.extensions_mut().insert(CallerContext {
        principal_prn,
        scope_prn,
        credential: Credential::ApiKey { key_id },
    });
```

Replace the test probe (`auth.rs:522-526`) with:

```rust
    /// The probe handler: proves the inner handler sees the `CallerContext` the middleware attached
    /// by echoing its three parts. An API key echoes its `key_id`; an OIDC token echoes `oidc`.
    async fn probe(axum::Extension(ctx): axum::Extension<CallerContext>) -> String {
        let credential = match &ctx.credential {
            Credential::ApiKey { key_id } => key_id.clone(),
            Credential::Oidc => "oidc".to_owned(),
        };
        format!("{}|{}|{}", ctx.principal_prn, ctx.scope_prn, credential)
    }
```

In `chat.rs:49` change the import to `use crate::domain::{CallerContext, Credential};` and replace lines 157-170 with:

```rust
    // One structured line per request — model/stream/status/latency/principal/scope/credential
    // ONLY. NEVER the prompt, messages, body, or the OpenAI key. `principal` is a service account
    // for an API key and a USER for an OIDC token (SMA-635): a principal PRN is an opaque id, not
    // the user's name or e-mail, and it is kept deliberately for attribution and audit. The "never
    // PII" bar is about prompt/message content. `key_id` is logged only for an API key.
    match &caller.credential {
        Credential::ApiKey { key_id } => tracing::info!(
            model = %model,
            stream = stream,
            status = status.as_u16(),
            latency_ms = started.elapsed().as_millis() as u64,
            principal = %caller.principal_prn,
            scope = %caller.scope_prn,
            auth = "api_key",
            key_id = %key_id,
            "chat completion proxied"
        ),
        Credential::Oidc => tracing::info!(
            model = %model,
            stream = stream,
            status = status.as_u16(),
            latency_ms = started.elapsed().as_millis() as u64,
            principal = %caller.principal_prn,
            scope = %caller.scope_prn,
            auth = "oidc",
            "chat completion proxied"
        ),
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-gateway
```

Expected: all pass, including `happy_path_reaches_handler_with_caller_context` (it still echoes `SA|SCOPE|KEY_ID`).

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add rs/crates/services/paigasus-gateway/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-gateway/src/domain.rs rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs rs/crates/services/paigasus-gateway/tests/chat_proxy.rs
git commit -m "feat(rs): carry the credential kind and scope in the gateway request log" -m "SMA-635 spec section 4.3. CallerContext holds a Credential (ApiKey or Oidc). The log line gains scope and auth; key_id stays for an API key only." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Widen `require_iam_auth` with the OIDC leg

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` (module doc `:3-25`; `require_iam_auth` `:48-124`; `require_authenticated` doc `:145-146`; `is_identity_not_provisioned` `:233-256`; `record_iam_call` doc `:274-279`; `authz_error` `:332-347`; tests)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs:67-73`
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (fake `:58-104`, header doc `:14-19`, new tests)
- Modify: `rs/crates/services/paigasus-gateway/tests/metrics.rs` (new fake and test)
- Modify: `rs/crates/services/paigasus-gateway/tests/service_info.rs:274-276` (comment)

**Interfaces:**
- Consumes: `OrgHeader`, `resolve_org` (Task 2); `GatewayError::{InvalidOrgHeader, OrgRequired}` and `From<OrgResolutionError>` (Task 3); `Credential`, `CallerContext` (Task 4).
- Produces: `pub const ORG_HEADER: &str = "paigasus-org";` in `adapters::http::auth`; the widened `require_iam_auth` (same signature: `pub async fn require_iam_auth(State(iam): State<Arc<dyn Iam>>, mut req: Request, next: Next) -> Response`).

- [ ] **Step 1: Update the existing unit rows and write the new ones (failing)**

In the `auth.rs` test module:

Add to the imports (after line 357):

```rust
    use axum::http::HeaderValue;
    use paigasus_proto::paigasus::iam::v1::{Membership, RoleGrantRef};
```

Add these helpers after `identity_not_provisioned_details` (after line 612):

```rust
    // ---- SMA-635: the OIDC leg of `require_iam_auth` ------------------------------------------

    const USER_TOKEN: &str = "user-oidc-access-token";
    const USER_PRN: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0";
    const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
    const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
    const ORG_B: &str = "0190a100-0000-7000-8000-0000000000b2";
    const ORG_B_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000b2";
    const TEAM_IN_A_PRN: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";

    /// What real IAM sends for a bearer that is not a JWT (`paigasus-iam` `convert.rs:141`):
    /// `Unauthenticated` with the reason `invalid-token`. Every key-leg failure row now reaches the
    /// token leg, and this is the answer that leg gets for an API key or garbage.
    fn rejected_token() -> TokenIntrospectOutcome {
        TokenIntrospectOutcome::Rpc(Code::Unauthenticated, Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidToken)))
    }

    /// The key-leg answer for an OIDC access token: it is not an API key.
    fn rejected_key() -> IntrospectOutcome {
        IntrospectOutcome::Rpc(Code::Unauthenticated, Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidToken)))
    }

    /// An active user whose memberships and `gateway_user` grants sit on the given PRNs.
    fn user_response(memberships: &[&str], grants: &[&str]) -> IntrospectResponse {
        IntrospectResponse {
            principal_prn: USER_PRN.to_owned(),
            status: "active".to_owned(),
            issuer: "https://issuer.example.com".to_owned(),
            subject: "user-1".to_owned(),
            expires_at: None,
            memberships: memberships
                .iter()
                .map(|prn| Membership {
                    principal_prn: USER_PRN.to_owned(),
                    node_prn: (*prn).to_owned(),
                    ..Default::default()
                })
                .collect(),
            role_grants: grants
                .iter()
                .map(|prn| RoleGrantRef {
                    scope_prn: (*prn).to_owned(),
                    role_key: "gateway_user".to_owned(),
                })
                .collect(),
        }
    }

    fn user_fake(user: IntrospectResponse, authz: AuthzOutcome) -> FakeIam {
        FakeIam::new(rejected_key(), authz).with_token_introspect(TokenIntrospectOutcome::Ok(user))
    }

    /// A request with `Bearer <token>` and one `paigasus-org` header per entry of `orgs` (raw
    /// bytes, so a test can send an obs-text byte).
    fn req_with_orgs(token: &str, orgs: &[&[u8]]) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri("/x").header(header::AUTHORIZATION, format!("Bearer {token}"));
        for org in orgs {
            builder = builder.header(ORG_HEADER, HeaderValue::from_bytes(org).expect("a valid header value"));
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn run(fake: FakeIam, req: HttpRequest<Body>) -> (StatusCode, String) {
        let resp = build_app(fake).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    // ---- log capture ------------------------------------------------------------------------

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogBuffer;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl LogBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        (buffer, tracing::subscriber::set_default(subscriber))
    }
```

Update the existing key-leg failure rows so that each configures the token leg with `rejected_token()`. Replace each fake construction as follows; the expected status of each row does not change:

- `introspect_unauthenticated_returns_401` (`:659`): `FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token())`
- `introspect_permission_denied_returns_401` (`:666`): `FakeIam::new(IntrospectOutcome::Rpc(Code::PermissionDenied, None), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token())`
- `introspect_unavailable_returns_503` (`:672`): `FakeIam::new(IntrospectOutcome::Rpc(Code::Unavailable, None), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token())`
- `introspect_connect_failure_returns_503` (`:678`): `FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Ok(true)).with_token_introspect(rejected_token())`
- `introspect_success_with_non_active_status_returns_401` (`:688`): `FakeIam::new(IntrospectOutcome::Ok(resp), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token())`

Replace `require_iam_auth_still_rejects_an_unprovisioned_identity` (`:866-879`) with:

```rust
    /// The discovery relaxation must NOT leak onto the chat path. Real IAM sends
    /// `identity-not-provisioned` on the TOKEN leg (`convert.rs:142`), so that is where this fake
    /// puts it. A user with no provisioned identity has no grants (SMA-635 spec §4.1 step 3).
    /// Two forms: a conclusive key leg gives 401; an inconclusive key leg gives 503, because
    /// `preserve_outage` keeps an IAM outage visible.
    #[tokio::test]
    async fn require_iam_auth_still_rejects_an_unprovisioned_identity() {
        let conclusive = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(status_of(conclusive, req_with_auth("Bearer validated-but-unprovisioned-token")).await, StatusCode::UNAUTHORIZED);

        let inconclusive = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(status_of(inconclusive, req_with_auth("Bearer validated-but-unprovisioned-token")).await, StatusCode::SERVICE_UNAVAILABLE);
    }
```

Add the new rows at the end of the test module:

```rust
    // ---- SMA-635 rows 1-11 ------------------------------------------------------------------

    /// Row 1: an OIDC bearer with a valid header reaches the handler as `Credential::Oidc`, with
    /// the org PRN as the scope.
    #[tokio::test]
    async fn an_oidc_bearer_with_a_valid_org_header_reaches_the_handler() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{USER_PRN}|{ORG_A_PRN}|oidc"));
    }

    /// Row 2: no header, one org reached twice (a membership on the org, a `gateway_user` grant
    /// on a team of the same org): the org is inferred.
    #[tokio::test]
    async fn without_a_header_one_org_reached_twice_is_inferred() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[TEAM_IN_A_PRN]), AuthzOutcome::Ok(true));
        let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{USER_PRN}|{ORG_A_PRN}|oidc"));
    }

    /// Rows 3 and 4: no header, and zero orgs or two orgs: `400 org-required`, and no
    /// authorization call.
    #[tokio::test]
    async fn without_a_header_zero_or_two_orgs_is_org_required() {
        for (label, memberships) in [("zero orgs", vec![]), ("two orgs", vec![ORG_A_PRN, ORG_B_PRN])] {
            let fake = user_fake(user_response(&memberships, &[]), AuthzOutcome::Unreachable);
            let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[])).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{label}");
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(json["error"]["code"], "org-required", "{label}");
            assert_eq!(json["error"]["param"], "paigasus-org", "{label}");
        }
    }

    /// Row 5 and Review Focus 1: a header that is not a UUID, a simple-form UUID, two headers, and
    /// a value with an obs-text byte (valid as a `HeaderValue`, but `to_str()` fails) each give
    /// `400 invalid-org-header` BEFORE any authorization call — never a 500 or a panic.
    #[tokio::test]
    async fn an_invalid_org_header_is_400_before_any_authorization() {
        let cases: [(&str, Vec<&[u8]>); 4] = [
            ("not a uuid", vec![b"acme".as_slice()]),
            ("simple form", vec![b"0190a1000000700080000000000000a1".as_slice()]),
            ("two headers", vec![ORG_A.as_bytes(), ORG_A.as_bytes()]),
            ("obs-text byte", vec![b"0190a100-0000-7000-8000-0000000000a\x80".as_slice()]),
        ];
        for (label, orgs) in cases {
            let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Unreachable);
            let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &orgs)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{label}");
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(json["error"]["code"], "invalid-org-header", "{label}");
            assert_eq!(json["error"]["param"], "paigasus-org", "{label}");
        }
    }

    /// Row 6: IAM denies the self-query: `403 insufficient-permissions`.
    #[tokio::test]
    async fn an_oidc_authz_deny_is_403() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(false));
        let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["error"]["code"], "insufficient-permissions");
    }

    /// Row 7 — THE self-query proof for OIDC (D9): the user's OWN token, the INTROSPECTED user PRN,
    /// `InvokeModel`, and the org PRN built from the header.
    #[tokio::test]
    async fn the_oidc_self_query_uses_the_users_token_principal_and_the_org_prn() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let recorded = fake.recorded.clone();
        let (status, _) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        let rec = recorded.lock().unwrap().take().expect("is_authorized_self was called");
        assert_eq!(rec.caller_key, USER_TOKEN);
        assert_eq!(rec.principal_prn, USER_PRN);
        assert_eq!(rec.action, INVOKE_MODEL_ACTION);
        assert_eq!(rec.resource_prn, ORG_A_PRN);
    }

    /// Review Focus 2: an upper-case header queries the LOWER-case canonical org PRN.
    #[tokio::test]
    async fn an_uppercase_org_header_queries_the_lowercase_org_prn() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let recorded = fake.recorded.clone();
        let upper = ORG_A.to_uppercase();
        let (status, _) = run(fake, req_with_orgs(USER_TOKEN, &[upper.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recorded.lock().unwrap().take().expect("called").resource_prn, ORG_A_PRN);
    }

    /// Row 8: the key leg is inconclusive and the OIDC leg rejects: 503, not 401.
    #[tokio::test]
    async fn an_inconclusive_key_leg_and_a_rejected_token_is_503() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Row 9 (D5): an API key with a header for ANOTHER org keeps the key's own scope, never calls
    /// `introspect_token` (the fake panics if it does: no token outcome is configured), and logs
    /// one warning that never holds the header value.
    #[tokio::test]
    async fn an_api_key_ignores_the_org_header_and_warns_once() {
        let (logs, _guard) = capture_logs();
        let fake = happy_fake();
        let recorded = fake.recorded.clone();
        let (status, body) = run(fake, req_with_orgs(CALLER_KEY, &[ORG_B.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{CALLER_SA}|{CALLER_SCOPE}|{CALLER_KEY_ID}"));
        assert_eq!(recorded.lock().unwrap().take().expect("called").resource_prn, CALLER_SCOPE);
        let text = logs.text();
        assert_eq!(text.matches("paigasus-org ignored for an API key").count(), 1, "{text}");
        assert!(text.contains(CALLER_KEY_ID), "the warning names the key: {text}");
        assert!(!text.contains(ORG_B), "the header value is never logged: {text}");
    }

    /// Row 10 (D4): a team-only `gateway_user` grant with a header for its org: the gateway asks
    /// IAM about the ORG PRN, never the team. The fake denies, so the answer is 403.
    #[tokio::test]
    async fn a_team_only_grant_is_authorized_against_the_org_prn() {
        let fake = user_fake(user_response(&[], &[TEAM_IN_A_PRN]), AuthzOutcome::Ok(false));
        let recorded = fake.recorded.clone();
        let (status, _) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(recorded.lock().unwrap().take().expect("called").resource_prn, ORG_A_PRN);
    }

    /// Row 11: `AuthEnforce` can reject an OIDC bearer on `IsAuthorized` with `principal-inactive`
    /// or `provisioning-failed` (`authn.rs:217-241`): 401. `forbidden` keeps the 500 "possible
    /// broken self-query" mapping, and so does a detail-less `PermissionDenied`.
    #[tokio::test]
    async fn authz_permission_denied_maps_by_iam_reason() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        for (reason, want) in [
            (ErrorReason::PrincipalInactive, StatusCode::UNAUTHORIZED),
            (ErrorReason::ProvisioningFailed, StatusCode::UNAUTHORIZED),
            (ErrorReason::Forbidden, StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Rpc(Code::PermissionDenied, Some(reason_details(reason))));
            assert_eq!(status_of(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, want, "{reason:?}");
        }
        let detail_less = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(status_of(detail_less, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// A non-active user introspection is a rejected credential (401), symmetric with the key leg.
    #[tokio::test]
    async fn a_non_active_oidc_user_is_401() {
        let user = IntrospectResponse {
            status: "disabled".to_owned(),
            ..user_response(&[ORG_A_PRN], &[])
        };
        let fake = user_fake(user, AuthzOutcome::Unreachable);
        assert_eq!(status_of(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, StatusCode::UNAUTHORIZED);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo nextest run -p paigasus-gateway --no-fail-fast adapters::http::auth
```

Expected: a compile error, `cannot find value ORG_HEADER`. Temporarily add `pub const ORG_HEADER: &str = "paigasus-org";` below `INVOKE_MODEL_ACTION` and run again: the new OIDC rows fail (401 in place of 200/400/403), and `authz_permission_denied_maps_by_iam_reason` fails on `PrincipalInactive` (500 in place of 401). Keep the constant; Step 3 documents it.

- [ ] **Step 3: Write the implementation**

Replace the module doc (`auth.rs:3-25`) with:

```rust
//! The gateway's authentication + authorization middleware — the security crux of the M0 slice.
//!
//! [`require_iam_auth`] runs BEFORE the chat handler on every protected request. It accepts two
//! credentials, tried in this order — the same order as [`require_authenticated`], so the two
//! middlewares cannot drift (SMA-635 D1):
//!
//! 1. **An API key** (a service account). `IntrospectApiKey` answers `active`: the key's own
//!    `scope_prn` is the scope. A `paigasus-org` header is ignored, with one warning (D5): a key is
//!    issued under one scope, and the header must not let its holder choose another resource.
//! 2. **An OIDC access token** (a console user, or a machine client with an OIDC token).
//!    `Introspect` answers `active`: the scope is ONE organization, from the `paigasus-org`
//!    header, or inferred when the user reaches exactly one organization (D2, D3). CONSEQUENCE OF
//!    D3: a client that sends no header works while its user has one organization, and starts to
//!    get `400 org-required` when the user joins a second one. A client that knows the
//!    organization must send the header.
//!
//! Both paths then run the D9 *self-query* and attach the resolved [`CallerContext`]. Any failure
//! short-circuits with a [`GatewayError`] rendered through the OpenAI error envelope.
//!
//! A user turn costs three IAM RPCs (`IntrospectApiKey`, `Introspect`, `IsAuthorized`); an
//! API-key turn costs two. There is no cache (SMA-635 spec §4.1).
//!
//! ## The self-query invariant (D9 — the whole point)
//! The authorization call ([`Iam::is_authorized_self`]) is made with the caller's OWN bearer as
//! the credential AND the caller's OWN principal PRN (the one IAM's introspect response just
//! returned) as the queried principal. Because IAM resolves that bearer to the same principal the
//! request names, it sees a principal asking about *itself* and applies no cross-principal
//! exposure gate. The middleware sources both from the SAME inbound request, so it can never query
//! a principal other than the authenticated caller — unit tests prove the recorded authz args for
//! both credentials.
//!
//! ## Two different IAM-error → HTTP mappings
//! The introspect and authz calls map gRPC status codes DIFFERENTLY, and they diverge on
//! `PermissionDenied`: on introspect it means an inactive or unprovisioned principal (a
//! client-auth failure → 401). On the authz call it is read by its `ErrorInfo` reason:
//! `principal-inactive` and `provisioning-failed` are IAM's bearer enforcement rejecting an OIDC
//! caller (→ 401); anything else means IAM's exposure gate denied our self-query, a plumbing bug
//! (→ 500). See [`introspect_error`] and [`authz_error`].
```

Change the imports (`auth.rs:39-41`) to:

```rust
use super::error::GatewayError;
use crate::adapters::iam::{Iam, IamError};
use crate::domain::{CallerContext, Credential, OrgHeader, resolve_org};
use paigasus_proto::paigasus::iam::v1::IntrospectApiKeyResponse;
```

Below `INVOKE_MODEL_ACTION`, keep and document the constant:

```rust
/// The request header an OIDC caller names its organization with (SMA-635 D2): one organization
/// UUID in the 36-character form.
pub const ORG_HEADER: &str = "paigasus-org";
```

Replace `require_iam_auth` (`auth.rs:48-124`) with:

```rust
/// Authenticate + authorize a request before it reaches the protected handler. Wired via
/// `from_fn_with_state(app_state.iam.clone(), require_iam_auth)`; the middleware's state
/// (`Arc<dyn Iam>`) is independent of the handler's `AppState`. On success the request carries a
/// [`CallerContext`] extension; on any failure it returns the mapped [`GatewayError`].
pub async fn require_iam_auth(State(iam): State<Arc<dyn Iam>>, mut req: Request, next: Next) -> Response {
    // 1. Bearer — the ONLY accepted credential source (no cookies, no query params).
    let Some(token) = bearer(req.headers()) else {
        return GatewayError::MissingBearer.into_response();
    };

    // 2. The API-key leg. An `active` key continues on the unchanged API-key path. Anything else
    //    falls through to the OIDC leg; `api_key_inconclusive` records whether this leg failed to
    //    reach a VERDICT, with the same rule `require_authenticated` uses.
    let started = Instant::now();
    let mut api_key_inconclusive = false;
    match iam.introspect_api_key(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return api_key_caller(iam.as_ref(), &token, resp, req, next).await;
        }
        // IAM answered, and the answer was "not active" — a verdict, not an outage.
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => {
            let label = iam_result(&err);
            api_key_inconclusive = introspect_error(err) == GatewayError::IamUnavailable;
            record_iam_call("introspect", label, started);
        }
    }

    // 3. The OIDC leg. Unlike `require_authenticated`, `identity-not-provisioned` is REJECTED
    //    here: a user with no provisioned identity has no grants. `introspect_error` maps that
    //    `PermissionDenied` to a 401, and `preserve_outage` widens it to a 503 when the key leg
    //    never reached a verdict.
    let started = Instant::now();
    let user = match iam.introspect_token(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect_token", "ok", started);
            resp
        }
        Ok(_) => {
            record_iam_call("introspect_token", "denied", started);
            return preserve_outage(api_key_inconclusive, GatewayError::InvalidCredential).into_response();
        }
        Err(err) => {
            let label = iam_result(&err);
            let mapped = introspect_error(err);
            record_iam_call("introspect_token", label, started);
            return preserve_outage(api_key_inconclusive, mapped).into_response();
        }
    };

    // 4. The organization (spec §4.2). Every membership node and every grant scope is evidence
    //    for inference; the header, when present, wins and is checked by IAM in step 5.
    let node_prns: Vec<&str> = user.memberships.iter().map(|m| m.node_prn.as_str()).chain(user.role_grants.iter().map(|g| g.scope_prn.as_str())).collect();
    let org_prn = match org_header(req.headers()).and_then(|header| resolve_org(header, &node_prns).map_err(GatewayError::from)) {
        Ok(prn) => prn.canonical(),
        Err(err) => return err.into_response(),
    };
    let principal_prn = user.principal_prn;

    // 5. The self-query against the ORG (D4): an org UUID that does not exist is a Deny, not an
    //    error, so the answer does not show whether the org exists.
    if let Err(denied) = authorize_self(iam.as_ref(), &token, &principal_prn, &org_prn, None).await {
        return denied;
    }

    // 6. Attach the resolved caller and proceed.
    req.extensions_mut().insert(CallerContext {
        principal_prn,
        scope_prn: org_prn,
        credential: Credential::Oidc,
    });
    next.run(req).await
}

/// The API-key path, unchanged since SMA-446 apart from the D5 warning: the key's own
/// `scope_prn` is the scope, and a `paigasus-org` header is never read for it.
async fn api_key_caller(iam: &dyn Iam, token: &str, resp: IntrospectApiKeyResponse, mut req: Request, next: Next) -> Response {
    if resp.scope_prn.is_empty() {
        // A missing scope is a plumbing bug (introspect should always return one), surfaced as a
        // distinct 500 diagnostic rather than a silent deny. The response body stays generic.
        tracing::error!(
            principal_prn = %resp.principal_prn,
            key_id = %resp.key_id,
            "introspect returned an empty scope_prn — IAM plumbing bug (SMA-446 D11)"
        );
        return GatewayError::MissingScope.into_response();
    }
    if req.headers().contains_key(ORG_HEADER) {
        // D5. The header VALUE is never logged: it is caller input.
        tracing::warn!(key_id = %resp.key_id, "paigasus-org ignored for an API key");
    }
    if let Err(denied) = authorize_self(iam, token, &resp.principal_prn, &resp.scope_prn, Some(&resp.key_id)).await {
        return denied;
    }
    req.extensions_mut().insert(CallerContext {
        principal_prn: resp.principal_prn,
        scope_prn: resp.scope_prn,
        credential: Credential::ApiKey { key_id: resp.key_id },
    });
    next.run(req).await
}

/// The D9 self-query, shared by both credentials: the caller's OWN token as the bearer, the
/// caller's OWN introspected principal, `InvokeModel`, and the resolved scope. `Err` carries the
/// response to return.
async fn authorize_self(iam: &dyn Iam, token: &str, principal_prn: &str, scope_prn: &str, key_id: Option<&str>) -> Result<(), Response> {
    let started = Instant::now();
    match iam.is_authorized_self(token, principal_prn, INVOKE_MODEL_ACTION, scope_prn).await {
        Ok(true) => {
            record_iam_call("authorize", "ok", started);
            Ok(())
        }
        Ok(false) => {
            record_iam_call("authorize", "denied", started);
            Err(GatewayError::AuthzDenied.into_response())
        }
        Err(err) => {
            record_iam_call("authorize", iam_result(&err), started);
            let mapped = authz_error(err);
            if mapped == GatewayError::Internal {
                // An exposure-gate denial of a self-query should be impossible — log it, since this
                // is the single most important thing to see (a broken D9 self-query).
                tracing::error!(
                    principal_prn = %principal_prn,
                    key_id = ?key_id,
                    "self-query IsAuthorized returned an unexpected error mapped to 500 — possible broken self-query (SMA-446 D9)"
                );
            }
            Err(mapped.into_response())
        }
    }
}

/// Read the `paigasus-org` header into an [`OrgHeader`]. A value that is not visible ASCII is an
/// invalid header at once (`HeaderValue::to_str` fails on an obs-text byte).
fn org_header(headers: &HeaderMap) -> Result<OrgHeader<'_>, GatewayError> {
    let mut values = headers.get_all(ORG_HEADER).iter();
    let Some(first) = values.next() else {
        return Ok(OrgHeader::Absent);
    };
    if values.next().is_some() {
        return Ok(OrgHeader::Many);
    }
    first.to_str().map(OrgHeader::One).map_err(|_| GatewayError::InvalidOrgHeader)
}
```

In the `require_authenticated` doc, replace the sentence at `:145-146` (`This relaxation is scoped to THIS middleware; \`require_iam_auth\` is unchanged.`) with:

```rust
/// caller, and exposes no per-principal data, so accepting widens nothing. This relaxation is
/// scoped to THIS middleware: [`require_iam_auth`] also tries the OIDC leg (SMA-635), but REJECTS
/// an unprovisioned identity, because a user with no provisioned identity holds no grant.
```

(Keep the text before "This relaxation" on line 145 as it is; replace only the two sentences.)

Replace `is_identity_not_provisioned` and add the reason reader and two statics (`auth.rs:220-256`):

```rust
/// The reasons IAM sends on `PermissionDenied`, hoisted into `LazyLock`s because `as_wire_reason`
/// allocates. `LazyLock<String>` + `.expect(...)`, matching the registry-derived statics in
/// `paigasus-iam`'s `adapters/grpc/convert.rs:33-49`: if the registry stopped declaring a reason,
/// an `Option` would make every comparison below silently false (review finding #8).
static IDENTITY_NOT_PROVISIONED: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
static PRINCIPAL_INACTIVE: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::PrincipalInactive
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
static PROVISIONING_FAILED: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::ProvisioningFailed
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});

/// The `ErrorInfo` reason of an IAM `Status`, read only on the IAM domain — never the message
/// string. `None` when the `Status` carries no `ErrorInfo` (version skew: IAM must roll before the
/// gateway, SMA-504) or a foreign domain carries it (a foreign service must not forge IAM's reason).
fn iam_reason(status: &Status) -> Option<String> {
    // `get_error_details` returns an OWNED `ErrorDetails`; bind it before borrowing out of it.
    let details = status.get_error_details();
    let Some(info) = details.error_info() else {
        tracing::warn!("IAM returned a PermissionDenied with no ErrorInfo — rolling-upgrade skew? (SMA-504)");
        return None;
    };
    (info.domain == *paigasus_proto::error::IAM_DOMAIN).then(|| info.reason.clone())
}

/// Is this IAM `Status` specifically "validated, but not yet provisioned"? SMA-504 discharges
/// ADR-0020 D4's tripwire: the reason decides, and a `Status` with no `ErrorInfo` fails CLOSED.
fn is_identity_not_provisioned(status: &Status) -> bool {
    status.code() == Code::PermissionDenied && iam_reason(status).as_deref() == Some(IDENTITY_NOT_PROVISIONED.as_str())
}
```

Replace the `record_iam_call` doc (`:274-279`) with:

```rust
/// Record an outbound IAM call's outcome for `gateway_iam_calls_total`/`_duration_seconds`.
/// `operation` is `"introspect"` (the API-key leg, both middlewares), `"introspect_token"` (the
/// OIDC leg, both middlewares since SMA-635), or `"authorize"` (the self-query,
/// [`require_iam_auth`] only); `result` is the bounded label [`iam_result`]/the call sites above
/// produce (`"ok"`/`"denied"`/`"unavailable"`/`"error"`) — never a raw gRPC status string.
```

Replace `authz_error` (`:332-347`) with:

```rust
/// Map an [`IamError`] from the **self-query authz** call to a [`GatewayError`]. Diverges from
/// [`introspect_error`] on `PermissionDenied`, which is read by its IAM reason (SMA-635 spec §4.1
/// step 5): `principal-inactive` and `provisioning-failed` are IAM's bearer enforcement rejecting
/// an OIDC caller (`authn.rs:217-241`) → 401. Any other `PermissionDenied` — `forbidden`, or no
/// `ErrorInfo` at all — means IAM's exposure gate denied a self-query, our bug → 500.
fn authz_error(err: IamError) -> GatewayError {
    match err {
        IamError::Connect(_) => GatewayError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::PermissionDenied => match iam_reason(&status).as_deref() {
                Some(reason) if reason == PRINCIPAL_INACTIVE.as_str() || reason == PROVISIONING_FAILED.as_str() => GatewayError::InvalidCredential,
                _ => GatewayError::Internal,
            },
            Code::Unauthenticated => GatewayError::InvalidCredential,
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
    }
}
```

In `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs`, replace the doc at `:67-73` with:

```rust
    /// Introspect a caller-presented OIDC token (IAM's `AuthnService.Introspect`). **Bearer-
    /// EXEMPT**, exactly like [`Iam::introspect_api_key`]: the token is the request body, so no
    /// `authorization` metadata is attached.
    ///
    /// Used by BOTH middlewares as their second leg: capability discovery accepts a console
    /// user's own session (ADR-0020 D4), and since SMA-635 the chat path accepts an OIDC caller
    /// too (it then authorizes the user against one organization).
```

- [ ] **Step 4: Give the integration fakes a real token leg and add the OIDC rows**

In `tests/chat_proxy.rs`:

Replace the header doc's "Covered:" paragraph (`:14-19`) with:

```rust
//! Covered: missing/invalid bearer → 401; authz denied → 403; allowed non-stream verbatim (incl.
//! a non-2xx passthrough); malformed body → 400; streaming (ordered, `text/event-stream`);
//! mid-stream error → terminal SSE event (status stays 200); oversized body → 413 inside the
//! OpenAI envelope (SMA-588); IAM down → 503; an OIDC user bearer through the real router
//! (SMA-635); the request log's credential fields;
//! and the load-bearing egress-hygiene assertion (the caller's credentials never reach upstream).
```

Add a variant to `Introspect` (after `Unavailable`, `:67`):

```rust
    /// An OIDC user (SMA-635): the key leg rejects the bearer, the token leg answers an active
    /// user with one membership on [`USER_ORG_PRN`].
    User,
```

Add constants after `CALLER_KEY_ID` (`:51`):

```rust
const USER_TOKEN: &str = "user-oidc-access-token";
const USER_PRN: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0";
const USER_ORG: &str = "0190a100-0000-7000-8000-0000000000a1";
const USER_ORG_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
```

Replace the `Iam` impl for `FakeIam` (`:88-104`) with:

```rust
#[async_trait::async_trait]
impl Iam for FakeIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        match self.introspect {
            Introspect::Active => Ok(active_response()),
            Introspect::Unauthenticated | Introspect::User => Err(IamError::Rpc(Status::unauthenticated("invalid key"))),
            Introspect::Unavailable => Err(IamError::Rpc(Status::unavailable("iam is down"))),
        }
    }

    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(self.allow)
    }

    /// Since SMA-635 the chat path tries this leg after a rejected or inconclusive key leg. Real
    /// IAM answers `Unauthenticated` for a bearer that is not a JWT (`convert.rs:141`), which is
    /// every API key and every garbage credential here.
    async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
        match self.introspect {
            Introspect::User => Ok(IntrospectResponse {
                principal_prn: USER_PRN.to_owned(),
                status: "active".to_owned(),
                issuer: "https://issuer.example.com".to_owned(),
                subject: "user-1".to_owned(),
                expires_at: None,
                memberships: vec![Membership {
                    principal_prn: USER_PRN.to_owned(),
                    node_prn: USER_ORG_PRN.to_owned(),
                    ..Default::default()
                }],
                role_grants: Vec::new(),
            }),
            Introspect::Active | Introspect::Unauthenticated | Introspect::Unavailable => Err(IamError::Rpc(Status::unauthenticated("invalid bearer token"))),
        }
    }
}
```

Change the proto import (`:41`) to `use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse, Membership};`.

Add after `the_request_log_names_the_credential_and_the_scope`:

```rust
/// SMA-635: an OIDC user bearer goes through the REAL router, middleware and handler. The log
/// line says `oidc` and the org scope and has no `key_id`; the upstream sees the real OpenAI key,
/// never the user's token and never the `paigasus-org` header.
#[tokio::test]
async fn an_oidc_user_is_proxied_with_the_org_scope() {
    let (logs, _guard) = capture_logs();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(
        FakeIam {
            introspect: Introspect::User,
            allow: true,
        },
        mock.base_url.clone(),
        ONE_MIB,
    );
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {USER_TOKEN}"))
        .header("paigasus-org", USER_ORG)
        .body(Body::from(NON_STREAM_BODY.to_owned()))
        .expect("build request");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = logs.text();
    let line = text.lines().find(|l| l.contains("chat completion proxied")).expect("one request log line");
    assert!(line.contains("oidc"), "{line}");
    assert!(line.contains(USER_ORG_PRN), "{line}");
    assert!(line.contains(USER_PRN), "{line}");
    assert!(!line.contains("key_id"), "no key_id for an OIDC caller: {line}");

    let recorded = mock.recorded().expect("the upstream received the proxied request");
    assert_eq!(recorded.header("authorization"), Some(format!("Bearer {REAL_KEY}").as_str()));
    assert!(recorded.header("paigasus-org").is_none(), "the org header never reaches the upstream");
}

/// SMA-635: an OIDC user whose org header is not a UUID gets the 400 through the real router,
/// and the upstream is never called.
#[tokio::test]
async fn an_oidc_user_with_an_invalid_org_header_is_400() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(
        FakeIam {
            introspect: Introspect::User,
            allow: true,
        },
        mock.base_url.clone(),
        ONE_MIB,
    );
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {USER_TOKEN}"))
        .header("paigasus-org", "acme")
        .body(Body::from(NON_STREAM_BODY.to_owned()))
        .expect("build request");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["code"], "invalid-org-header");
    assert!(mock.recorded().is_none(), "a refused request never reaches the upstream");
}
```

In `tests/metrics.rs`, change the proto import (`:30`) to `use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse, Membership};`, add `use tonic::Status;` after it, and add after `successful_proxied_request_records_iam_and_upstream_metrics`:

```rust
const USER_PRN: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0";
const USER_ORG_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";

/// An OIDC user (SMA-635): the key leg rejects, the token leg answers an active user with one
/// org, and the self-query allows.
struct UserIam;

#[async_trait::async_trait]
impl Iam for UserIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        Err(IamError::Rpc(Status::unauthenticated("not an API key")))
    }
    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(true)
    }
    async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
        Ok(IntrospectResponse {
            principal_prn: USER_PRN.to_owned(),
            status: "active".to_owned(),
            issuer: "https://issuer.example.com".to_owned(),
            subject: "user-1".to_owned(),
            expires_at: None,
            memberships: vec![Membership {
                principal_prn: USER_PRN.to_owned(),
                node_prn: USER_ORG_PRN.to_owned(),
                ..Default::default()
            }],
            role_grants: Vec::new(),
        })
    }
}

/// SMA-635 spec §4.4: the OIDC path records the key leg as `denied` (a verdict, not an outage)
/// and the token leg as `ok`. The label NAMES do not change, so the Grafana panel keeps working.
#[tokio::test]
async fn an_oidc_request_records_a_denied_key_leg_and_an_ok_token_leg() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let cfg = OpenAiConfig {
        base_url: mock.base_url.clone(),
        api_key: SecretString::from("sk-real-openai-key".to_string()),
        extra_ca_bundle_path: None,
    };
    let openai = OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).expect("client builds");
    let handle = paigasus_observability::init("test-gateway-oidc-metrics");
    let state = AppState {
        iam: Arc::new(UserIam),
        openai: Arc::new(openai),
        max_request_bytes: 1_048_576,
        capabilities: Capabilities { chat_stream: true },
    };
    let app: Router = router(state).merge(paigasus_observability::metrics_router(handle.clone()));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer user-oidc-access-token")
        .body(Body::from(NON_STREAM_BODY))
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);

    let out = handle.render();
    let has = |operation: &str, result: &str| {
        out.lines()
            .any(|l| l.starts_with("gateway_iam_calls_total") && l.contains(&format!(r#"operation="{operation}""#)) && l.contains(&format!(r#"result="{result}""#)))
    };
    assert!(has("introspect", "denied"), "the key leg is a verdict (denied):\n{out}");
    assert!(has("introspect_token", "ok"), "the token leg succeeded:\n{out}");
    assert!(has("authorize", "ok"), "the self-query allowed:\n{out}");
}
```

In `tests/service_info.rs`, replace the comment at `:274-276` with:

```rust
    // The chat path (`require_iam_auth`) tries the OIDC leg too since SMA-635, but REJECTS an
    // unprovisioned identity there: a user with no provisioned identity has no grant. So the SAME
    // token must not gain chat access.
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-gateway
cd ..
moon run repo:error-code-single-site repo:observability-drift
```

Expected: every gateway test passes, including all pre-existing `require_authenticated` rows and the three integration binaries. `repo:observability-drift` passes (label names unchanged).

- [ ] **Step 6: Prove the new rows bite**

Temporarily change the OIDC leg's first arm guard in `require_iam_auth` from `Ok(resp) if resp.status == "active" =>` to `Ok(resp) if resp.status == "sma-635-mutation" =>`, so every user is rejected. This mutation COMPILES; a `return` in place of the block would leave unreachable code, which is a hard error under `warnings = deny` and proves nothing. Run `cargo nextest run -p paigasus-gateway --no-fail-fast adapters::http::auth`. Expected: rows 1, 2, 3/4, 5, 6, 7, 10, 11 and the uppercase row fail; the key-leg rows still pass. Restore the guard with the Edit tool (never `git checkout --`: it also reverts this task's uncommitted work), then `touch rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` and run the suite again: all pass.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs rs/crates/services/paigasus-gateway/tests/chat_proxy.rs rs/crates/services/paigasus-gateway/tests/metrics.rs rs/crates/services/paigasus-gateway/tests/service_info.rs
git commit -m "feat(rs): accept an oidc user bearer on the gateway chat surface" -m "SMA-635 spec section 4.1. require_iam_auth tries the API-key leg, then the OIDC leg, resolves one organization, and runs the self-query against the org PRN. An API key ignores the paigasus-org header with a warning. authz_error reads the IAM reason on PermissionDenied." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The terminal SSE frame starts a new record

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:51-63` (doc and constant), tests `:253-262` and a new test
- Modify: `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts:40-47`

**Interfaces:**
- Consumes: nothing new.
- Produces: `TERMINAL_SSE_ERROR` = `"\n\ndata: {\"error\":{\"message\":\"upstream stream error\",\"type\":\"api_error\",\"param\":null,\"code\":\"upstream-error\"}}\n\n"`.

- [ ] **Step 1: Write the failing test**

Add to the `chat.rs` test module:

```rust
    /// SMA-635 spec §4.6: an upstream failure INSIDE a record must not join the terminal frame to
    /// the partial record. The frame starts with a blank line, so it is a record of its own and
    /// parses as JSON by itself.
    #[tokio::test]
    async fn the_terminal_frame_is_its_own_record_after_a_partial_one() {
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"par"))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let out: Vec<Bytes> = terminal_sse_error_stream(inner).map(|r| r.unwrap()).collect().await;
        let text = String::from_utf8(out.iter().flat_map(|b| b.to_vec()).collect()).unwrap();
        let records: Vec<&str> = text.split("\n\n").filter(|record| !record.is_empty()).collect();
        let last = records.last().expect("at least one record");
        let payload = last.strip_prefix("data: ").expect("the last record is a data record of its own");
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("the terminal record parses as JSON on its own");
        assert_eq!(parsed["error"]["code"], "upstream-error");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo nextest run -p paigasus-gateway the_terminal_frame_is_its_own_record_after_a_partial_one
```

Expected: FAIL at `the terminal record parses as JSON on its own` (the frame is joined to `...par`).

- [ ] **Step 3: Write the implementation**

In `chat.rs`, replace the constant at line 63 with:

```rust
const TERMINAL_SSE_ERROR: &str = "\n\ndata: {\"error\":{\"message\":\"upstream stream error\",\"type\":\"api_error\",\"param\":null,\"code\":\"upstream-error\"}}\n\n";
```

and add to its doc comment (after line 62):

```rust
///
/// The frame STARTS with a blank line (SMA-635 spec §4.6). An upstream failure can fall inside a
/// record; without the blank line the frame would join that partial record and no SSE parser
/// could read it. After a clean record boundary the extra blank line is an empty record, which
/// dispatches nothing, so a client that reads a clean boundary sees no change.
```

Update `the_terminal_sse_frame_carries_a_registered_code` (`:257`):

```rust
        let payload = TERMINAL_SSE_ERROR.trim_start_matches('\n').strip_prefix("data: ").expect("an SSE data frame").trim_end();
```

In `ts/packages/paigasus-sdk/tests/terminal-frame.test.ts`, replace the `it` body at lines 43-47 with:

```ts
  it('is a well-formed SSE data record carrying the registry code', () => {
    // SMA-635 § 4.6: the frame opens with a blank line, so a failure inside a record cannot join
    // the frame to the partial record.
    expect(FRAME.startsWith('\n\ndata: ')).toBe(true);
    expect(FRAME.endsWith('\n\n')).toBe(true);
    expect(FRAME).toContain('"code":"upstream-error"');
  });
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-gateway
cd ..
moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck
```

Expected: all pass. `terminal_stream_emits_terminal_event_on_first_error_then_stops` and `mid_stream_error_emits_terminal_sse_event` stay green; every other `terminal-frame.test.ts` case stays green (the leading blank line is an empty record).

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs ts/packages/paigasus-sdk/tests/terminal-frame.test.ts
git commit -m "fix(rs): start the terminal sse frame with a blank line" -m "SMA-635 spec section 4.6. A failure inside a record no longer joins the terminal frame to the partial record." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Record D4 and D10 in the IAM role table

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:674` (insert two `Case`s before the closing `];` of `starter_policy_table`)

**Interfaces:**
- Consumes: `grant`, `universe`, `Case` (the existing table harness), `uni.org_o.prn()`.
- Produces: two table rows.

- [ ] **Step 1: Write the rows**

Insert before the `];` that closes `cases` in `starter_policy_table` (after the `org_admin denies CreateUser at Root` case):

```rust
            // -- SMA-635 D4 and D10. The gateway authorizes a user's InvokeModel against the ORG
            // PRN, and org_admin does not carry InvokeModel: a person needs a gateway_user grant,
            // made out of band with GrantRole. Pinned so a change to either role becomes a
            // visible test change, not a silent change of who can spend on model calls.
            Case {
                name: "org_admin does not hold InvokeModel on its own org (SMA-635 D4, D10)",
                grants: vec![grant(92, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::InvokeModel,
                resource: uni.org_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "gateway_user at an org allows InvokeModel on the org itself (SMA-635 D4)",
                grants: vec![grant(93, &uni.principal, "gateway_user", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::InvokeModel,
                resource: uni.org_o.prn().clone(),
                expect: Effect::Allow,
            },
```

- [ ] **Step 2: Run the table**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-iam-core starter_policy_table
```

Expected: PASS at once. These rows record decisions; they drive no code. If the second row FAILS, the gateway's D4 target (the org PRN itself) is not covered by an org-level `gateway_user` grant in Cedar: STOP and report to the controller, because the design depends on it.

- [ ] **Step 3: Prove the first row bites**

Temporarily add `Action::InvokeModel,` to `ORG_ADMIN_ACTIONS` and run the same command. Expected: the first new row fails, and `starter_policy_content_is_pinned_to_the_declared_revision` fails too. Remove the line with the Edit tool, then `touch rs/crates/libs/paigasus-iam-core/src/authz/roles.rs` and run again: pass.

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add rs/crates/libs/paigasus-iam-core/src/authz/roles.rs
git commit -m "test(rs): pin that org_admin does not hold invokemodel" -m "SMA-635 D4 and D10. A second row proves an org-level gateway_user grant covers the org PRN the gateway queries." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: SDK per-call `org` and `correlationId`

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/chat.ts:168-170` (interface), `:209-244` (request build)
- Modify: `ts/packages/paigasus-sdk/tests/chat.test.ts` (new `describe` block at the end)

**Interfaces:**
- Consumes: `CORRELATION_HEADER` from `./errors/map-error` (already imported).
- Produces:

```ts
export interface ChatCallOptions {
  readonly signal?: AbortSignal;
  readonly org?: string;
  readonly correlationId?: string;
}
export interface ChatClient {
  completions(request: Record<string, unknown>, options?: ChatCallOptions): Promise<ChatResult>;
}
export const ORG_HEADER = 'paigasus-org';
```

- [ ] **Step 1: Write the failing tests**

Append to `ts/packages/paigasus-sdk/tests/chat.test.ts`:

```ts
describe('per-call headers (SMA-635 § 5)', () => {
  const ORG = '0190a100-0000-7000-8000-0000000000a1';
  const CORRELATION = '11111111-2222-4333-8444-555555555555';

  function jsonFetch() {
    return vi.fn((_url: string | URL | Request, _init?: RequestInit) => Promise.resolve(new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } })));
  }

  function sentHeaders(fetchImpl: ReturnType<typeof jsonFetch>): Headers {
    const call = fetchImpl.mock.calls[0];
    if (call === undefined) throw new Error('fetch was not called');
    return new Headers(call[1]?.headers);
  }

  it('sends paigasus-org and paigasus-correlation-id when both fields are set', async () => {
    const fetchImpl = jsonFetch();
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'TOKEN' });
    await client.completions({ model: 'm', messages: [] }, { org: ORG, correlationId: CORRELATION });
    const headers = sentHeaders(fetchImpl);
    expect(headers.get('paigasus-org')).toBe(ORG);
    expect(headers.get('paigasus-correlation-id')).toBe(CORRELATION);
  });

  it('sends neither header when neither field is set', async () => {
    const fetchImpl = jsonFetch();
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'TOKEN' });
    await client.completions({ model: 'm', messages: [] });
    const headers = sentHeaders(fetchImpl);
    expect(headers.has('paigasus-org')).toBe(false);
    expect(headers.has('paigasus-correlation-id')).toBe(false);
  });

  // Without the check, fetch throws on such a value and the SDK reports a false `network` failure.
  it.each([
    ['CR', 'a\rb'],
    ['LF', 'a\nb'],
    ['NUL', 'a\u0000b'],
    ['a character outside Latin-1', 'org-€'],
  ])('refuses an org with %s as a TypeError and sends nothing', async (_label, org) => {
    const fetchImpl = jsonFetch();
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'TOKEN' });
    await expect(client.completions({ model: 'm', messages: [] }, { org })).rejects.toThrow(TypeError);
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it('refuses a correlation id with a line break as a TypeError', async () => {
    const fetchImpl = jsonFetch();
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'TOKEN' });
    await expect(client.completions({ model: 'm', messages: [] }, { correlationId: 'a\nb' })).rejects.toThrow(TypeError);
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/packages/paigasus-sdk exec vitest run tests/chat.test.ts
```

Expected: the first test fails (`expected null to be '0190a100-…'`) and the four refusal rows fail (the promise resolves).

- [ ] **Step 3: Write the implementation**

In `chat.ts`, replace the `ChatClient` interface (lines 168-170) with:

```ts
/** The `paigasus-org` request header: the organization a user's call acts in (SMA-635 D2). */
export const ORG_HEADER = 'paigasus-org';

/**
 * Per-call options.
 *
 * `org` is the organization UUID, sent as `paigasus-org`. The SDK does not check the UUID form;
 * the gateway is the authority and answers `400 invalid-org-header`. With an API key the gateway
 * IGNORES it (the key's own scope wins) and logs a warning — no error (SMA-635 D5). Without it,
 * the gateway infers the organization for a user who reaches exactly one; such a client starts to
 * get `400 org-required` when its user joins a second organization (D3), so a client that knows
 * the organization should send it.
 *
 * `correlationId` is sent as `paigasus-correlation-id`. The gateway adopts an inbound id, so the
 * caller's log and the gateway's log share one id.
 *
 * A value with CR, LF, NUL or a character above U+00FF is refused with a `TypeError`: it is not a
 * valid header value, and `fetch` would otherwise throw and read as a network failure.
 */
export interface ChatCallOptions {
  readonly signal?: AbortSignal;
  readonly org?: string;
  readonly correlationId?: string;
}

export interface ChatClient {
  completions(request: Record<string, unknown>, options?: ChatCallOptions): Promise<ChatResult>;
}

/** True when `value` cannot be a header value: NUL, CR, LF, or a character outside Latin-1. */
function invalidHeaderValue(value: string): boolean {
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i);
    if (code === 0 || code === 10 || code === 13 || code > 0xff) return true;
  }
  return false;
}
```

In `completions`, after the `JSON.stringify` try/catch (after line 228), add:

```ts
      // Refused BEFORE the timer, like the serialization above: the caller's input, not the
      // service, is at fault (SMA-635 § 5).
      const headers: Record<string, string> = { authorization: `Bearer ${auth.bearer}`, 'content-type': 'application/json' };
      for (const [name, value] of [
        [ORG_HEADER, callOptions?.org],
        [CORRELATION_HEADER, callOptions?.correlationId],
      ] as const) {
        if (value === undefined) continue;
        if (invalidHeaderValue(value)) throw new TypeError(`@paigasus/sdk: the ${name} value is not a valid HTTP header value (CR, LF, NUL or a character above U+00FF).`);
        headers[name] = value;
      }
```

and change the `fetchImpl` call's `headers` line (line 241) to `headers,`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck ts:lint
pnpm -C ts exec prettier --write packages/paigasus-sdk/src/chat.ts packages/paigasus-sdk/tests/chat.test.ts
moon run ts:fmt
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add ts/packages/paigasus-sdk/src/chat.ts ts/packages/paigasus-sdk/tests/chat.test.ts
git commit -m "feat(ts): send the org and correlation id headers from the chat client" -m "SMA-635 spec section 5. completions takes org and correlationId per call and refuses a value that is not a valid header value." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The browser's SSE parser `lib/chat-stream.ts`

**Files:**
- Create: `ts/apps/gateway-console/lib/chat-stream.ts`
- Create: `ts/apps/gateway-console/tests/unit/chat-stream.test.ts`

**Interfaces:**
- Consumes: nothing (pure; no `server-only`, no `@paigasus/*` import).
- Produces:

```ts
export type ChatStreamError = { readonly message: string; readonly correlationId: string | null; readonly reason: string | null };
export type ChatStreamEvent = { readonly kind: 'delta'; readonly text: string } | { readonly kind: 'done' } | { readonly kind: 'error'; readonly error: ChatStreamError };
export type ChatStreamParser = { push(chunk: Uint8Array): ChatStreamEvent[]; end(): ChatStreamEvent[] };
export const MAX_PENDING_RECORD: number; // 64 * 1024
export const INCOMPLETE_STREAM_MESSAGE: string; // 'The answer stopped before it was complete.'
export const GENERIC_STREAM_ERROR: string; // 'The answer failed.'
export function createChatStreamParser(): ChatStreamParser;
```

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/chat-stream.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The browser's reader of the playground stream (SMA-635 spec § 6.3, § 7.1).
import { describe, expect, it } from 'vitest';
import { GENERIC_STREAM_ERROR, INCOMPLETE_STREAM_MESSAGE, MAX_PENDING_RECORD, createChatStreamParser, type ChatStreamEvent } from '../../lib/chat-stream';

const encode = (s: string): Uint8Array => new TextEncoder().encode(s);
const delta = (text: string): string => `data: ${JSON.stringify({ choices: [{ index: 0, delta: { content: text } }] })}\n\n`;

function all(chunks: readonly (string | Uint8Array)[], end = true): ChatStreamEvent[] {
  const parser = createChatStreamParser();
  const out: ChatStreamEvent[] = [];
  for (const chunk of chunks) out.push(...parser.push(typeof chunk === 'string' ? encode(chunk) : chunk));
  if (end) out.push(...parser.end());
  return out;
}

describe('createChatStreamParser', () => {
  it('appends choices[0].delta.content and ends at [DONE]', () => {
    expect(all([delta('Hel'), delta('lo'), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'Hel' }, { kind: 'delta', text: 'lo' }, { kind: 'done' }]);
  });

  it('keeps a record split over two chunks', () => {
    const record = delta('split');
    expect(all([record.slice(0, 10), record.slice(10), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'split' }, { kind: 'done' }]);
  });

  it('keeps a multi-byte character split over two chunks', () => {
    const bytes = encode(delta('é€'));
    const cut = bytes.indexOf(0xe2) + 1; // inside the three bytes of '€'
    expect(all([bytes.slice(0, cut), bytes.slice(cut), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'é€' }, { kind: 'done' }]);
  });

  it('shows a paigasus-error event as an error and stops', () => {
    const error = { message: 'upstream stream error', correlationId: 'c-1', rawReason: 'upstream-error' };
    const out = all([delta('part'), `\n\nevent: paigasus-error\ndata: ${JSON.stringify(error)}\n\n`, delta('after')]);
    expect(out).toEqual([
      { kind: 'delta', text: 'part' },
      { kind: 'error', error: { message: 'upstream stream error', correlationId: 'c-1', reason: 'upstream-error' } },
    ]);
  });

  // The route handler injects a paigasus-error event for this frame, so the raw frame is ignored.
  it('ignores the gateway raw {"error": …} record', () => {
    const raw = '\n\ndata: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n';
    expect(all([delta('a'), raw, 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'a' }, { kind: 'done' }]);
  });

  it('reports an end with no [DONE] and no paigasus-error as an error', () => {
    expect(all([delta('cut')])).toEqual([
      { kind: 'delta', text: 'cut' },
      { kind: 'error', error: { message: INCOMPLETE_STREAM_MESSAGE, correlationId: null, reason: null } },
    ]);
  });

  it('ignores a comment line', () => {
    expect(all([': keep-alive\n\n', delta('x'), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'x' }, { kind: 'done' }]);
  });

  it('bounds the pending record and resynchronises at the next boundary', () => {
    const huge = `data: ${'x'.repeat(MAX_PENDING_RECORD + 10)}`;
    expect(all([huge, 'tail-of-the-huge-record\n\n', delta('next'), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'next' }, { kind: 'done' }]);
  });

  it('uses a generic message when a paigasus-error event carries no usable JSON', () => {
    expect(all(['event: paigasus-error\ndata: not-json\n\n'])).toEqual([{ kind: 'error', error: { message: GENERIC_STREAM_ERROR, correlationId: null, reason: null } }]);
  });

  it('accepts CRLF record delimiters', () => {
    expect(all([delta('crlf').replace(/\n\n$/, '\r\n\r\n'), 'data: [DONE]\r\n\r\n'])).toEqual([{ kind: 'delta', text: 'crlf' }, { kind: 'done' }]);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/chat-stream.test.ts
```

Expected: FAIL, `Failed to resolve import "../../lib/chat-stream"`.

- [ ] **Step 3: Write the implementation**

Create `ts/apps/gateway-console/lib/chat-stream.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The browser's reader of the playground's SSE body (SMA-635 spec § 6.3). PURE: no DOM, no
// 'server-only' and no @paigasus/* import, so the client component and a node vitest both run it.
// It never touches a token: the route handler holds the session, the browser holds only this
// stream.
//
// The rules: a `data:` record with `choices` yields its `choices[0].delta.content`; a `data:`
// record WITHOUT `choices` is ignored (this includes the gateway's raw `{"error": …}` frame, for
// which the route handler injects a `paigasus-error` event); `data: [DONE]` ends the turn; an
// `event: paigasus-error` record is an error; an end with neither is an error; a comment line is
// ignored. The pending record is bounded the way @paigasus/sdk's createTerminalFrameParser bounds
// its own, and a partial multi-byte character survives a chunk boundary (a streaming decoder).

export type ChatStreamError = { readonly message: string; readonly correlationId: string | null; readonly reason: string | null };

export type ChatStreamEvent = { readonly kind: 'delta'; readonly text: string } | { readonly kind: 'done' } | { readonly kind: 'error'; readonly error: ChatStreamError };

export type ChatStreamParser = { push(chunk: Uint8Array): ChatStreamEvent[]; end(): ChatStreamEvent[] };

/** 64 KiB, the same bound as @paigasus/sdk's terminal-frame parser (chat.ts MAX_PENDING_RECORD). */
export const MAX_PENDING_RECORD = 64 * 1024;
export const INCOMPLETE_STREAM_MESSAGE = 'The answer stopped before it was complete.';
export const GENERIC_STREAM_ERROR = 'The answer failed.';

/** A blank line ends a record; the grammar's line terminator is CRLF, LF or CR. */
const RECORD_DELIMITER = /(?:\r\n|\r|\n){2}/;

function errorOf(payload: string): ChatStreamError {
  let body: unknown;
  try {
    body = JSON.parse(payload);
  } catch {
    return { message: GENERIC_STREAM_ERROR, correlationId: null, reason: null };
  }
  if (typeof body !== 'object' || body === null) return { message: GENERIC_STREAM_ERROR, correlationId: null, reason: null };
  const fields = body as { message?: unknown; correlationId?: unknown; rawReason?: unknown };
  return {
    message: typeof fields.message === 'string' && fields.message !== '' ? fields.message : GENERIC_STREAM_ERROR,
    correlationId: typeof fields.correlationId === 'string' ? fields.correlationId : null,
    reason: typeof fields.rawReason === 'string' ? fields.rawReason : null,
  };
}

function contentOf(payload: string): string | null {
  let body: unknown;
  try {
    body = JSON.parse(payload);
  } catch {
    return null;
  }
  if (typeof body !== 'object' || body === null) return null;
  const choices = (body as { choices?: unknown }).choices;
  if (!Array.isArray(choices)) return null;
  const first: unknown = choices[0];
  const delta = typeof first === 'object' && first !== null ? (first as { delta?: unknown }).delta : undefined;
  const content = typeof delta === 'object' && delta !== null ? (delta as { content?: unknown }).content : undefined;
  return typeof content === 'string' && content !== '' ? content : null;
}

function eventOf(record: string): ChatStreamEvent | null {
  let name = 'message';
  const data: string[] = [];
  for (const line of record.split(/\r\n|\r|\n/)) {
    if (line === '' || line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') name = value;
    else if (field === 'data') data.push(value);
  }
  if (data.length === 0) return null;
  const payload = data.join('\n');
  if (name === 'paigasus-error') return { kind: 'error', error: errorOf(payload) };
  if (payload === '[DONE]') return { kind: 'done' };
  const text = contentOf(payload);
  return text === null ? null : { kind: 'delta', text };
}

export function createChatStreamParser(): ChatStreamParser {
  const decoder = new TextDecoder('utf-8');
  let buffer = '';
  let resynchronising = false;
  let finished = false;

  function drain(): ChatStreamEvent[] {
    const out: ChatStreamEvent[] = [];
    while (!finished) {
      const match = RECORD_DELIMITER.exec(buffer);
      if (match === null) break;
      const record = buffer.slice(0, match.index);
      buffer = buffer.slice(match.index + match[0].length);
      if (resynchronising) {
        resynchronising = false;
        continue;
      }
      const event = eventOf(record);
      if (event === null) continue;
      out.push(event);
      if (event.kind !== 'delta') finished = true;
    }
    if (finished) buffer = '';
    if (buffer.length > MAX_PENDING_RECORD) {
      buffer = '';
      resynchronising = true;
    }
    return out;
  }

  return {
    push(chunk) {
      if (finished) return [];
      buffer += decoder.decode(chunk, { stream: true });
      return drain();
    },
    end() {
      if (finished) return [];
      // Flush the decoder, and close a last record the producer left without a blank line.
      buffer += `${decoder.decode()}\n\n`;
      const out = drain();
      if (finished) return out;
      finished = true;
      return [...out, { kind: 'error', error: { message: INCOMPLETE_STREAM_MESSAGE, correlationId: null, reason: null } }];
    },
  };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/chat-stream.test.ts
pnpm -C ts exec prettier --write apps/gateway-console/lib/chat-stream.ts apps/gateway-console/tests/unit/chat-stream.test.ts
moon run gateway-console-ts:typecheck ts:lint ts:fmt
```

Expected: 10 tests pass; typecheck, lint and fmt pass. The full `gateway-console-ts:test` runs in Task 10.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add ts/apps/gateway-console/lib/chat-stream.ts ts/apps/gateway-console/tests/unit/chat-stream.test.ts
git commit -m "feat(ts): add the playground's sse stream parser" -m "SMA-635 spec section 6.3. A pure parser for the browser: deltas, done, the paigasus-error event, an incomplete end, and a bounded pending record." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: The chat route handler `POST /gateway/api/chat`

**Files:**
- Create: `ts/apps/gateway-console/lib/chat-route.ts`
- Create: `ts/apps/gateway-console/app/api/chat/route.ts`
- Create: `ts/apps/gateway-console/tests/unit/chat-route.test.ts`
- Modify: `ts/apps/gateway-console/proxy.ts:27-30`
- Modify: `ts/apps/gateway-console/tests/unit/proxy.test.ts:52` (and one new test after `:62`)

**Interfaces:**
- Consumes: `createChatClient`, `createTerminalFrameParser`, `ChatClient`, `ChatCallOptions`, `ChatResult` from `@paigasus/sdk/chat` (Task 8); `ErrorReason`, `mapError`, `PaigasusError` from `@paigasus/sdk/errors`; `isUuid`, `neverReachedIam`, `sessionExpired`, `requestCorrelationId` from `@paigasus/console-core`; `authRuntime` (`lib/auth.ts`), `optionalSession` (`lib/console.ts`), `getRuntimeConfig` (`lib/config.ts`).
- Produces:

```ts
export const CHAT_HEADER_TIMEOUT_MS = 35_000;
export const GATEWAY_DEFAULT_MAX_REQUEST_BYTES = 1_048_576;
export const UPSTREAM_REJECTED_MESSAGE = 'The model provider rejected the request.';
export type ChatRouteDeps = {
  readonly publicOrigin: () => Promise<string>;
  readonly session: () => Promise<{ readonly accessToken: string } | null>;
  readonly gatewayBaseUrl: () => string | null;
  readonly correlationId: () => Promise<string | null>;
  readonly chatClient: (options: { readonly baseUrl: string; readonly headerTimeoutMs: number }, auth: { readonly bearer: string }) => ChatClient;
  readonly maxBodyBytes: number;
};
export function createChatRoute(deps: ChatRouteDeps): (request: Request) => Promise<Response>;
```

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/chat-route.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The playground's route handler (SMA-635 spec § 6.2, § 7.1), driven through its factory with an
// injected chat client. No Next server, no gateway.
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChatCallOptions, ChatClient, ChatResult } from '@paigasus/sdk/chat';
import { mapError } from '@paigasus/sdk/errors';
import { logger } from '@paigasus/console-core';
import { CHAT_HEADER_TIMEOUT_MS, UPSTREAM_REJECTED_MESSAGE, createChatRoute, type ChatRouteDeps } from '../../lib/chat-route';

const ORIGIN = 'https://console.test';
const ORG = '0190a100-0000-7000-8000-0000000000e1';
const CORRELATION = '11111111-2222-4333-8444-555555555555';
const GOOD = { org: ORG, model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }] };
const TERMINAL = '\n\ndata: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n';

afterEach(() => {
  vi.restoreAllMocks();
});

function streamOf(chunks: readonly string[], opts: { error?: boolean; onCancel?: () => void } = {}): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  let i = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      const next = chunks[i];
      i += 1;
      if (next !== undefined) controller.enqueue(encoder.encode(next));
      else if (opts.error === true) controller.error(new TypeError('terminated'));
      else controller.close();
    },
    cancel() {
      opts.onCancel?.();
    },
  });
}

function setup(result: ChatResult, overrides: Partial<ChatRouteDeps> = {}) {
  const completions = vi.fn((_request: Record<string, unknown>, _options?: ChatCallOptions): Promise<ChatResult> => Promise.resolve(result));
  const client: ChatClient = { completions };
  const chatClient = vi.fn((_options: { readonly baseUrl: string; readonly headerTimeoutMs: number }, _auth: { readonly bearer: string }): ChatClient => client);
  const route = createChatRoute({
    publicOrigin: () => Promise.resolve(ORIGIN),
    session: () => Promise.resolve({ accessToken: 'session-token' }),
    gatewayBaseUrl: () => 'http://gateway.test',
    correlationId: () => Promise.resolve(CORRELATION),
    chatClient,
    maxBodyBytes: 1_000,
    ...overrides,
  });
  return { route, completions, chatClient };
}

function post(body: BodyInit | null, headers: Record<string, string> = {}): Request {
  return new Request(`${ORIGIN}/gateway/api/chat`, { method: 'POST', headers: { origin: ORIGIN, 'content-type': 'application/json', ...headers }, body, duplex: 'half' } as RequestInit);
}

/** A FRESH stream per call: a ReadableStream can be read once, so a shared constant would lock. */
const streamOk = (): ChatResult => ({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"hi"}}]}\n\n', 'data: [DONE]\n\n']), correlationId: 'gw-corr', requestId: 'gw-req' });

async function errorOf(response: Response): Promise<{ presentation: string; rawReason: string | null; correlationId: string | null; message: string }> {
  return ((await response.json()) as { error: { presentation: string; rawReason: string | null; correlationId: string | null; message: string } }).error;
}

describe('the local checks, in order', () => {
  it('answers 403 when Origin is absent or foreign, and never calls the gateway', async () => {
    const { route, completions } = setup(streamOk());
    for (const origin of [undefined, 'https://evil.test']) {
      const request = new Request(`${ORIGIN}/gateway/api/chat`, { method: 'POST', headers: { 'content-type': 'application/json', ...(origin === undefined ? {} : { origin }) }, body: JSON.stringify(GOOD) });
      const response = await route(request);
      expect(response.status).toBe(403);
      expect((await errorOf(response)).presentation).toBe('forbidden');
    }
    expect(completions).not.toHaveBeenCalled();
  });

  it('answers 415 for a body that is not application/json', async () => {
    const { route } = setup(streamOk());
    const response = await route(post(JSON.stringify(GOOD), { 'content-type': 'text/plain' }));
    expect(response.status).toBe(415);
    expect((await errorOf(response)).rawReason).toBe('unsupported-content-type');
  });

  it('accepts application/json with parameters', async () => {
    const { route } = setup(streamOk());
    expect((await route(post(JSON.stringify(GOOD), { 'content-type': 'Application/JSON; charset=utf-8' }))).status).toBe(200);
  });

  it('answers 401 JSON, not a redirect, with no session', async () => {
    const { route, completions } = setup(streamOk(), { session: () => Promise.resolve(null) });
    const response = await route(post(JSON.stringify(GOOD)));
    expect(response.status).toBe(401);
    expect(response.headers.get('location')).toBeNull();
    const error = await errorOf(response);
    expect(error.presentation).toBe('relogin');
    expect(error.correlationId).toBe(CORRELATION);
    expect(completions).not.toHaveBeenCalled();
  });

  it('answers 413 for a declared content-length over the limit', async () => {
    const { route, completions } = setup(streamOk());
    const response = await route(post(JSON.stringify(GOOD), { 'content-length': '5000' }));
    expect(response.status).toBe(413);
    expect((await errorOf(response)).rawReason).toBe('request-too-large');
    expect(completions).not.toHaveBeenCalled();
  });

  // Review Focus 3.
  it('answers 413 for a chunked body over the limit with no content-length', async () => {
    const { route, completions } = setup(streamOk());
    const response = await route(post(streamOf(['x'.repeat(600), 'x'.repeat(600)])));
    expect(response.status).toBe(413);
    expect(completions).not.toHaveBeenCalled();
  });

  it('answers 400 invalid-request-body for a body that is not JSON', async () => {
    const { route } = setup(streamOk());
    const response = await route(post('{not json'));
    expect(response.status).toBe(400);
    expect((await errorOf(response)).rawReason).toBe('invalid-request-body');
  });

  it.each([
    ['org is not a UUID', { ...GOOD, org: 'acme' }],
    ['model is empty', { ...GOOD, model: '' }],
    ['messages is empty', { ...GOOD, messages: [] }],
    ['a role is not allowed', { ...GOOD, messages: [{ role: 'tool', content: 'x' }] }],
    ['content is not a string', { ...GOOD, messages: [{ role: 'user', content: 1 }] }],
    ['an extra top-level field', { ...GOOD, temperature: 2 }],
    ['an extra message field', { ...GOOD, messages: [{ role: 'user', content: 'x', name: 'n' }] }],
  ])('answers 400 invalid-request-schema when %s', async (_label, body) => {
    const { route, completions } = setup(streamOk());
    const response = await route(post(JSON.stringify(body)));
    expect(response.status).toBe(400);
    expect((await errorOf(response)).rawReason).toBe('invalid-request-schema');
    expect(completions).not.toHaveBeenCalled();
  });
});

describe('the gateway call', () => {
  it('sends only model, messages and stream: true, with the org, the correlation id, the signal, the bearer and 35 s', async () => {
    const { route, completions, chatClient } = setup(streamOk());
    const request = post(JSON.stringify(GOOD));
    await route(request);
    expect(chatClient).toHaveBeenCalledWith({ baseUrl: 'http://gateway.test', headerTimeoutMs: CHAT_HEADER_TIMEOUT_MS }, { bearer: 'session-token' });
    const [body, options] = completions.mock.calls[0] ?? [];
    expect(body).toEqual({ model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }], stream: true });
    expect(options?.org).toBe(ORG);
    expect(options?.correlationId).toBe(CORRELATION);
    expect(options?.signal).toBe(request.signal);
  });

  it('streams with the SSE headers and relays each chunk unchanged', async () => {
    const { route } = setup(streamOk());
    const response = await route(post(JSON.stringify(GOOD)));
    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toBe('text/event-stream');
    expect(response.headers.get('cache-control')).toBe('no-cache, no-transform');
    expect(response.headers.get('x-accel-buffering')).toBe('no');
    expect(response.headers.get('paigasus-correlation-id')).toBe(CORRELATION);
    expect(await response.text()).toBe('data: {"choices":[{"delta":{"content":"hi"}}]}\n\ndata: [DONE]\n\n');
  });

  it('injects one paigasus-error event after a chunk that holds the terminal frame', async () => {
    const { route } = setup({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"par', TERMINAL]), correlationId: 'gw-corr', requestId: 'gw-req' });
    const text = await (await route(post(JSON.stringify(GOOD)))).text();
    expect(text.startsWith(`data: {"choices":[{"delta":{"content":"par${TERMINAL}`)).toBe(true);
    const match = /\n\nevent: paigasus-error\ndata: (.+)\n\n$/.exec(text);
    expect(match).not.toBeNull();
    const event = JSON.parse(match?.[1] ?? '{}') as { rawReason: string; correlationId: string };
    expect(event.rawReason).toBe('upstream-error');
    expect(event.correlationId).toBe('gw-corr');
  });

  it('injects a network paigasus-error event when the gateway stream itself fails, then closes', async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    const { route } = setup({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"a"}}]}\n\n'], { error: true }), correlationId: null, requestId: null });
    const text = await (await route(post(JSON.stringify(GOOD)))).text();
    const match = /\n\nevent: paigasus-error\ndata: (.+)\n\n$/.exec(text);
    const event = JSON.parse(match?.[1] ?? '{}') as { transport: { kind: string; cause: string }; correlationId: string };
    expect(event.transport).toEqual({ kind: 'transport', cause: 'network' });
    expect(event.correlationId).toBe(CORRELATION);
  });

  // Review Focus 4.
  it('a cancel of the response body cancels the gateway stream', async () => {
    let cancelled = false;
    const { route } = setup({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"a"}}]}\n\n', 'more'], { onCancel: () => (cancelled = true) }), correlationId: null, requestId: null });
    const response = await route(post(JSON.stringify(GOOD)));
    const reader = response.body?.getReader();
    await reader?.read();
    await reader?.cancel();
    expect(cancelled).toBe(true);
  });

  it.each([
    ['an HTTP error keeps its status', mapError({ kind: 'http', status: 403, headers: new Headers(), body: { error: { message: 'no', type: 'invalid_request_error', param: null, code: 'insufficient-permissions' } } }), 403],
    ['a timeout is 504', mapError({ kind: 'transport', cause: 'timeout', message: 'chat header timeout' }), 504],
    ['a network failure is 502', mapError({ kind: 'transport', cause: 'network', message: 'fetch failed' }), 502],
  ])('the error arm: %s', async (_label, error, status) => {
    const { route } = setup({ kind: 'error', error });
    const response = await route(post(JSON.stringify(GOOD)));
    expect(response.status).toBe(status);
    expect((await errorOf(response)).correlationId).toBe(CORRELATION);
  });

  it('replaces the upstream text of a body that is not a Paigasus envelope, and keeps the correlation id', async () => {
    const error = mapError({
      kind: 'http',
      status: 401,
      headers: new Headers({ 'paigasus-correlation-id': 'gw-corr' }),
      body: { error: { message: 'Incorrect API key provided: sk-proj-****abcd', type: 'invalid_request_error', param: null, code: 'invalid_api_key' } },
    });
    const { route } = setup({ kind: 'error', error });
    const response = await route(post(JSON.stringify(GOOD)));
    const text = await response.text();
    expect(response.status).toBe(401);
    expect(text).not.toContain('sk-proj');
    const parsed = JSON.parse(text) as { error: { message: string; correlationId: string } };
    expect(parsed.error.message).toBe(UPSTREAM_REJECTED_MESSAGE);
    expect(parsed.error.correlationId).toBe('gw-corr');
  });

  it('answers 502 for the json arm, which a stream: true request never expects', async () => {
    const { route } = setup({ kind: 'json', status: 200, body: {}, correlationId: null, requestId: null });
    expect((await route(post(JSON.stringify(GOOD)))).status).toBe(502);
  });
});
```

Add to `ts/apps/gateway-console/tests/unit/proxy.test.ts`: in the `it.each` list at line 52 append `'/gateway/api/chat'`, and after the `it.each` add:

```ts
  // publicPaths is an EXACT-match set (@paigasus/auth middleware.ts), so opening /api/chat opens
  // no other path.
  it('still sends a visitor with no cookie on /gateway/api/chat/x to login', () => {
    const res = proxy(request('/gateway/api/chat/x'));
    expect(new URL(res.headers.get('location') ?? '', ORIGIN).pathname).toBe('/gateway/auth/login');
  });
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/chat-route.test.ts tests/unit/proxy.test.ts
```

Expected: `chat-route.test.ts` fails to resolve `../../lib/chat-route`; `proxy.test.ts` fails on `lets /gateway/api/chat through with no cookie` (a login redirect).

- [ ] **Step 3: Write the implementation**

Create `ts/apps/gateway-console/lib/chat-route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The playground's route handler, as a FACTORY (SMA-635 spec § 6.2). app/api/chat/route.ts only
// connects the real dependencies; the unit tests call this factory with doubles.
//
// Order: Origin (403), content type (415), session (401), body (413 / 400), then the gateway call.
// With the Origin check, the media-type check and the SameSite=Lax session cookie, a cross-site
// form post has three separate controls against it. Every local failure is `{ "error":
// <PaigasusError> }` with the request's correlation id.
//
// The stream is relayed UNCHANGED and at once, chunk by chunk. The gateway's terminal frame and a
// failure of the gateway stream itself each produce ONE injected `event: paigasus-error` record;
// its leading blank line closes any partial record. A pull-based ReadableStream is used, not a
// TransformStream: a TransformStream is errored together with its source, so it could not inject
// an event after a source error. Its cancel() cancels the SDK body, so a browser cancel reaches
// the gateway even when request.signal does not fire (plan Task 1, M2).
import 'server-only';
import { z } from 'zod';
import { createTerminalFrameParser, type ChatClient } from '@paigasus/sdk/chat';
import { ErrorReason, mapError, type PaigasusError } from '@paigasus/sdk/errors';
import { isUuid, neverReachedIam, sessionExpired } from '@paigasus/console-core';

/** Longer than the gateway's 30 s first-byte wait (config.rs:188) plus its three IAM RPCs. */
export const CHAT_HEADER_TIMEOUT_MS = 35_000;
/** The gateway's default `max_request_bytes` (config.rs:190). The gateway enforces its own limit too. */
export const GATEWAY_DEFAULT_MAX_REQUEST_BYTES = 1_048_576;
/** Replaces an upstream message that is not a Paigasus envelope: an OpenAI 401 holds a masked piece of the gateway's key. */
export const UPSTREAM_REJECTED_MESSAGE = 'The model provider rejected the request.';

const CORRELATION_HEADER = 'paigasus-correlation-id';

export type ChatRouteDeps = {
  readonly publicOrigin: () => Promise<string>;
  readonly session: () => Promise<{ readonly accessToken: string } | null>;
  readonly gatewayBaseUrl: () => string | null;
  readonly correlationId: () => Promise<string | null>;
  readonly chatClient: (options: { readonly baseUrl: string; readonly headerTimeoutMs: number }, auth: { readonly bearer: string }) => ChatClient;
  readonly maxBodyBytes: number;
};

const ChatBody = z.strictObject({
  org: z.string().refine(isUuid),
  model: z.string().min(1),
  messages: z.array(z.strictObject({ role: z.enum(['system', 'user', 'assistant']), content: z.string() })).min(1),
});

function localError(status: number, presentation: PaigasusError['presentation'], message: string, correlationId: string | null, reason: ErrorReason | null = null, rawReason: string | null = null): PaigasusError {
  return { ...neverReachedIam({ presentation, message, transport: { kind: 'http', status } }), reason, rawReason, correlationId };
}

function errorResponse(status: number, error: PaigasusError, correlationId: string | null): Response {
  const headers = new Headers({ 'cache-control': 'no-store' });
  if (correlationId !== null) headers.set(CORRELATION_HEADER, correlationId);
  return Response.json({ error }, { status, headers });
}

function originOf(value: string | null): string | null {
  if (value === null || !URL.canParse(value)) return null;
  return new URL(value).origin;
}

function mediaTypeOf(request: Request): string {
  return (request.headers.get('content-type') ?? '').split(';')[0]?.trim().toLowerCase() ?? '';
}

type BodyRead = { readonly ok: true; readonly text: string } | { readonly ok: false; readonly tooLarge: boolean };

/** Reads at most `max` bytes. A declared or a counted size over it is 413; a read failure is not. */
async function readBounded(request: Request, max: number): Promise<BodyRead> {
  const declared = request.headers.get('content-length');
  if (declared !== null && Number(declared) > max) return { ok: false, tooLarge: true };
  if (request.body === null) return { ok: true, text: '' };
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > max) {
        await reader.cancel();
        return { ok: false, tooLarge: true };
      }
      chunks.push(value);
    }
  } catch {
    return { ok: false, tooLarge: false };
  }
  const all = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    all.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return { ok: true, text: new TextDecoder().decode(all) };
}

/** An HTTP error whose body was not a Paigasus envelope keeps its id but loses the upstream text. */
function scrub(error: PaigasusError): PaigasusError {
  return error.transport.kind === 'http' && error.reason === null ? { ...error, message: UPSTREAM_REJECTED_MESSAGE } : error;
}

function withCorrelation(error: PaigasusError, correlationId: string | null): PaigasusError {
  return error.correlationId === null ? { ...error, correlationId } : error;
}

function statusOf(error: PaigasusError): number {
  if (error.transport.kind === 'http') return error.transport.status;
  if (error.transport.kind === 'transport' && error.transport.cause === 'timeout') return 504;
  return 502;
}

function relay(source: ReadableStream<Uint8Array>, ids: { readonly correlationId: string | null; readonly requestId: string | null }): ReadableStream<Uint8Array> {
  const reader = source.getReader();
  const parser = createTerminalFrameParser(200, ids);
  const encoder = new TextEncoder();
  const event = (error: PaigasusError): Uint8Array => encoder.encode(`\n\nevent: paigasus-error\ndata: ${JSON.stringify(withCorrelation(scrub(error), ids.correlationId))}\n\n`);
  let finished = false;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (finished) return;
      let read: ReadableStreamReadResult<Uint8Array>;
      try {
        read = await reader.read();
      } catch {
        finished = true;
        controller.enqueue(event(mapError({ kind: 'transport', cause: 'network', message: 'the gateway stream failed' })));
        controller.close();
        return;
      }
      if (read.done) {
        finished = true;
        controller.close();
        return;
      }
      controller.enqueue(read.value);
      for (const error of parser.push(read.value)) controller.enqueue(event(error));
    },
    cancel(reason) {
      finished = true;
      return reader.cancel(reason);
    },
  });
}

export function createChatRoute(deps: ChatRouteDeps): (request: Request) => Promise<Response> {
  return async function chatRoute(request: Request): Promise<Response> {
    const correlationId = await deps.correlationId();
    const fail = (status: number, error: PaigasusError): Response => errorResponse(status, error, correlationId);

    const expected = originOf(await deps.publicOrigin());
    const origin = originOf(request.headers.get('origin'));
    if (origin === null || origin !== expected) return fail(403, localError(403, 'forbidden', 'The request did not come from this console.', correlationId));

    if (mediaTypeOf(request) !== 'application/json') {
      return fail(415, localError(415, 'invalid-input', 'The request body must be application/json.', correlationId, ErrorReason.UNSUPPORTED_CONTENT_TYPE, 'unsupported-content-type'));
    }

    const session = await deps.session();
    if (session === null) return fail(401, { ...sessionExpired(), correlationId });

    const read = await readBounded(request, deps.maxBodyBytes);
    if (!read.ok) {
      return read.tooLarge
        ? fail(413, localError(413, 'invalid-input', 'The request body is too large.', correlationId, ErrorReason.REQUEST_TOO_LARGE, 'request-too-large'))
        : fail(400, localError(400, 'invalid-input', 'The request body could not be read.', correlationId, ErrorReason.INVALID_REQUEST_BODY, 'invalid-request-body'));
    }
    let json: unknown;
    try {
      json = JSON.parse(read.text);
    } catch {
      return fail(400, localError(400, 'invalid-input', 'The request body is not JSON.', correlationId, ErrorReason.INVALID_REQUEST_BODY, 'invalid-request-body'));
    }
    const parsed = ChatBody.safeParse(json);
    if (!parsed.success) return fail(400, localError(400, 'invalid-input', 'The chat request has the wrong shape.', correlationId, ErrorReason.INVALID_REQUEST_SCHEMA, 'invalid-request-schema'));

    const baseUrl = deps.gatewayBaseUrl();
    if (baseUrl === null) return fail(502, localError(502, 'degraded', 'The gateway is not configured.', correlationId));

    const { org, model, messages } = parsed.data;
    const client = deps.chatClient({ baseUrl, headerTimeoutMs: CHAT_HEADER_TIMEOUT_MS }, { bearer: session.accessToken });
    // The gateway request is built HERE: no other client field is forwarded.
    const result = await client.completions({ model, messages, stream: true }, correlationId === null ? { signal: request.signal, org } : { signal: request.signal, org, correlationId });

    switch (result.kind) {
      case 'error':
        return fail(statusOf(result.error), withCorrelation(scrub(result.error), correlationId));
      case 'json':
        return fail(502, localError(502, 'degraded', 'The gateway answered without a stream.', correlationId));
      case 'stream': {
        const headers = new Headers({ 'content-type': 'text/event-stream', 'cache-control': 'no-cache, no-transform', 'x-accel-buffering': 'no' });
        if (correlationId !== null) headers.set(CORRELATION_HEADER, correlationId);
        return new Response(relay(result.body, { correlationId: result.correlationId ?? correlationId, requestId: result.requestId }), { status: 200, headers });
      }
    }
  };
}
```

Create `ts/apps/gateway-console/app/api/chat/route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// POST /gateway/api/chat (SMA-635 spec § 6.2). Public in proxy.ts: it answers its OWN 401, never a
// login redirect. The logic is lib/chat-route.ts; this file only connects the real dependencies.
// Nothing here runs at module scope except the factory call, whose dependencies are all lazy.
import { createChatClient } from '@paigasus/sdk/chat';
import { requestCorrelationId } from '@paigasus/console-core';
import { authRuntime } from '../../../lib/auth';
import { GATEWAY_DEFAULT_MAX_REQUEST_BYTES, createChatRoute } from '../../../lib/chat-route';
import { getRuntimeConfig } from '../../../lib/config';
import { optionalSession } from '../../../lib/console';

export const dynamic = 'force-dynamic';

const handle = createChatRoute({
  publicOrigin: async () => (await authRuntime()).publicOrigin,
  session: () => optionalSession(),
  gatewayBaseUrl: () => getRuntimeConfig().PAIGASUS_SERVICES['gateway'] ?? null,
  correlationId: () => requestCorrelationId(),
  chatClient: (options, auth) => createChatClient(options, auth),
  maxBodyBytes: GATEWAY_DEFAULT_MAX_REQUEST_BYTES,
});

export async function POST(request: Request): Promise<Response> {
  return handle(request);
}
```

In `ts/apps/gateway-console/proxy.ts`, change line 28 to:

```ts
  // '/api/chat' (SMA-635): the playground route answers its own 401 JSON. An exact match, so it
  // opens no other path.
  publicPaths: [...authRoutePaths(), '/', '/healthz', '/api/chat'],
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/chat-route.test.ts tests/unit/proxy.test.ts
pnpm -C ts exec prettier --write apps/gateway-console/lib/chat-route.ts apps/gateway-console/app/api/chat/route.ts apps/gateway-console/proxy.ts apps/gateway-console/tests/unit/chat-route.test.ts apps/gateway-console/tests/unit/proxy.test.ts
moon run gateway-console-ts:test gateway-console-ts:typecheck ts:lint ts:fmt
```

Expected: all pass. `gateway-console-ts:test` builds the app first (`~:build`), so the new route also passes `next build`.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add ts/apps/gateway-console/lib/chat-route.ts ts/apps/gateway-console/app/api/chat/route.ts ts/apps/gateway-console/proxy.ts ts/apps/gateway-console/tests/unit/chat-route.test.ts ts/apps/gateway-console/tests/unit/proxy.test.ts
git commit -m "feat(ts): add the playground chat route handler" -m "SMA-635 spec section 6.2. POST /gateway/api/chat checks origin, media type, session and body, calls the gateway with the user's bearer, and relays the stream with one injected paigasus-error event on a failure." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: The playground page, the `Playground` component and the org link

**Files:**
- Modify: `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts:32-44` (types), `:79-95`
- Create: `ts/apps/gateway-console/app/(console)/orgs/[org]/playground/page.tsx`
- Modify: `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx:13-16` (import), `:52` (link)
- Create: `ts/apps/gateway-console/app/_components/playground-notice.ts`
- Create: `ts/apps/gateway-console/app/_components/playground.tsx`
- Create: `ts/apps/gateway-console/tests/unit/playground-notice.test.ts`
- Create: `ts/apps/gateway-console/tests/unit/organization-head.test.ts`
- Create: `ts/apps/gateway-console/tests/unit/playground.test.tsx`
- Modify: `ts/apps/gateway-console/README.md:84` (the AC 3 limit line)

**Interfaces:**
- Consumes: `createChatStreamParser` (Task 9); `GatewayView`, `gatewayView` (`app/_components/gateway-state.ts`); `loadSettingsPrelude`; `PageError`; `lifecycleOf`, `NodeLifecycle`.
- Produces:

```ts
// load.ts
export type OrganizationHead =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | { readonly kind: 'ok'; readonly orgId: string; readonly orgPrn: string; readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle } };
export async function loadOrganizationHead(tenancy: Pick<Tenancy, 'getOrganization'>, org: string): Promise<OrganizationHead>;
// playground-notice.ts
export const STREAMING_OFF_TEXT = 'Streaming is off on this gateway.';
export const GATEWAY_UNAVAILABLE_TEXT = 'The gateway is not available.';
export type ComposerNotice = { readonly enabled: true } | { readonly enabled: false; readonly text: string };
export function composerNotice(view: GatewayView): ComposerNotice;
// playground.tsx
export const CHAT_PATH = '/gateway/api/chat';
export const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';
export function Playground(props: { readonly orgId: string; readonly notice: ComposerNotice }): ReactElement;
```

DOM contract used by the e2e rows (Task 13): labels `Model` and `Message`; buttons `Send` and `Stop`; `data-testid="composer-notice"`; `data-testid="assistant-turn"` with `data-status` in `streaming | done | stopped | failed`, holding `data-testid="turn-text"`; `data-testid="playground-error"` with `data-correlation-id`; `data-testid="playground-link"` on the org page.

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/playground-notice.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The composer rule (SMA-635 spec § 6.1).
import { describe, expect, it } from 'vitest';
import type { GatewayView } from '../../app/_components/gateway-state';
import { GATEWAY_UNAVAILABLE_TEXT, STREAMING_OFF_TEXT, composerNotice } from '../../app/_components/playground-notice';

const view = (state: GatewayView['state'], streaming: boolean): GatewayView => ({ state, reason: null, version: null, capabilities: streaming ? ['gateway.chat.stream'] : [], streaming });

describe('composerNotice', () => {
  it('enables the composer when the gateway is available with the stream capability', () => {
    expect(composerNotice(view('available', true))).toEqual({ enabled: true });
  });
  it('says streaming is off when the gateway is available without the capability', () => {
    expect(composerNotice(view('available', false))).toEqual({ enabled: false, text: STREAMING_OFF_TEXT });
  });
  it.each(['degraded', 'absent'] as const)('says the gateway is not available when it is %s', (state) => {
    expect(composerNotice(view(state, false))).toEqual({ enabled: false, text: GATEWAY_UNAVAILABLE_TEXT });
  });
});
```

Create `ts/apps/gateway-console/tests/unit/organization-head.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The org page's head, shared with the playground page (SMA-635 spec § 6.1).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Code } from '@connectrpc/connect';
import { logger } from '@paigasus/console-core';
import { denial } from '@paigasus/console-core/testing';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { loadOrganizationHead } from '../../app/(console)/orgs/[org]/load';

type Tenancy = Parameters<typeof loadOrganizationHead>[0];
const ORG = '0190A100-0000-7000-8000-0000000000E1';

afterEach(() => {
  vi.restoreAllMocks();
});

function tenancy(answer: () => Promise<unknown>): { tenancy: Tenancy; getOrganization: ReturnType<typeof vi.fn> } {
  const getOrganization = vi.fn(answer);
  return { tenancy: { getOrganization } as unknown as Tenancy, getOrganization };
}

describe('loadOrganizationHead', () => {
  it('is not-found for a value that is not a UUID, with no IAM call', async () => {
    const t = tenancy(() => Promise.resolve({}));
    expect(await loadOrganizationHead(t.tenancy, 'acme')).toEqual({ kind: 'not-found' });
    expect(t.getOrganization).not.toHaveBeenCalled();
  });

  it('answers the lower-case id and the org PRN', async () => {
    const t = tenancy(() => Promise.resolve({ organization: { prn: 'x', slug: 'acme', name: 'Acme', status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE } }));
    const head = await loadOrganizationHead(t.tenancy, ORG);
    expect(head).toMatchObject({ kind: 'ok', orgId: ORG.toLowerCase(), orgPrn: `prn:pgs:iam:::organization/${ORG.toLowerCase()}`, organization: { name: 'Acme', slug: 'acme' } });
  });

  it('is an error for a denial, and not-found for a not-found answer', async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    const denied = await loadOrganizationHead(tenancy(() => Promise.reject(denial())).tenancy, ORG);
    expect(denied.kind === 'error' ? denied.error.presentation : null).toBe('forbidden');
    expect(await loadOrganizationHead(tenancy(() => Promise.reject(denial({ code: Code.NotFound, reason: 'not-found' }))).tenancy, ORG)).toEqual({ kind: 'not-found' });
  });
});
```

Create `ts/apps/gateway-console/tests/unit/playground.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The playground client component (SMA-635 spec § 6.3). fetch is stubbed; the stream is a real
// ReadableStream, so the component's reader and the pure parser run for real.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { CHAT_PATH, MISSING_ROLE_TEXT, Playground } from '../../app/_components/playground';

const ORG = '0190a100-0000-7000-8000-0000000000e1';
const ENABLED = { enabled: true } as const;
const delta = (text: string): string => `data: ${JSON.stringify({ choices: [{ delta: { content: text } }] })}\n\n`;

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function sse(chunks: readonly string[]): Response {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    }),
    { status: 200, headers: { 'content-type': 'text/event-stream' } },
  );
}

async function send(text = 'hi'): Promise<void> {
  const user = userEvent.setup();
  await user.type(screen.getByLabelText('Model'), 'gpt-e2e');
  await user.type(screen.getByLabelText('Message'), text);
  await user.click(screen.getByRole('button', { name: 'Send' }));
}

describe('Playground', () => {
  it('disables the composer and shows the notice when the notice says so', () => {
    render(<Playground orgId={ORG} notice={{ enabled: false, text: 'Streaming is off on this gateway.' }} />);
    expect(screen.getByTestId('composer-notice').textContent).toBe('Streaming is off on this gateway.');
    expect((screen.getByLabelText('Message') as HTMLTextAreaElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('keeps Send disabled while the model field is empty', () => {
    render(<Playground orgId={ORG} notice={ENABLED} />);
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('posts org, model and the history to the route, and streams the answer', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) => Promise.resolve(sse([delta('Hello'), delta(' world'), 'data: [DONE]\n\n'])));
    vi.stubGlobal('fetch', fetchMock);
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const answer = await screen.findByTestId('assistant-turn');
    await waitFor(() => expect(answer.getAttribute('data-status')).toBe('done'));
    expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('Hello world');
    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(url).toBe(CHAT_PATH);
    expect(JSON.parse(String(init?.body))).toEqual({ org: ORG, model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }] });
  });

  it('names the missing role for insufficient-permissions, with the correlation id', async () => {
    vi.stubGlobal('fetch', () => Promise.resolve(Response.json({ error: { message: 'no', rawReason: 'insufficient-permissions', correlationId: 'cid-1' } }, { status: 403 })));
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const error = await screen.findByTestId('playground-error');
    expect(error.textContent).toContain(MISSING_ROLE_TEXT);
    expect(error.getAttribute('data-correlation-id')).toBe('cid-1');
  });

  it('Stop keeps the partial answer and marks it stopped', async () => {
    const encoder = new TextEncoder();
    vi.stubGlobal('fetch', (_url: string, init?: RequestInit) => {
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(encoder.encode(delta('partial')));
          init?.signal?.addEventListener('abort', () => controller.error(new DOMException('aborted', 'AbortError')));
        },
      });
      return Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }));
    });
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const answer = await screen.findByTestId('assistant-turn');
    await waitFor(() => expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('partial'));
    await userEvent.setup().click(screen.getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(answer.getAttribute('data-status')).toBe('stopped'));
    expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('partial');
  });

  it('does not send a failed empty turn again', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) => Promise.resolve(Response.json({ error: { message: 'down', rawReason: null, correlationId: null } }, { status: 502 })));
    vi.stubGlobal('fetch', fetchMock);
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send('first');
    await screen.findByTestId('playground-error');
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Message'), 'second');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    const second = JSON.parse(String(fetchMock.mock.calls[1]?.[1]?.body)) as { messages: { role: string; content: string }[] };
    expect(second.messages).toEqual([
      { role: 'user', content: 'first' },
      { role: 'user', content: 'second' },
    ]);
  });

  it('aborts a running turn when it unmounts', async () => {
    let seen: AbortSignal | undefined;
    vi.stubGlobal('fetch', (_url: string, init?: RequestInit) => {
      seen = init?.signal ?? undefined;
      return new Promise<Response>(() => undefined);
    });
    const view = render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    await waitFor(() => expect(seen).toBeDefined());
    view.unmount();
    expect(seen?.aborted).toBe(true);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/playground-notice.test.ts tests/unit/organization-head.test.ts tests/unit/playground.test.tsx
```

Expected: FAIL to resolve `playground-notice`, `playground`, and the export `loadOrganizationHead`.

- [ ] **Step 3: Write the implementation**

In `load.ts`, add after the `OrganizationSettings` type (after line 42):

```ts
/** The page head both the org page and the playground page load first (SMA-635 spec § 6.1). */
export type OrganizationHead =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | { readonly kind: 'ok'; readonly orgId: string; readonly orgPrn: string; readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle } };
```

Replace `loadOrganizationSettings` (lines 79-95) with:

```ts
/**
 * The URL's UUID and GetOrganization: a value that is not a UUID, an invalid-input answer (IAM's
 * prn-mismatch) and a not-found answer are all not-found; any other failure is the page's error,
 * which PageError renders as the 403 view for a denial.
 */
export async function loadOrganizationHead(tenancy: Pick<Tenancy, 'getOrganization'>, org: string): Promise<OrganizationHead> {
  if (!isUuid(org)) return { kind: 'not-found' };
  const orgId = org.toLowerCase();
  const orgPrn = organizationPrn(orgId);
  const got = await callIam(() => tenancy.getOrganization({ prn: orgPrn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' || got.error.presentation === 'not-found' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const organization = got.value.organization;
  if (organization === undefined) return { kind: 'not-found' };
  return { kind: 'ok', orgId, orgPrn, organization: { name: organization.name, slug: organization.slug, lifecycle: lifecycleOf(organization) } };
}

export async function loadOrganizationSettings(deps: OrganizationSettingsDeps, params: SettingsParams & { readonly org: string }): Promise<OrganizationSettings> {
  const head = await loadOrganizationHead(deps.tenancy, params.org);
  if (head.kind !== 'ok') return head;
  const [section, projects] = await Promise.all([
    loadServiceAccountSection(deps, { ownerPrn: head.orgPrn, lifecycle: head.organization.lifecycle, saOffset: params.saOffset, keyOffset: params.keyOffset, sa: params.sa }),
    loadProjects(deps.tenancy, head.orgPrn),
  ]);
  return { kind: 'ok', orgId: head.orgId, orgPrn: head.orgPrn, organization: head.organization, section, projects };
}
```

Create `ts/apps/gateway-console/app/_components/playground-notice.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The playground composer's rule (SMA-635 spec § 6.1, D7). Pure, like gateway-state.ts: the server
// page computes it and the client component only renders it. The route handler does NOT check the
// capability (D8): the gateway's `400 streaming-disabled` is the authority.
import type { GatewayView } from './gateway-state';

export const STREAMING_OFF_TEXT = 'Streaming is off on this gateway.';
export const GATEWAY_UNAVAILABLE_TEXT = 'The gateway is not available.';

export type ComposerNotice = { readonly enabled: true } | { readonly enabled: false; readonly text: string };

export function composerNotice(view: GatewayView): ComposerNotice {
  if (view.state !== 'available') return { enabled: false, text: GATEWAY_UNAVAILABLE_TEXT };
  if (!view.streaming) return { enabled: false, text: STREAMING_OFF_TEXT };
  return { enabled: true };
}
```

Create `ts/apps/gateway-console/app/_components/playground.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The playground (SMA-635 spec § 6.3). CLIENT component. The conversation lives in React state
// only; nothing is stored. Each turn sends the full history, a stopped turn's partial answer stays
// in it, and a failed turn with no content is not sent again. The browser never holds a token: it
// posts to the zone's own route handler, which holds the session.
'use client';

import { useEffect, useRef, useState, type FormEvent, type ReactElement } from 'react';
import { Input, Label, PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { createChatStreamParser, type ChatStreamError } from '../../lib/chat-stream';
import type { ComposerNotice } from './playground-notice';

/** The route under the zone's basePath: a browser fetch does not add the basePath. */
export const CHAT_PATH = '/gateway/api/chat';
/** D10: a person needs gateway_user at the org, granted out of band. */
export const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';
const UNREACHABLE_TEXT = 'The console could not reach the chat route.';

type Role = 'user' | 'assistant';
type TurnStatus = 'streaming' | 'done' | 'stopped' | 'failed';
type Turn = { readonly id: number; readonly role: Role; readonly content: string; readonly status: TurnStatus };
type ShownError = { readonly message: string; readonly correlationId: string | null };

function shown(message: string, correlationId: string | null, reason: string | null): ShownError {
  return { message: reason === 'insufficient-permissions' ? MISSING_ROLE_TEXT : message, correlationId };
}

async function errorOfResponse(response: Response): Promise<ShownError> {
  try {
    const body = (await response.json()) as { error?: { message?: unknown; correlationId?: unknown; rawReason?: unknown } };
    const error = body.error ?? {};
    return shown(typeof error.message === 'string' ? error.message : `HTTP ${String(response.status)}`, typeof error.correlationId === 'string' ? error.correlationId : null, typeof error.rawReason === 'string' ? error.rawReason : null);
  } catch {
    return { message: `HTTP ${String(response.status)}`, correlationId: null };
  }
}

function fromStream(error: ChatStreamError): ShownError {
  return shown(error.message, error.correlationId, error.reason);
}

export function Playground({ orgId, notice }: { readonly orgId: string; readonly notice: ComposerNotice }): ReactElement {
  const [model, setModel] = useState('');
  const [draft, setDraft] = useState('');
  const [turns, setTurns] = useState<readonly Turn[]>([]);
  const [error, setError] = useState<ShownError | null>(null);
  const [running, setRunning] = useState(false);
  const controllerRef = useRef<AbortController | null>(null);
  const nextId = useRef(0);

  // A client-side navigation away aborts the running turn, so the upstream stops too.
  useEffect(() => () => controllerRef.current?.abort(), []);

  const update = (id: number, change: (turn: Turn) => Turn): void => {
    setTurns((all) => all.map((turn) => (turn.id === id ? change(turn) : turn)));
  };

  async function send(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!notice.enabled || running || model.trim() === '' || draft.trim() === '') return;
    const history = turns.filter((turn) => !(turn.status === 'failed' && turn.content === '')).map((turn) => ({ role: turn.role, content: turn.content }));
    const question: Turn = { id: nextId.current++, role: 'user', content: draft, status: 'done' };
    const answer: Turn = { id: nextId.current++, role: 'assistant', content: '', status: 'streaming' };
    setTurns((all) => [...all, question, answer]);
    setDraft('');
    setError(null);
    setRunning(true);
    const controller = new AbortController();
    controllerRef.current = controller;
    const fail = (problem: ShownError): void => {
      setError(problem);
      update(answer.id, (turn) => ({ ...turn, status: 'failed' }));
    };
    try {
      const response = await fetch(CHAT_PATH, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ org: orgId, model: model.trim(), messages: [...history, { role: 'user', content: question.content }] }),
        signal: controller.signal,
      });
      if (!response.ok || response.body === null) {
        fail(await errorOfResponse(response));
        return;
      }
      const parser = createChatStreamParser();
      const reader = response.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        const events = done ? parser.end() : parser.push(value);
        let over = done;
        for (const item of events) {
          if (item.kind === 'delta') {
            update(answer.id, (turn) => ({ ...turn, content: turn.content + item.text }));
          } else {
            over = true;
            if (item.kind === 'done') update(answer.id, (turn) => ({ ...turn, status: 'done' }));
            else fail(fromStream(item.error));
          }
        }
        if (over) {
          if (!done) await reader.cancel();
          break;
        }
      }
    } catch {
      if (controller.signal.aborted) update(answer.id, (turn) => ({ ...turn, status: 'stopped' }));
      else fail({ message: UNREACHABLE_TEXT, correlationId: null });
    } finally {
      if (controllerRef.current === controller) controllerRef.current = null;
      setRunning(false);
    }
  }

  const disabled = !notice.enabled;
  return (
    <section className="flex flex-col gap-4" data-testid="playground">
      {notice.enabled ? null : (
        <p role="status" data-testid="composer-notice">
          {notice.text}
        </p>
      )}
      <ol className="flex flex-col gap-2">
        {turns.map((turn) => (
          <li key={turn.id} data-testid={turn.role === 'user' ? 'user-turn' : 'assistant-turn'} data-status={turn.status} className="rounded border p-2">
            <p data-testid="turn-text" className="whitespace-pre-wrap">
              {turn.content}
            </p>
            {turn.status === 'stopped' ? <p className="text-xs">Stopped</p> : null}
          </li>
        ))}
      </ol>
      {error === null ? null : (
        <div role="alert" data-testid="playground-error" data-correlation-id={error.correlationId ?? ''}>
          <p>{error.message}</p>
          {error.correlationId === null ? null : (
            <p className="text-xs">
              Reference: <code>{error.correlationId}</code>
            </p>
          )}
        </div>
      )}
      <form aria-label="Chat composer" className="flex flex-col gap-2" onSubmit={(event) => void send(event)}>
        <Label htmlFor="playground-model">Model</Label>
        <Input id="playground-model" value={model} onChange={(event) => setModel(event.target.value)} required disabled={disabled} />
        <Label htmlFor="playground-message">Message</Label>
        <textarea id="playground-message" className="rounded border p-2" rows={3} value={draft} onChange={(event) => setDraft(event.target.value)} disabled={disabled} />
        <div className="flex gap-2">
          <button type="submit" className={PRIMARY_BUTTON_CLASS} disabled={disabled || running || model.trim() === ''}>
            Send
          </button>
          <button type="button" className={SECONDARY_BUTTON_CLASS} disabled={!running} onClick={() => controllerRef.current?.abort()}>
            Stop
          </button>
        </div>
      </form>
    </section>
  );
}
```

Create `ts/apps/gateway-console/app/(console)/orgs/[org]/playground/page.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org]/playground (SMA-635 spec § 6.1). The same head as the org page: a value that
// is not a UUID is not-found, and a GetOrganization denial or absence gives the org page's
// not-found or 403 view. The gateway's discovery state decides the composer (D7).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { isUuid } from '@paigasus/console-core';
import { gatewayView } from '../../../../_components/gateway-state';
import { PageError } from '../../../../_components/page-error';
import { Playground } from '../../../../_components/playground';
import { composerNotice } from '../../../../_components/playground-notice';
import { GATEWAY_BASE_PATH } from '../../../../../lib/nav';
import { loadSettingsPrelude } from '../../../settings-prelude';
import { loadOrganizationHead } from '../load';

type Props = { params: Promise<{ org: string }> };

export default async function PlaygroundPage({ params }: Props): Promise<ReactElement> {
  const { org } = await params;
  if (!isUuid(org)) notFound();
  const { clients, gateway } = await loadSettingsPrelude();
  const head = await loadOrganizationHead(clients.tenancy, org);
  if (head.kind === 'not-found') notFound();
  if (head.kind === 'error') return <PageError error={head.error} />;
  return (
    <div className="flex flex-col gap-6 p-6" data-testid="playground-page">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: head.organization.name, href: `${GATEWAY_BASE_PATH}/orgs/${head.orgId}` }, { label: 'Playground' }]} />
      <h1 className="text-xl font-semibold">Playground</h1>
      <Playground orgId={head.orgId} notice={composerNotice(gatewayView(gateway))} />
    </div>
  );
}
```

In `app/(console)/orgs/[org]/page.tsx`, add `import { ZoneLink } from '@paigasus/app-shell';` (merge into the existing `@paigasus/app-shell` import on line 14: `import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';`), and after line 52 (`<GatewayStateLine … />`) add:

```tsx
      <p>
        <ZoneLink prefetch={false} href={`${path}/playground`} className="hover:underline" data-testid="playground-link">
          Playground
        </ZoneLink>
      </p>
```

In `ts/apps/gateway-console/README.md`, replace line 84 (the "Acceptance criterion 3 is not delivered at all" line) with:

```markdown
- Acceptance criterion 3 is delivered by SMA-635: the organization page links to `/gateway/orgs/<org>/playground`, which streams a chat completion through `POST /gateway/api/chat` with the user's own bearer. A person needs the `gateway_user` role on the organization. The console has no control that grants it to a person: an organization admin grants it out of band with IAM `GrantRole` (SMA-635 D10). The playground shows "You need the gateway_user role on this organization" until then. `org_admin` alone does not hold `InvokeModel`.
- The gateway has no rate limit and no spend budget. The playground opens model spend to every person who holds `gateway_user` (SMA-635 spec §9).
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/playground-notice.test.ts tests/unit/organization-head.test.ts tests/unit/playground.test.tsx
pnpm -C ts exec prettier --write "apps/gateway-console/app/(console)/orgs/[org]/load.ts" "apps/gateway-console/app/(console)/orgs/[org]/page.tsx" "apps/gateway-console/app/(console)/orgs/[org]/playground/page.tsx" apps/gateway-console/app/_components/playground.tsx apps/gateway-console/app/_components/playground-notice.ts apps/gateway-console/tests/unit/playground-notice.test.ts apps/gateway-console/tests/unit/organization-head.test.ts apps/gateway-console/tests/unit/playground.test.tsx apps/gateway-console/README.md
moon run gateway-console-ts:test gateway-console-ts:typecheck ts:lint ts:fmt
```

Expected: all pass. `gateway-console-ts:test` includes the existing `tests/integration/org-settings-load.test.ts`, which proves the loader refactor kept the org page's behaviour.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add "ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts" "ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx" "ts/apps/gateway-console/app/(console)/orgs/[org]/playground/page.tsx" ts/apps/gateway-console/app/_components/playground.tsx ts/apps/gateway-console/app/_components/playground-notice.ts ts/apps/gateway-console/tests/unit/playground-notice.test.ts ts/apps/gateway-console/tests/unit/organization-head.test.ts ts/apps/gateway-console/tests/unit/playground.test.tsx ts/apps/gateway-console/README.md
git commit -m "feat(ts): add the gateway console playground page" -m "SMA-635 spec sections 6.1 and 6.3. The org page links to a playground that streams a chat turn, with Stop, the composer notices and the missing-role text." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: E2E infrastructure — fake IAM default, `e2e-bin`, `test-e2e` inputs

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts:25-42` (divergence list), `:291-305` (`defaults`)
- Create: `ts/packages/paigasus-console-core/tests/integration/introspect-api-key-default.test.ts`
- Modify: `rs/crates/services/paigasus-gateway/moon.yml` (append task `e2e-bin` under `tasks:`)
- Modify: `ts/apps/gateway-console/moon.yml` (`test-e2e.deps` after line with `'iam-console-ts:build'`; `test-e2e.inputs` before the `/ts/apps/iam-console/**/*` entry)
- Modify: `ci/affected-graph/run.sh:342`, `:347`, `:377`, `:382`, `:390`, `:751`

**Interfaces:**
- Consumes: nothing new.
- Produces: the fake IAM's default `authn.introspectApiKey` answer (`Unauthenticated`, reason `invalid-token`); Moon task `paigasus-gateway-rs:e2e-bin` (writes `rs/target/debug/paigasus-gateway`, never cached); `gateway-console-ts:test-e2e` depends on it and keys on the gateway's sources.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-console-core/tests/integration/introspect-api-key-default.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-635 spec § 7.2. Real IAM answers IntrospectApiKey for a bearer that is not an API key with
// Unauthenticated + reason invalid-token (paigasus-iam convert.rs:141). The fake's default now does
// the same. Before, it answered Unimplemented, which the gateway maps to an OUTAGE, so every
// playground request took the 503 branch.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposeTransports } from '@paigasus/sdk/iam';
import { callIam } from '../../src/errors';
import { createIamClients } from '../../src/iam-clients';
import { logger } from '../../src/logger';
import { startFakeIam, type FakeIam } from '@paigasus/console-core/testing';

describe('the fake IAM introspectApiKey default', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    fake = await startFakeIam();
  });
  afterEach(async () => {
    vi.restoreAllMocks();
    await fake.close();
  });
  afterAll(() => disposeTransports());

  it('answers Unauthenticated with the reason invalid-token, as IAM does', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'unused' });
    const result = await callIam(() => clients.authn.introspectApiKey({ token: 'an-oidc-token' }));
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.rawReason).toBe('invalid-token');
    expect(result.error.transport).toMatchObject({ kind: 'grpc', code: 16 });
  });

  it('still lets a scripted handler answer', async () => {
    fake.setHandlers({ 'authn.introspectApiKey': () => ({ principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000aa', status: 'active', keyId: 'k-1', scopePrn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000e1' }) });
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'unused' });
    const result = await callIam(() => clients.authn.introspectApiKey({ token: 'pgs_sk_x' }));
    expect(result.ok).toBe(true);
  });
});
```

If `createIamClients` requires a `correlationId` field, pass `correlationId: '0198f2c1-8888-7000-8000-00000000beef'` as `error-info-round-trip.test.ts:40` does.

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/integration/introspect-api-key-default.test.ts
```

Expected: the first test fails (`rawReason` is `null`, code 12 `Unimplemented`).

- [ ] **Step 3: Change the fake's default**

In `fake-iam.ts`, in `defaults(method)` (inside the `switch`, before `default:`), add:

```ts
      // SMA-635. What IAM answers for a bearer that is not an API key (paigasus-iam convert.rs:141).
      // The real gateway tries this leg first for EVERY chat request; Unimplemented would read as
      // an IAM outage there and send every playground request to the 503 branch.
      case 'authn.introspectApiKey':
        throw iamError(Code.Unauthenticated, 'invalid-token', 'invalid bearer token');
```

In the header's divergence list (after the Cedar default-ALLOW bullet, before line 43), add:

```ts
//   - An UNSCRIPTED `authn.introspectApiKey` rejects every token as `invalid-token`. IAM also
//     accepts a real API key (a token with its configured prefix and a stored hash); the fake
//     knows no key. A test that needs an ACTIVE key must script this method (SMA-635).
```

- [ ] **Step 4: Add the uncached binary task**

Append under `tasks:` in `rs/crates/services/paigasus-gateway/moon.yml`:

```yaml
  # SMA-635. The gateway-console `playground` Playwright project runs THIS binary, so the e2e tier
  # must never run an old one. The Rust `build` task declares no `outputs` (.moon/tasks/rust.yml)
  # and CI restores both rs/target and .moon/cache, so a cache hit on `build` can leave a stale
  # binary in rs/target/debug. This task is NEVER cached: cargo's own fingerprint decides what to
  # rebuild, and a no-change run costs seconds (plan Task 1, M4). `--locked` and
  # `/rs/.cargo/config.toml` satisfy repo:affected-smoke's A8 and A10.
  e2e-bin:
    command: 'cargo build --locked --bin paigasus-gateway'
    deps: ['^:build']
    inputs:
      - '@group(sources)'
      - 'Cargo.toml'
      - '@group(upstreams)'
      - '/rs/Cargo.lock'
      - '/rs/.cargo/config.toml'
    options:
      cache: false
```

- [ ] **Step 5: Make `test-e2e` depend on it and key on the gateway's sources**

In `ts/apps/gateway-console/moon.yml`, in `test-e2e.deps`, after `- 'iam-console-ts:build'` add:

```yaml
      # SMA-635. The `playground` project runs the REAL gateway binary, built from the commit under
      # test by this uncached task. This edge SCHEDULES the build; the gateway inputs below are what
      # SELECT this task (only `inputs` confer affectedness on Moon 2.5.3).
      - 'paigasus-gateway-rs:e2e-bin'
```

In `test-e2e.inputs`, before the `# SMA-512 PR 4. The two-zone tier runs the iam-console standalone server` comment, add:

```yaml
      # SMA-635. The `playground` project runs the real gateway binary, so a gateway edit — its own
      # sources, its manifest, the lock, or any crate in its `fileGroups.upstreams` — must select
      # this task. Without these lines an edit to auth.rs runs no e2e row.
      - '/rs/crates/services/paigasus-gateway/src/**/*'
      - '/rs/crates/services/paigasus-gateway/Cargo.toml'
      - '/rs/Cargo.lock'
      - '/rs/crates/libs/paigasus-kernel/src/**/*'
      - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
      - '/rs/crates/libs/paigasus-logging/src/**/*'
      - '/rs/crates/libs/paigasus-logging/Cargo.toml'
      - '/rs/crates/libs/paigasus-observability/src/**/*'
      - '/rs/crates/libs/paigasus-observability/Cargo.toml'
      - '/rs/crates/libs/paigasus-proto-derive/src/**/*'
      - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
      - '/rs/crates/libs/paigasus-proto/src/**/*'
      - '/rs/crates/libs/paigasus-proto/Cargo.toml'
      - '/rs/crates/libs/paigasus-service-info/src/**/*'
      - '/rs/crates/libs/paigasus-service-info/Cargo.toml'
```

- [ ] **Step 6: Run the affected-graph suite and read the new rows**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
/bin/bash ci/affected-graph/run.sh
```

Expected: FAIL with an `unexpected` row `gateway-console-ts:test-e2e` in exactly these six cases: `proto->svc-info-deep`, `proto->svc-info-ci`, `lockfile->all-lint`, `lockfile->all-lint-ci`, `kernel->consumer-tasks`, `gateway->sdk`. If ANY other row appears (another case, another task, or a `missing` row), STOP and report it: the change reaches further than planned.

- [ ] **Step 7: Re-baseline the six cases**

In `ci/affected-graph/run.sh`, append `,gateway-console-ts:test-e2e` to the expected CSV of each of the six cases (the CSV string lines at `:342`, `:347`, `:377`, `:382`, `:390` and `:751`), and add this comment line directly above each of the six `run_task_case`/`run_task_case_ci` calls:

```bash
  # SMA-635: gateway-console-ts:test-e2e keys on the gateway's sources and upstreams (its playground project runs the real binary).
```

Run again:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
/bin/bash ci/affected-graph/run.sh
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/integration/introspect-api-key-default.test.ts
moon run paigasus-console-core-ts:test gateway-console-ts:test ts:lint ts:fmt
moon run paigasus-gateway-rs:e2e-bin
ls -l rs/target/debug/paigasus-gateway
```

Expected: the affected-graph suite passes; the console-core and gateway-console suites pass (`tests/unit/e2e-read-only.test.ts` included: the `test-e2e` script lines did not change); `e2e-bin` builds and the binary exists.

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add ts/packages/paigasus-console-core/testing/fake-iam.ts ts/packages/paigasus-console-core/tests/integration/introspect-api-key-default.test.ts rs/crates/services/paigasus-gateway/moon.yml ts/apps/gateway-console/moon.yml ci/affected-graph/run.sh
git commit -m "ci(ci): build the current gateway binary for the console e2e tier" -m "SMA-635 spec section 7.2. paigasus-gateway-rs:e2e-bin is uncached, gateway-console-ts:test-e2e depends on it and keys on the gateway sources, and six affected-graph cases gain that task. The fake IAM rejects an unscripted IntrospectApiKey as invalid-token, as IAM does." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: The `playground` Playwright project and rows R22 to R29

**Files:**
- Modify: `ts/apps/gateway-console/playwright.config.ts:67-70`
- Modify: `ts/apps/gateway-console/tests/e2e/support/paths.ts` (append)
- Modify: `ts/apps/gateway-console/tests/e2e/support/login.ts:19`
- Create: `ts/apps/gateway-console/tests/e2e/support/gateway-env.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/gateway-process.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/mock-openai.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/playground-harness.ts`
- Create: `ts/apps/gateway-console/tests/e2e/playground-stream.spec.ts` (R22, R23, R27)
- Create: `ts/apps/gateway-console/tests/e2e/playground-authz.spec.ts` (R24, R25, R26)
- Create: `ts/apps/gateway-console/tests/e2e/playground-token-leak.spec.ts` (R28)
- Modify: `ts/apps/gateway-console/tests/e2e/capabilities.spec.ts` (R29, append)
- Modify: `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts:3-11,18`
- Create: `ts/apps/gateway-console/tests/unit/gateway-env.test.ts`
- Modify: `ts/apps/gateway-console/README.md:76-80` (tiers and rows)

**Interfaces:**
- Consumes: `freePort`, `stop`, `waitForHealth`, `serverEnv`, `closeInOrder`, `Harness` (`support/harness.ts`); `worldHandlers`, `DEFAULT_IAM_DESCRIPTOR`, `ORG_ID`, `ORG_PRN`, `PRINCIPAL_PRN` (`support/world.ts`); `signIn` (`support/login.ts`); `startFakeIam`, `startFakeIdp`, `startTlsTerminator`, `testTls` (`@paigasus/console-core/testing`); the binary from Task 12; the DOM contract of Task 11.
- Produces:

```ts
// paths.ts
export const REPO_ROOT: string;
// gateway-env.ts (pure: no import at all, so a vitest unit test can load it)
export type GatewayLogLine = { readonly fields?: Readonly<Record<string, unknown>> };
export function gatewayEnv(values: Readonly<Record<string, string>>, parent?: NodeJS.ProcessEnv): NodeJS.ProcessEnv;
export function parseGatewayLog(output: string): GatewayLogLine[];
// gateway-process.ts
export const GATEWAY_BIN: string;
export const GATEWAY_OPENAI_KEY = 'sk-e2e-openai-key';
export type GatewayProcess = { readonly url: string; output(): string; close(): Promise<void> };
export function startGateway(opts: { readonly iamGrpcUrl: string; readonly openAiUrl: string }): Promise<GatewayProcess>;
// mock-openai.ts
export type MockMode = { readonly kind: 'complete'; readonly body: string } | { readonly kind: 'stepped'; readonly first: string; readonly rest: string } | { readonly kind: 'endless'; readonly chunk: string; readonly everyMs: number } | { readonly kind: 'break-mid-record'; readonly partial: string };
export type MockRequest = { readonly authorization: string | null; readonly body: string };
export type MockOpenAi = { readonly url: string; readonly requests: readonly MockRequest[]; setMode(mode: MockMode): void; release(): void; waitForClose(timeoutMs: number): Promise<boolean>; close(): Promise<void> };
export function delta(text: string): string;
export const DONE: string;
export function startMockOpenAi(): Promise<MockOpenAi>;
// playground-harness.ts
export type PlaygroundHarness = { readonly origin: string; readonly iam: FakeIam; readonly idp: FakeIdp; readonly mock: MockOpenAi; url(path: string): string; serverOutput(): string; gatewayLog(): readonly GatewayLogLine[]; useWorld(options?: Pick<WorldOptions, 'overrides' | 'allow'>): void };
export const test; // base.extend<{ world: undefined }, { harness: PlaygroundHarness }>
export { expect } from '@playwright/test';
```

- [ ] **Step 1: Write the failing unit tests**

Create `ts/apps/gateway-console/tests/unit/gateway-env.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Review Focus 5 (SMA-635). The e2e gateway child must see ONLY the GATEWAY_* values the harness
// gives it: an inherited RUST_LOG can silence the `chat completion proxied` line that row R22
// reads, and an inherited GATEWAY_* can reconfigure the gateway under test.
import { describe, expect, it } from 'vitest';
// gateway-env.ts has no import at all: harness.ts calls @playwright/test's `test.extend` at module
// scope, which must not run under vitest.
import { gatewayEnv, parseGatewayLog } from '../e2e/support/gateway-env';

describe('gatewayEnv', () => {
  it('drops every inherited GATEWAY_* variable and RUST_LOG, and keeps the rest', () => {
    const env = gatewayEnv({ GATEWAY_LOG_LEVEL: 'info' }, { PATH: '/bin', RUST_LOG: 'warn', GATEWAY_STREAM_ENABLED: 'false', GATEWAY_UPSTREAM__OPENAI__API_KEY: 'sk-leak', NODE_ENV: 'test', HOME: '/h' });
    expect(env['PATH']).toBe('/bin');
    expect(env['HOME']).toBe('/h');
    expect(env['RUST_LOG']).toBeUndefined();
    expect(env['GATEWAY_STREAM_ENABLED']).toBeUndefined();
    expect(env['GATEWAY_UPSTREAM__OPENAI__API_KEY']).toBeUndefined();
    expect(env['GATEWAY_LOG_LEVEL']).toBe('info');
  });
});

describe('parseGatewayLog', () => {
  it('reads JSON lines and skips everything else', () => {
    const lines = parseGatewayLog('{"fields":{"message":"chat completion proxied","auth":"oidc"}}\nplain text\n{"fields":{"mess');
    expect(lines).toEqual([{ fields: { message: 'chat completion proxied', auth: 'oidc' } }]);
  });
});
```

In `tests/unit/e2e-rows.test.ts`, change line 18 to:

```ts
const ROWS = ['R1', 'R2', 'R3', 'R4', 'R4b', 'R5', 'R6', 'R7', 'R8', 'R9', 'R10', 'R11', 'R12', 'R13', 'R14', 'R15', 'R16', 'R17', 'R18', 'R19', 'R20', 'R21', 'R22', 'R23', 'R24', 'R25', 'R26', 'R27', 'R28', 'R29'];
```

and append to the header comment (after line 11): `// SMA-635 § 7.2 adds eight: R22-R28 (the playground project, which runs the real gateway binary) and R29 (the streaming-off composer row, in the single-zone project).`

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/gateway-env.test.ts tests/unit/e2e-rows.test.ts
```

Expected: `gateway-env.test.ts` fails to resolve `gateway-env`; `e2e-rows.test.ts` fails on `R22 has exactly one test` through `R29`.

- [ ] **Step 3: Write the support modules**

Append to `tests/e2e/support/paths.ts`:

```ts
// The repository root (SMA-635): the playground project runs rs/target/debug/paigasus-gateway.
export const REPO_ROOT = fileURLToPath(new URL('../../../../../..', import.meta.url));
```

In `tests/e2e/support/login.ts`, change line 19 so that both harnesses can sign in:

```ts
export async function signIn(page: Page, harness: Pick<Harness, 'idp' | 'url'>, path = '/gateway/overview'): Promise<SignedIn> {
```

Create `tests/e2e/support/gateway-env.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The pure half of the playground's gateway child (SMA-635, Review Focus 5). NO import at all, so
// tests/unit/gateway-env.test.ts can load it under vitest: harness.ts calls @playwright/test's
// `test.extend` at module scope, which must not run there.

export type GatewayLogLine = { readonly fields?: Readonly<Record<string, unknown>> };

/** The parent's env minus RUST_LOG and every GATEWAY_* variable, plus `values`. */
export function gatewayEnv(values: Readonly<Record<string, string>>, parent: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { NODE_ENV: 'production' };
  for (const [key, value] of Object.entries(parent)) {
    if (key === 'NODE_ENV' || key === 'RUST_LOG' || key.startsWith('GATEWAY_')) continue;
    env[key] = value;
  }
  return { ...env, ...values };
}

/** Every complete JSON line of the gateway's output. A partial last line is skipped. */
export function parseGatewayLog(output: string): GatewayLogLine[] {
  const lines: GatewayLogLine[] = [];
  for (const line of output.split('\n')) {
    if (!line.startsWith('{')) continue;
    try {
      lines.push(JSON.parse(line) as GatewayLogLine);
    } catch {
      // A line the gateway has not finished writing.
    }
  }
  return lines;
}
```

Create `tests/e2e/support/gateway-process.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The REAL paigasus-gateway child of the playground project (SMA-635 spec § 7.2, D9). It is built
// from the commit under test by `paigasus-gateway-rs:e2e-bin`, configured ONLY through GATEWAY_*
// variables (config.rs:244-248), started from a directory that holds no gateway.toml, and started
// with every inherited GATEWAY_* variable and RUST_LOG removed (gateway-env.ts). It logs JSON lines
// on stdout (paigasus-logging), which row R22 reads.
//
// Read-only on disk: `existsSync` is the only fs call (tests/unit/e2e-read-only.test.ts).
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { gatewayEnv } from './gateway-env';
import { freePort, stop, waitForHealth } from './harness';
import { REPO_ROOT } from './paths';

export const GATEWAY_BIN = path.join(REPO_ROOT, 'rs', 'target', 'debug', 'paigasus-gateway');
/** The key the gateway sends upstream. The mock records it; a user token must never appear there. */
export const GATEWAY_OPENAI_KEY = 'sk-e2e-openai-key';
const MAX_START_ATTEMPTS = 3;

export type GatewayProcess = { readonly url: string; output(): string; close(): Promise<void> };

export async function startGateway(opts: { readonly iamGrpcUrl: string; readonly openAiUrl: string }): Promise<GatewayProcess> {
  if (!existsSync(GATEWAY_BIN)) throw new Error(`${GATEWAY_BIN} does not exist. Run \`moon run paigasus-gateway-rs:e2e-bin\` (gateway-console-ts:test-e2e depends on it).`);
  const cwd = path.dirname(GATEWAY_BIN);
  if (existsSync(path.join(cwd, 'gateway.toml'))) throw new Error(`${cwd} holds a gateway.toml; the e2e gateway must be configured only through GATEWAY_* variables`);
  const failures: string[] = [];
  for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt += 1) {
    const port = await freePort();
    let output = '';
    const child: ChildProcess = spawn(GATEWAY_BIN, [], {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: gatewayEnv({
        GATEWAY_HTTP_ADDR: `127.0.0.1:${String(port)}`,
        GATEWAY_LOG_LEVEL: 'info',
        GATEWAY_IAM__GRPC_ADDR: opts.iamGrpcUrl,
        GATEWAY_IAM__TLS__MODE: 'loopback_insecure',
        GATEWAY_UPSTREAM__OPENAI__BASE_URL: opts.openAiUrl,
        GATEWAY_UPSTREAM__OPENAI__API_KEY: GATEWAY_OPENAI_KEY,
        GATEWAY_METRICS__ENABLED: 'false',
        GATEWAY_STREAM_ENABLED: 'true',
      }),
    });
    child.stdout?.on('data', (chunk: Buffer) => {
      output += chunk.toString('utf8');
    });
    child.stderr?.on('data', (chunk: Buffer) => {
      output += chunk.toString('utf8');
    });
    const url = `http://127.0.0.1:${String(port)}`;
    const state = await waitForHealth(`${url}/healthz`, child, () => output, 'paigasus-gateway');
    if (state === 'ready') return { url, output: () => output, close: () => stop(child) };
    failures.push(`attempt ${String(attempt)}: the gateway exited before it answered\n${output}`);
    await stop(child);
  }
  throw new Error(`paigasus-gateway failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
}
```

Create `tests/e2e/support/mock-openai.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A mock OpenAI upstream that a test controls STEP BY STEP (SMA-635 spec § 7.2). The gateway posts
// `{base_url}/v1/chat/completions` here. Modes:
//   complete          — the whole body at once.
//   stepped           — `first`, then WAIT for release(), then `rest`. A buffering regression
//                       anywhere between here and the page hides `first` until release, and R22 fails.
//   endless           — `chunk` every `everyMs` until the connection closes; waitForClose() sees it.
//   break-mid-record  — `partial` (which ends inside a record), then the socket is destroyed, so
//                       reqwest sees a truncated chunked body.
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo, Socket } from 'node:net';

export type MockMode =
  | { readonly kind: 'complete'; readonly body: string }
  | { readonly kind: 'stepped'; readonly first: string; readonly rest: string }
  | { readonly kind: 'endless'; readonly chunk: string; readonly everyMs: number }
  | { readonly kind: 'break-mid-record'; readonly partial: string };

export type MockRequest = { readonly authorization: string | null; readonly body: string };

export type MockOpenAi = {
  readonly url: string;
  readonly requests: readonly MockRequest[];
  setMode(mode: MockMode): void;
  release(): void;
  waitForClose(timeoutMs: number): Promise<boolean>;
  close(): Promise<void>;
};

export function delta(text: string): string {
  return `data: ${JSON.stringify({ id: 'chatcmpl-e2e', object: 'chat.completion.chunk', choices: [{ index: 0, delta: { content: text } }] })}\n\n`;
}

export const DONE = 'data: [DONE]\n\n';

export async function startMockOpenAi(): Promise<MockOpenAi> {
  let mode: MockMode = { kind: 'complete', body: `${delta('ok')}${DONE}` };
  const requests: MockRequest[] = [];
  const sockets = new Set<Socket>();
  let gate: (() => void) | null = null;
  let released = false;
  let closed = false;
  const closeWaiters: (() => void)[] = [];

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    requests.push({ authorization: req.headers.authorization ?? null, body: Buffer.concat(chunks).toString('utf8') });
    const current = mode;
    closed = false;
    res.on('close', () => {
      closed = true;
      for (const waiter of closeWaiters.splice(0)) waiter();
    });
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    switch (current.kind) {
      case 'complete':
        res.end(current.body);
        return;
      case 'stepped':
        res.write(current.first);
        await new Promise<void>((resolve) => {
          if (released) {
            released = false;
            resolve();
          } else {
            gate = resolve;
          }
        });
        res.end(current.rest);
        return;
      case 'endless': {
        const timer = setInterval(() => res.write(current.chunk), current.everyMs);
        res.on('close', () => clearInterval(timer));
        res.write(current.chunk);
        return;
      }
      case 'break-mid-record':
        res.write(current.partial);
        setTimeout(() => res.socket?.destroy(), 50);
        return;
    }
  }

  const server = createServer((req, res) => {
    handle(req, res).catch(() => res.destroy());
  });
  server.on('connection', (socket: Socket) => {
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
  });
  const url = await new Promise<string>((resolve, reject) => {
    server.listen(0, '127.0.0.1', () => {
      const address = server.address() as AddressInfo | string | null;
      if (address === null || typeof address === 'string') reject(new Error('mock OpenAI: no port'));
      else resolve(`http://127.0.0.1:${String(address.port)}`);
    });
  });

  return {
    url,
    requests,
    setMode(next) {
      mode = next;
      released = false;
      gate = null;
    },
    release() {
      if (gate === null) {
        released = true;
        return;
      }
      const open = gate;
      gate = null;
      open();
    },
    waitForClose(timeoutMs) {
      if (closed) return Promise.resolve(true);
      return new Promise<boolean>((resolve) => {
        const timer = setTimeout(() => resolve(false), timeoutMs);
        closeWaiters.push(() => {
          clearTimeout(timer);
          resolve(true);
        });
      });
    },
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
```

Create `tests/e2e/support/playground-harness.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The `playground` project's stack (SMA-635 spec § 7.2, D9), ONE per worker (workers: 1):
//
//   browser --https--> TLS terminator --http--> standalone server.js --http--> REAL paigasus-gateway --http--> mock OpenAI
//                                                    |                              \--h2c--> fake IAM (gRPC)
//                                                    \--h2c--> fake IAM (gRPC), fake IAM (HTTP service-info)
//
// PAIGASUS_SERVICES.gateway is the REAL gateway, so discovery reads the gateway's own descriptor.
// No Keycloak and no container. The fake IAM is not Cedar: these rows prove the WIRING (the right
// PRN, principal, action and bearer reach IAM, and a deny is shown), never the Cedar decision.
import { spawn } from 'node:child_process';
import path from 'node:path';
import { test as base } from '@playwright/test';
import { startFakeIam, startFakeIdp, startTlsTerminator, testTls, type FakeIam, type FakeIdp } from '@paigasus/console-core/testing';
import { parseGatewayLog, type GatewayLogLine } from './gateway-env';
import { startGateway } from './gateway-process';
import { closeInOrder, freePort, serverEnv, stop, waitForHealth } from './harness';
import { DONE, delta, startMockOpenAi, type MockOpenAi } from './mock-openai';
import { STANDALONE_APP_DIR } from './paths';
import { DEFAULT_IAM_DESCRIPTOR, worldHandlers, type WorldOptions } from './world';

const isCI = !!process.env.CI;
const MAX_START_ATTEMPTS = 3;

export type PlaygroundHarness = {
  readonly origin: string;
  readonly iam: FakeIam;
  readonly idp: FakeIdp;
  readonly mock: MockOpenAi;
  url(path: string): string;
  serverOutput(): string;
  gatewayLog(): readonly GatewayLogLine[];
  useWorld(options?: Pick<WorldOptions, 'overrides' | 'allow'>): void;
};

async function startStack(): Promise<{ harness: PlaygroundHarness; close: () => Promise<void> }> {
  const tls = testTls();
  const started: (() => Promise<void>)[] = [];
  try {
    const idp = await startFakeIdp({ cert: tls });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: worldHandlers() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEFAULT_IAM_DESCRIPTOR);
    const mock = await startMockOpenAi();
    started.unshift(() => mock.close());
    const gateway = await startGateway({ iamGrpcUrl: iam.grpcUrl, openAiUrl: mock.url });
    started.unshift(() => gateway.close());

    let output = '';
    const failures: string[] = [];
    for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt += 1) {
      const port = await freePort();
      const terminator = await startTlsTerminator({ target: `http://127.0.0.1:${String(port)}`, tls });
      const closeTerminator = (): Promise<void> => terminator.close();
      started.unshift(closeTerminator);
      output = '';
      const child = spawn(process.execPath, [path.join(STANDALONE_APP_DIR, 'server.js')], {
        cwd: STANDALONE_APP_DIR,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: serverEnv({
          PORT: String(port),
          HOSTNAME: '127.0.0.1',
          NEXT_TELEMETRY_DISABLED: '1',
          NODE_EXTRA_CA_CERTS: tls.certPath,
          PAIGASUS_ZONE: 'gateway',
          PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' }),
          PAIGASUS_OIDC_ISSUER: idp.issuer,
          PAIGASUS_OIDC_CLIENT_ID: idp.clientId,
          PAIGASUS_OIDC_CLIENT_SECRET: idp.clientSecret,
          PAIGASUS_PUBLIC_ORIGIN: terminator.origin,
          PAIGASUS_SESSION_STORE: 'memory',
          PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl, gateway: gateway.url }),
          PAIGASUS_IAM_GRPC_URL: iam.grpcUrl,
          PAIGASUS_DISCOVERY_NEGATIVE_MS: '1',
          PAIGASUS_DISCOVERY_FRESH_MS: '2',
          PAIGASUS_DISCOVERY_STALE_MS: '3',
        }),
      });
      const stopChild = (): Promise<void> => stop(child);
      started.unshift(stopChild);
      child.stdout?.on('data', (chunk: Buffer) => {
        output += chunk.toString('utf8');
      });
      child.stderr?.on('data', (chunk: Buffer) => {
        output += chunk.toString('utf8');
      });
      const state = await waitForHealth(`http://127.0.0.1:${String(port)}/gateway/healthz`, child, () => output);
      if (state === 'ready') {
        const harness: PlaygroundHarness = {
          origin: terminator.origin,
          iam,
          idp,
          mock,
          url: (fullPath) => `${terminator.origin}${fullPath}`,
          serverOutput: () => output,
          gatewayLog: () => parseGatewayLog(gateway.output()),
          useWorld: (options = {}) => {
            iam.setHandlers(worldHandlers(options));
            iam.setServiceInfo(DEFAULT_IAM_DESCRIPTOR);
            mock.setMode({ kind: 'complete', body: `${delta('ok')}${DONE}` });
          },
        };
        return { harness, close: () => closeInOrder(started.splice(0)) };
      }
      failures.push(`attempt ${String(attempt)}: the server exited before it answered\n${output}`);
      started.splice(0, 2);
      await closeInOrder([stopChild, closeTerminator]);
    }
    throw new Error(`the gateway-console server failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
  } catch (error) {
    await closeInOrder(started.splice(0)).catch((closeError: unknown) => {
      console.error('the playground stack failed to start, and closing what had started failed too:', closeError);
    });
    throw error;
  }
}

export const test = base.extend<{ world: undefined }, { harness: PlaygroundHarness }>({
  harness: [
    // eslint-disable-next-line no-empty-pattern -- Playwright requires an object pattern as the first argument.
    async ({}, use) => {
      const { harness, close } = await startStack();
      try {
        await use(harness);
      } finally {
        await close();
      }
    },
    // The single-zone fixture's budget plus the gateway's own start (up to 3 attempts).
    { scope: 'worker', timeout: isCI ? 600_000 : 360_000 },
  ],
  world: [
    async ({ harness }, use) => {
      harness.useWorld();
      await use(undefined);
    },
    { auto: true },
  ],
});

export { expect } from '@playwright/test';
```

In `playwright.config.ts`, replace lines 67-70 (`projects: [...]`) with:

```ts
  // SMA-635 adds a THIRD project, `playground`: the real paigasus-gateway binary, its own fake IAM
  // and a mock OpenAI server (tests/e2e/support/playground-harness.ts). Selected by the file-name
  // prefix `playground`, with the same basename anchoring as the two above; single-zone excludes
  // that prefix, so R19, R20 and capabilities.spec.ts keep their exact call sets.
  projects: [
    { name: 'single-zone', testMatch: /[\\/](?!two-zone|playground)[^\\/]*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
    { name: 'two-zone', testMatch: /[\\/]two-zone[^\\/]*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
    { name: 'playground', testMatch: /[\\/]playground[^\\/]*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
  ],
```

- [ ] **Step 4: Write the rows**

Create `tests/e2e/playground-stream.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-635 spec § 7.2 rows 1, 2 and 6, against the REAL gateway binary.
import type { Page } from '@playwright/test';
import { signIn } from './support/login';
import { DONE, delta } from './support/mock-openai';
import { expect, test } from './support/playground-harness';
import { ORG_ID, ORG_PRN, PRINCIPAL_PRN } from './support/world';
import { GATEWAY_OPENAI_KEY } from './support/gateway-process';

const PLAYGROUND = `/gateway/orgs/${ORG_ID}/playground`;

async function send(page: Page, text = 'hello'): Promise<void> {
  await page.getByLabel('Model').fill('gpt-e2e');
  await page.getByLabel('Message').fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
}

test('R22: a streamed answer reaches the page chunk by chunk through the real gateway, logged as oidc (SMA-635 § 7.2 row 1)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'stepped', first: delta('Hello'), rest: `${delta(' world')}${DONE}` });
  const logBefore = harness.gatewayLog().length;
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  const answer = page.getByTestId('assistant-turn').last();
  // The mock holds chunk 2 until release(): a buffering regression hides chunk 1 here and fails.
  await expect(answer.getByTestId('turn-text')).toHaveText('Hello');
  await expect(answer).toHaveAttribute('data-status', 'streaming');
  harness.mock.release();
  await expect(answer.getByTestId('turn-text')).toHaveText('Hello world');
  await expect(answer).toHaveAttribute('data-status', 'done');

  // Only the real binary writes this line: the row fails if anything else served the call.
  const proxied = harness
    .gatewayLog()
    .slice(logBefore)
    .filter((line) => line.fields?.['message'] === 'chat completion proxied');
  expect(proxied).toHaveLength(1);
  expect(proxied[0]?.fields).toMatchObject({ auth: 'oidc', scope: ORG_PRN, principal: PRINCIPAL_PRN });
  expect(harness.mock.requests.at(-1)?.authorization).toBe(`Bearer ${GATEWAY_OPENAI_KEY}`);
});

test('R23: Stop closes the upstream connection and keeps the partial answer (SMA-635 § 7.2 row 2)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'endless', chunk: delta('tick '), everyMs: 100 });
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  const answer = page.getByTestId('assistant-turn').last();
  await expect(answer.getByTestId('turn-text')).toContainText('tick');
  await page.getByRole('button', { name: 'Stop' }).click();
  expect(await harness.mock.waitForClose(5_000)).toBe(true);
  await expect(answer).toHaveAttribute('data-status', 'stopped');
  await expect(answer.getByTestId('turn-text')).toContainText('tick');
});

test('R27: an upstream failure inside a record reaches the page as a paigasus-error with a correlation id (SMA-635 § 7.2 row 6)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'break-mid-record', partial: `${delta('partial ')}data: {"choices":[{"delta":{"content":"cu` });
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  const error = page.getByTestId('playground-error');
  await expect(error).toBeVisible();
  await expect(error).toHaveAttribute('data-correlation-id', /^[0-9a-f-]{36}$/);
  await expect(page.getByTestId('assistant-turn').last().getByTestId('turn-text')).toContainText('partial');
});
```

Create `tests/e2e/playground-authz.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-635 spec § 7.2 rows 3, 4 and 5. The fake IAM is not Cedar: these rows prove the wiring.
import type { Page } from '@playwright/test';
import { signIn } from './support/login';
import { expect, test } from './support/playground-harness';
import { ORG_ID, ORG_PRN, PRINCIPAL_PRN } from './support/world';

const PLAYGROUND = `/gateway/orgs/${ORG_ID}/playground`;
// app/_components/playground.tsx MISSING_ROLE_TEXT. Copied, not imported: that module is a
// React client component, and this file runs under plain Playwright.
const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';

async function send(page: Page): Promise<void> {
  await page.getByLabel('Model').fill('gpt-e2e');
  await page.getByLabel('Message').fill('hello');
  await page.getByRole('button', { name: 'Send' }).click();
}

test('R24: IAM denies InvokeModel, and the playground names the missing role (SMA-635 § 7.2 row 3)', async ({ page, harness }) => {
  // Denies ONLY InvokeModel: the page's own reads stay allowed, so the deny can only come from the chat call.
  harness.useWorld({ overrides: { 'authz.isAuthorized': (req) => ({ allowed: req.action !== 'InvokeModel', determiningPolicies: [], reason: '' }) } });
  const before = harness.mock.requests.length;
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  await expect(page.getByTestId('playground-error')).toContainText(MISSING_ROLE_TEXT);
  expect(harness.mock.requests.length).toBe(before);
});

test('R25: the gateway self-query names the org of the URL, InvokeModel, the user and the session bearer (SMA-635 § 7.2 row 4)', async ({ page, harness }) => {
  const { accessToken } = await signIn(page, harness, PLAYGROUND);
  const before = harness.iam.callsTo('authz.isAuthorized').length;
  await send(page);
  await expect(page.getByTestId('assistant-turn').last()).toHaveAttribute('data-status', 'done');
  const invoke = harness.iam
    .callsTo('authz.isAuthorized')
    .slice(before)
    .filter((call) => (call.request as { action: string }).action === 'InvokeModel');
  expect(invoke).toHaveLength(1);
  const [call] = invoke;
  const request = call?.request as { principalPrn: string; action: string; resourcePrn: string };
  expect(request.resourcePrn).toBe(ORG_PRN);
  // The e2e world's Introspect answers PRINCIPAL_PRN for the signed-in user (support/world.ts).
  expect(request.principalPrn).toBe(PRINCIPAL_PRN);
  expect(call?.token).toBe(accessToken);
});

test('R26: /gateway/api/chat with no session answers 401 JSON, not a redirect (SMA-635 § 7.2 row 5)', async ({ harness, playwright }) => {
  const api = await playwright.request.newContext({ ignoreHTTPSErrors: true });
  const before = harness.mock.requests.length;
  try {
    const response = await api.post(harness.url('/gateway/api/chat'), {
      headers: { origin: harness.origin, 'content-type': 'application/json' },
      data: { org: ORG_ID, model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }] },
      maxRedirects: 0,
    });
    expect(response.status()).toBe(401);
    expect(response.headers()['content-type']).toContain('application/json');
    const body = (await response.json()) as { error: { presentation: string } };
    expect(body.error.presentation).toBe('relogin');
  } finally {
    await api.dispose();
  }
  expect(harness.mock.requests.length).toBe(before);
});
```

Create `tests/e2e/playground-token-leak.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017 for the playground route (SMA-635 spec § 7.1): neither the SSE body of a streamed turn
// nor the JSON body of a denied turn carries the access or the refresh token. It lives in the
// playground project because the single-zone project's fake gateway serves no chat route
// (fake-gateway.ts:3-5).
import type { Page } from '@playwright/test';
import { signIn } from './support/login';
import { DONE, delta } from './support/mock-openai';
import { expect, test } from './support/playground-harness';
import { ORG_ID } from './support/world';

const PLAYGROUND = `/gateway/orgs/${ORG_ID}/playground`;

async function turn(page: Page, text: string): Promise<{ body: string; headers: string }> {
  const answered = page.waitForResponse((response) => new URL(response.url()).pathname === '/gateway/api/chat');
  await page.getByLabel('Message').fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
  const response = await answered;
  await response.finished();
  return { body: await response.text(), headers: JSON.stringify(await response.allHeaders()) };
}

test('R28: no /gateway/api/chat SSE or JSON body or header carries a token (ADR-0017, SMA-635)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'complete', body: `${delta('fine')}${DONE}` });
  const { accessToken, refreshToken } = await signIn(page, harness, PLAYGROUND);
  await page.getByLabel('Model').fill('gpt-e2e');
  const streamed = await turn(page, 'first');
  await expect(page.getByTestId('assistant-turn').last()).toHaveAttribute('data-status', 'done');

  harness.useWorld({ overrides: { 'authz.isAuthorized': (req) => ({ allowed: req.action !== 'InvokeModel', determiningPolicies: [], reason: '' }) } });
  const denied = await turn(page, 'second');

  expect(streamed.body).toContain('data:');
  expect(denied.body).toContain('insufficient-permissions');
  for (const token of [accessToken, refreshToken]) {
    expect(token.length).toBeGreaterThanOrEqual(16);
    for (const seen of [streamed.body, streamed.headers, denied.body, denied.headers]) expect(seen).not.toContain(token);
  }
});
```

Append to `tests/e2e/capabilities.spec.ts`:

```ts
test('R29: the playground composer is disabled with a notice when the gateway does not advertise gateway.chat.stream (SMA-635 § 7.2)', async ({ page, harness }) => {
  harness.useWorld({ gatewayDescriptor: { service: 'gateway', version: '0.0.0-e2e', capabilities: [] } });
  await signIn(page, harness, `/gateway/orgs/${ORG_ID}/playground`);
  await expect(page.getByTestId('composer-notice')).toHaveText('Streaming is off on this gateway.');
  await expect(page.getByLabel('Message')).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Send' })).toBeDisabled();
});
```

and add `import { ORG_ID } from './support/world';` to its imports.

In `ts/apps/gateway-console/README.md`, in the "Browser" bullet list (lines 76-80), add after the `two-zone` bullet:

```markdown
  - **`playground`** (SMA-635) — the REAL `paigasus-gateway` binary, built from the commit under test by `paigasus-gateway-rs:e2e-bin` (never cached), configured only through `GATEWAY_*` variables, with its own fake IAM and a mock OpenAI server that the test controls step by step. No container. It covers rows R22 through R28: chunk-by-chunk streaming with the gateway's own `auth=oidc` log line, Stop, the `insufficient-permissions` deny, the self-query wiring, the 401 without a session, a failure inside a record, and the token-leak scan of the chat route. The fake IAM is not Cedar: the rows prove the wiring, not the decision.
```

and change the row sentence (line 80) to end with: `…then R13–R21 for the settings pages of SMA-636, then R22–R29 for the playground of SMA-635 (R29, the streaming-off composer, is in the single-zone project).`

- [ ] **Step 5: Run the unit tests and the e2e tier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
pnpm -C ts/apps/gateway-console exec vitest run tests/unit/gateway-env.test.ts tests/unit/e2e-rows.test.ts tests/unit/e2e-read-only.test.ts tests/unit/hydration.test.ts
pnpm -C ts exec prettier --write apps/gateway-console/playwright.config.ts apps/gateway-console/tests apps/gateway-console/README.md
moon run gateway-console-ts:test gateway-console-ts:typecheck ts:lint ts:fmt
moon run gateway-console-ts:test-e2e
```

Expected: the unit tests pass; `gateway-console-ts:test-e2e` runs `paigasus-gateway-rs:e2e-bin` first, then all three projects pass, including R22 to R29. Docker must be running (the `two-zone` project). If R22 fails at `toHaveText('Hello')`, the stream is buffered somewhere: compare with Task 1 M3 before changing any row. If R23 fails at `waitForClose`, compare with Task 1 M2.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
git add ts/apps/gateway-console/playwright.config.ts ts/apps/gateway-console/tests/e2e/support/paths.ts ts/apps/gateway-console/tests/e2e/support/login.ts ts/apps/gateway-console/tests/e2e/support/gateway-env.ts ts/apps/gateway-console/tests/e2e/support/gateway-process.ts ts/apps/gateway-console/tests/e2e/support/mock-openai.ts ts/apps/gateway-console/tests/e2e/support/playground-harness.ts ts/apps/gateway-console/tests/e2e/playground-stream.spec.ts ts/apps/gateway-console/tests/e2e/playground-authz.spec.ts ts/apps/gateway-console/tests/e2e/playground-token-leak.spec.ts ts/apps/gateway-console/tests/e2e/capabilities.spec.ts ts/apps/gateway-console/tests/unit/e2e-rows.test.ts ts/apps/gateway-console/tests/unit/gateway-env.test.ts ts/apps/gateway-console/README.md
git commit -m "test(ts): run the playground rows against the real gateway binary" -m "SMA-635 spec section 7.2. A third Playwright project starts the current paigasus-gateway, a fake IAM and a step-controlled mock OpenAI. Rows R22 to R28 cover streaming, Stop, deny, the self-query wiring, the 401, a mid-record failure and the token scan; R29 covers the streaming-off composer." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: The full gate graph

**Files:**
- None, unless a gate reds. A fix goes into a new commit in the task that owns the file.

**Interfaces:**
- Consumes: every commit of Tasks 1-13.
- Produces: a green local run of the CI target set, and a list of gates re-run with the other bash.

- [ ] **Step 1: Check the bash state of this Mac**

```bash
which -a bash
/opt/homebrew/bin/bash --version | sed -n 1p
/bin/bash --version | sed -n 1p
```

Expected: `/opt/homebrew/bin/bash` 5.3.x and `/bin/bash` 3.2.57. The root CLAUDE.md ("This development Mac only") holds the split: no single local bash runs every gate.

- [ ] **Step 2: Run the CI target set under system bash 3.2**

Moon resolves `bash` through `PATH`. A bash-only shim directory puts `/bin/bash` first without moving `python3` to 3.9 (`/bin` first would break `cargo_moon_parity.py` on `tomllib`).

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
mkdir -p "$SCRATCH/bash32" && ln -sf /bin/bash "$SCRATCH/bash32/bash"
export PATH="$SCRATCH/bash32:$PATH"
git fetch origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main --include-relations
```

Expected: every task passes EXCEPT the four gates below, whose verdict under bash 3.2 is not a finding. Docker must run (`gateway-console-ts:test-e2e`, the IAM suites). `:breaking` may red in a worktree (a worktree's `.git` is a file); re-run it as in Task 3 Step 6.

On any other red, follow the root CLAUDE.md diagnosis procedure, and do step 0 (copy the Moon CI report and the task's state directory out of the repo) BEFORE any re-run.

- [ ] **Step 3: Re-run the gates that need another bash**

| Gate | Bash it needs | Why (root CLAUDE.md) | Command |
|---|---|---|---|
| `repo:affected-smoke` | `/bin/bash` 3.2 | bash 5.3.15 deadlocks on a here-string over 512 bytes on this Mac | already covered by Step 2's shim; confirm with `/bin/bash ci/affected-graph/run.sh` |
| `repo:ruff-ci` | `/opt/homebrew/bin/bash` 5.x | `mapfile` (bash 4+) | `/opt/homebrew/bin/bash ci/ruff/run.sh` |
| `repo:next-public-free` | `/opt/homebrew/bin/bash` 5.x | `mapfile` (bash 4+) | `/opt/homebrew/bin/bash ci/next-public/run.sh` |
| `repo:publish-metadata` | `/opt/homebrew/bin/bash` 5.x | `declare -A` (bash 4+); under 3.2 it dies with an empty stdout | `/opt/homebrew/bin/bash ci/publish-metadata/run.sh` |
| `repo:actionlint` | `/opt/homebrew/bin/bash` 5.x AND a healthy pipe | the full gate needs bash 5; it exits rc 2 at once when a new pipe holds only 512 bytes | `/opt/homebrew/bin/bash ci/actionlint/run.sh` |

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-635-gateway-user-auth
/bin/bash ci/affected-graph/run.sh
/opt/homebrew/bin/bash ci/ruff/run.sh
/opt/homebrew/bin/bash ci/next-public/run.sh
/opt/homebrew/bin/bash ci/publish-metadata/run.sh
/opt/homebrew/bin/bash ci/actionlint/run.sh
```

Expected: each ends with its pass line. Read `repo:actionlint`'s preflight line first: `pipe capacity 65536 bytes` gives a real local verdict; a `small` message (rc 2) means this host is in the 512-byte state and the gate has NO local verdict — record that, and let CI decide. A wall of "expected rc 0" rows or an empty stdout with a one-line `declare`/`mapfile` stderr means the wrong bash ran the gate, never a finding.

- [ ] **Step 4: Report**

Report to the controller: the Step 2 result (tasks passed, and any red with its diagnosis), the five Step 3 results, `repo:actionlint`'s preflight line, and `git log --oneline origin/main..HEAD`. Do not push; the pipeline's next stage owns that.

---

## Spec coverage map

| Spec section | Task |
|---|---|
| §1 Problem, §2 Outcome 1 | Tasks 2, 4, 5 |
| §2 Outcome 2, 3 | Tasks 9, 10, 11 |
| §2 Outcome 4 | Task 12 (`e2e-bin`), Task 13 (R22 log line) |
| §3 D1 two-leg order | Task 5 |
| §3 D2 header, D3 inference | Tasks 2, 5 |
| §3 D4 org PRN only | Tasks 5 (row 10), 7 |
| §3 D5 key ignores header | Task 5 (row 9), Task 8 (doc) |
| §3 D6 error shape, D8 no capability check | Task 10 |
| §3 D7 composer disabled | Task 11, Task 13 (R29) |
| §3 D9 playground project | Tasks 12, 13 |
| §3 D10 out-of-band grant | Task 7, Task 11 (text, README) |
| §4.1 `require_iam_auth` order and cost | Task 5 |
| §4.2 `resolve_org`, dependencies | Task 2 |
| §4.3 `CallerContext`, log line | Task 4 |
| §4.4 metrics | Task 5 (label change, `metrics.rs` row) |
| §4.5 error codes | Task 3 |
| §4.6 terminal frame | Task 6 |
| §4.7 doc comments | Task 3 (`error.rs`), Task 5 (`auth.rs`, `client.rs`, `chat_proxy.rs`, `service_info.rs`), Task 4 (`chat.rs`), Task 11 (README) |
| §4.8 Rust tests, existing rows, rows 1-11, `roles.rs` | Tasks 2, 5, 7 |
| §5 SDK | Task 8 |
| §6.1 page | Task 11 |
| §6.2 route handler | Task 10 |
| §6.3 browser | Tasks 9, 11 |
| §7.1 unit tests, existing test updates | Tasks 8, 9, 10, 11; token-leak in Task 13 (R28) |
| §7.2 e2e topology, fake IAM default, current binary, rows | Tasks 12, 13 |
| §7.3 measurements | Task 1 |
| §8 out of scope | no task (nothing is built) |
| §9 risks | Task 11 (spend in README), Task 13 (fake-not-Cedar in README and harness header), Task 1 (build time) |
| §10 consequences D3, D5, D10 | Task 5 (`auth.rs` doc), Task 8 (SDK doc), Task 11 (README, 403 text) |
| §11 changelog | no task |
