# `@paigasus/gateway-console`

The AI Gateway zone of the Paigasus console. It is a Next.js 16 app, mounted at `/gateway`, that
runs as a standalone server, beside `@paigasus/iam-console` (mounted at `/iam`) in a multi-zone
deployment. This task builds only the project skeleton and the public (signed-out) shell; the
console views land in later tasks of SMA-512 pull request 3.

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

| Task                                       | Command                                 |
| ------------------------------------------ | --------------------------------------- |
| Build (asserts the standalone `server.js`) | `moon run gateway-console-ts:build`     |
| Unit tests, Tailwind guard                 | `moon run gateway-console-ts:test`      |
| Type check                                 | `moon run gateway-console-ts:typecheck` |

Do not run `moon ci` against this app before SMA-512's task 6: `repo:affected-smoke` and
`repo:next-env-drift` are red by construction until that task closes the known-red window (see
the plan's "Known-red window" section).

## Known limits (task 1)

- Only the project skeleton, `lib/config.ts` and the public (signed-out) shell exist so far. The
  composition root, the console views, the test doubles and the e2e tier land in later tasks.
- The kernel's napi binding cannot load in a Next build, because `@paigasus/node-bindings` ships
  no `.node` binary. The defect stays open for every Node consumer of `@paigasus/kernel` (SMA-634).
