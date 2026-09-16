# `@paigasus/gateway-console`

The AI Gateway zone of the Paigasus console. It is a Next.js 16 app, mounted at `/gateway`, that
runs as a standalone server, beside `@paigasus/iam-console` (mounted at `/iam`) in a multi-zone
deployment. It has the login, the console shell, the zone overview and the organization scope
route. Design: `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md`.

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
  - **`single-zone`** — a production build runs through the standalone server behind an in-process TLS terminator, with the fake IAM, a fake gateway and a fake HTTPS IdP. The session store is `memory`, so the zone map holds one zone. It covers rows R1 through R7, plus R4b, a second capability case that pairs with R4.
  - **`two-zone`** (SMA-512 pull request 4) — a worker fixture starts a `redis:8-alpine` container (the first container this repository starts this way), both apps' standalone servers (`iam-console` and `gateway-console`), and one TLS terminator that path-routes to both. The session store is `redis`, so a session set on one zone is visible on the other. It covers rows R8 through R12. Together they prove the cross-zone session, the gateway zone's isolation from an IAM-only session, and that the two apps' static chunks do not collide under one origin.

  Each row is one test. `tests/unit/e2e-rows.test.ts` holds the full row list (R1–R7, plus R4b and R7's 403 control, then R8–R12). It fails when a row is missing, duplicated, or renamed.

## Known limits

- Acceptance criteria 1 and 2 (the two-zone properties) are now proved by the `two-zone` Playwright project above (SMA-512 pull request 4). Row R8 proves AC 1: a session from the IAM zone carries into the gateway zone with no second authorization. Row R10 proves AC 2: a cold login at the gateway zone works with zero connections to `iam-console`.
- Acceptance criterion 3 is not delivered at all: the gateway's chat route authenticates Paigasus API keys and never an OIDC token, so a playground built on a console session could not make one real call. A follow-up issue owns widening `require_iam_auth` and deciding the authorization resource for a user principal.
- The organization scope route changes the URL and the breadcrumbs and nothing else. It exists so a later settings screen has a working shape to hang off, and so the switcher is not a dead control.
- The organization switcher runs `myScopes()` on every console page render — one `Introspect`, one `ListRoleGrants` walk and up to 50 tenancy reads. The cost is not measured, and this zone pays it independently of `iam-console`, which pays the same cost.
- A lost `cache()` memoization inside `lib/console.ts` (there must be exactly one `createConsoleRuntime()` call) is observable only in the browser tier, and this app ships that rule today with no automated control behind it.
- A stopped zone app is invisible to ADR-0020 discovery: discovery probes services, not zone apps, so a zone whose app is down still renders an _available_ nav entry pointing at a dead route. A later ingress layer is where that would be caught.
- The `absent` gateway state is covered only at the unit tier: `lib/config.ts` refuses to parse a `PAIGASUS_SERVICES` map with no `gateway` entry, so the integration tier cannot reach that branch.
- The presentation-copy table (`app/_components/error-copy.ts`) stays per zone: `iam-console` keeps its own table, worded for IAM, and nothing gates the two tables against each other. That is intended.
- About 250 lines of server composition and view code are byte-identical with `iam-console`, and nothing in the repository gates a divergence between the two copies. See spec § 13 for the full list of files and why this is recorded rather than extracted.
- The kernel's napi binding cannot load in a Next build, because `@paigasus/node-bindings` ships no `.node` binary. The defect stays open for every Node consumer of `@paigasus/kernel` (SMA-634).
