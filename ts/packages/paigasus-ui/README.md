# @paigasus/ui

Shared, hand-written React component library for Paigasus's console and future zone apps: a
design-token layer (`src/styles/tokens.css`), seven components built on the unified `radix-ui`
package plus `cmdk`, and a jsdom test tier with `axe-core` assertions. Source-only exports — a
consumer resolves `@paigasus/ui` and `@paigasus/ui/styles.css` straight through
`moduleResolution: bundler`; there is no build step yet.

## What a consumer must do

1. Depend on `@paigasus/ui` (catalog / workspace version).
2. Import the token layer once, from the app's own global stylesheet:
   `@import '@paigasus/ui/styles.css';`
3. Add a Tailwind `@source` line covering this package's source — see "The per-consumer
   `@source` obligation" below. Skipping this is the single most likely way to break a new
   consumer, and the breakage looks like missing styling, not a build error.

## The axe limit: contrast is not covered

The jsdom test tier runs `axe-core` against rendered components (see
`expectNoAxeViolations` in the test harness) and asserts on roles, accessible names, ARIA state
and ARIA labelling. It does **not**, and cannot, assert on `color-contrast`: axe's
`color-contrast` rule needs real layout (computed styles, actual pixel geometry, font
rendering) to measure contrast ratios, and jsdom performs no layout at all — every element
reports a zero-size box. axe silently skips (does not fail) any rule it cannot evaluate in that
environment, so a green `expectNoAxeViolations()` run says nothing about whether a token pairing
is readable. Do not read the green as covering contrast. Contrast is a design responsibility,
decided once in the token layer (`src/styles/tokens.css`), not re-verified per component.

## The per-consumer `@source` obligation

Tailwind v4 does not scan `node_modules` by default, which is how pnpm resolves
`@paigasus/ui` into a consumer's tree. A green `paigasus-console-ts:test` — the
`ci/tailwind-source/run.mjs` guard described in that directory's own README — proves only that
**the console** still reaches this package's classes in production. It says nothing about a
second zone app.

Every new consumer must add, to its own global stylesheet:

- its own `@source` line resolving to `ts/packages/paigasus-ui/src`. **Tailwind resolves an
  `@source` path relative to the stylesheet that declares it, not to the repository root**, so
  each consumer must work out its own relative path rather than copying another app's. The
  console's stylesheet sits at `ts/apps/paigasus-console/app/globals.css` and therefore writes
  `@source '../../../packages/paigasus-ui/src';` — an app at a different depth needs a different
  number of `../` segments. Copying that line verbatim points Tailwind at a path that does not
  exist, and a non-existent `@source` is not an error: it scans nothing and the package's classes
  are silently absent from the production build, which is the exact failure this section exists to
  prevent.
- its own `@import '@paigasus/ui/styles.css'`, and
- its own probe assertion (a copy of, or a task wired the same way as,
  `ci/tailwind-source/run.mjs` against that app's own build output).

Without all three, the app's production build silently drops this package's classes — the
failure surfaces as broken styling in the browser, not as a build error, and no existing CI gate
catches it for an app other than the console. See `ci/tailwind-source/README.md` for the guard
mechanics and CLAUDE.md's Gotchas section for why the scan root is CWD-relative.

## The `shadcn` CLI is not usable in this package

`components.json` is committed, configured for shadcn's monorepo mode, per ADR-0021's
"configured once" instruction. **Running it is still not safe.** Do not run `pnpm dlx shadcn add
<component>` and commit its output without reading this section first.

**Measured against `shadcn@latest`, 2026-09 (Task 9, fix round 1).** Two separate defects, and
fixing one does not fix the other:

1. **Wrong file location**, caused by this package's `tsconfig.json` carrying no `@/*` path — the
   CLI's `aliases.components`/`aliases.utils` in `components.json` are `@/...` specifiers, and
   with nothing to resolve them the CLI writes to a **literal** `./@/components/` directory
   instead of `src/components/`. Verified fixable: adding a `paths: { "@/*": ["./src/*"] }`
   mapping to `tsconfig.json` and re-running (`shadcn add tooltip`) does land the file at
   `src/components/tooltip.tsx`.
2. **A spurious dependency, unaffected by the above fix.** Even with file placement corrected,
   the generated component still contains `import { cn } from "cn"` and `package.json` gains
   `"cn": "^0.2.6"` — a real, unrelated package on the npm registry that happens to share a name
   with this package's own `cn` utility (`src/lib/cn.ts`). This is baked into the shadcn registry
   item's source, not a resolvable consequence of `aliases.utils`. Generated components also
   pull in `lucide-react` (not a dependency here) and expect a sibling `Button` component that
   does not exist in this package.

Every component in `src/components/` — `Dialog` (`dialog.tsx`), `DropdownMenu`
(`dropdown-menu.tsx`), `Popover` (`popover.tsx`), `Command` (`command.tsx`), `Combobox`
(`combobox.tsx`), `Label`/`Input`/`Field` (`form.tsx`), `EmptyState` (`empty-state.tsx`) and
`ErrorState` (`error-state.tsx`), alongside the `Table` family (`table.tsx`) this list's shape
follows — was therefore **hand-written** against the unified `radix-ui` package, following the shape and
comment density of `src/components/table.tsx`, rather than generated and normalised. If a future
task uses the CLI as a starting point regardless, it must still, at minimum:

- delete the spurious `cn` dependency from `package.json` and re-point the import at `../lib/cn`;
- remove any individual `@radix-ui/react-*` packages the CLI adds and re-point imports at the
  unified `radix-ui` package;
- replace any `lucide-react` icon and any dependency on a local `Button` component that does not
  exist here;
- add the SPDX header above `'use client'`, and do the type-level rework `tsconfig.base.json`'s
  `exactOptionalPropertyTypes`, `verbatimModuleSyntax`, `noUncheckedIndexedAccess` and
  `isolatedModules` demand — shadcn output satisfies none of the first three.

## `cmdk` and `radix-ui` can split Radix's Dialog — `Command.Dialog` is banned

`cmdk` depends on `@radix-ui/react-dialog`, `@radix-ui/react-id`, `@radix-ui/react-primitive`
and `@radix-ui/react-compose-refs` individually, while this package depends on the unified
`radix-ui`, which declares the same modules as ordinary dependencies of its own.

**Today there is exactly ONE copy of each.** `ts/pnpm-lock.yaml` holds a single
`@radix-ui/react-dialog@1.1.23` resolution and a single snapshot, shared by `radix-ui@1.6.7`,
`cmdk@1.1.1` and `@radix-ui/react-alert-dialog`. `radix-ui` declares no `bundledDependencies`.
An earlier revision of this section, and of the spec, claimed two copies already existed; that
was wrong, and the lockfile is what settles it.

**The hazard is latent, not present.** `radix-ui` pins its Radix dependencies **exactly**
(`"@radix-ui/react-dialog": "1.1.23"`), while `cmdk` uses a **caret** (`"^1.1.6"`). They
deduplicate only because the exact pin happens to fall inside the caret. A `radix-ui` bump past
cmdk's range splits them into two resolutions, with no install error and no warning.

**So the ban stands.** Nothing here may compose across Radix copies: a trigger from one copy
does not open content from the other, the failure is silent at runtime (no error, no warning),
and jsdom tests may not surface it either. cmdk's own Radix modules are reachable only through
`Command.Dialog`, so `Command.Dialog` is **banned** — do not use it, even though `cmdk` exports
it. It costs nothing today, and it is what makes the split above a non-event when it happens.
`Combobox` (`src/components/combobox.tsx`) composes `radix-ui`'s own `Popover` with cmdk's
plain `Command`; `Dialog` (`src/components/dialog.tsx`) comes from `radix-ui` only. See the
header comment in `src/components/command.tsx` for the fuller version of this rule.
