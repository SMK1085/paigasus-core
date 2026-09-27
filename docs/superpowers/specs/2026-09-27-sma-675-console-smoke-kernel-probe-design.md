# SMA-675: Make the console image smoke prove that the kernel loads

- **Linear:** [SMA-675](https://linear.app/smaschek/issue/SMA-675)
- **Branch:** `feature/sma-675-console-smoke-kernel-probe`
- **Related:** SMA-634 (PR 291, the per-zone probe route), SMA-670 (PR 310, the smoke rows and
  `ci/images/console-selftest.sh`), SMA-688 (PR 311, the release path that calls the same smoke).
- **Status:** APPROVED by Sven on 2026-09-27, with the decisions in §12 (Q1 to Q5). The spec was
  written by an unattended Stage 1 run, with the spec-challenger's findings folded in (§13). §14
  lists the edits made at approval: the decisions, and a re-check against origin/main `4051df5e`.

## 1. Problem

`smoke_consoles` in `ci/images/run.sh` sends one request per zone to a `(console)` route
(`console_probe_path_for`: `/iam/orgs`, `/gateway/overview`). It accepts only a 3xx. The comment
above that step says that this is "the only runtime proof this suite has that a console image
loads the kernel".

That claim is false. The 3xx comes from `proxy.ts`, not from the route. The proxy checks only that
the `__Host-pgs_sid` cookie is present (ADR-0017 decision 7). It imports no
`@paigasus/console-core`. A request without the cookie gets its redirect before Next routes it, so
the `(console)` layout module never loads, and the kernel's wasm never evaluates. SMA-634 measured
that a path that does not exist also answers 307.

SMA-634 also measured the obvious fix. A forged `__Host-pgs_sid` cookie passes the proxy. But every
`(console)` route then answers 500 with
`AuthConfigError: PAIGASUS_SESSION_STORE cannot be "memory" when PAIGASUS_ZONES declares more than one zone`
(`ts/packages/paigasus-auth/src/runtime.ts:129`). A wasm failure also answers 500, so that status
cannot be the control.

Consequence: the one runtime check of the console image cannot tell a healthy image from an image
whose kernel does not load.

## 2. Facts read from the code (origin/main `ad1c44b7`, re-checked on `4051df5e`)

Every path and line number below was re-checked on origin/main `4051df5e` (after SMA-705 and
SMA-695). §14 lists what changed.

| # | Fact | Where |
|---|------|-------|
| F1 | The wasm glue evaluates at module scope: `import * as wasm from "./paigasus_wasm_bg.wasm"`, then `wasm.__wbindgen_start()`. | `rs/crates/bindings/paigasus-wasm/paigasus_wasm.js` |
| F2 | `@paigasus/kernel`'s `.` export is `src/wasm.ts` under every condition. It re-exports the glue. | `ts/packages/paigasus-kernel/package.json` |
| F3 | `@paigasus/console-core` imports the kernel in `prn-tenancy.ts`. Its package has no `sideEffects: false`, so a bundler keeps that module. | `ts/packages/paigasus-console-core/src/prn-tenancy.ts:14` |
| F4 | Both `(console)` layouts import `@paigasus/console-core` and `lib/console.ts` (which calls `createConsoleRuntime`). The first thing the layout does is `currentSession()`, which redirects to login when there is no session. | `ts/apps/*/app/(console)/layout.tsx` |
| F5 | The Turbopack build emits the wasm as a separate file `*paigasus_wasm_bg*.wasm` under `.next/server/chunks`. The console build and the Dockerfile builder stage both check it. The image copies the standalone tree to `/app`. | `ts/apps/*/moon.yml:91-110`, `ts/Dockerfile:48,75` |
| F6 | The Redis session store reads key `pgs:sess:<sid>` (key prefix `''`) and accepts a JSON body only when `isSessionRecord` passes. Any other body is deleted and read as absent. | `ts/packages/paigasus-auth/src/adapters/redis-store.ts:123,163-186`, `core/session.ts` |
| F7 | A record is served without a refresh when `now < absoluteExpiresAt` and `now + skew < accessExpiresAt` (skew default 30 s). No IdP call happens then. | `core/single-flight.ts:181-186` |
| F8 | IAM calls return an `IamResult`, not a throw. `mayI()` with no principal PRN, `myScopes()` with a failed WhoAmI, and the discovery probe each give a failure value that the page renders (`PageError`, `SectionError`, `gatewayView`). | `ts/packages/paigasus-console-core/src/runtime.ts:98-117`, `app/(console)/orgs/page.tsx`, `app/(console)/overview/page.tsx` |
| F9 | The repo already pins a Redis image by digest for the kind tier: `redis:7.4-alpine@sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499`. | `ci/kind/manifests/redis.yaml:23` |
| F10 | `smoke_consoles` runs from `all-consoles` (`images.yml` "Build + smoke both consoles") and from `smoke <console-key>` (`images.yml` "Console release sequence", after `load-oci`). It also runs in each console `images-build-<key>` job of `release.yml`, on amd64 AND arm64. All three paths get the Redis sidecar and the route row. Only the two `images.yml` paths get the D4 control (§12 Q5). A PR runs `images.yml` on amd64 only; arm64 runs on `main` and on a `workflow_dispatch`. | `.github/workflows/images.yml:88,185-213`, `.github/workflows/release.yml:1246` |
| F11 | `ci/images/console-selftest.sh` copies named functions out of `run.sh` with awk and runs a mutation row for each named `::error::`. A new row function must be added to its `FUNCS`. It also pins each row CALL line in `smoke_consoles` as a whole line (`pin_rows P1a`, `P1b`, `P1c`, `P2`). | `ci/images/console-selftest.sh:33-38,562-623` |
| F12 | The root layout of each app sets `metadata.title` to `Paigasus IAM` / `Paigasus AI Gateway`. So every HTML page of the app holds the brand text, also `(public)`, `error.tsx` and not-found pages. The brand text is NOT a `(console)` marker. | `ts/apps/*/app/layout.tsx:9` |
| F13 | The `(console)` layout passes `toSessionView(session)` to `Providers`, a `'use client'` component. `toSessionView` copies `idTokenClaims.email` into the view. So a value in the seeded record's `email` claim reaches the RSC payload in the HTML only when the `(console)` layout ran and read that record. | `ts/apps/*/app/(console)/layout.tsx`, `ts/apps/*/app/providers.tsx`, `ts/packages/paigasus-auth/src/core/session.ts:65-70` |
| F14 | The proxy's login redirect sets `returnTo=<basePath><pathname>` (`src/middleware.ts:134`). `requireSession` sets `returnTo=<basePath>/` (`src/next/get-session.ts:133`). So the two 3xx causes have different `Location` values. | `ts/packages/paigasus-auth/src/middleware.ts:134`, `src/next/get-session.ts:133` |
| F15 | The Redis store waits one command timeout (default 1000 ms) for its first connect (`connectWithin`). A slow first connect makes `store.get` fail, `getSession` returns null, and `requireSession` answers 3xx. `curl --retry` does not retry a 3xx. | `ts/packages/paigasus-auth/src/adapters/redis-store.ts:264-319`, `src/config.ts:101` |
| F16 | With `PAIGASUS_SESSION_STORE=redis`, `descriptorCacheFor` (the discovery descriptor cache) also uses Redis. This is a new code path in the smoke. | `ts/packages/paigasus-console-core/src/discovery.ts:342-349` |
| F17 | Without `PAIGASUS_SESSION_REDIS_URL`, `createAuthRuntime` throws `AuthConfigError`, and a `(console)` route answers 500 with an intact wasm. | `ts/packages/paigasus-auth/src/runtime.ts:102` |
| F18 | SMA-705 added a public `<basePath>/readyz` route per zone (listed in each `proxy.ts` `publicPaths`). It answers 200 only after one OIDC discovery succeeded, so it answers 503 in the smoke (the IdP is a dummy). `ci/images/run.sh` and the image `HEALTHCHECK` keep `<basePath>/healthz`. The D4 readiness gate therefore uses `/healthz`, not `/readyz`. The `(console)` layouts, the probe pages and `core/single-flight.ts` did not change. | `ts/apps/*/app/readyz/route.ts`, `ts/apps/*/proxy.ts:30-31`, `docs/ops/RUNBOOK-containers.md:347-352` |

F1 to F4 together mean: if the `(console)` layout module loads, the wasm evaluates. If the wasm
cannot compile, the module load throws, and the route cannot render. F8 means that with a valid
session and no IAM, the route can still render. §5 M1 measures this last point before the design is
trusted.

## 3. Goals

1. A request with a seeded session reaches a `(console)` route, the layout evaluates, and the route
   answers **200** with a per-run value that only the `(console)` layout can put in the body.
2. A control proves that the same probe fails when the kernel cannot load.
3. The existing cookie-less 3xx row stays, and its message states only what it proves: the proxy
   gate is in effect.
4. `docs/ops/RUNBOOK-containers.md` and the comment in `run.sh` record what the probe proves after
   the change, and what it does not prove.

## 4. Decisions

### D1. The session store: a Redis sidecar container, not a single-zone memory store

**Chosen:** each smoke run starts one Redis container on a per-run Docker network. Each console
container joins that network and gets `PAIGASUS_SESSION_STORE=redis` and
`PAIGASUS_SESSION_REDIS_URL=redis://<redis-name>:6379`. The image is the kind tier's digest pin
(F9). The discovery descriptor cache then also uses this Redis (F16). That is a new code path in
the smoke; M6 covers it.

Each console container also gets `--add-host iam:127.0.0.1 --add-host gateway:127.0.0.1`. The
IAM and gateway calls of the `(console)` layout then fail at once with "connection refused", and
do not depend on the DNS of the runner or the development host.

**Rejected: a `requireSession` 3xx as the positive proof (no seeded record).** A 3xx with
`returnTo=<basePath>/` (F14) also proves that the `(console)` layout module ran, so the kernel
loaded. It needs Redis but no seeded record and no `SessionRecord` coupling. This spec keeps the
200 because the ticket asks for a render, and a 200 also proves that the page module and the RSC
render complete with the kernel loaded. **Sven decided (§12 Q4): keep the 200.** The weaker
variant comes back only as the M1 fallback (D3a, §12 Q1).

**Rejected: a public route handler that calls the kernel.** Next builds route handlers and pages
in separate module graphs (`ts/CLAUDE.md`, SMA-511). A route handler that loads the kernel does
not prove that the page graph loads it. Do not propose this again.

**Rejected: a single-zone `PAIGASUS_ZONES` with the memory store.** The memory store is in-process.
Nothing outside the container can seed a session into it, so the route can never answer 200. The
best it can give is a 3xx from `requireSession`, which has the same status class as the proxy's
3xx. It also changes `PAIGASUS_ZONES`, which the smoke comments name as a hardcoded two-zone
assumption.

**Rejected: a stub IAM server.** F8 says that the route renders without IAM. A stub adds a second
sidecar and a gRPC fixture for no gain in the kernel proof. **Sven decided (§12 Q1) that it is
NOT the fallback if M1 fails.** The fallback is D3a: a weaker assertion with the D4 control still
required.

### D2. The seeded session is a literal record written with `redis-cli`

The smoke writes one `SessionRecord` (version 2) per zone with
`docker exec <redis> redis-cli SET pgs:sess:<sid> <json> PX 600000`, and expects the reply `OK`.
The record holds only dummy values, plus one per-run nonce in the `email` claim (D3):

```json
{"version":2,"rev":1,"accessToken":"smoke-access-token","accessExpiresAt":<now+600000>,
 "absoluteExpiresAt":<now+600000>,"idToken":"smoke.id.token",
 "idTokenClaims":{"iss":"https://idp.example.com","sub":"smoke-user","email":"smoke-<first 12 sid chars>@example.com"},
 "principal":{"principalPrn":null,"issuer":"https://idp.example.com","subject":"smoke-user",
  "memberships":[],"roleGrants":[],"grantsAvailable":false}}
```

`<now>` comes from the sidecar's clock: `docker exec <redis> redis-cli TIME`, seconds times 1000
plus the microseconds divided by 1000. Do not use the host's `date`: after a development Mac
sleeps, the Docker Desktop VM clock can differ from the host clock, and the consoles read the VM
clock. Ten minutes is far above the 30 s refresh skew (F7), so no
refresh and no IdP call happens during the smoke. The sid is 64 hex characters from
`/dev/urandom` through `od`. No `refreshToken` field is written, so a refresh can never start.

There is deliberately no second copy of `isSessionRecord` in bash. If the record shape changes (a
version 3), the store deletes the seeded record (F6), the layout redirects, and the new 200 row
fails with a message that names this coupling. That is the same "fail loudly" rule that
`CONSOLE_SMOKE_ENV` already states for the config schema.

That rule works for `CONSOLE_SMOKE_ENV` only because `images.yml`'s `pull_request.paths` lists
`ts/apps/*/lib/config.ts`. So this change adds the new coupled files to that filter, each with a
comment (§6.4): `ts/packages/paigasus-auth/src/core/session.ts`,
`ts/apps/*/app/(console)/layout.tsx`, `ts/apps/iam-console/app/(console)/orgs/page.tsx` and
`ts/apps/gateway-console/app/(console)/overview/page.tsx`. Without this, a PR that bumps
`SessionRecord.version` goes green, and the first red is on `main` or in `release.yml`.

This change also adds one pointer line to the `SessionRecord.version` comment in
`core/session.ts`: "the console image smoke seeds a literal record of this version
(`ci/images/run.sh`, `console_seed_session`); change it too." This is decided here, not left to
the implementer.

### D3. The positive row asserts 200 and the per-run session nonce

`console_kernel_route_row` sends ONE request `GET <origin><basePath><probePath>` with
`Cookie: __Host-pgs_sid=<sid>`, no `-L`, `--max-time 30`, as
`curl -sS -o <body-file> -w '%{http_code} %{redirect_url}'`. Status, `Location` and body so come
from the same request. It passes only when:

1. the status is `200`, and
2. the body file holds the nonce `smoke-<first 12 sid chars>@example.com` from the seeded record
   (D2). The row matches the file with `grep -F -q` on the file (not on a pipe; `repo:actionlint`
   check 13) and checks the rc: 0 is a match, 1 is "no nonce", any other rc is an infrastructure
   error with its own message.

**Why a nonce and not the brand label.** The brand label is also the root `metadata.title` (F12),
so every HTML page holds it, `error.tsx` and not-found pages included. A brand-label check can
never see "200 without the marker". The nonce reaches the HTML only through
`toSessionView(session)` → `Providers` (F13). So it proves that the `(console)` layout ran and
read this record from Redis. This also catches a probe route that moves out of `(console)`, and a
future Suspense boundary that turns a `requireSession` redirect into a streamed 200
(`app/_components/page-error.tsx:11-13`). There is no `console_layout_marker_for` function.

Each failure has its own named message:

- a 3xx whose `Location` holds `returnTo=<basePath><probePath>` (percent-encoded as the proxy
  writes it): the proxy did not see the cookie. This points to the request, not to the store.
- a 3xx whose `Location` holds `returnTo=<basePath>/`: `requireSession` found no session. This
  points to D2's coupling, the store wiring, or a slow first Redis connect (F15). This is the only
  case that is retried: a bounded bash loop, 3 tries, 1 s apart, each try one new request. The
  message after the last try names the three causes.
- any other 3xx: prints the `Location`.
- a 404 (the route is gone), a 5xx (the kernel or the runtime config), and a 200 without the nonce.
- a curl failure (rc not 0): its own message with the curl rc.

### D3a. The M1 fallback (Sven's decision, §12 Q1 option (b))

This section applies ONLY when M1 shows that a seeded session with no IAM does not give a 200.
Then the route row accepts one of two results, and nothing else:

1. a 200 whose body holds the per-run nonce (D3, unchanged), OR
2. a 3xx whose `Location` holds `returnTo=<basePath>/` (the `requireSession` redirect, F14), after
   the D3 bounded retry. The row prints a named OK line that says which of the two it saw. The
   proxy 3xx (`returnTo=<basePath><probePath>`) stays a failure with its own message.

The D4 control stays required, with no change: it is what proves that a `requireSession` 3xx
comes from a `(console)` layout that loaded the kernel. The implementer records the M1 result
(status, `Location`, and the cause of the non-200 if found) in the PR description. The PR also
states whether the row runs in the D3 form or in the D3a form. If M1 gives 200, D3a is not
implemented, and the row stays as D3 states it.

In the D3a form, the self-test (§6.2) adds one passing row for the accepted `requireSession` 3xx
and keeps the red row for the proxy 3xx.

### D4. The control: the same probe against the same image with a broken wasm chunk

`console_kernel_control_row` proves that D3 can see a kernel failure. It runs on every smoke run,
not once by hand, so the control cannot go stale.

1. Find the chunk paths with the image's own node (the runtime base has no shell): walk
   `/app/apps/<app>/.next/server/chunks` and print every file whose name matches
   `*paigasus_wasm_bg*.wasm` (the same glob as `ts/Dockerfile:48` and `moon.yml:95`). At least one
   path is required, as in the build checks. Zero is an error that names the count. Every match is
   corrupted in step 3, so a build that writes one file per layer cannot give a false control.
2. `docker create` a second container `smoke-<app>-nokernel-<RUN_ID>` from the same image. The
   main container and the control container get their env, network, `--add-host`, zone and port
   flags from ONE function, `console_container_args` (§6.1). The control therefore cannot miss
   `PAIGASUS_SESSION_REDIS_URL` (F17). The main container also uses `docker create` + `docker
   start` from the same argument list.
3. `docker cp` a host file of 8 bytes (`\0asm` followed by a wrong version number) over each chunk
   path. The host file has mode 0644, set with `chmod` after `mktemp`: `docker cp` makes the file
   root-owned, and a 0600 file then fails with EACCES, not with a compile error. `docker cp` works
   on a created container with no shell.
4. `docker start` it, wait for `<basePath>/healthz` to answer 200 (the public page does not load
   the kernel), and then send the D3 request with a sid seeded for this container.
5. The row passes only when BOTH are true: the answer is a 5xx, AND the FULL `docker logs` of the
   control container hold a kernel-specific line. The exact text (for example `CompileError` or
   `WebAssembly`) is measured in M4 and written into the row as a literal. The row searches the
   full log, not a tail: Next preloads route modules at start (`preloadEntriesOnStart`, default
   `true` in next 16.3.5), so the compile error can be logged before the probe request. A 5xx
   without the kernel line is its own error ("the control failed for a reason that is not the
   kernel") and prints the last 30 log lines. A 200 means that the D3 row cannot see a kernel
   failure, which is the defect this issue exists to close. Any other status is an error that
   prints the status and the last 30 log lines.
6. **Remove the control container as soon as its row finishes** (Sven's decision, §12 Q2), on
   every exit path of the row: pass or fail, after its logs are read. Use `docker rm -f <name>`.
   A failed remove prints a `::warning::` and does not change the row's verdict; the EXIT-trap
   cleanup removes the container again. This limits the disk use of the `images.yml` job, whose
   disk has overflowed once. The main container is not removed early: later rows still use it.

The broken container is registered in `CONSOLE_SMOKE_NAMES` before `docker create`, as the main
container is. It stays registered after step 6, so an abort between `docker create` and step 6
still removes it. `docker rm -f` on a container that is already gone is harmless.

**The control runs only on the two `images.yml` paths of F10 (Sven's decision, §12 Q5).** It does
not run on the `release.yml` path. The mechanism:

- `smoke_consoles` takes a new first argument `--kernel-control=on` or `--kernel-control=off`.
  Any other first word, or no such word, is a usage error with its own `::error::` (rc 1). This
  keeps the SMA-670 rule: no function reads a global.
- The two dispatch arms (`smoke <console-key>` and `all-consoles`) read the environment variable
  `PAIGASUS_SMOKE_KERNEL_CONTROL`. Unset or `on` passes `--kernel-control=on`. `off` passes
  `--kernel-control=off`. Any other value is a usage error (rc 1). The default is ON, so a
  missing setting runs more checks, not fewer. A local run and both `images.yml` steps therefore
  need no change.
- `release.yml`'s "Smoke this service" step (`release.yml:1245-1246`) sets
  `PAIGASUS_SMOKE_KERNEL_CONTROL: off` in its `env:`, with a comment that names SMA-675 Q5. The
  step also smokes the cargo service keys; the `smoke` arm reads the variable only on its console
  branch, so a cargo smoke ignores it.
- With `off`, `smoke_consoles` prints one line per zone: `<app>: kernel control row skipped
  (PAIGASUS_SMOKE_KERNEL_CONTROL=off, the release path; SMA-675 Q5)`. The route row, the Redis
  sidecar and the cookie-less row still run on the release path.

The release path therefore still pulls the Redis image and still runs the route row (§11). The
arm64 proof of the control comes from the `images.yml` dispatch run of AC 8.

### D5. The cookie-less 3xx row stays, with a corrected message

The existing row is still useful: it proves that the proxy gate is in effect. Its comment and its
OK line change from "the route exists and nothing 500s" to "the proxy redirects a request with no
session cookie". The long RESIDUAL comment block above it is replaced by a short statement that
points to the two new rows.

### D6. Network and cleanup

A per-run network `smoke-net-<RUN_ID>` and a Redis container `smoke-redis-<RUN_ID>`. The Redis
container is registered in `CONSOLE_SMOKE_NAMES` before it is created. A new script-global
`CONSOLE_SMOKE_NETWORK` holds the network name, and `console_smoke_cleanup` removes it after the
containers, because Docker refuses to remove a network that still has endpoints. Readiness is a
bounded loop over `docker exec <redis> redis-cli PING`, one second apart. The number of tries is
an argument (`console_smoke_redis_start <network> <name> <tries>`), as `CONSOLE_HC_DEADLINE` is:
the smoke passes 20, and the self-test passes 2. No Redis port is published to the host.

The network and the Redis container carry the label `paigasus.smoke=console`. A killed run can
leave them behind, and Docker's default address pools hold only about 30 bridge networks. The
RUNBOOK documents the prune: `docker rm -f $(docker ps -aq --filter label=paigasus.smoke=console)`
then `docker network prune --filter label=paigasus.smoke=console`.

**A failed network create.** If `docker network create` fails, the smoke prints one named
`::error::`, sets `ec=1`, skips Redis and the kernel rows (the route row and the control row) for
both zones, and starts the main containers WITHOUT `--network` and with the old memory-store env,
so the page, HEALTHCHECK, chunk and uid rows still run and report. A Redis start failure is
handled the same way: the kernel rows are skipped with one message for each zone, and the other
rows run.

### D7. One Redis for both zones, one sid per container

Both zones share one Redis in production (design doc § 6.7), so one sidecar per run is the
production shape. Each console container (main and control) gets its own sid, so one container's
reads cannot delete or change the record of another.

## 5. Measurements the implementation must take first

These are not measured yet. The implementer takes them on a local build before writing the rows,
and records each result in the PR description.

| # | Measurement | Expected | If different |
|---|-------------|----------|--------------|
| M1 | `/iam/orgs` and `/gateway/overview` with a seeded Redis session and no IAM reachable | 200, body holds the per-run nonce (D2, D3) | Use the D3a fallback (Sven's decision, §12 Q1). Record the M1 result in the PR description in both cases. |
| M2 | Time to answer M1, with `--add-host iam:127.0.0.1 --add-host gateway:127.0.0.1` | under 10 s (each IAM call fails at once with "connection refused") | Raise `--max-time` with a comment, or stop if above 30 s. |
| M3 | The number of `*paigasus_wasm_bg*.wasm` files under `/app/apps/<app>/.next/server/chunks`, per zone, and their names | at least 1 | Change D4 step 1 to the measured shape. |
| M4 | The M1 request against the D4 broken-chunk container, and the exact kernel line in its FULL `docker logs`. Record WHEN the line appears: at start (preload) or at the request. | 5xx, and a line such as `CompileError` / `WebAssembly` | Try a zero-byte file. If the route still answers 200, the kernel does not load where F1-F4 say. Stop and report. |
| M5 | `<basePath>` and `<basePath>/healthz` against the D4 container, and where in the log the compile error is | 200 | If the public page 500s too, the control container cannot tell "started" from "broken". Use `/healthz` only as the readiness gate. |
| M6 | The existing steps 1 to 4 (page, HEALTHCHECK program, chunk, other prefix) with `PAIGASUS_SESSION_STORE=redis`, and the discovery descriptor cache on Redis (F16) | unchanged | Investigate before continuing. |
| M7 | The M1 request with a sid that is not seeded | 3xx, `Location` holds `returnTo=<basePath>/` | Informational. It confirms D3's `requireSession` message. |
| M8 | The M1 request with no cookie | 3xx, `Location` holds `returnTo=<basePath><probePath>` | Informational. It confirms D3's proxy message and the exact percent-encoding to match. |

## 6. Components

### 6.1 `ci/images/run.sh`

- `CONSOLE_SMOKE_ENV` keeps only the values that do not depend on the run. It no longer holds
  `PAIGASUS_SESSION_STORE`. The store env comes from `console_container_args` (below), which gets
  the Redis URL as an argument. The SMA-670 rule allows no script-global except `ROOT`
  (`run.sh:1209-1213`), and the self-test copies functions only. So no new function reads a
  global: every run-dependent value is an argument, and bash 3.2 under `set -u` cannot stop with
  `unbound variable` in the self-test.
- A new constant `CONSOLE_SMOKE_REDIS_IMAGE` with the F9 digest, and a comment that names
  `ci/kind/manifests/redis.yaml` as the other copy. `smoke_consoles` passes it as an argument.
  `ci/kind/manifests/redis.yaml` gets the reverse comment above its `image:` line, which names
  `ci/images/run.sh` `CONSOLE_SMOKE_REDIS_IMAGE`. No check compares the two copies (Sven's
  decision, §12 Q3).
- The `smoke` (console branch) and `all-consoles` dispatch arms read
  `PAIGASUS_SMOKE_KERNEL_CONTROL` and pass `--kernel-control=on|off` as the first argument of
  `smoke_consoles` (D4, §12 Q5).
- New functions, each in the SMA-670 row shape (`<fn> … || ec=1`, errexit off inside, every
  capture guarded, one named `::error::` per cause). The full signatures:
  - `console_container_args <app> <zone> <zones_json> <network> <redis_url> <env_file>` prints the
    `docker create` argument list, one argument per line: every `-e` of `CONSOLE_SMOKE_ENV` (passed
    in, see below), `PAIGASUS_ZONE`, `PAIGASUS_ZONES`, and, when `<network>` is not empty,
    `--network <network>`, `-e PAIGASUS_SESSION_STORE=redis`,
    `-e PAIGASUS_SESSION_REDIS_URL=<redis_url>`, and the two `--add-host` flags. When `<network>`
    is empty (the D6 fallback), it prints `-e PAIGASUS_SESSION_STORE=memory` and no network flags.
    It always prints `-p 0:3000`. The caller reads the list into an indexed array with a
    `while read` loop over a FILE (no `mapfile`, no here-string). The fixed env reaches the
    function as a file: `smoke_consoles` writes `CONSOLE_SMOKE_ENV` to a file and passes its path
    as `<env_file>`.
  - `console_smoke_redis_start <network> <name> <image> <tries>` (network, container, labels,
    readiness)
  - `console_seed_session <redis-name> <sid>` (D2; reads `now` from `redis-cli TIME`)
  - `console_kernel_route_row <origin> <base_path> <probe_path> <sid> <name> <tries>` (D3; the
    nonce is derived from `<sid>` inside the function)
  - `console_kernel_control_row <app> <image> <base_path> <probe_path> <redis_name> <args_file>
    <kernel_line>`: `<args_file>` is the output of `console_container_args` for this zone, so the
    control uses the SAME list as the main container. The function makes its own name, seeds its
    own sid, reads its host port with `docker port <name> 3000`, and searches the full log for
    `<kernel_line>` (measured in M4). It removes its own container before it returns (D4 step 6).
- `smoke_consoles`: create the network and start Redis once before the zone loop; for each zone,
  build the argument list with `console_container_args`, then `docker create` + `docker start`
  the main container from it, seed a sid, and call the route row and, when `--kernel-control=on`,
  the control row after the cookie-less row. A network or Redis failure follows D6 and does not
  abort the script.
- `console_smoke_cleanup`: remove every container first, then the network.
- The comment above `console_probe_path_for` and the RESIDUAL block in the step: rewrite them to
  match D5.

### 6.2 `ci/images/console-selftest.sh`

Add each new function to `FUNCS`. Add, for each named `::error::`, one row that stubs `docker` and
`curl` and must red with that substring. At minimum:

- D3: a 3xx with the proxy `returnTo`, a 3xx with the `requireSession` `returnTo` (asserts that
  the stub `curl` was called 3 times), another 3xx, 404, 500, a curl failure, a `grep` rc 2, and a
  200 without the nonce. Each must red with its own message. A 200 with the nonce must pass. A
  `requireSession` 3xx on try 1 and a 200 with the nonce on try 2 must pass.
- D4: chunk count 0; chunk count 2 (asserts two `docker cp` calls); `docker cp` failure; the
  control answers 200; the control answers 302; the control answers 500 without the kernel line.
  A 500 with the kernel line must pass. Every one of these rows, the passing row included, asserts
  from the stub argv that the row calls `docker rm -f <control-name>` before it returns (D4 step
  6, §12 Q2).
- The Q5 switch: `smoke_consoles` with `--kernel-control=off` calls no `console_kernel_control_row`
  (asserted from the stub argv: no `docker create` of a `-nokernel-` name) and prints the skip
  line; with a missing or unknown first word it reds with its usage message. For the dispatch arm,
  a row asserts that `PAIGASUS_SMOKE_KERNEL_CONTROL=bogus` gives rc 1 with the usage message. If
  the dispatch arm cannot be reached by the awk copy, test it with a stubbed `docker` through the
  real script instead, and say so in the PR.
- D3a (only if M1 fails): a `requireSession` 3xx on every try passes with the named OK line.
- `console_container_args`: with a network, the list holds the Redis URL, `--network` and both
  `--add-host` flags; without one, it holds `PAIGASUS_SESSION_STORE=memory` and no network flag.
- D2: `redis-cli` replies something other than `OK`; `redis-cli TIME` fails.
- D6: `PING` never answers `PONG` within a bound of 2.
- Cleanup: add `console_smoke_cleanup` to `FUNCS`. A stub-argv row asserts that every
  `docker rm -f` comes before `docker network rm`.
- **Call-site pins.** Add a `pin_rows` row, in the P1a-P1c shape, for each new call line in
  `smoke_consoles`: the network create, the Redis start, `console_container_args`, each
  `console_seed_session` call, the `console_kernel_route_row … || ec=1` line and the
  `console_kernel_control_row … || ec=1` line. Also pin the `docker rm -f` line inside
  `console_kernel_control_row` (D4 step 6). Also pin the `grep -F` line inside
  `console_kernel_route_row` and the log-search line inside `console_kernel_control_row`, in the
  P2 shape. Without these pins, a later edit can delete a row call, every stub row stays green,
  and the smoke again proves nothing.

The harness still runs under bash 3.2 and bash 5 (its header rule). No `mapfile`, no
`declare -A`, no here-string.

### 6.3 `docs/ops/RUNBOOK-containers.md`

Add a bullet in the `smoke_consoles` list (near the existing "The zone row of `smoke_consoles`
proves only that a basePath is in effect"):

- what the kernel row proves: a seeded session reaches the `(console)` layout, which imports the
  kernel at module scope, and the route renders 200 with the per-run nonce from the seeded record
  (the brand label is not a marker: every page holds it in `<title>`);
- what the control proves: the same probe answers 5xx, with a kernel compile error in the log,
  when the image's wasm chunk is corrupted, so a 200 cannot come from a path that skips the kernel;
- what it does NOT prove: that IAM calls work (the smoke has no IAM), that a real login works (the
  kind e2e tier covers that), or that a kernel function returns correct values (the kernel tests
  cover that);
- that the smoke now starts a Redis container from the pinned digest, on a per-run network, and
  that the discovery descriptor cache uses that Redis too;
- that the console release chain in `release.yml` now pulls `redis:7.4-alpine` from Docker Hub, on
  amd64 and arm64;
- that the control runs only on the `images.yml` paths, and that `release.yml` turns it off with
  `PAIGASUS_SMOKE_KERNEL_CONTROL=off` (§12 Q5); the control container is removed as soon as its
  row finishes (§12 Q2);
- that the Redis digest has two copies, `ci/images/run.sh` and `ci/kind/manifests/redis.yaml`,
  which name each other, and that no check compares them (§12 Q3);
- the prune by the label `paigasus.smoke=console` after a killed run (D6).

Put the new bullet after the zone-row bullet ("The zone row of `smoke_consoles` proves only that a
basePath is in effect", `RUNBOOK-containers.md:431` on `4051df5e`). Re-checked on `4051df5e`: the
RUNBOOK does not claim anywhere that the smoke proves the kernel load. Line 56 is about the wasm
build inputs, not the smoke, so it needs no change. The SMA-705 bullet (lines 347-352) says that
`ci/images/run.sh` uses `/healthz`; that stays true.

Update the `pull_request.paths` filter list (RUNBOOK lines 33-43) with the new files of §6.4. Also
update the filter rule (lines 45-62). The current rule lists a file only when a change to it "can
break the image build but not the ordinary build", and it excludes a file that is already a Moon
task input. The four new files do not fit that rule: they are on the filter because the image
SMOKE depends on them at runtime. Add a second clause to the rule for this case, and name the four
files.

### 6.4 `.github/workflows/images.yml` and `release.yml`

- `images.yml` `pull_request.paths`: add `ts/packages/paigasus-auth/src/core/session.ts`,
  `ts/apps/*/app/(console)/layout.tsx`, `ts/apps/iam-console/app/(console)/orgs/page.tsx` and
  `ts/apps/gateway-console/app/(console)/overview/page.tsx`, each with a comment that names the
  coupling (D2, the probe route). Check that the path filter accepts the parentheses literally;
  quote each entry.
- No other `images.yml` change. The runner has Docker, and the Redis image is pulled by digest (a
  multi-arch index digest, per `ci/kind/manifests/redis.yaml:5`). Both `images.yml` smoke steps
  (`images.yml:192` and `:211`) leave `PAIGASUS_SMOKE_KERNEL_CONTROL` unset, so the control runs.
- `release.yml:1245-1246` ("Smoke this service") runs the same `smoke` subcommand on amd64 and
  arm64. Its one edit: an `env:` block with `PAIGASUS_SMOKE_KERNEL_CONTROL: off` and a comment
  that names SMA-675 Q5 (D4). The route row and the Redis sidecar still run there. The arm64 leg
  must be proven before merge with a `gh workflow run images.yml --ref <branch>` run (AC 8), so
  that the first arm64 run of this code is not a release run.

### 6.5 `ts/packages/paigasus-auth/src/core/session.ts`

Add the one pointer line to the `SessionRecord.version` comment (D2). No code change.

## 7. Error handling

Every new capture follows the rule written above `smoke_consoles`: guarded with `|| var=""`, then an
explicit check with its own `::error::`. One zone's failure sets `ec=1` and does not hide the other
zone. A failure in the shared network create or Redis start sets `ec=1` with one named message,
skips only the kernel rows, and lets the other rows run (D6). Docker's own message is printed on
separate lines after the annotation, as the existing `docker run` failure does.

## 8. Acceptance criteria

1. `ci/images/run.sh all-consoles` and `ci/images/run.sh smoke <console-key>` each run, per zone, a
   row that sends a seeded session to the zone's `(console)` probe route and passes only on 200
   with the per-run nonce of the seeded record in the body. Exception, only if M1 shows no 200:
   the row runs in the D3a form and also passes on a `requireSession` 3xx
   (`returnTo=<basePath>/`); the proxy 3xx still reds (§12 Q1).
2. Per zone, on the two `images.yml` paths, a control row runs the same probe against the same
   image with every wasm chunk corrupted, and passes only on a 5xx with a kernel-specific line in
   the full container log. A 200 from the control reds the smoke. The main and the control
   container get their arguments from one function. The control container is removed as soon as
   its row finishes (§12 Q2). On the `release.yml` path, `PAIGASUS_SMOKE_KERNEL_CONTROL=off` skips
   the control with one named line per zone; an unknown value reds with a usage error (§12 Q5).
3. `CONSOLE_SMOKE_ENV` no longer sets `PAIGASUS_SESSION_STORE=memory`. The smoke starts a Redis
   container from a digest-pinned image on a per-run, labelled network, and the cleanup removes
   the Redis container, both console containers per zone, and then the network, also when the
   script aborts.
4. The cookie-less 3xx row stays, and its message says that it proves the proxy gate only.
5. `ci/images/console-selftest.sh` has a red row for every new named `::error::`, a cleanup-order
   row, and a `pin_rows` pin for every new call line in `smoke_consoles` (§6.2). It stays green on
   the clean baseline, under `/bin/bash` 3.2 and a bash 5.
6. `docs/ops/RUNBOOK-containers.md` states what the probe proves and what it does not prove, the
   new PR path-filter entries, the Docker Hub pull on the release chain and the label prune (§6.3).
7. The PR description records M1 to M8, and states whether the route row runs in the D3 form or
   the D3a form (§12 Q1).
8. `images.yml` is green on the PR for amd64, and a `gh workflow run images.yml --ref <branch>`
   run is green for arm64. The PR records the dispatch run's URL.
9. `images.yml` `pull_request.paths` lists the four coupled files of §6.4, and the
   `SessionRecord.version` comment names the smoke (§6.5).
10. `ci/images/run.sh` and `ci/kind/manifests/redis.yaml` each hold the Redis digest with a
    comment that names the other copy (§12 Q3).

## 9. Test strategy

- **Self-test (fast, every change):** the stubbed rows in §6.2. They prove each message and each
  verdict of the new functions. The `pin_rows` pins prove that each call line is still in
  `smoke_consoles`.
- **Real smoke (local, Docker):** `ci/images/run.sh all-consoles` on a clean build. Both zones must
  pass every row.
- **Delete-the-feature proof (local, once, recorded in the PR):** per the repo memory rule "red-first
  is not proof", take each of these mutations on a local branch, run `all-consoles`, and record that
  the smoke reds with the named message. Revert each with an edit, not `git checkout --`, so the fix
  under test stays.
  1. Remove the `Cookie` header from the D3 request. Expected: the proxy 3xx message (not the
     `requireSession` one).
  2. Seed the record with `version` 3. Expected: the `requireSession` 3xx message after 3 tries.
  3. Put `PAIGASUS_SESSION_STORE=memory` back. Expected: the 5xx message on the main container.
  4. Make the D4 overwrite a no-op (copy the original chunk back). Expected: the control reds with
     "answered 200".
  5. Drop `PAIGASUS_SESSION_REDIS_URL` from the control only (a temporary edit that bypasses
     `console_container_args`). Expected: the control reds with "a 5xx that is not the kernel".
  6. Derive the nonce from a different sid than the seeded one. Expected: the "200 without the
     nonce" message.
  7. Set `console_probe_path_for iam` to a path that does not exist. Expected: the 404 message.
  8. Delete the `console_kernel_route_row … || ec=1` line. Expected: the self-test pin reds.
  9. Delete the `docker rm -f` line in `console_kernel_control_row`. Expected: the self-test pin
     and the argv rows red (§12 Q2).
  10. Run `PAIGASUS_SMOKE_KERNEL_CONTROL=off ci/images/run.sh all-consoles`. Expected: the skip
      line per zone, no `-nokernel-` container in `docker ps -a` during the run, and a green
      route row (§12 Q5).
- **CI:** the `images.yml` job on the PR (amd64), plus one `workflow_dispatch` run for arm64.

## 10. Files expected to change

- `ci/images/run.sh`
- `ci/images/console-selftest.sh`
- `docs/ops/RUNBOOK-containers.md`
- `.github/workflows/images.yml` (the `pull_request.paths` filter only)
- `.github/workflows/release.yml` (one `env:` entry on "Smoke this service", §12 Q5)
- `ci/kind/manifests/redis.yaml` (one comment line only, §12 Q3)
- `ts/packages/paigasus-auth/src/core/session.ts` (one comment line only)

No app code and no package logic change is expected. If M1 fails, the route row takes the D3a
form (§12 Q1). That changes only `ci/images/run.sh` and its self-test, so this list does not grow.

## 11. Residual risk

- The smoke has no IAM. The 200 proves that the layout and the page render with IAM absent. It does
  not prove any IAM path. The kind tier stays the proof for that.
- The seeded record is a literal. A `SessionRecord` version bump reds the smoke until the literal
  changes (D2). That is intended. The `SessionRecord.version` comment names the smoke (§6.5), and
  `images.yml` runs on a PR that touches `core/session.ts` (§6.4).
- The console release chain in `release.yml` now pulls `redis:7.4-alpine` from Docker Hub and
  starts one more container (Redis) for each console smoke, on amd64 and arm64. The D4 control
  does not run there (§12 Q5). A Docker Hub outage or rate limit can now red a console release.
- The control does not run on the release path. So the release path's route row is not
  cross-checked against a broken kernel in the same run. The `images.yml` runs on the same code
  are the proof that the row can see a kernel failure (§12 Q5).
- If M1 fails and the row takes the D3a form, the route row no longer proves that the page module
  and the RSC render complete. It proves only that the `(console)` layout ran, and with the D4
  control, that the kernel loaded (§12 Q1).
- The path filter lists the two probe pages and the `(console)` layouts by name. A new probe path
  in `console_probe_path_for` must add its page to the filter. Nothing checks this.
- Two copies of the Redis digest (`run.sh`, `ci/kind/manifests/redis.yaml`). Nothing checks that
  they are equal. A difference is harmless for correctness.
- The control breaks the chunk file, not the import path. A future bundler that inlines the wasm
  into a JS chunk makes D4 step 1 find zero files. The row then reds with the count message, and the
  build checks in `moon.yml` red first.

## 12. Open questions (all answered by Sven on 2026-09-27)

- **Q1 (only if M1 fails).** If a seeded session with no IAM does not render 200, which fallback
  does Sven want: (a) a minimal IAM stub container that answers WhoAmI and the discovery probe, or
  (b) a weaker assertion: the seeded route answers 200 OR a named `requireSession` 3xx whose
  `Location` differs from the proxy's, with the D4 control still required? The ticket asks for a
  200, so (b) is a scope change that Sven must accept.

  **ANSWER (Sven, 2026-09-27): fallback (b).** If M1 shows no 200, accept a 200 OR a named
  `requireSession` 3xx (`returnTo=<basePath>/`), with the D4 control still required. Record the M1
  result in the PR. See D3a.
- **Q2.** Is one extra container per zone per run (the D4 control) acceptable on the `images.yml`
  job, whose disk has already overflowed once? The control adds no image layers (`docker create`
  from the existing image plus one 8-byte file), so the expected disk cost is near zero. The
  runtime cost is one more container start per zone.

  **ANSWER (Sven, 2026-09-27): accepted.** One extra control container per zone per `images.yml`
  run. Remove each control container as soon as its row finishes, to limit disk use (D4 step 6).
- **Q3.** Should the Redis digest have one source of truth shared with
  `ci/kind/manifests/redis.yaml`, with a check, or are two commented copies enough? This spec
  chooses two commented copies.

  **ANSWER (Sven, 2026-09-27): two copies with cross-reference comments, no new check** (§6.1,
  AC 10).
- **Q4.** Is the 200 worth the `SessionRecord` coupling? The weaker variant asserts a
  `requireSession` 3xx (identified by `returnTo=<basePath>/`, F14). It also proves that the
  `(console)` layout module ran and the kernel loaded, needs no seeded record and no M1. This spec
  keeps the 200 because the ticket asks for a render, and a 200 also proves the page module and
  the RSC render. Sven confirms or picks the weaker variant.

  **ANSWER (Sven, 2026-09-27): keep the 200**, with the per-run nonce and the seeded
  `SessionRecord`, as written.
- **Q5.** Must the D4 control run on the `release.yml` path too? It tests the probe, not the
  release candidate, and it doubles the container starts for each console release. This spec runs
  it on every path (it is cheap and cannot go stale). The alternative is a flag so that only
  `images.yml` runs it.

  **ANSWER (Sven, 2026-09-27): the control runs only in `images.yml`**, not on the `release.yml`
  path. The mechanism is `PAIGASUS_SMOKE_KERNEL_CONTROL` (default on; `release.yml` sets `off`),
  see D4.

## 13. Challenge changelog

**Verdict:** APPROVE WITH CHANGES (spec-challenger, Opus, 2026-09-27). Every finding was checked
against the code on `main` (`83d446fc`).

**Folded:**

- BLOCKER, the D3 layout marker is on every page: confirmed at `ts/apps/*/app/layout.tsx:9`
  (F12). D3 now asserts a per-run nonce in the `email` claim, which reaches the HTML only through
  `toSessionView` → `Providers` (F13, confirmed). `console_layout_marker_for` is removed.
  Mutation 6 covers it.
- MAJOR, no call-site pins: confirmed (`console-selftest.sh:562-623`). §6.2 adds `pin_rows` for
  every new call line and two inner lines. AC 5 requires them.
- MAJOR, a PR cannot see the new couplings: confirmed (`images.yml:32-62`). §6.4 adds four paths;
  the RUNBOOK filter list changes; the `SessionRecord.version` pointer line is now decided (§6.5).
- MAJOR, the release path is not named: confirmed (`release.yml:1246`, `images.yml:88`). F10,
  §6.4 and §11 name it. AC 8 now requires amd64 on the PR plus an arm64 dispatch run. The RUNBOOK
  states the Docker Hub pull.
- MAJOR, the control passes on any 5xx: confirmed (`runtime.ts:102`, F17). One function
  (`console_container_args`) builds the argument list for both containers; the control also
  requires a kernel line in the full log; the host file is 0644. Mutation 5 covers it.
- MAJOR, one message for two 3xx causes, no retry: confirmed (F14, F15). D3 reads
  `%{redirect_url}`, has two messages, and retries only the `requireSession` 3xx (3 tries, 1 s).
- MAJOR, the control row interface is undefined: §6.1 now gives every signature. No new function
  reads a global; the env reaches it as a file argument.
- MINOR, chunk glob: D4 uses `*paigasus_wasm_bg*.wasm`, requires at least one, corrupts every
  match.
- MINOR, Next preloads route modules: D4 and M4/M5 search the full log and record where the line
  is.
- MINOR, 20 s self-test sleep: the tries are an argument; the self-test passes 2.
- MINOR, a failed network create: D6 now defines it (skip the kernel rows only).
- MINOR, leaked networks: a label and a documented prune (D6, §6.3).
- MINOR, DNS: `--add-host` for `iam` and `gateway` (D1, M2).
- MINOR, the descriptor cache on Redis: named in D1 (F16), M6 and the RUNBOOK bullet.
- MINOR, host clock: `now` comes from `redis-cli TIME` (D2).
- MINOR, one request for status and body: D3 uses one `curl -o <file> -w`, and `grep -F` on the
  file with a checked rc.
- MINOR, no cleanup row: `console_smoke_cleanup` joins `FUNCS` with an order row.
- QUESTION, a route handler: the rejection is recorded in D1.
- QUESTION, 3xx-only variant: the reason for the 200 is in D1; the choice is Q4.
- QUESTION, control on the release path: the default is in D4; the choice is Q5.

**Rejected:** none. Every finding matched the code. One detail differs from the suggestion: for a
failed network create, the fallback main containers use the old memory-store env, because a
redis-store env with no reachable Redis is not the tested baseline for the other rows.

## 14. Approval changelog (2026-09-27)

**Decisions applied (Sven):**

- Q4: keep the 200 with the per-run nonce and the seeded `SessionRecord`. D1 and §12 say so.
- Q1: new section D3a (fallback (b), used only if M1 shows no 200). The M1 row of §5, AC 1, AC 7,
  §6.2, §10 and §11 follow it. D1 no longer names the stub IAM server as the fallback.
- Q2: new D4 step 6 (remove the control container when its row finishes), with self-test argv
  rows, a pin, mutation 9 and AC 2.
- Q3: the reverse comment in `ci/kind/manifests/redis.yaml` (§6.1, §6.3, §10, AC 10).
- Q5: the control runs only on the `images.yml` paths. New mechanism in D4:
  `smoke_consoles --kernel-control=on|off`, driven by `PAIGASUS_SMOKE_KERNEL_CONTROL` (default on),
  and `release.yml` sets `off`. F10, §6.1, §6.2, §6.3, §6.4, §10, §11, mutation 10 and AC 2
  follow it. `release.yml` is now on the file list (one `env:` entry). The mechanism itself was
  chosen at approval time by the unattended setup run, not by Sven: default ON so that a missing
  setting runs more checks, not fewer.

**Re-check against origin/main `4051df5e`** (the spec was written on `ad1c44b7`; SMA-705 PR 331 and
SMA-695 PR 332 merged since):

- §1: `runtime.ts:127` is now `runtime.ts:129` (SMA-705 added two comment lines). F17's
  `runtime.ts:102` is unchanged.
- New F18: SMA-705 added a public `<basePath>/readyz` route. It answers 503 in the smoke (no IdP
  discovery), so the D4 readiness gate stays on `/healthz`. `run.sh` and the `HEALTHCHECK` still
  use `/healthz`.
- §6.3: the RUNBOOK does not claim that the smoke proves the kernel load (the old "line 56"
  pointer was a guess; line 56 is about wasm build inputs). The new bullet goes after the zone-row
  bullet at line 431. The path-filter list is at lines 33-43, and its rule at lines 45-62 needs a
  second clause, because the four new files are on the filter for a runtime coupling, not a
  build-only input.
- §6.4: `release.yml:1246` is the `run:` line of the step at `1245-1246`; the `images.yml` smoke
  calls are at lines 192 and 211.
- SMA-695 (chart Ingress optional) touches only `charts/`, `ci/helm-render/` and the chart RUNBOOK.
  It does not affect this spec.
- Every other path and line number in §2 was re-read on `4051df5e` and is unchanged: the SMA-705
  diff does not touch `ci/images/`, the workflows, the `(console)` layouts, the probe pages,
  `core/session.ts`, `core/single-flight.ts`, `adapters/redis-store.ts`, `config.ts` or
  `console-core`.
