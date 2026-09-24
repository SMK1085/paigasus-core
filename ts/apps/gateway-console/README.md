# `@paigasus/gateway-console`

The AI Gateway zone of the Paigasus console. It is a Next.js 16 app, mounted at `/gateway`, that
runs as a standalone server, beside `@paigasus/iam-console` (mounted at `/iam`) in a multi-zone
deployment. It has the login, the console shell, the zone overview, and the organization and
project settings pages (SMA-636). Design: `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md`
and `docs/superpowers/specs/2026-09-18-sma-636-gateway-org-project-settings-design.md`.

## Run it locally

`next dev` cannot start this app on its own: nine environment variables have no default, two of them
are schema-enforced `https` URLs, and the app calls a real OIDC provider, an IAM gRPC endpoint and
two service-info endpoints. The dev stack supplies all of it:

```bash
pnpm --dir ts dev:stack
```

It starts a fake IdP, a fake IAM, a fake gateway, a TLS terminator and a Redis container, then runs
both console zones under `next dev` and prints the URL to open:
`https://127.0.0.1:8443/iam`. Startup takes about 9 seconds; shutdown takes about 6. **Docker is
required**, and `PAIGASUS_SESSION_STORE` is forced to `redis`: `createAuthRuntime` refuses the
memory store once `PAIGASUS_ZONES` names more than one zone. Accept the self-signed certificate
once for each of the two origins it uses: the terminator on port 8443, and the fake OIDC provider
on port 8444, where login redirects — both ports are fixed, so each exception persists across runs.
Hot reload works: the terminator tunnels the WebSocket. A restart logs you out, because Redis is
new each run. On shutdown the command restores `next-env.d.ts` and removes the
`AGENTS.md`/`CLAUDE.md` files Next generates, so it leaves the working tree as it found it.

## Rules that the code depends on

- Every file in `lib/` starts with `import 'server-only'`. A client component that imports one fails the build.
- Relative imports in `app/`, `lib/` and `proxy.ts` have no file extension (Turbopack, see `CLAUDE.md`).
- No page skips its IAM call because `mayI()` said no. `mayI()` (`@paigasus/console-core`'s `authorize.ts`) only hides a button, a section or a nav entry. IAM decides.
- No `NEXT_PUBLIC_` anywhere (`repo:next-public-free`). Every deployment value is read at request time.

## Environment

The image reads these variables at the first request. A parse failure stops every request.

| Group     | Variables                                                                                                                                                                                                                                              |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Zone      | `PAIGASUS_ZONE=gateway`, `PAIGASUS_ZONES` (JSON, the same in every zone)                                                                                                                                                                               |
| Auth      | the `authEnvShape` keys (`ts/packages/paigasus-auth/README.md`). `PAIGASUS_SESSION_STORE=redis` is **mandatory** in a multi-zone deployment: `@paigasus/auth`'s `createAuthRuntime` throws on `memory` when `PAIGASUS_ZONES` names more than one zone. |
| Discovery | `PAIGASUS_SERVICES` (JSON; must carry **both** an `iam` and a `gateway` entry — `lib/config.ts` refuses a map missing either one), optional `PAIGASUS_DISCOVERY_*_MS`                                                                                  |
| IAM       | `PAIGASUS_IAM_GRPC_URL` (IAM's gRPC address: absolute `http:` or `https:`, no credentials, no query, no fragment)                                                                                                                                      |

See `.env.local.example` for a working set of values for `next dev`.

## Commands

Every command below goes through Moon, which resolves its tools from the proto shims. A
non-interactive shell does not have them on `PATH`, and the command then runs the globally pinned
tool or none at all. Export them once per shell first:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

| Task                                       | Command                                 |
| ------------------------------------------ | --------------------------------------- |
| Build (asserts the standalone `server.js`) | `moon run gateway-console-ts:build`     |
| Unit and integration tests, Tailwind guard | `moon run gateway-console-ts:test`      |
| Browser tier                               | `moon run gateway-console-ts:test-e2e`  |
| Type check                                 | `moon run gateway-console-ts:typecheck` |

Only the browser tier needs Docker or a live service. The browser tier now holds a two-zone
Playwright project (see below). The two projects share one Moon task,
`gateway-console-ts:test-e2e`. So a run with no Docker daemon also loses the single-zone rows, not
only the two-zone ones. There is no skip hatch. A run fails loudly when Docker is unreachable,
instead of reporting green having proved nothing.

## Test tiers

- **Unit and integration** (`tests/unit`, `tests/integration`, vitest). The integration tests talk real gRPC to an in-process fake IAM and a fake gateway (`@paigasus/console-core`'s `testing/fake-iam.ts` and `testing/fake-gateway.ts`).
- **Browser** (`tests/e2e`, Playwright). `gateway-console-ts:test-e2e` runs two Playwright projects, selected by file name:
  - **`single-zone`** — a production build runs through the standalone server behind an in-process TLS terminator, with the fake IAM, a fake gateway and a fake HTTPS IdP. The session store is `memory`, so the zone map holds one zone. It covers rows R1 through R7, plus R4b, a second capability case that pairs with R4, and rows R13 through R20 for the settings pages of SMA-636.
  - **`two-zone`** (SMA-512 pull request 4) — a worker fixture starts a `redis:8-alpine` container (the first container this repository starts this way), both apps' standalone servers (`iam-console` and `gateway-console`), and one TLS terminator that path-routes to both. The session store is `redis`, so a session set on one zone is visible on the other. It covers rows R8 through R12, and row R21 for the settings pages of SMA-636. Together they prove the cross-zone session, the gateway zone's isolation from an IAM-only session, and that the two apps' static chunks do not collide under one origin.
  - **`playground`** (SMA-635) — the REAL `paigasus-gateway` binary, built from the commit under test by `paigasus-gateway-rs:e2e-bin` (never cached), configured only through `GATEWAY_*` variables, with its own fake IAM and a mock OpenAI server that the test controls step by step. No container. It covers rows R22 through R28: chunk-by-chunk streaming with the gateway's own `auth=oidc` log line, Stop, the `insufficient-permissions` deny, the self-query wiring, the 401 without a session, a failure inside a record, and the token-leak scan of the chat route. The fake IAM is not Cedar: the rows prove the wiring, not the decision.

  Each row is one test. `tests/unit/e2e-rows.test.ts` holds the full row list (R1–R7, plus R4b and R7's 403 control, then R8–R12, then R13–R21 for the settings pages of SMA-636, then R22–R29 for the playground of SMA-635 (R29, the streaming-off composer, is in the single-zone project)). It fails when a row is missing, duplicated, or renamed.

## Known limits

- Acceptance criteria 1 and 2 (the two-zone properties) are now proved by the `two-zone` Playwright project above (SMA-512 pull request 4). Row R8 proves AC 1: a session from the IAM zone carries into the gateway zone with no second authorization. Row R10 proves AC 2: a cold login at the gateway zone works with zero connections to `iam-console`.
- Acceptance criterion 3 is delivered by SMA-635: the organization page links to `/gateway/orgs/<org>/playground`, which streams a chat completion through `POST /gateway/api/chat` with the user's own bearer. A person needs the `gateway_user` role on the organization. The console has no control that grants it to a person: an organization admin grants it out of band with IAM `GrantRole` (SMA-635 D10). The playground shows "You need the gateway_user role on this organization" until then. `org_admin` alone does not hold `InvokeModel`.
- The gateway has no rate limit and no spend budget. The playground opens model spend to every person who holds `gateway_user` (SMA-635 spec §9).
- The settings pages list the service accounts that the organization or the project owns, and their API keys (SMA-636). An account that a team owns is not visible in this zone, so an organization admin has no complete list of live keys here.
- Creating a service account makes two IAM calls, not one atomic call. A failed `gateway_user` grant leaves an account that cannot call models until someone uses "Allow model calls".
- An account that got `gateway_user` outside the console, at an ancestor scope, shows "Can call models: Yes". The console cannot show where the grant comes from.
- There is no "Stop model calls" control: `RevokeRole` needs a grant id, and only a platform admin can list another principal's grants. Archive the account or revoke its keys instead.
- After an archive, IAM evicts the account's keys from its API-key cache. How fast every IAM replica stops accepting them depends on IAM's cache configuration, which the console does not check.
- `FormError`, `node-status.ts`, `section-error.tsx` and the two-step confirm button are copies of iam-console's. Nothing gates a divergence.
- The organization page makes one `ListProjects` call per shown team (at most 50, at most 8 in flight). Row R19 counts the calls of one render; nothing measures their latency against a real IAM.
- With JavaScript off, the settings pages are read-only: every mutation control renders after hydration.
- The organization switcher runs `myScopes()` on every console page render — one `WhoAmI` (SMA-632), one `ListRoleGrants` walk and up to 50 tenancy reads. The cost is not measured, and this zone pays it independently of `iam-console`, which pays the same cost.
- A lost `cache()` memoization inside `lib/console.ts` (there must be exactly one `createConsoleRuntime()` call) is observable only in the browser tier, and this app ships that rule today with no automated control behind it.
- A stopped zone app is invisible to ADR-0020 discovery: discovery probes services, not zone apps, so a zone whose app is down still renders an _available_ nav entry pointing at a dead route. A later ingress layer is where that would be caught.
- The `absent` gateway state is covered only at the unit tier: `lib/config.ts` refuses to parse a `PAIGASUS_SERVICES` map with no `gateway` entry, so the integration tier cannot reach that branch.
- The presentation-copy table (`app/_components/error-copy.ts`) stays per zone: `iam-console` keeps its own table, worded for IAM, and nothing gates the two tables against each other. That is intended.
- About 250 lines of server composition and view code are byte-identical with `iam-console`, and nothing in the repository gates a divergence between the two copies. See spec § 13 for the full list of files and why this is recorded rather than extracted.
- This app loads `@paigasus/kernel` through its WASM binding, on the server too (SMA-634, ADR-0022). The napi binding cannot load in a Next build: pnpm installs a `file:` target once, at install time, by the package's `files` allowlist, and Turbopack refuses a `link:` symlink whose target is outside its root. So the kernel's `.` export is the wasm entry under every condition, and `@paigasus/kernel/napi` stays reachable for the kernel's own tests alone. `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` and its four glue files are COMMITTED, written only by `moon run paigasus-kernel-ts:generate-wasm`, and held to the Rust source by a four-check drift gate. A wasm that cannot load 500s EVERY `(console)` page, not only the PRN pages.
- Row R16's token-exposure scan cannot read two classes of response, and both are excluded from the row's residue check on purpose: a prefetch response the router issues but the interceptor could not buffer (an unbuffered prefetch), and a 3xx redirect, whose body Playwright's `Response.body()` refuses to read. Every response header, a redirect's included, is still scanned. This is the honest limit of spec § 7.2 row 4, not a gap the row hides.
- A successful key issue revalidates the page (plan SPEC DEVIATION 8). The token panel survives a revalidated section that turns into an error or a denial, but not a page-level error: if `GetOrganization` or `GetProject` fails on that render, the whole page is replaced and the token is lost.
- The project page's breadcrumb link to its organization page uses the default prefetch, because `@paigasus/app-shell`'s `Breadcrumbs` takes no `prefetch` option. For a `project_admin` with no access to the organization, every project page view prefetches a 403 render of the organization page.
