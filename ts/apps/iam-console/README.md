# `@paigasus/iam-console`

The IAM zone of the Paigasus console (ADR-0017). It is a Next.js 16 app, mounted at `/iam`, that
runs as a standalone server. It has the login, the console shell, the tenancy screens, the 403
view and the capability-gated audit screen. Design: `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md`.

## Rules that the code depends on

- Every file in `lib/` starts with `import 'server-only'`. A client component that imports one fails the build.
- Relative imports in `app/`, `lib/` and `proxy.ts` have no file extension (Turbopack, see `CLAUDE.md`).
- No page and no Server Action skips its IAM call because `mayI()` said no. `mayI()` (`lib/authorize.ts`) only hides a button, a form, a section or a nav entry. IAM decides.
- Every Server Action gets its client through `iamClients()`, so `requireSession()` runs for each action. `tests/unit/actions-structure.test.ts` holds this rule, and a new action must be added to its list.
- URLs use UUIDs. A segment that is not a UUID is a 404 with no IAM call. A team or project whose IAM answer disagrees with the URL is a 404.
- There is no `loading.tsx` under `app/(console)/`, and none may be added: its Suspense boundary lets Next send status 200 before a page's `forbidden()` throws, so the 403 status is lost (e2e R4 asserts it).
- No `NEXT_PUBLIC_` anywhere (`repo:next-public-free`). Every deployment value is read at request time.

## Environment

The image reads these variables at the first request. A parse failure stops every request.

| Group     | Variables                                                                                                                                    |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Zone      | `PAIGASUS_ZONE=iam`, `PAIGASUS_ZONES` (JSON, the same in every zone)                                                                         |
| Auth      | the `authEnvShape` keys (`ts/packages/paigasus-auth/README.md`). Use `PAIGASUS_SESSION_STORE=redis` in a deployment with more than one zone. |
| Discovery | `PAIGASUS_SERVICES` (JSON; it must contain `iam`, IAM's HTTP address), optional `PAIGASUS_DISCOVERY_*_MS`                                    |
| IAM       | `PAIGASUS_IAM_GRPC_URL` (IAM's gRPC address: absolute `http:` or `https:`, no credentials, no query, no fragment)                            |

`.env.local.example` still carries the pre-rename variable set. This branch could not update it: a
session permission rule denied access to `.env.*` paths. The table below, plus spec § 10, are the
contract until someone syncs that file.

Deployment assumptions (spec § 10):

- IAM's `authn.issuers[].audiences` accepts the audience of the console client's access tokens.
- The ingress forwards `Host` (or `X-Forwarded-Host`) unchanged. Otherwise Next's Server Action origin check fails.
- The console reaches IAM's gRPC port over h2c in the cluster, or over TLS with a CA that Node trusts (`NODE_EXTRA_CA_CERTS`).
- The OIDC client registers `https://<origin>/iam/auth/callback` as a redirect URI and `https://<origin>/iam/` as the post-logout URI.

## Commands

| Task                                                                         | Command                             |
| ---------------------------------------------------------------------------- | ----------------------------------- |
| Build (asserts the standalone `server.js`)                                   | `moon run iam-console-ts:build`     |
| Unit, integration, standalone smoke, client-boundary fixture, Tailwind guard | `moon run iam-console-ts:test`      |
| Browser tier                                                                 | `moon run iam-console-ts:test-e2e`  |
| Type check                                                                   | `moon run iam-console-ts:typecheck` |

No tier needs Docker or a live service.

## Test tiers

- **Unit and integration** (`tests/unit`, `tests/integration`, vitest). The integration tests talk real gRPC to an in-process fake IAM (`tests/support/fake-iam.ts`), and MSW serves `GET /v1/service-info`. Loaders (`load.ts`) take their IAM client and `mayI` as arguments, and commands (`commands.ts`) take their IAM client only, so the tests call them with no session and no Next runtime.
- **Client boundary** (`tests/build-guard/client-boundary.test.ts`). `next build` on `tests/fixtures/client-imports-sdk` must fail with the `server-only` error; the same code in a server component (`tests/fixtures/server-imports-sdk`) must build.
- **Browser** (`tests/e2e`, Playwright). A production build runs through the standalone server behind an in-process TLS terminator, with the fake IAM and a fake HTTPS IdP. The session store is `memory`, so the zone map holds one zone. Each row of the spec's § 9.4 table is one test (`R1`–`R12`); `tests/unit/e2e-rows.test.ts` holds that.

## Known limits

- The attach form takes a raw principal PRN: IAM has no user lookup RPC.
- `mayI()` fails open: when `IsAuthorized` fails, a button shows, and IAM then denies the action.
- The e2e tier runs one zone. A two-zone test belongs to SMA-513.
- `Introspect` reports no role grants (SMA-633), so the app asks `IsAuthorized` about itself instead of reading grants from the session (a recorded departure from ADR-0017 decision 8).
- `forbidden()` needs the experimental `authInterrupts` flag. E2e row R4 asserts the real HTTP 403, so a Next upgrade that changes the flag reds CI. The fallback, an inline view with status 200, is recorded in spec § 6.2 and not built.
- The 403 view shows the correlation id only when `FORBIDDEN_VIEW_CORRELATION` (`lib/correlation.ts`) is `'header'`. When it is `'fallback'`, `callIam` logs the id with the path, and the section and form 403s show it.
- The console layout calls `myScopes()` on every console page for the organization switcher. That is one `Introspect`, up to ten `ListRoleGrants` pages and up to 50 tenancy reads per render. The cost is not measured (spec § 12). If it is too slow, a short-lived per-session cache is the next step.
- `lib/prn-tenancy.ts` is a recorded ADR-0005 exception (decision D6, fallback C). `tests/unit/prn-tenancy.test.ts` holds it to the kernel: it replays every vector of the kernel parity corpus.
- The kernel's napi binding cannot load in a Next build, because `@paigasus/node-bindings` ships no `.node` binary. The defect stays open for every Node consumer of `@paigasus/kernel` (SMA-634).
