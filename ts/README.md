# ts/

TypeScript workspace for paigasus-core, managed with [pnpm](https://pnpm.io/) and orchestrated by [Moon](https://moonrepo.dev).

## Layout

- `pnpm-workspace.yaml` — declares the workspace members (`packages/*`, `apps/*`) and the dependency catalog. The `catalog:` block is the single bump-point for shared versions across the workspace; per-package `package.json` references entries as `"<dep>": "catalog:"`.
- `package.json` — workspace root. Private, ESM-only, declares the workspace-wide devDependencies (TypeScript, ESLint, Prettier, plugins, Vitest) referenced via `catalog:`.
- `tsconfig.base.json` — shared `compilerOptions` every package's `tsconfig.json` extends. Strict; `moduleResolution: bundler`; ES2022 target.
- `eslint.config.js` — flat config. Type-checked TS rules across `**/*.{ts,tsx,mts,cts}`; React rules (`@eslint-react/eslint-plugin`, `eslint-plugin-react-hooks`, `eslint-plugin-jsx-a11y`) glob-scoped to `**/*.{tsx,jsx}` only, so non-React libraries don't see them.
- `.prettierrc.js`, `.prettierignore` — formatting config. `printWidth: 200` (cross-stack with py/rs).
- `moon.yml` — workspace parent project (`layer: configuration`). Owns the whole-tree `lint`/`fmt` (run once from `ts/`); `build`/`typecheck`/`test` are routed per-project by layer (`.moon/tasks/typescript-project.yml`), not to the root — Moon owns those fan-out graphs (SMA-401).
- `packages/*` — publishable libraries; each is a uv-style first-class Moon project (id `paigasus-<short>-ts`):
  - `paigasus-proto` (`@paigasus/proto`) — protobuf-es types generated from `contracts/` (extensionless imports; see CLAUDE.md, Turbopack)
  - `paigasus-kernel` (`@paigasus/kernel`) — the Rust kernel for Node (napi) and the browser (wasm); a Next build cannot load the napi binding (SMA-634)
  - `paigasus-sdk` (`@paigasus/sdk`) — server-only Connect clients for the IAM gRPC services, and the error map
  - `paigasus-ui` (`@paigasus/ui`) — shared React components for the console
  - `paigasus-next-config` (`@paigasus/next-config`) — the Next config factory, runtime config and the ESLint boundary and source rules
  - `paigasus-auth` (`@paigasus/auth`) — the BFF session: OIDC login, the session store and the proxy
  - `paigasus-discovery` (`@paigasus/discovery`) — capability discovery with three service states
  - `paigasus-app-shell` (`@paigasus/app-shell`) — the console chrome: header, navigation, switchers, cross-zone links
  - `paigasus-console-core` (`@paigasus/console-core`) — server-only console composition shared by every zone: the IAM client factory, provisioning, the principal resolver, `mayI()`, `myScopes()`, `callIam`, the JSON-lines logger, discovery and the PRN readers. It is the one package allowed to import both `@paigasus/auth` and `@paigasus/sdk`, because `@paigasus/auth` must not import `@paigasus/sdk` and the sdk boundary block bans every other `@paigasus/*` package. Its `createConsoleRuntime` factory must be called exactly once per app, at module scope, because each accessor is a React `cache()` wrapper and a second call makes a second memoization identity — outside a server render `cache()` is a pass-through, so no unit or integration test can catch a violation; only an e2e `Introspect` count can. Two of its exports are deliberately NOT memoized and must stay that way: `iamClientsForToken`, which takes a token the caller already holds and so has no per-request identity to cache, and `iamClientsForAction`, which must return its `relogin` failure to a Server Action rather than redirect — memoizing it would let one action's expired-session result be replayed to another. Its `testing/` subpath sits outside `src/` and carries no `server-only` guard on purpose, because vitest and Playwright harnesses import it outside a Next server.
- `apps/*` — deployables, one Next.js app per console zone (id `<name>-ts`):
  - `iam-console` (`@paigasus/iam-console`) — Next.js 16 (App Router) console zone for IAM, mounted at `/iam`; see its [README](apps/iam-console/README.md) for the environment and the test tiers
  - `gateway-console` (`@paigasus/gateway-console`) — Next.js 16 (App Router) console zone for the AI Gateway, mounted at `/gateway`; see its [README](apps/gateway-console/README.md) for the environment and the test tiers

## Commands

`lint`/`fmt` run once over the whole workspace from the `ts` Moon project; `typecheck`, `test`, and `build` fan out per project (Moon owns those graphs by layer — SMA-401), so they are addressed with a TypeScript-scoped query — a bare `moon run :test` would also run the `rust`/`py` workspaces:

| Task                     | Command                                                     |
| ------------------------ | ----------------------------------------------------------- |
| Lint                     | `moon run ts:lint`                                          |
| Format check             | `moon run ts:fmt`                                           |
| Type check               | `moon run :typecheck --query "language=typescript"`         |
| Test                     | `moon run :test --query "language=typescript"`              |
| Build (all TS)           | `moon run :build --query "language=typescript"`             |
| Build (one app)          | `moon run iam-console-ts:build`                             |
| E2E (one app)            | `moon run iam-console-ts:test-e2e`                          |
| Tailwind guard (one app) | `node ci/tailwind-source/run.mjs --app ts/apps/iam-console` |

Notes:

- For env parity, invoke pnpm via `moon run ts:<task>` so Moon's pinned Node (`.moon/toolchains.yml`) is used, not whatever's on PATH.
- `Type check`, `Test`, and `Build` use a TypeScript-scoped query: the `ts` root no longer defines those tasks — Moon's per-project tasks own them (`typecheck`/`build` since SMA-394; `test` since SMA-401, as vitest has no central config and runs per-package) — and a bare `moon run :build`/`:test` would also hit the `rust`/`py` workspaces, so the query scopes it to TS. `lint`/`fmt` still run once from the `ts` project.
- Per-package install: `pnpm --filter @paigasus/<name> add <dep>`. For dev deps: `pnpm --filter @paigasus/<name> add -D <dep>`.
- The `catalog:` block in `pnpm-workspace.yaml` is the single bump-point for shared versions. To bump a tool, edit the catalog entry — every package picks up the new version on the next `pnpm install`.
- All packages currently ship `private: true` with `"exports": { ".": "./src/index.ts" }`. Before any first publish: drop `private`, add `description`/`repository`/`homepage`/`keywords`, switch `exports` to `./dist/index.js`, and wire `tsup` per package (these MUST land together; see the SMA-359 design spec §H).
- The `test` task runs `vitest run --passWithNoTests`. Drop the flag once the first real test lands.

**Status:** workspace bootstrapped in SMA-359; packages are empty stubs.
