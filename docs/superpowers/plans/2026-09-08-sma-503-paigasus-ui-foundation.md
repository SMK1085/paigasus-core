# `@paigasus/ui` Tailwind v4 + Radix Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the `@paigasus/ui` stub into a working shared React component package on Tailwind v4 and Radix, with the `@source` reachability proven by a production build.

**Architecture:** The package stays source-only — no build step, no `dist`. Tailwind scans its files on disk from the consuming app, which is why the app's `@source` line is load-bearing and why a build-level assertion is the acceptance test. Navigation is injected through React context so the package never imports `next/*`. Tests run in one jsdom Vitest project with a resolver that bans `next` outright.

**Tech Stack:** Tailwind v4 (CSS-first `@theme`), `radix-ui` 1.6.7 (unified), `cmdk`, Vitest 5 + jsdom + React Testing Library, `axe-core`, Moon, pnpm workspaces.

**Spec:** `docs/superpowers/specs/2026-09-08-sma-503-paigasus-ui-foundation-design.md` (revision 2)

## Global Constraints

- **Worktree.** All work happens in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-503` on branch `feature/sma-503-ts-paigasus-ui-tailwind-radix`. A subagent dispatched from this session is pinned to the MAIN checkout — its first action must be `EnterWorktree` with that path, then `git branch --show-current` to confirm.
- **PATH.** Every shell command needs `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first, shims before bin.
- **SPDX header on every source file.** `// SPDX-License-Identifier: Apache-2.0` for `.ts`/`.tsx`/`.mjs`; `/* SPDX-License-Identifier: Apache-2.0 */` for `.css`. `components.json` gets none — JSON admits no comment.
- **`'use client'`** is the first statement in every component file that uses Radix, React state or context. The SPDX comment sits above it. `src/index.ts` never carries it.
- **Commits** are conventional with a workspace scope, subject lowercase and ≤100 chars: `feat(ts): …`. Keep one contiguous footer and never write a bare `#NNN` in the body — write "SMA-503". End every commit message with:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  ```
- **Prettier** is a separate whole-tree CI gate. Run `moon run ts:fmt` after touching any `.ts`, `.tsx`, `.css`, `.mjs` or `.json` file. `printWidth` is 200, single quotes, trailing commas.
- **Versions are exact.** `radix-ui` ^1.6.7, `cmdk` ^1.1.1, `class-variance-authority` ^0.7.1, `clsx` ^2.1.1, `tailwind-merge` ^3.6.0, `tw-animate-css` ^1.4.0, `tailwindcss` ^4.3.3, `@tailwindcss/postcss` ^4.3.3, `jsdom` ^30.0.1, `@testing-library/react` ^16.3.3, `@testing-library/dom` ^10.4.1, `@testing-library/user-event` ^14.6.7, `@testing-library/jest-dom` ^7.0.1, `axe-core` ^4.13.0.
- **Every new dependency goes in `ts/pnpm-workspace.yaml`'s `catalog:`** and is referenced as `"catalog:"` from the package manifest. This workspace catalogs single-consumer dependencies too.
- **Never compose across the two Radix copies.** `cmdk` carries its own `@radix-ui/react-dialog`. `Command.Dialog` is banned. `Dialog` comes from `radix-ui` only.
- **The AC-3 assertion script must never live under `ts/apps/paigasus-console/`.** Tailwind's scan root is that directory, so a script containing the sentinel literal would generate the utility it asserts on.
- **Do not touch `ts/eslint.config.js` or `ts/apps/paigasus-console/next.config.ts`.** Both belong to SMA-502. The one exception is `transpilePackages`, and only if Task 1's M5 proves it necessary — in which case it waits for the rebase.

---

## File Structure

**Created**

| Path | Responsibility |
|---|---|
| `ts/packages/paigasus-ui/src/styles/tokens.css` | the whole theming model: `@theme`, light/dark token redefinition, the `dark:` custom variant, sentinel B |
| `ts/packages/paigasus-ui/src/lib/cn.ts` | `clsx` + `tailwind-merge` class joiner |
| `ts/packages/paigasus-ui/src/nav/link.tsx` | `LinkComponent`, `LinkProvider`, `useLinkComponent`, `Link` |
| `ts/packages/paigasus-ui/src/components/*.tsx` | the seeded component set; `table.tsx` carries sentinel A |
| `ts/packages/paigasus-ui/src/index.ts` | the single public barrel |
| `ts/packages/paigasus-ui/vitest.config.ts` | one jsdom project plus the `next/*` resolver ban |
| `ts/packages/paigasus-ui/tests/setup.ts` | jsdom polyfills, jest-dom matchers |
| `ts/packages/paigasus-ui/tests/axe.ts` | `expectNoAxeViolations`, scoped to `document.body` |
| `ts/packages/paigasus-ui/tests/fixtures/imports-next.ts` | AC-1 negative control |
| `ts/packages/paigasus-ui/README.md` | the three things §15 requires a reader to know |
| `ts/packages/paigasus-ui/components.json` | shadcn monorepo configuration |
| `ts/apps/paigasus-console/app/globals.css` | `@import 'tailwindcss'`, the token layer, the `@source` line |
| `ts/apps/paigasus-console/postcss.config.mjs` | `@tailwindcss/postcss` |
| `ci/tailwind-source/run.mjs` | the AC-3 assertion, with `--self-test` and `--negative-control` |
| `ci/tailwind-source/README.md` | why the script lives outside the console |
| `docs/superpowers/specs/2026-09-08-sma-503-measurements.md` | M1–M7 results |

**Modified**

| Path | Change |
|---|---|
| `ts/pnpm-workspace.yaml` | new `catalog:` entries |
| `ts/packages/paigasus-ui/package.json` | deps, devDeps, peers, the `./styles.css` export |
| `ts/packages/paigasus-ui/tsconfig.json` | drop `rootDir`, widen `include` |
| `ts/packages/paigasus-ui/moon.yml` | append `vitest.config.ts` to `test` inputs |
| `ts/apps/paigasus-console/package.json` | `@paigasus/ui`, `tailwindcss`, `@tailwindcss/postcss` |
| `ts/apps/paigasus-console/app/layout.tsx` | `import './globals.css';`, wrap in `LinkProvider` |
| `ts/apps/paigasus-console/app/page.tsx` | render one seeded component |
| `ts/apps/paigasus-console/moon.yml` | `sources` file group, `dependsOn`, three task input sets, the replaced `test` |
| `ci/affected-graph/run.sh` | the `ui->console` strict-equality case |

---

## Task 1: Preconditions — minimal scaffold and measurements M1, M4, M5, M7

**This task gates every other task.** Its purpose is to settle three questions that would change the design rather than the code. Build the smallest thing that can answer them.

**Files:**
- Create: `ts/packages/paigasus-ui/src/styles/tokens.css`, `ts/packages/paigasus-ui/src/lib/cn.ts`, `ts/packages/paigasus-ui/src/components/table.tsx`
- Create: `ts/apps/paigasus-console/app/globals.css`, `ts/apps/paigasus-console/postcss.config.mjs`
- Create: `docs/superpowers/specs/2026-09-08-sma-503-measurements.md`
- Modify: `ts/pnpm-workspace.yaml`, `ts/packages/paigasus-ui/package.json`, `ts/packages/paigasus-ui/src/index.ts`, `ts/apps/paigasus-console/package.json`, `ts/apps/paigasus-console/app/layout.tsx`, `ts/apps/paigasus-console/app/page.tsx`

**Interfaces:**
- Consumes: nothing.
- Produces: `cn(...inputs: ClassValue[]): string` from `src/lib/cn.ts`. `Table`, `TableHeader`, `TableBody`, `TableRow`, `TableHead`, `TableCell` from `src/components/table.tsx`. The two sentinel strings `--paigasus-ui-source-probe` and `--paigasus-token-probe`.

- [ ] **Step 1: Add the catalog entries**

In `ts/pnpm-workspace.yaml`, inside the existing `catalog:` block, after the `vite-plugin-wasm` entry:

```yaml
  # @paigasus/ui foundation (SMA-503, ADR-0021). Tailwind v4 is CSS-first: there is no
  # tailwind.config.*, so `tailwindcss` is present for the `@theme`/`@source` syntax the
  # package's CSS uses and `@tailwindcss/postcss` is what the consuming Next app runs.
  tailwindcss: ^4.3.3
  '@tailwindcss/postcss': ^4.3.3
  # Radix as ONE package, not ten @radix-ui/react-* entries. Note cmdk pulls its own
  # individual @radix-ui/react-dialog, so two copies exist in the tree — @paigasus/ui never
  # composes across them (cmdk's Command.Dialog is banned; see src/components/command.tsx).
  radix-ui: ^1.6.7
  cmdk: ^1.1.1
  'class-variance-authority': ^0.7.1
  clsx: ^2.1.1
  'tailwind-merge': ^3.6.0
  # Supplies animate-in / fade-in-* / zoom-in-* / slide-in-from-*, which shadcn's generated
  # components reference. It does NOT supply data-[state=open]:, which is core Tailwind v4.
  'tw-animate-css': ^1.4.0
  # jsdom component-test tier (SMA-503 AC 2 + AC 4). axe-core is used directly rather than
  # through vitest-axe, which last published 2025-01-22 and pulls four transitive deps for a
  # fifteen-line matcher.
  jsdom: ^30.0.1
  '@testing-library/react': ^16.3.3
  '@testing-library/dom': ^10.4.1
  '@testing-library/user-event': ^14.6.7
  '@testing-library/jest-dom': ^7.0.1
  'axe-core': ^4.13.0
```

- [ ] **Step 2: Rewrite the `@paigasus/ui` manifest**

Replace `ts/packages/paigasus-ui/package.json` with:

```json
{
  "name": "@paigasus/ui",
  "_comment_exports": "Source-only exports. Bundler-aware consumers only (Next/Vitest/tsc walk through TS via moduleResolution: bundler). Switch to ./dist/index.js when tsup wiring lands — must happen IN LOCKSTEP with flipping `private: false` for publishable packages. ./styles.css is the design-token layer; a consumer imports it from its own stylesheet AND must add its own Tailwind `@source` line (SMA-503 README).",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": {
    "node": ">=24"
  },
  "license": "Apache-2.0",
  "exports": {
    ".": "./src/index.ts",
    "./styles.css": "./src/styles/tokens.css"
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "class-variance-authority": "catalog:",
    "clsx": "catalog:",
    "cmdk": "catalog:",
    "radix-ui": "catalog:",
    "tailwind-merge": "catalog:"
  },
  "peerDependencies": {
    "react": "catalog:",
    "react-dom": "catalog:",
    "tailwindcss": "catalog:"
  },
  "devDependencies": {
    "@testing-library/dom": "catalog:",
    "@testing-library/jest-dom": "catalog:",
    "@testing-library/react": "catalog:",
    "@testing-library/user-event": "catalog:",
    "@types/react": "catalog:",
    "@types/react-dom": "catalog:",
    "axe-core": "catalog:",
    "jsdom": "catalog:",
    "react": "catalog:",
    "react-dom": "catalog:",
    "tailwindcss": "catalog:",
    "tw-animate-css": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

`react` and `react-dom` appear as BOTH peers and devDependencies. The peer declares the consumer contract; the devDependency is what makes the package's own Vitest run resolve them. Whether pnpm auto-installs a `catalog:` peer is not proven here, so this does not rely on it.

- [ ] **Step 3: Write the token layer**

Create `ts/packages/paigasus-ui/src/styles/tokens.css`:

```css
/* SPDX-License-Identifier: Apache-2.0 */

/*
 * The whole theming model for @paigasus/ui (ADR-0021 decision 2).
 *
 * Light is the bare :root. Dark redefines ONLY the same custom properties, in two places:
 * the system preference, and an explicit [data-theme] choice. No component names a raw
 * colour; every component names a token. That is what makes the product brandable by
 * configuration rather than by fighting a vendor theme.
 *
 * A consumer imports this file from its own stylesheet AND must add its own Tailwind
 * `@source` line covering this package's src/ — see README.md. Importing this file alone
 * does not make Tailwind scan the components.
 */

@import 'tw-animate-css';

/*
 * Tailwind v4's built-in `dark:` variant keys on prefers-color-scheme ALONE, so an explicit
 * [data-theme="light"] could not override a `dark:` utility — and shadcn's copy-in components
 * ship `dark:` classes. Redefining the variant routes those classes through the same mechanism
 * as the tokens below, so the two never disagree.
 */
@custom-variant dark (&:where([data-theme='dark'], [data-theme='dark'] *));

@theme {
  --color-background: var(--pgs-background);
  --color-foreground: var(--pgs-foreground);
  --color-muted: var(--pgs-muted);
  --color-muted-foreground: var(--pgs-muted-foreground);
  --color-border: var(--pgs-border);
  --color-input: var(--pgs-input);
  --color-ring: var(--pgs-ring);
  --color-primary: var(--pgs-primary);
  --color-primary-foreground: var(--pgs-primary-foreground);
  --color-destructive: var(--pgs-destructive);
  --color-destructive-foreground: var(--pgs-destructive-foreground);
  --color-popover: var(--pgs-popover);
  --color-popover-foreground: var(--pgs-popover-foreground);
  --radius-pgs: var(--pgs-radius);
}

:root {
  /*
   * Sentinel B (SMA-503 spec §10.2). Sentinel A proves Tailwind SCANNED the package's source.
   * This one proves the `@import '@paigasus/ui/styles.css'` in the consuming app actually
   * resolved — without it, that import could fail, the whole token layer could be absent from
   * the built CSS, and sentinel A would still pass. ci/tailwind-source/run.mjs asserts both.
   * Do not remove, and do not reference it from anywhere else.
   */
  --paigasus-token-probe: 1;

  --pgs-radius: 0.5rem;

  --pgs-background: oklch(1 0 0);
  --pgs-foreground: oklch(0.21 0.006 285.9);
  --pgs-muted: oklch(0.967 0.001 286.4);
  --pgs-muted-foreground: oklch(0.552 0.016 285.9);
  --pgs-border: oklch(0.92 0.004 286.3);
  --pgs-input: oklch(0.92 0.004 286.3);
  --pgs-ring: oklch(0.705 0.015 286.1);
  --pgs-primary: oklch(0.21 0.006 285.9);
  --pgs-primary-foreground: oklch(0.985 0 0);
  --pgs-destructive: oklch(0.577 0.245 27.3);
  --pgs-destructive-foreground: oklch(0.985 0 0);
  --pgs-popover: oklch(1 0 0);
  --pgs-popover-foreground: oklch(0.21 0.006 285.9);
}

/* The viewer's system setting, unless an explicit light choice overrides it. */
@media (prefers-color-scheme: dark) {
  :root:not([data-theme='light']) {
    --pgs-background: oklch(0.141 0.005 285.8);
    --pgs-foreground: oklch(0.985 0 0);
    --pgs-muted: oklch(0.274 0.006 286.0);
    --pgs-muted-foreground: oklch(0.705 0.015 286.1);
    --pgs-border: oklch(1 0 0 / 10%);
    --pgs-input: oklch(1 0 0 / 15%);
    --pgs-ring: oklch(0.552 0.016 285.9);
    --pgs-primary: oklch(0.985 0 0);
    --pgs-primary-foreground: oklch(0.21 0.006 285.9);
    --pgs-destructive: oklch(0.704 0.191 22.2);
    --pgs-destructive-foreground: oklch(0.985 0 0);
    --pgs-popover: oklch(0.21 0.006 285.9);
    --pgs-popover-foreground: oklch(0.985 0 0);
  }
}

/* An explicit dark choice, so the toggle wins in both directions. */
:root[data-theme='dark'] {
  --pgs-background: oklch(0.141 0.005 285.8);
  --pgs-foreground: oklch(0.985 0 0);
  --pgs-muted: oklch(0.274 0.006 286.0);
  --pgs-muted-foreground: oklch(0.705 0.015 286.1);
  --pgs-border: oklch(1 0 0 / 10%);
  --pgs-input: oklch(1 0 0 / 15%);
  --pgs-ring: oklch(0.552 0.016 285.9);
  --pgs-primary: oklch(0.985 0 0);
  --pgs-primary-foreground: oklch(0.21 0.006 285.9);
  --pgs-destructive: oklch(0.704 0.191 22.2);
  --pgs-destructive-foreground: oklch(0.985 0 0);
  --pgs-popover: oklch(0.21 0.006 285.9);
  --pgs-popover-foreground: oklch(0.985 0 0);
}
```

- [ ] **Step 4: Write `cn`**

Create `ts/packages/paigasus-ui/src/lib/cn.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

/**
 * Join class names, letting a later Tailwind utility win over an earlier conflicting one.
 *
 * This uses plain `tailwind-merge`, which knows Tailwind's own scale names. The token layer
 * in src/styles/tokens.css maps onto those names deliberately, so no configuration is needed.
 * If a genuinely NEW scale is ever added, `extendTailwindMerge` becomes necessary — that is
 * the trigger to revisit this file (SMA-503 spec §12).
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
```

- [ ] **Step 5: Write the Table, carrying sentinel A**

Create `ts/packages/paigasus-ui/src/components/table.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ComponentPropsWithoutRef, ReactElement } from 'react';
import { cn } from '../lib/cn';

/*
 * Sentinel A (SMA-503 spec §10.2). `[--paigasus-ui-source-probe:1]` is an arbitrary
 * custom-property utility that exists ONLY here, and it compiles to a single declaration in
 * the built CSS. ci/tailwind-source/run.mjs asserts it reaches a production console build,
 * which is what proves the app's Tailwind `@source` line still covers this package.
 *
 * It is a dedicated probe rather than a real style so that the assertion cannot pass for the
 * wrong reason. Do not remove it, do not rename it, and do not write the literal anywhere
 * under ts/apps/paigasus-console/ — that directory is Tailwind's scan root, so a second copy
 * there would generate the utility independently and silently disarm the gate.
 */
const SOURCE_PROBE = '[--paigasus-ui-source-probe:1]';

export function Table({ className, ...props }: ComponentPropsWithoutRef<'table'>): ReactElement {
  return (
    <div className="relative w-full overflow-x-auto">
      <table className={cn(SOURCE_PROBE, 'w-full caption-bottom text-sm', className)} {...props} />
    </div>
  );
}

export function TableHeader({ className, ...props }: ComponentPropsWithoutRef<'thead'>): ReactElement {
  return <thead className={cn('[&_tr]:border-b', className)} {...props} />;
}

export function TableBody({ className, ...props }: ComponentPropsWithoutRef<'tbody'>): ReactElement {
  return <tbody className={cn('[&_tr:last-child]:border-0', className)} {...props} />;
}

export function TableRow({ className, ...props }: ComponentPropsWithoutRef<'tr'>): ReactElement {
  return <tr className={cn('border-border hover:bg-muted/50 border-b transition-colors', className)} {...props} />;
}

export function TableHead({ className, ...props }: ComponentPropsWithoutRef<'th'>): ReactElement {
  return <th className={cn('text-muted-foreground h-10 px-2 text-left align-middle font-medium', className)} {...props} />;
}

export function TableCell({ className, ...props }: ComponentPropsWithoutRef<'td'>): ReactElement {
  return <td className={cn('p-2 align-middle', className)} {...props} />;
}
```

**Note on the probe.** `cn` runs `twMerge`, which passes an unrecognised arbitrary utility through unchanged — but Tailwind's scanner reads the SOURCE TEXT, not the runtime result, so the literal in `SOURCE_PROBE` is what generates the CSS. Both facts must hold; step 12 measures the built output rather than assuming either.

- [ ] **Step 6: Export from the barrel**

Replace `ts/packages/paigasus-ui/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

/*
 * The single public surface of @paigasus/ui.
 *
 * This file must NEVER carry a 'use client' directive: it would force every consumer of any
 * export into the client bundle. Each component file carries its own.
 */
export { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from './components/table';
export { cn } from './lib/cn';
```

- [ ] **Step 7: Wire the console**

Create `ts/apps/paigasus-console/postcss.config.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
/** @type {import('postcss-load-config').Config} */
export default {
  plugins: {
    '@tailwindcss/postcss': {},
  },
};
```

Create `ts/apps/paigasus-console/app/globals.css`:

```css
/* SPDX-License-Identifier: Apache-2.0 */
@import 'tailwindcss';

/*
 * The design-token layer from @paigasus/ui. Proven to reach the built CSS by sentinel B
 * (--paigasus-token-probe) in ci/tailwind-source/run.mjs.
 */
@import '@paigasus/ui/styles.css';

/*
 * LOAD-BEARING (ADR-0021 decision 4, SMA-503 AC 3). Tailwind's automatic source detection
 * defaults to the current working directory — this app's own directory — and excludes
 * node_modules, through which pnpm resolves @paigasus/ui. Without this line the package's
 * classes are silently absent from a PRODUCTION build, which looks like broken styling
 * rather than a build error.
 *
 * Every new zone app needs its own copy of this line and its own probe assertion. Deleting
 * it reds paigasus-console-ts:test.
 */
@source '../../../packages/paigasus-ui/src';

body {
  background-color: var(--color-background);
  color: var(--color-foreground);
}
```

Modify `ts/apps/paigasus-console/app/layout.tsx` to import the stylesheet — without this Next emits no CSS at all:

```tsx
// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from 'react';
import './globals.css';

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
```

Modify `ts/apps/paigasus-console/app/page.tsx` to render the component, so the integration is real rather than a CSS scan alone:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';

export default function Page() {
  return (
    <main className="p-8">
      <h1 className="mb-4 text-2xl font-semibold">Paigasus console</h1>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Service</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow>
            <TableCell>iam</TableCell>
            <TableCell>unknown</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </main>
  );
}
```

Add to `ts/apps/paigasus-console/package.json`'s `dependencies`: `"@paigasus/ui": "workspace:*"`, `"tailwindcss": "catalog:"`, `"@tailwindcss/postcss": "catalog:"`.

- [ ] **Step 8: Install**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
```
Expected: exit 0, `ts/pnpm-lock.yaml` updated.

- [ ] **Step 9: Take M1 and M4 — run the production build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build; echo "rc=$?"
```

Record: the exit code, whether the build reports Turbopack or webpack, and the full path of every CSS file it wrote. Find them with:

```bash
find ts/apps/paigasus-console/.next -name '*.css' -not -path '*/cache/*' | sort
```

**M1 verdict:** if the build fails to resolve `@paigasus/ui/styles.css`, change the `@import` in `globals.css` to the relative path `../../../packages/paigasus-ui/src/styles/tokens.css`, rebuild, and record the change in both the measurements file and spec §5.2.

**M4 verdict — this one can stop the task.** Confirm the CSS output path, then run:

```bash
CSS=$(find ts/apps/paigasus-console/.next/static -name '*.css' -not -path '*/cache/*')
grep -c -- '--paigasus-ui-source-probe' $CSS
grep -c -- '--paigasus-token-probe' $CSS
```

Do **not** hardcode `.next/static/css/`. Next 16.3.4 builds with Turbopack and writes CSS under
`.next/static/chunks/`; a hardcoded directory expands to nothing and the grep dies at glob
expansion rather than reporting a count. Use the `find` above, which is layout-independent.

Both must be ≥ 1. Then **delete the `@source` line from `globals.css`**, rebuild, and re-grep. `--paigasus-ui-source-probe` must now be **absent**.

**If it is still present, STOP and report.** That means Tailwind's automatic scan already reaches `ts/packages/paigasus-ui/src`, the `@source` line is decorative, and spec §10 needs redesigning rather than patching. Do not continue to Task 2.

Restore the line by reverting the edit (`git checkout -- ts/apps/paigasus-console/app/globals.css` is safe here because the file is committed only after this step — if it is not yet committed, re-type the line). **Never restore by moving a `.bak` file back**: a rolled-back modification time makes the next build reuse the earlier artifact.

- [ ] **Step 10: Take M5 — `transpilePackages`**

Step 9's build already answers this. If it succeeded with the unmodified `next.config.ts`, then Next compiled the package's TypeScript through the pnpm symlink and `transpilePackages` is **not** needed — record that.

If it failed with a parse or loader error naming a file under `ts/packages/paigasus-ui/`, then it IS needed. Record that, add `transpilePackages: ['@paigasus/ui']` temporarily to `next.config.ts` to unblock the remaining tasks, and **write a prominent note in the measurements file** that this line must move into SMA-502's `next.config` factory during the rebase and must not ship as a direct edit.

- [ ] **Step 11: Take M7 — SPDX above `'use client'`**

Step 9's build compiled `table.tsx`, which carries the SPDX line comment above `'use client'`. If the build succeeded, SWC accepted it — record that. If the build warned that the directive was ignored, move the SPDX comment below the directive in every component file and record the change.

Search the build output for the phrase `use client` to be sure a warning was not missed.

- [ ] **Step 12: Write the measurements file**

Create `docs/superpowers/specs/2026-09-08-sma-503-measurements.md` with one section per measurement: the exact command, the verbatim output, and the verdict. Follow `docs/superpowers/specs/2026-09-01-sma-606-measurements.md` for shape. M2, M3 and M6 stay marked "not yet taken" — later tasks fill them in.

- [ ] **Step 13: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
```
If it reports differences, run `pnpm -C ts exec prettier --write .` and re-run.

```bash
git add ts/pnpm-workspace.yaml ts/pnpm-lock.yaml ts/packages/paigasus-ui ts/apps/paigasus-console docs/superpowers/specs/2026-09-08-sma-503-measurements.md
git commit -m "feat(ts): scaffold @paigasus/ui tokens and the console Tailwind wiring (SMA-503)

Adds the design-token layer, cn, a Table carrying the AC-3 source sentinel,
and the console's globals.css with the load-bearing @source line.

Records measurements M1, M4, M5 and M7. M4 confirms the @source line is
load-bearing: deleting it removes the probe from the production CSS.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 2: The jsdom test harness and the `next/*` resolver ban

**Files:**
- Create: `ts/packages/paigasus-ui/vitest.config.ts`, `ts/packages/paigasus-ui/tests/setup.ts`, `ts/packages/paigasus-ui/tests/fixtures/imports-next.ts`, `ts/packages/paigasus-ui/tests/no-next-imports.test.ts`, `ts/packages/paigasus-ui/tests/table.test.tsx`
- Modify: `ts/packages/paigasus-ui/tsconfig.json`, `ts/packages/paigasus-ui/moon.yml`

**Interfaces:**
- Consumes: `Table` and friends from Task 1.
- Produces: a working `pnpm exec vitest run` in the package, with `globals: true` and jsdom. Later tasks add `*.test.tsx` files and need nothing else.

- [ ] **Step 1: Fix the package tsconfig, or `ts:lint` reds**

Replace `ts/packages/paigasus-ui/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "jsx": "react-jsx",
    // ts/tsconfig.base.json is ES2022-only and carries no DOM lib, so without this tsc reds
    // with TS2812 on every DOM property a component touches. Same value as
    // ts/apps/paigasus-console/tsconfig.json.
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "noEmit": true
  },
  // tests/ and vitest.config.ts must be in the program: ts/eslint.config.js applies
  // type-checked rules with projectService:true to every **/*.{ts,tsx}, and `ts:lint` runs
  // `eslint .` over the whole tree — a file in no program errors there. No rootDir: it
  // rejects files outside src/, and it is inert under noEmit. Same shape and same reason as
  // ts/packages/paigasus-kernel/tsconfig.json.
  "include": ["src/**/*", "tests/**/*", "vitest.config.ts"]
}
```

- [ ] **Step 2: Write the Vitest config with the resolver ban**

Create `ts/packages/paigasus-ui/vitest.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { defineConfig, type Plugin } from 'vitest/config';

/**
 * AC 1 (SMA-503, ADR-0021 decision 3): @paigasus/ui imports nothing from `next/*`.
 *
 * pnpm's isolated node_modules already makes `next` unresolvable from this package, because
 * next is a dependency of @paigasus/console and not of this one. That protection is real but
 * INCIDENTAL — adding next to this package's devDependencies for any reason would disarm it
 * silently, with no red anywhere. This plugin does not depend on where pnpm places a package.
 *
 * It cannot see `import type { Metadata } from 'next'`, which verbatimModuleSyntax erases
 * before Vite resolves anything. SMA-502's eslint boundary rule is what covers that.
 *
 * tests/no-next-imports.test.ts is the negative control. Without it this plugin is untested
 * code that could stop matching and never red.
 */
function banNextImports(): Plugin {
  return {
    name: 'paigasus-ban-next-imports',
    enforce: 'pre',
    resolveId(source: string) {
      if (source === 'next' || source.startsWith('next/')) {
        throw new Error(`@paigasus/ui must not import from \`next\`: got "${source}" (ADR-0021 decision 3)`);
      }
      return null;
    },
  };
}

export default defineConfig({
  plugins: [banNextImports()],
  test: {
    name: 'ui',
    environment: 'jsdom',
    setupFiles: ['./tests/setup.ts'],
    include: ['tests/**/*.test.{ts,tsx}'],
    // React Testing Library registers its automatic cleanup only when a global afterEach
    // exists, and vitest defaults globals to false. Without this the DOM accumulates between
    // tests and axe then reports duplicate-id and duplicate-landmark violations belonging to
    // an EARLIER test — a flake that reads as a real accessibility failure.
    globals: true,
  },
});
```

- [ ] **Step 3: Write the test setup**

Create `ts/packages/paigasus-ui/tests/setup.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import '@testing-library/jest-dom/vitest';

/*
 * Browser APIs jsdom does not implement but Radix needs. This list is MEASURED (SMA-503 M2):
 * add only what a failing test actually demands, and delete anything no test needs. A copied
 * list is a list nobody can justify later.
 */
if (!('ResizeObserver' in globalThis)) {
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
}

if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = (): boolean => false;
  Element.prototype.setPointerCapture = (): void => {};
  Element.prototype.releasePointerCapture = (): void => {};
}

if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = (): void => {};
}
```

- [ ] **Step 4: Write the AC-1 negative-control fixture**

Create `ts/packages/paigasus-ui/tests/fixtures/imports-next.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

/*
 * The AC-1 negative control. This module deliberately violates the next/* ban so that
 * tests/no-next-imports.test.ts can prove the resolver plugin in vitest.config.ts actually
 * fires. It is never imported by src/.
 *
 * The @ts-expect-error is load-bearing twice over. It suppresses the "cannot find module"
 * error tsc raises today — and if `next` ever BECOMES resolvable from this package, the
 * directive turns unused and tsc reds on that instead. So this line also guards pnpm's
 * isolation, which is the incidental protection described in vitest.config.ts.
 */
// @ts-expect-error `next` is deliberately absent from @paigasus/ui's dependencies.
import NextLink from 'next/link';

export default NextLink;
```

- [ ] **Step 5: Write the failing tests**

Create `ts/packages/paigasus-ui/tests/no-next-imports.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';

describe('the next/* resolver ban', () => {
  it('rejects a module that imports next/link', async () => {
    await expect(import('./fixtures/imports-next')).rejects.toThrow(/must not import from `next`/);
  });

  it('does not reject an ordinary relative import', async () => {
    await expect(import('../src/lib/cn')).resolves.toBeDefined();
  });
});
```

Create `ts/packages/paigasus-ui/tests/table.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '../src/components/table';

describe('Table', () => {
  it('renders in jsdom with no Next runtime', () => {
    render(
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Service</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow>
            <TableCell>iam</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'Service' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: 'iam' })).toBeInTheDocument();
  });

  it('carries the AC-3 source sentinel on the table element', () => {
    render(<Table />);
    expect(screen.getByRole('table').className).toContain('[--paigasus-ui-source-probe:1]');
  });
});
```

- [ ] **Step 6: Run the tests and take M2**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run
```
Expected: all four tests pass.

If a test fails on a missing browser API, add only that API to `tests/setup.ts` and re-run. Record in the measurements file which polyfills were actually needed, and **delete any that no test demanded** — an unjustified polyfill is a claim nobody can check later. This is M2; also record the resolved Vitest and jsdom versions from `pnpm -C ts list vitest jsdom`.

- [ ] **Step 7: Append `vitest.config.ts` to the package's test inputs**

Modify `ts/packages/paigasus-ui/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-ui-ts'
layer: 'library'
language: 'typescript'

tasks:
  test:
    # APPEND, not `options.merge: replace`. The inherited definition in
    # .moon/tasks/typescript-project.yml supplies @group(sources), @group(tests), package.json
    # and /ts/pnpm-lock.yaml; replacing would silently drop all four. That is exactly the
    # defect this branch fixes on paigasus-console-ts:build (SMA-503 spec §11.1).
    #
    # vitest.config.ts matches neither src/**/* nor tests/**/*, so without this line an edit
    # to the resolver ban or the jsdom environment serves a cached PASS.
    inputs:
      - 'vitest.config.ts'
```

Verify the merge did not replace:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --id paigasus-ui-ts | grep -A20 '"test"' | grep -E 'pnpm-lock|vitest.config'
```
Expected: both appear.

- [ ] **Step 8: Run the package's Moon tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-ui-ts:typecheck paigasus-ui-ts:test
```
Expected: both pass.

- [ ] **Step 9: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
git add ts/packages/paigasus-ui
git commit -m "test(ts): add the @paigasus/ui jsdom harness and the next/* resolver ban (SMA-503)

One jsdom vitest project with globals enabled, so React Testing Library
registers its automatic cleanup. A Vite resolveId plugin bans next and
next/*, with a fixture-based negative control that proves it fires.

The tsconfig drops rootDir and widens include, following the kernel
package, so tests/ and vitest.config.ts sit in a program that the typed
eslint rules can walk.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 3: The axe helper and its negative control

**Files:**
- Create: `ts/packages/paigasus-ui/tests/axe.ts`, `ts/packages/paigasus-ui/tests/axe.test.tsx`
- Modify: `ts/packages/paigasus-ui/tests/table.test.tsx`

**Interfaces:**
- Consumes: the jsdom harness from Task 2.
- Produces: `expectNoAxeViolations(target?: Element): Promise<void>` — every later component test calls it with no argument.

- [ ] **Step 1: Write the failing negative-control test**

Create `ts/packages/paigasus-ui/tests/axe.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';

describe('expectNoAxeViolations', () => {
  /*
   * The helper's own negative control. Without this, a helper that silently stopped running —
   * a wrong tag set, a swallowed promise, an empty target — would look identical to a clean
   * pass on every component test in the package.
   */
  it('rejects a control with no accessible name', async () => {
    render(<input type="text" />);
    await expect(expectNoAxeViolations()).rejects.toThrow(/axe found 1 violation/);
  });

  it('resolves for a correctly labelled control', async () => {
    render(
      <>
        <label htmlFor="probe">Probe</label>
        <input id="probe" type="text" />
      </>,
    );
    await expect(expectNoAxeViolations()).resolves.toBeUndefined();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/axe.test.tsx
```
Expected: FAIL — `Failed to resolve import "./axe"`.

- [ ] **Step 3: Write the helper**

Create `ts/packages/paigasus-ui/tests/axe.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import axe, { type Result } from 'axe-core';

/*
 * AC 4 (SMA-503). axe-core is used directly rather than through vitest-axe, which last
 * published on 2025-01-22 and pulls chalk, redent, lodash-es and dom-accessibility-api for a
 * matcher that is about fifteen lines of code. This repository is public and runs an OSV
 * gate, so fewer transitive dependencies is the better trade.
 *
 * STATED LIMIT: axe cannot evaluate color-contrast in jsdom, because jsdom performs no
 * layout. These assertions cover roles, accessible names, ARIA state and labelling. They do
 * NOT cover contrast — that stays a design responsibility, decided in the token layer. The
 * package README repeats this so a green here is not over-read.
 */
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'];

const DISABLED_RULES = {
  // Fires on every isolated component render, because a fragment has no landmark wrapper.
  // It has no meaning outside a full page, which this tier never renders.
  region: { enabled: false },
};

function describeViolations(violations: Result[]): string {
  const body = violations
    .map((v) => {
      const nodes = v.nodes.map((n) => `      ${n.html}`).join('\n');
      return `  [${v.impact ?? 'unknown'}] ${v.id}: ${v.help}\n    ${v.helpUrl}\n${nodes}`;
    })
    .join('\n');
  return `axe found ${String(violations.length)} violation${violations.length === 1 ? '' : 's'}:\n${body}`;
}

/**
 * Assert the rendered tree has no axe violations.
 *
 * The default target is `document.body`, NOT the container React Testing Library returns.
 * Radix Dialog, DropdownMenu and Popover render their content into a portal on document.body,
 * so a container-scoped assertion would check a subtree holding only the trigger — and the
 * three most accessibility-critical components would go green for the wrong reason.
 */
export async function expectNoAxeViolations(target: Element = document.body): Promise<void> {
  const results = await axe.run(target, {
    runOnly: { type: 'tag', values: TAGS },
    rules: DISABLED_RULES,
  });
  if (results.violations.length > 0) {
    throw new Error(describeViolations(results.violations));
  }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/axe.test.tsx
```
Expected: both PASS. If the first test's message does not match `/axe found 1 violation/`, read the actual message and fix the assertion — do not loosen it to a bare `rejects.toThrow()`.

- [ ] **Step 5: Add an axe assertion to the Table test**

Append to `ts/packages/paigasus-ui/tests/table.test.tsx`, inside the `describe`:

```tsx
  it('has no axe violations', async () => {
    render(
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Service</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow>
            <TableCell>iam</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    await expectNoAxeViolations();
  });
```

Add `import { expectNoAxeViolations } from './axe';` to that file's imports.

- [ ] **Step 6: Run the full package suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run
```
Expected: all pass.

- [ ] **Step 7: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
git add ts/packages/paigasus-ui
git commit -m "test(ts): add the axe helper with its own negative control (SMA-503)

Runs axe-core directly against document.body, because Radix portals its
overlay content out of the render container. Carries a deliberately broken
fixture so a helper that stopped running cannot look like a clean pass.

Records the jsdom contrast limit at the call site.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 4: Navigation injection

**Files:**
- Create: `ts/packages/paigasus-ui/src/nav/link.tsx`, `ts/packages/paigasus-ui/tests/link.test.tsx`
- Modify: `ts/packages/paigasus-ui/src/index.ts`, `ts/apps/paigasus-console/app/layout.tsx`

**Interfaces:**
- Consumes: `cn` from Task 1.
- Produces:
  - `type LinkComponent = ComponentType<{ href: string; className?: string; children?: ReactNode }>`
  - `function LinkProvider(props: { link: LinkComponent; children: ReactNode }): ReactElement`
  - `function useLinkComponent(): LinkComponent`
  - `const Link: FC<{ href: string; className?: string; children?: ReactNode }>`

  Task 9 onward may use `Link` inside components. SMA-510 consumes `LinkProvider`.

- [ ] **Step 1: Write the failing tests**

Create `ts/packages/paigasus-ui/tests/link.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Link, LinkProvider } from '../src/nav/link';

function StubLink({ href, className, children }: { href: string; className?: string; children?: ReactNode }): ReactElement {
  return (
    <a data-testid="stub" href={href} className={className}>
      {children}
    </a>
  );
}

describe('Link', () => {
  it('renders a plain anchor with no provider, so the package needs no framework', () => {
    render(<Link href="/iam">IAM</Link>);
    const anchor = screen.getByRole('link', { name: 'IAM' });
    expect(anchor).toHaveAttribute('href', '/iam');
    expect(anchor).not.toHaveAttribute('data-testid');
  });

  it('uses the injected component when a provider supplies one', () => {
    render(
      <LinkProvider link={StubLink}>
        <Link href="/gateway">Gateway</Link>
      </LinkProvider>,
    );
    expect(screen.getByTestId('stub')).toHaveAttribute('href', '/gateway');
  });

  it('passes className through to the injected component', () => {
    render(
      <LinkProvider link={StubLink}>
        <Link href="/gateway" className="text-primary">
          Gateway
        </Link>
      </LinkProvider>,
    );
    expect(screen.getByTestId('stub')).toHaveClass('text-primary');
  });

  it('has no axe violations', async () => {
    render(<Link href="/iam">IAM</Link>);
    await expectNoAxeViolations();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/link.test.tsx
```
Expected: FAIL — cannot resolve `../src/nav/link`.

- [ ] **Step 3: Implement**

Create `ts/packages/paigasus-ui/src/nav/link.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import { createContext, useContext, type ComponentType, type ReactElement, type ReactNode } from 'react';

/**
 * The navigation contract (ADR-0021 decision 3, SMA-503 AC 1).
 *
 * @paigasus/ui imports nothing from next/*. The package defines this contract; the
 * application supplies the implementation. SMA-510's app shell consumes the same contract.
 */
export interface LinkProps {
  href: string;
  className?: string;
  children?: ReactNode;
}

export type LinkComponent = ComponentType<LinkProps>;

function AnchorLink({ href, className, children }: LinkProps): ReactElement {
  return (
    <a href={href} className={className}>
      {children}
    </a>
  );
}

/*
 * The context default is a plain <a>. That default is what makes AC 2 work: a component tree
 * renders in jsdom with no provider and no Next runtime present.
 */
const LinkContext = createContext<LinkComponent>(AnchorLink);

export function LinkProvider({ link, children }: { link: LinkComponent; children: ReactNode }): ReactElement {
  return <LinkContext.Provider value={link}>{children}</LinkContext.Provider>;
}

export function useLinkComponent(): LinkComponent {
  return useContext(LinkContext);
}

export function Link(props: LinkProps): ReactElement {
  const Resolved = useLinkComponent();
  return <Resolved {...props} />;
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/link.test.tsx
```
Expected: all four PASS.

- [ ] **Step 5: Export from the barrel**

Add to `ts/packages/paigasus-ui/src/index.ts`:

```ts
export { Link, LinkProvider, useLinkComponent, type LinkComponent, type LinkProps } from './nav/link';
```

- [ ] **Step 6: Wire the console's provider**

Replace `ts/apps/paigasus-console/app/layout.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import NextLink from 'next/link';
import type { ReactNode } from 'react';
import { LinkProvider } from '@paigasus/ui';
import './globals.css';

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>
        {/* The injection point for ADR-0021 decision 3: @paigasus/ui never imports next/*. */}
        <LinkProvider link={NextLink}>{children}</LinkProvider>
      </body>
    </html>
  );
}
```

`LinkProvider` uses React context, so this makes `layout.tsx` a client boundary. If `next build` in step 7 objects, extract the provider into a small `app/providers.tsx` carrying `'use client'` and render it from the server layout.

- [ ] **Step 7: Rebuild the console**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build; echo "rc=$?"
```
Expected: rc=0.

- [ ] **Step 8: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
git add ts/packages/paigasus-ui ts/apps/paigasus-console
git commit -m "feat(ts): inject navigation into @paigasus/ui through context (SMA-503)

Components take a link component from context, defaulting to a plain
anchor. That default is what lets the package render in bare jsdom with
no Next runtime, which is AC 2.

The console injects next/link at the root layout.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 5: The AC-3 assertion script

**Files:**
- Create: `ci/tailwind-source/run.mjs`, `ci/tailwind-source/README.md`

**Interfaces:**
- Consumes: sentinel A from Task 1's `table.tsx`, sentinel B from Task 1's `tokens.css`, and a built `.next` from Task 4's build.
- Produces: a CLI with three modes — no flag (real run), `--self-test`, `--negative-control`. Task 6 wires it into Moon.

- [ ] **Step 1: Write the README first, because the placement rule is the point**

Create `ci/tailwind-source/README.md`:

```markdown
# `repo` gate: Tailwind `@source` reachability (SMA-503 AC 3)

Asserts that a production `next build` of `@paigasus/console` still emits the CSS that
`@paigasus/ui` contributes. Two independent sentinels:

| Sentinel | Declared in | Proves |
|---|---|---|
| `--paigasus-ui-source-probe` | `ts/packages/paigasus-ui/src/components/table.tsx` | Tailwind SCANNED the package's source, i.e. the app's `@source` line still covers it |
| `--paigasus-token-probe` | `ts/packages/paigasus-ui/src/styles/tokens.css` | the app's `@import '@paigasus/ui/styles.css'` RESOLVED, i.e. the token layer reached the output |

Sentinel A alone is not enough: the `@import` can fail, the whole token layer can be absent,
and sentinel A still passes.

## Why this script lives at the repository root

**It must never move under `ts/apps/paigasus-console/`.** Tailwind's automatic scan root is
the current working directory, and Moon runs `next build` from the console's own directory.
Tailwind extracts class candidates from any non-ignored text file. A script placed there and
containing the literal `[--paigasus-ui-source-probe:1]` would make Tailwind generate that
utility **from the script itself** — so the assertion would pass with the `@source` line
deleted. Excluding the script from its own "appears nowhere in the app" check reopens the
same hole from the other side.

## Invocation

Run by `paigasus-console-ts:test`, which depends on `~:build`. Three modes, in order:

- `--self-test` — drives the verdict function over synthetic fixtures in a temporary
  directory, proving the assertions can both pass and fail.
- `--negative-control` — asserts the script reports red against CSS lacking the probes.
- no flag — the real run, against `ts/apps/paigasus-console/.next`.

## Limitations

- **Nothing pins these three invocation lines.** `ci/affected-graph/ci_targets.py`'s
  `check_self_scheduled_coverage` scans `repo:*` tasks only, and this runs under
  `paigasus-console-ts:test`. Deleting the `--negative-control` line reds nothing. The
  alternative is a new `repo:*` gate running a full `next build` on every affected pull
  request, which was judged too expensive.
- **Coverage is per-consumer.** A green here says nothing about a second zone app. Every new
  app needs its own `@source` line and its own assertion.
- The script proves the CSS was EMITTED. It does not prove the page references it.
```

- [ ] **Step 2: Write the script**

Create `ci/tailwind-source/run.mjs`. Note the sentinel names are assembled from parts so the
literal `[--paigasus-ui-source-probe:1]` never appears here either — belt and braces, since a
future refactor could move this file:

```js
// SPDX-License-Identifier: Apache-2.0

/*
 * SMA-503 AC 3 — assert a production console build still emits @paigasus/ui's CSS.
 *
 * THIS FILE MUST STAY OUTSIDE ts/apps/paigasus-console/. See README.md for why: that
 * directory is Tailwind's scan root, and a copy of the sentinel there would generate the very
 * utility this script asserts on.
 */

import { existsSync, readFileSync, readdirSync, mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const CONSOLE_DIR = join(REPO_ROOT, 'ts', 'apps', 'paigasus-console');

const PROBE_SOURCE = '--paigasus' + '-ui-source-probe';
const PROBE_TOKEN = '--paigasus' + '-token-probe';

/** Files that count as "the console's own sources" for assertion 3. */
const APP_SCAN_ROOTS = ['app'];
const APP_SCAN_FILES = ['next.config.ts', 'postcss.config.mjs', 'tsconfig.json', 'package.json'];
const APP_SCAN_EXCLUDE = new Set(['.next', 'node_modules']);

function walk(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (APP_SCAN_EXCLUDE.has(entry.name)) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

/**
 * Resolve the CSS files of the CURRENT build.
 *
 * Two things make this harder than a glob.
 *
 * MEASURED in Task 1: Next 16.3.4 uses Turbopack, which writes CSS under
 * .next/static/chunks/, NOT .next/static/css/. A hardcoded directory therefore matches
 * nothing and the gate fails OPEN — which is exactly why assertion 4 ("at least one CSS file
 * was read") is load-bearing rather than a formality.
 *
 * And staleness: Next writes content-hashed CSS, Moon hydrates `outputs: ['.next']` from a
 * tarball on a cache hit, and CI restores .moon/cache across runs — so a file from an earlier
 * build can sit beside the current one and satisfy a walk while the current build has lost the
 * probe. The build manifest names only what THIS build produced, so it is the primary
 * resolver; the walk is the fallback, and the fallback is safe only because the build task
 * removes .next/static first (see the M6 step).
 */
function walkCss(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walkCss(full, out);
    else if (entry.name.endsWith('.css')) out.push(full);
  }
  return out;
}

function currentBuildCssFiles(nextDir) {
  const manifestPath = join(nextDir, 'app-build-manifest.json');
  if (existsSync(manifestPath)) {
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
    const files = new Set();
    for (const assets of Object.values(manifest.pages ?? {})) {
      for (const asset of assets) {
        const full = join(nextDir, asset);
        if (asset.endsWith('.css') && existsSync(full)) files.add(full);
      }
    }
    if (files.size > 0) return [...files];
  }
  // Fallback: the manifest is absent or names no CSS (Turbopack may not list it). Walk the
  // whole of .next/static rather than one hardcoded directory.
  const walked = walkCss(join(nextDir, 'static'));
  if (walked.length === 0) {
    throw new Error(`no CSS found under ${join(nextDir, 'static')} and no CSS in ${manifestPath} — was \`next build\` run?`);
  }
  return walked;
}

/**
 * The verdict function. Returns an array of failure messages; empty means pass.
 * Split out from the real run so --self-test can drive it over fixtures.
 */
export function verdict({ cssFiles, appFiles, readFile }) {
  const failures = [];

  // Assertion 4 first: an empty file set must fail loudly, not pass quietly.
  if (cssFiles.length === 0) {
    failures.push('no CSS file was read — the build produced none, or the manifest named none');
    return failures;
  }

  const css = cssFiles.map(readFile).join('\n');
  if (!css.includes(PROBE_SOURCE)) {
    failures.push(`sentinel A (${PROBE_SOURCE}) is absent from the built CSS — the app's Tailwind \`@source\` line no longer covers ts/packages/paigasus-ui/src`);
  }
  if (!css.includes(PROBE_TOKEN)) {
    failures.push(`sentinel B (${PROBE_TOKEN}) is absent from the built CSS — the app's \`@import '@paigasus/ui/styles.css'\` did not resolve`);
  }

  for (const file of appFiles) {
    const text = readFile(file);
    if (text.includes(PROBE_SOURCE) || text.includes(PROBE_TOKEN)) {
      failures.push(`${file} contains a sentinel — the console's own sources must not, or the assertion passes without the package`);
    }
  }

  return failures;
}

function realRun() {
  const nextDir = join(CONSOLE_DIR, '.next');
  const cssFiles = currentBuildCssFiles(nextDir);

  const appFiles = [];
  for (const root of APP_SCAN_ROOTS) {
    const full = join(CONSOLE_DIR, root);
    if (existsSync(full)) walk(full, appFiles);
  }
  for (const file of APP_SCAN_FILES) {
    const full = join(CONSOLE_DIR, file);
    if (existsSync(full)) appFiles.push(full);
  }

  const failures = verdict({ cssFiles, appFiles, readFile: (f) => readFileSync(f, 'utf8') });
  if (failures.length > 0) {
    for (const f of failures) console.error(`FAIL: ${f}`);
    console.error('== tailwind-source guard FAILED ==');
    process.exit(1);
  }
  console.log(`tailwind-source guard: both sentinels present across ${String(cssFiles.length)} CSS file(s)`);
}

function selfTest() {
  const dir = mkdtempSync(join(tmpdir(), 'tailwind-source-'));
  let checks = 0;
  const expect = (label, actual, wanted) => {
    checks += 1;
    if (actual !== wanted) {
      console.error(`SELF-TEST FAIL: ${label} — expected ${String(wanted)} failure(s), got ${String(actual)}`);
      process.exit(2);
    }
  };
  try {
    const good = join(dir, 'good.css');
    writeFileSync(good, `:root{${PROBE_TOKEN}:1}\n.x{${PROBE_SOURCE}:1}\n`);
    const noSource = join(dir, 'no-source.css');
    writeFileSync(noSource, `:root{${PROBE_TOKEN}:1}\n`);
    const noToken = join(dir, 'no-token.css');
    writeFileSync(noToken, `.x{${PROBE_SOURCE}:1}\n`);
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const dirtyApp = join(dir, 'dirty.tsx');
    writeFileSync(dirtyApp, `const leak = '${PROBE_SOURCE}';\n`);

    const read = (f) => readFileSync(f, 'utf8');
    expect('a good build passes', verdict({ cssFiles: [good], appFiles: [cleanApp], readFile: read }).length, 0);
    expect('a missing sentinel A fails', verdict({ cssFiles: [noSource], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a missing sentinel B fails', verdict({ cssFiles: [noToken], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('an empty CSS set fails', verdict({ cssFiles: [], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a sentinel in the app fails', verdict({ cssFiles: [good], appFiles: [dirtyApp], readFile: read }).length, 1);
    expect('both sentinels missing fails twice', verdict({ cssFiles: [cleanApp], appFiles: [cleanApp], readFile: read }).length, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  console.log(`tailwind-source self-test: ${String(checks)} checks passed`);
}

function negativeControl() {
  const dir = mkdtempSync(join(tmpdir(), 'tailwind-source-neg-'));
  try {
    // Mirrors the real Turbopack layout measured in Task 1: CSS under static/chunks/.
    const nextDir = join(dir, '.next');
    mkdirSync(join(nextDir, 'static', 'chunks'), { recursive: true });
    writeFileSync(join(nextDir, 'static', 'chunks', 'a.css'), 'body{color:red}\n');
    writeFileSync(join(nextDir, 'app-build-manifest.json'), JSON.stringify({ pages: { '/page': ['static/chunks/a.css'] } }));
    const cssFiles = currentBuildCssFiles(nextDir);
    const failures = verdict({ cssFiles, appFiles: [], readFile: (f) => readFileSync(f, 'utf8') });
    if (failures.length !== 2) {
      console.error(`NEGATIVE CONTROL FAIL: expected 2 failures on probe-free CSS, got ${String(failures.length)}`);
      process.exit(2);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  console.log('tailwind-source negative control: reported red as expected');
}

const mode = process.argv[2];
if (mode === '--self-test') selfTest();
else if (mode === '--negative-control') negativeControl();
else if (mode === undefined) realRun();
else {
  console.error(`unknown mode: ${mode}`);
  process.exit(2);
}
```

- [ ] **Step 3: Run the self-test and the negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
node ci/tailwind-source/run.mjs --self-test; echo "rc=$?"
node ci/tailwind-source/run.mjs --negative-control; echo "rc=$?"
```
Expected: both print their summary line and exit 0.

- [ ] **Step 4: Take M6 — does the manifest name the current build's CSS?**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cat ts/apps/paigasus-console/.next/app-build-manifest.json | head -40
node ci/tailwind-source/run.mjs; echo "rc=$?"
```
Expected: rc=0 and a line naming at least one CSS file.

**If `app-build-manifest.json` does not exist or names no CSS**, the script's fallback walk takes over — and the walk alone cannot tell a current file from a stale one. In that case ALSO change `paigasus-console-ts:build`'s command in Task 6 to `rm -rf .next/static && pnpm exec next build`. That removes every previously emitted chunk while preserving `.next/cache`, so builds do not become cold, and it is what makes the fallback safe. Record the change in the measurements file and in spec §10.3.

Record M6's verdict, the manifest's actual shape, and the real directory Turbopack wrote CSS to. Task 1 measured `.next/static/chunks/` on Next 16.3.4 — confirm it still holds rather than assuming it.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
git add ci/tailwind-source
git commit -m "ci(repo): assert Tailwind @source reachability for @paigasus/ui (SMA-503)

Two sentinels: one proves Tailwind scanned the package, the other proves
the token stylesheet import resolved. Resolves the CSS file set from the
build manifest, because a hydrated .next can hold a stale file that
satisfies a glob.

Lives outside ts/apps/paigasus-console deliberately. That directory is
Tailwind's scan root, so a script holding the sentinel literal would
generate the utility it asserts on.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 6: Moon wiring

**Files:**
- Modify: `ts/apps/paigasus-console/moon.yml`

**Interfaces:**
- Consumes: the script from Task 5.
- Produces: `paigasus-console-ts:test` running the three invocations; correct affectedness for all three console tasks.

- [ ] **Step 1: Replace the console's `moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-console-ts'
layer: 'application'
language: 'typescript'

# Graph documentation, and what `moon query projects --affected` follows. This confers NO
# affectedness on its own: dependsOn schedules an upstream and never selects a downstream
# (CLAUDE.md, Moon 2.5.3). The task `inputs` below are what make these tasks affected by a
# change inside @paigasus/ui.
dependsOn:
  - 'paigasus-ui-ts'

# The inherited `sources` group from .moon/tasks/typescript-project.yml is `src/**/*`, and
# this app has no src/ — its code is in app/. MEASURED before this fix with
# `moon query projects --id paigasus-console-ts`: the resolved group was exactly
# ts/apps/paigasus-console/src/**/*, matching nothing, so editing app/page.tsx selected NO
# console task at all. repo:input-liveness cannot see this — ci/affected-graph/task_inputs.py
# keys its scan to the `repo` project by exact id. (SMA-503 spec §11.1)
fileGroups:
  sources:
    - 'app/**/*'

tasks:
  build:
    command: 'pnpm exec next build'
    # NOTE the `merge: replace` below: this task inherits NOTHING, so every input it needs is
    # listed here. That is how it lost /ts/pnpm-lock.yaml — see the comment on repo:next-env-drift
    # in the root moon.yml, which records that a Next upgrade never re-keyed this task.
    inputs:
      - '@group(sources)'
      - 'tsconfig.json'
      - 'package.json'
      - 'next.config.ts'
      # New, and at the project root, so no source group reaches it. Editing or deleting it
      # changes the whole CSS pipeline.
      - 'postcss.config.mjs'
      # SMA-503 AC 3. Without this, a change inside @paigasus/ui leaves this task's cache key
      # unchanged, Moon serves a cached .next, `next build` never runs, and the assertion in
      # `test` passes against stale CSS — a green that means nothing.
      - '/ts/packages/paigasus-ui/src/**/*'
      # The Tailwind, PostCSS and Next versions all come from here.
      - '/ts/pnpm-lock.yaml'
    outputs:
      - '.next'
    options:
      merge: replace

  typecheck:
    # Appended, not replaced. A type error introduced in @paigasus/ui would otherwise leave
    # this task's cache key unchanged. `next build` typechecks too, so the coverage exists in
    # practice — this makes it deliberate rather than accidental.
    inputs:
      - '/ts/packages/paigasus-ui/src/**/*'

  test:
    # SMA-503 AC 3. Three invocations of the guard, mirroring the self-scheduled repo:* gates:
    # the self-test and the negative control prove the assertion CAN fire before the real run
    # is trusted. Moon does not enable errexit for `script:` blocks — the same latent defect
    # the nats-permissions, promtool and publish-metadata tasks document — hence the explicit
    # `set -euo pipefail`.
    #
    # The vitest line stays so this task keeps the meaning of its name and SMA-510 has a place
    # to add real unit tests.
    #
    # The guard lives at the REPOSITORY ROOT and must stay there: this directory is Tailwind's
    # automatic scan root, so a script here containing the sentinel literal would make Tailwind
    # generate the utility the script asserts on. See ci/tailwind-source/README.md.
    script: |
      set -euo pipefail
      pnpm exec vitest run --passWithNoTests
      node ../../../ci/tailwind-source/run.mjs --self-test
      node ../../../ci/tailwind-source/run.mjs --negative-control
      node ../../../ci/tailwind-source/run.mjs
    deps:
      - '~:build'
      # repo:next-env-drift ALSO depends on this project's build, because `next typegen` writes
      # into .next. Without this edge, that gate and this task would run concurrently over the
      # same directory — the flake shape the root moon.yml already records for wasm-pack.
      - 'repo:next-env-drift'
    inputs:
      - '@group(sources)'
      - 'tsconfig.json'
      - 'package.json'
      - 'next.config.ts'
      - 'postcss.config.mjs'
      # The guard itself. Without this, deleting an assertion from the script does not re-key
      # this task, and a weakened gate serves a cached PASS.
      - '/ci/tailwind-source/**/*'
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/pnpm-lock.yaml'
    options:
      merge: replace
```

- [ ] **Step 2: Take M3 — verify Moon merged the file group rather than overriding it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --id paigasus-console-ts | grep -A8 '"sources"'
```
Expected: the resolved globs contain **both** `ts/apps/paigasus-console/src/**/*` and `ts/apps/paigasus-console/app/**/*`. Record this as M3.

If only `app/**/*` appears, Moon overrode rather than merged. That is still correct for this project — record the corrected behaviour in the measurements file and in spec §11.1, and check whether any other project relies on the merge assumption.

- [ ] **Step 3: Verify the resolved inputs of all three tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --id paigasus-console-ts | grep -E 'paigasus-ui/src|pnpm-lock|tailwind-source|postcss'
```
Expected: `/ts/packages/paigasus-ui/src/**/*` appears three times (build, typecheck, test), `pnpm-lock` at least twice, `tailwind-source` once, `postcss.config.mjs` twice.

- [ ] **Step 4: Run the console tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:test
```
Expected: build runs, then the three guard invocations print their lines, and the task passes.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/paigasus-console/moon.yml
git commit -m "build(ts): key the console tasks on @paigasus/ui and run the source guard (SMA-503)

Adds the app/**/* source group the project never had — the inherited
src/**/* matched nothing, so editing app/page.tsx selected no console task
at all. Restores the pnpm lockfile input that merge:replace had dropped
from build.

The test task now runs the Tailwind source guard with its self-test and
negative control, ordered after repo:next-env-drift so the two do not race
over .next.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 7: Prove the assertions bite

Two distinct failures need two distinct proofs. Nothing is committed by this task except the measurements file.

- [ ] **Step 1: Proof 1 — the `@source` line**

Delete the `@source '../../../packages/paigasus-ui/src';` line from `ts/apps/paigasus-console/app/globals.css`, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:test --force; echo "rc=$?"
```
Expected: **FAIL**, with `sentinel A (--paigasus-ui-source-probe) is absent`. Sentinel B must still pass — the token import is unaffected.

Restore with `git checkout -- ts/apps/paigasus-console/app/globals.css`. **Do not restore from a `.bak` copy**: a rolled-back modification time makes the next run reuse the earlier artifact, producing a failure that looks real and is not.

- [ ] **Step 2: Proof 1b — the token `@import`**

Delete the `@import '@paigasus/ui/styles.css';` line, re-run as above.
Expected: **FAIL** with `sentinel B (--paigasus-token-probe) is absent`. Restore with `git checkout --`.

- [ ] **Step 3: Proof 2 — the cache path, which proof 1 never reaches**

Proof 1 edits `app/globals.css`, which is an input of both console tasks, so the build re-runs and only the non-cached path is exercised. This proof edits a file in the PACKAGE instead.

Remove the `SOURCE_PROBE` string from `ts/packages/paigasus-ui/src/components/table.tsx` (replace the constant's value with `''`). Then confirm the console tasks are genuinely **selected**:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks --affected > /tmp/affected.json
python3 -c "
import json
d = json.load(open('/tmp/affected.json'))
for project, tasks in d.get('tasks', {}).items():
    for task in tasks:
        print(f'{project}:{task}')
" | sort
```

**Parse the JSON — never `grep -o '\"target\"'`.** Every dep entry carries its own `target` key, so grepping counts SCHEDULED upstreams as SELECTIONS, and this proof turns on exactly that distinction.

Expected: `paigasus-console-ts:build`, `paigasus-console-ts:test` and `paigasus-console-ts:typecheck` all appear.

Then:
```bash
moon run paigasus-console-ts:test; echo "rc=$?"
```
Expected: **FAIL** on sentinel A. Restore with `git checkout -- ts/packages/paigasus-ui/src/components/table.tsx`, then re-run and confirm it passes.

- [ ] **Step 4: Proof 3 — the script's own inputs**

Comment out the `--negative-control` line in the console's `moon.yml` `test` script, run `moon run paigasus-console-ts:test`, and confirm the task **re-runs** rather than serving a cached pass. This proves `/ci/tailwind-source/**/*` and the project's `moon.yml` are keyed correctly. Restore with `git checkout --`.

- [ ] **Step 5: Record all four proofs and commit the measurements file**

Append a "Proofs" section to `docs/superpowers/specs/2026-09-08-sma-503-measurements.md` with the exact commands and the verbatim failure lines.

```bash
git add docs/superpowers/specs/2026-09-08-sma-503-measurements.md
git commit -m "docs(ts): record the SMA-503 measurements and gate proofs (SMA-503)

M1-M7, plus four proofs that the AC-3 assertions fail when the thing they
test is broken: the @source line, the token @import, a package-source edit
reaching the cached path, and the guard's own inputs.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Confirm the working tree is clean**

```bash
git status --short
```
Expected: empty. If any proof edit survived, the restore was missed — fix it before continuing.

---

## Task 8: The continuous guard on the input that makes AC 3 real

**Files:**
- Modify: `ci/affected-graph/run.sh`

Task 7's proofs are one-time. Removing `/ts/packages/paigasus-ui/src/**/*` from the console's inputs later would red nothing, and that input is the only thing making the proof real.

- [ ] **Step 1: Derive the expected set — do not guess it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git stash list   # confirm nothing of yours is stashed; the stack is shared across worktrees
echo '/* probe */' >> ts/packages/paigasus-ui/src/styles/tokens.css
moon query tasks --affected --upstream none > /tmp/ui-affected.json
python3 -c "
import json
d = json.load(open('/tmp/ui-affected.json'))
print(','.join(sorted(f'{p}:{t}' for p, ts in d.get('tasks', {}).items() for t in ts)))
"
git checkout -- ts/packages/paigasus-ui/src/styles/tokens.css
```

Record the exact comma-separated list. Compare the flags against how `run_task_case_ci` invokes `moon query` in `ci/affected-graph/run.sh` (see `_assert_task_case_impl`) and use the same traversal, so the recorded set matches what the harness will compute.

- [ ] **Step 2: Add the case**

In `ci/affected-graph/run.sh`, immediately after the `kernel->consumer-tasks` case, add:

```bash
  # SMA-503 — a @paigasus/ui SOURCE edit must select the console's build, test and typecheck.
  # `paigasus-console-ts:test` runs the Tailwind @source guard against the build's output, and
  # the ONLY thing making that proof real is `/ts/packages/paigasus-ui/src/**/*` sitting in
  # both tasks' `inputs`. Remove it and the guard reads a cached .next produced before the
  # change — a green that means nothing. Nothing else in the repo notices, so this case is the
  # control. Strict equality: re-baseline deliberately when the set legitimately changes.
  run_task_case_ci "ui->console" "ts/packages/paigasus-ui/src/styles/tokens.css" \
    "<PASTE THE SET FROM STEP 1>"
```

Replace the placeholder with step 1's exact output. **Do not leave the placeholder.**

- [ ] **Step 3: Run the gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke
```
Expected: PASS. If the new case reds with a set that differs from step 1's, the traversal flags differ — read `_assert_task_case_impl` and re-derive rather than editing the expected string to match the error.

- [ ] **Step 4: Prove the case bites**

Temporarily remove `/ts/packages/paigasus-ui/src/**/*` from the console `test` task's inputs, re-run `moon run repo:affected-smoke`, and confirm it **fails** on the `ui->console` case. Restore with `git checkout --`.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "ci(repo): assert a @paigasus/ui source edit selects the console tasks (SMA-503)

The Tailwind source guard is only real while the console's build and test
key on the package's sources. Removing that input reds nothing today; this
strict-equality case is the control.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 9: Overlay primitives — Dialog and DropdownMenu

**Files:**
- Create: `ts/packages/paigasus-ui/components.json`, `ts/packages/paigasus-ui/src/components/dialog.tsx`, `ts/packages/paigasus-ui/src/components/dropdown-menu.tsx`, `ts/packages/paigasus-ui/tests/dialog.test.tsx`, `ts/packages/paigasus-ui/tests/dropdown-menu.test.tsx`
- Modify: `ts/packages/paigasus-ui/src/index.ts`

**Interfaces:**
- Consumes: `cn` (Task 1), `expectNoAxeViolations` (Task 3).
- Produces: `Dialog`, `DialogTrigger`, `DialogContent`, `DialogHeader`, `DialogTitle`, `DialogDescription`, `DialogFooter`, `DialogClose`; `DropdownMenu`, `DropdownMenuTrigger`, `DropdownMenuContent`, `DropdownMenuItem`, `DropdownMenuSeparator`, `DropdownMenuLabel`.

- [ ] **Step 1: Commit the shadcn configuration**

Create `ts/packages/paigasus-ui/components.json` (no SPDX header — JSON admits no comment):

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": true,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/styles/tokens.css",
    "baseColor": "zinc",
    "cssVariables": true,
    "prefix": ""
  },
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/cn",
    "ui": "@/components",
    "lib": "@/lib",
    "hooks": "@/hooks"
  }
}
```

`tailwind.config` is empty because Tailwind v4 is CSS-first — there is no `tailwind.config.*` in this repository.

- [ ] **Step 2: Generate, then normalise**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm dlx shadcn@latest add dialog dropdown-menu
```

Then normalise every generated file. This is **type-level work, not formatting** — `ts/tsconfig.base.json` sets `exactOptionalPropertyTypes`, `verbatimModuleSyntax`, `noUncheckedIndexedAccess` and `isolatedModules`, and shadcn output satisfies none of the first three:

1. Add the SPDX header above `'use client'`.
2. Re-point every `@radix-ui/react-*` import at the unified `radix-ui` package: `import { Dialog as DialogPrimitive } from 'radix-ui';`.
3. **Remove any individual `@radix-ui/react-*` the CLI added to `package.json`**, then re-run `pnpm -C ts install`. The CLI installs them at literal versions, which conflicts with both the unified-`radix-ui` decision and the catalog policy.
4. Change the `cn` import to `../lib/cn`.
5. Convert `import * as React from 'react'` to named type imports where `verbatimModuleSyntax` demands it.
6. Give every exported component an explicit return type.

If the CLI cannot run offline, hand-write the two components against `radix-ui`'s `Dialog` and `DropdownMenu` following `table.tsx`'s shape. The result must be the same.

- [ ] **Step 3: Write the tests**

Create `ts/packages/paigasus-ui/tests/dialog.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from '../src/components/dialog';

function Fixture() {
  return (
    <Dialog>
      <DialogTrigger>Open</DialogTrigger>
      <DialogContent>
        <DialogTitle>Revoke key</DialogTitle>
        <DialogDescription>This cannot be undone.</DialogDescription>
      </DialogContent>
    </Dialog>
  );
}

describe('Dialog', () => {
  it('opens on trigger activation and exposes a named dialog role', async () => {
    const user = userEvent.setup();
    render(<Fixture />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open' }));
    expect(screen.getByRole('dialog', { name: 'Revoke key' })).toBeInTheDocument();
  });

  it('has no axe violations while open', async () => {
    const user = userEvent.setup();
    render(<Fixture />);
    await user.click(screen.getByRole('button', { name: 'Open' }));
    // No argument: the content is portalled onto document.body, so a container-scoped
    // assertion would check a subtree holding only the trigger.
    await expectNoAxeViolations();
  });
});
```

Create `ts/packages/paigasus-ui/tests/dropdown-menu.test.tsx` with the same shape: assert the trigger opens a `menu` role holding `menuitem` children, and assert `expectNoAxeViolations()` while open.

- [ ] **Step 4: Run, and extend the polyfills only if a test demands it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run
```
Expected: all pass. If a Radix component fails on a missing browser API, add only that API to `tests/setup.ts` and update M2 in the measurements file.

- [ ] **Step 5: Export, typecheck, format, commit**

Add both components' exports to `src/index.ts`. Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-ui-ts:typecheck && moon run ts:lint && moon run ts:fmt
git add ts/packages/paigasus-ui ts/pnpm-lock.yaml docs/superpowers/specs/2026-09-08-sma-503-measurements.md
git commit -m "feat(ts): seed the Dialog and DropdownMenu primitives (SMA-503)

Both come from Radix through the unified radix-ui package. Tests assert
the open state's roles and run axe against document.body, because Radix
portals overlay content out of the render container.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 10: Combobox — Popover, Command and the two-Radix-copy rule

**Files:**
- Create: `ts/packages/paigasus-ui/src/components/popover.tsx`, `ts/packages/paigasus-ui/src/components/command.tsx`, `ts/packages/paigasus-ui/src/components/combobox.tsx`, `ts/packages/paigasus-ui/tests/combobox.test.tsx`
- Modify: `ts/packages/paigasus-ui/src/index.ts`

**Interfaces:**
- Consumes: `cn` (Task 1), `expectNoAxeViolations` (Task 3).
- Produces: `Combobox` with props `{ items: ComboboxItem[]; value?: string; onValueChange?: (value: string) => void; placeholder?: string; emptyMessage?: string }`, and `interface ComboboxItem { value: string; label: string }`. `Popover` and `Command` stay internal — they are **not** exported from the barrel.

- [ ] **Step 1: Generate and normalise**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm dlx shadcn@latest add popover command
```

Normalise exactly as Task 9 step 2, and additionally:

**Put this comment at the top of `src/components/command.tsx`, below the SPDX header:**

```tsx
/*
 * `cmdk` brings its OWN individual @radix-ui/react-dialog, @radix-ui/react-id,
 * @radix-ui/react-primitive and @radix-ui/react-compose-refs, while this package depends on
 * the unified `radix-ui`, which bundles its own pinned copies of the same modules. Two copies
 * of @radix-ui/react-dialog therefore exist in the tree.
 *
 * NOTHING HERE MAY COMPOSE ACROSS THE TWO COPIES. A trigger from one copy does not open
 * content from the other, the failure is silent at runtime, and jsdom tests may not surface
 * it. cmdk's duplicate Radix modules are reachable only through `Command.Dialog`, so
 * `Command.Dialog` IS BANNED. Combobox composes radix-ui's Popover with cmdk's plain Command;
 * Dialog comes from radix-ui only. (SMA-503 spec §8)
 */
```

Delete any `CommandDialog` export the CLI generated.

- [ ] **Step 2: Write the failing test**

Create `ts/packages/paigasus-ui/tests/combobox.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Combobox } from '../src/components/combobox';

const ITEMS = [
  { value: 'iam', label: 'IAM' },
  { value: 'gateway', label: 'Gateway' },
];

describe('Combobox', () => {
  it('opens, filters and reports the chosen value', async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    render(<Combobox items={ITEMS} placeholder="Select a service" onValueChange={onValueChange} />);

    await user.click(screen.getByRole('combobox'));
    await user.type(screen.getByRole('combobox'), 'gate');
    await user.click(screen.getByRole('option', { name: 'Gateway' }));

    expect(onValueChange).toHaveBeenCalledWith('gateway');
  });

  it('shows the empty message when nothing matches', async () => {
    const user = userEvent.setup();
    render(<Combobox items={ITEMS} emptyMessage="No service found." />);
    await user.click(screen.getByRole('combobox'));
    await user.type(screen.getByRole('combobox'), 'zzz');
    expect(screen.getByText('No service found.')).toBeInTheDocument();
  });

  it('has no axe violations while open', async () => {
    const user = userEvent.setup();
    render(<Combobox items={ITEMS} placeholder="Select a service" />);
    await user.click(screen.getByRole('combobox'));
    await expectNoAxeViolations();
  });
});
```

- [ ] **Step 3: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/combobox.test.tsx
```
Expected: FAIL — cannot resolve `../src/components/combobox`.

- [ ] **Step 4: Implement the Combobox**

Create `ts/packages/paigasus-ui/src/components/combobox.tsx` composing `Popover` (from `radix-ui`) with `Command` (from `cmdk`). It must render an element with `role="combobox"` that accepts typed input, list `role="option"` children, call `onValueChange` on selection, and render `emptyMessage` when the filter matches nothing.

Follow the shape of the shadcn combobox recipe, but adapted: this repository's version is a single self-contained component taking `items`, not a copy-paste snippet the consumer assembles. Give every prop an explicit type, and give the component an explicit `ReactElement` return type.

- [ ] **Step 5: Run to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/combobox.test.tsx
```
Expected: all three PASS. Extend `tests/setup.ts` only if a test demands a missing API, and update M2.

- [ ] **Step 6: Export only the Combobox**

Add to `src/index.ts`:

```ts
export { Combobox, type ComboboxItem, type ComboboxProps } from './components/combobox';
```

`Popover` and `Command` stay internal. Do not export them.

- [ ] **Step 7: Typecheck, lint, format, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-ui-ts:typecheck && moon run ts:lint && moon run ts:fmt
git add ts/packages/paigasus-ui ts/pnpm-lock.yaml
git commit -m "feat(ts): seed the Combobox on Radix Popover and cmdk (SMA-503)

ADR-0021 forbids hand-rolling a combobox, and shadcn's is built on cmdk,
so cmdk is a dependency. It carries its own individual Radix packages, so
two copies of react-dialog exist in the tree; Command.Dialog is banned and
nothing composes across the copies.

Popover and Command stay internal to the Combobox.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 11: Form primitives and state components

**Files:**
- Create: `ts/packages/paigasus-ui/src/components/form.tsx`, `ts/packages/paigasus-ui/src/components/empty-state.tsx`, `ts/packages/paigasus-ui/src/components/error-state.tsx`, and one test file per component
- Modify: `ts/packages/paigasus-ui/src/index.ts`

**Interfaces:**
- Consumes: `cn` (Task 1), `Link` (Task 4), `expectNoAxeViolations` (Task 3).
- Produces: `Label`, `Input`, `Field`; `EmptyState`, `ErrorState`.
  - `Field` props: `{ label: string; htmlFor: string; description?: string; error?: string; children: ReactNode }`
  - `EmptyState` props: `{ title: string; description?: string; action?: ReactNode }`
  - `ErrorState` props: `{ title: string; description?: string; retry?: () => void }`

- [ ] **Step 1: Write the failing tests**

Create `ts/packages/paigasus-ui/tests/form.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Field, Input } from '../src/components/form';

describe('Field', () => {
  it('associates the label with the control', () => {
    render(
      <Field label="Key name" htmlFor="key-name">
        <Input id="key-name" />
      </Field>,
    );
    expect(screen.getByLabelText('Key name')).toBeInTheDocument();
  });

  it('exposes the error to assistive technology', () => {
    render(
      <Field label="Key name" htmlFor="key-name" error="Name is required.">
        <Input id="key-name" />
      </Field>,
    );
    const input = screen.getByLabelText('Key name');
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription(/Name is required\./);
  });

  it('has no axe violations', async () => {
    render(
      <Field label="Key name" htmlFor="key-name" description="Shown in the audit log.">
        <Input id="key-name" />
      </Field>,
    );
    await expectNoAxeViolations();
  });
});
```

Create `ts/packages/paigasus-ui/tests/empty-state.test.tsx` and `ts/packages/paigasus-ui/tests/error-state.test.tsx`. Each asserts the title renders as a heading, the optional description renders, the action or retry control is reachable by role, and `expectNoAxeViolations()` passes.

- [ ] **Step 2: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/form.test.tsx tests/empty-state.test.tsx tests/error-state.test.tsx
```
Expected: FAIL on unresolved imports.

- [ ] **Step 3: Implement**

`form.tsx` builds `Label` on `radix-ui`'s `Label` primitive and composes `Field` so that the error and description reach the control through `aria-describedby`, and the error sets `aria-invalid`. Cloning the child to inject those attributes is acceptable; document the choice in a comment.

`empty-state.tsx` and `error-state.tsx` are plain composition — a heading, optional prose, an optional action slot. They may use `Link` from `../nav/link` for a navigational action.

Every component carries `'use client'` below its SPDX header, and an explicit return type.

- [ ] **Step 4: Run to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run
```
Expected: the whole suite passes.

- [ ] **Step 5: Export, verify, commit**

Add every public component to `src/index.ts`. Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-ui-ts:typecheck && moon run ts:lint && moon run ts:fmt
git add ts/packages/paigasus-ui
git commit -m "feat(ts): seed the form primitives and the empty and error states (SMA-503)

Field wires the label, description and error to the control through
aria-describedby and aria-invalid, so the axe assertion covers the
association rather than only the markup.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 12: Documentation and the full-graph run

**Files:**
- Create: `ts/packages/paigasus-ui/README.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Write the package README**

Create `ts/packages/paigasus-ui/README.md` covering exactly three things a later reader would otherwise get wrong:

1. **The axe limit.** axe cannot evaluate `color-contrast` in jsdom, because jsdom performs no layout. The assertions cover roles, accessible names, ARIA state and labelling. Contrast is a design responsibility decided in the token layer. Do not read the green as covering it.
2. **The per-consumer `@source` obligation.** The console's green says nothing about a second zone app. Every new consumer must add its own `@source` line covering `ts/packages/paigasus-ui/src`, its own `@import '@paigasus/ui/styles.css'`, and its own probe assertion — otherwise its build silently drops this package's classes, and the failure looks like broken styling rather than a build error.
3. **The post-`shadcn add` normalisation rule.** The CLI installs individual `@radix-ui/react-*` packages at literal versions. Remove them, re-point the imports at the unified `radix-ui`, re-run `pnpm -C ts install`, and normalise for `exactOptionalPropertyTypes`, `verbatimModuleSyntax` and `noUncheckedIndexedAccess`. Nothing catches this automatically.

Also state the two-Radix-copy rule and that `Command.Dialog` is banned.

- [ ] **Step 2: Add a CLAUDE.md gotcha**

Add one bullet to the Gotchas section, in the style of its neighbours. Cover: Tailwind v4's scan root is the cwd, so every consumer of `@paigasus/ui` needs its own `@source` line; the guard lives at `ci/tailwind-source/` and must never move under the console because that directory is the scan root; the console's `build` uses `merge: replace` and therefore lists `/ts/pnpm-lock.yaml` by hand; and `repo:affected-smoke`'s `ui->console` case is the only control on the input that makes the guard real.

Do **not** touch the `<!-- ci-targets:begin -->` block — no new `repo:*` gate was added, so the `T` array is unchanged.

- [ ] **Step 3: Run the full graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci --base origin/main --include-relations
```

Expected: all pass. Note that `repo:release-parity*` aborts INCONCLUSIVE at rc=2 inside an agent session because `proto` emits NDJSON — that is not a red, and it is not a pass either.

If a task fails and Moon does not attribute it, follow the diagnosis procedure in `CLAUDE.md` between the `moon-diagnosis` markers. **Capture `.moon/cache/ciReport.json` and the failing task's state directory before re-running anything** — a passing re-run overwrites the evidence.
<!-- moon-diagnosis:ok -->

- [ ] **Step 4: Check the new dependencies against the OSV gate**

The run above includes `:osv`. If it reports a finding against a newly added package, fix it by bumping — a range-scoped selector in `ts/pnpm-workspace.yaml`'s `overrides:` block — rather than by adding an `osv-scanner.toml` waiver. A waiver needs a reason that says why the vulnerable path is unreachable here.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
git add ts/packages/paigasus-ui/README.md CLAUDE.md
git commit -m "docs(ts): document the @paigasus/ui limits and obligations (SMA-503)

Records the three things a reader would otherwise get wrong: axe cannot
check contrast in jsdom, every consumer needs its own Tailwind @source
line, and shadcn's CLI must be normalised back onto the unified radix-ui
package after every add.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Rebase onto SMA-502 — before the pull request, never after

Per spec §4, the pull request does not open until SMA-502 is merged.

- [ ] Confirm SMA-502 is on `main`: `git log origin/main --oneline | grep -i 'sma-502'`
- [ ] `git fetch origin && git rebase origin/main`
- [ ] Resolve any `ts/pnpm-lock.yaml` conflict by taking `origin/main`'s version and re-running `pnpm -C ts install`. **Never hand-merge a lockfile.**
- [ ] Read the post-502 `outputs` value on `paigasus-console-ts:build`. Confirm the CSS directory the guard reads is inside what Moon hydrates. If SMA-502 narrowed it and the CSS path fell out, restore coverage — otherwise the guard is green on a cold run and red on a cache hit.
- [ ] If Task 1's M5 found `transpilePackages` necessary, move it into SMA-502's `next.config` factory now and remove the temporary direct edit.
- [ ] Confirm SMA-502's boundary rule covers the `ui → React only` edge, including type-only imports. If it does not, that is a follow-up issue, not a change here.
- [ ] Re-run the full graph command from Task 12 step 3.

---

## Self-Review

**Spec coverage.** §4 → the rebase checklist and Task 1's M5 note. §5 → Task 1 steps 3, 6, 7. §5.1 `dark:` variant → Task 1 step 3. §5.2 layout import → Task 1 step 7. §6 `'use client'` and M5/M7 → Task 1 steps 5, 10, 11. §7 navigation → Task 4. §7.1 the three controls → Task 2 steps 2, 4, 5. §8 component set and the cmdk rule → Tasks 9, 10, 11. §8.1 normalisation → Task 9 step 2, Task 12 step 1. §9 harness, globals, cleanup → Task 2. §9.1 axe → Task 3. §9.2 the limit → Task 3 step 3 and Task 12 step 1. §9.3 tsconfig → Task 2 step 1. §10.1 M4 → Task 1 step 9. §10.2 both sentinels → Task 1 steps 3, 5. §10.3 script placement, four assertions, M6 → Task 5. §10.4 self-test and negative control → Task 5 steps 2, 3 and Task 6 step 1. §10.5 both proofs → Task 7. §11 all three task input sets → Task 6. §11.1 both defects → Task 6 steps 1, 2. §11.2 next-env-drift ordering → Task 6 step 1. §11.3 the affected-graph case → Task 8. §12 dependencies → Task 1 steps 1, 2. §13 SPDX → Global Constraints, Task 9 step 1. §15 README → Task 12 step 1.

**Type consistency.** `cn(...inputs: ClassValue[]): string` is defined in Task 1 and used in Tasks 1, 9, 10, 11. `expectNoAxeViolations(target?: Element): Promise<void>` is defined in Task 3 and called with no argument everywhere after. `LinkComponent`, `LinkProvider`, `useLinkComponent`, `Link`, `LinkProps` are defined in Task 4 and consumed in Task 4 step 6 and Task 11. `ComboboxItem` and `ComboboxProps` are defined and exported in Task 10 only. `verdict({ cssFiles, appFiles, readFile })` is defined once in Task 5 and driven by both `--self-test` and `--negative-control` in the same file. The sentinel strings `--paigasus-ui-source-probe` and `--paigasus-token-probe` are spelled identically in Tasks 1, 5, 7.

**Placeholders.** One deliberate placeholder exists: `<PASTE THE SET FROM STEP 1>` in Task 8 step 2, which cannot be written in advance because it is a strict-equality set that must be derived by running the query. Step 2 says so explicitly and forbids leaving it. There are no others.
