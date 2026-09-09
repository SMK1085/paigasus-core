# SMA-503 — `@paigasus/ui`: Tailwind v4 + Radix foundation

**Date:** 2026-09-08
**Linear:** [SMA-503](https://linear.app/smaschek/issue/SMA-503/ts-paigasusui-tailwind-v4-radix-foundation)
**ADR:** [ADR-0021 — Tailwind + Radix primitives (shadcn-style)](https://app.notion.com/p/3bb830e8fbaa81fa847ecf9892f6bd96)
**Blocks:** SMA-510 (`@paigasus/app-shell`)
**Revision:** 2 — rewritten after an adversarial review returned NEEDS REWORK with six blockers.
Section 17 records what changed and what was rejected.

## 1. Problem

`ts/packages/paigasus-ui` holds a stub. It has a React peer dependency, a `tsconfig.json`, and a
`src/index.ts` that exports nothing. Every console surface needs a component foundation before it
can be built. This issue supplies that foundation.

The surfaces are operator and admin UI: tenancy trees, Cedar policy editing, role grants, API-key
issuance, audit-log tables, and dead-letter-queue inspection. The workload is dense and
table-heavy. It contains many accessibility-critical interactive primitives.

## 2. Decision summary

ADR-0021 already fixed the technology choice. This spec does not re-open it. The ADR decides:

1. Tailwind v4 with CSS-first configuration, plus Radix primitives, with shadcn-style copy-in
   components owned in this repository.
2. Design tokens as CSS custom properties. Light and dark differ only by token redefinition.
3. `@paigasus/ui` never imports from `next/*`. Navigation is injected.
4. Tailwind v4's `@source` must cover the package's `src`.
5. Radix owns the accessibility-critical primitives. Hand-rolled menus, dialogs and comboboxes are
   out of scope.

## 3. Measurements

Six facts this design rests on are not yet measured. Each is listed here, with its section. M1,
M4 and M5 are **preconditions**: take them before writing implementation code, because a
different result changes the design rather than the code.

| ID | Question | Section | Kind |
|---|---|---|---|
| M1 | Does `@import '@paigasus/ui/styles.css'` resolve through `@tailwindcss/postcss` under pnpm's symlinked `node_modules` in a Next 16 production build? | 5 | precondition |
| M2 | Which browser APIs do the Radix components actually need in jsdom, under Vitest 5 with React Testing Library? | 9 | validation |
| M3 | Does Moon merge a project's `fileGroups` with the inherited ones, or override them? | 11.1 | validation |
| M4 | What is Tailwind v4's automatic scan root for this build, which build mode does Next 16 use, and where does it write CSS? | 10.1, 10.3 | precondition |
| M5 | Does Next compile `@paigasus/ui`'s TypeScript without `transpilePackages`, given pnpm resolves it through a `node_modules` symlink to a path outside `node_modules`? | 6 | precondition |
| M6 | Does `.next/app-build-manifest.json` name the CSS files of the current build, and does a Moon cache hydration leave stale files beside them? | 10.3 | validation |
| M7 | Does Next's SWC accept an SPDX line comment above a `'use client'` directive? | 6 | validation |

## 4. Relationship to SMA-502 — a hard dependency

SMA-502 (`@paigasus/next-config`) started at the same time as this issue, in a parallel session.
It owns two things that SMA-503's acceptance criteria name:

- **The eslint boundary rule.** SMA-503 AC 1 says the `next/*` ban is "enforced by the boundary
  lint rule". That rule is SMA-502's deliverable. Its scope names the exact edge (`ui → React
  only`) and its AC 3 exercises the rule against a deliberately wrong import. The rule does not
  exist in `ts/eslint.config.js` today.
- **The console's `next.config.ts`.** SMA-502 replaces it with a factory and changes the Moon
  `build` task's `outputs`.

**Decision (Sven, 2026-09-08): SMA-503 assumes SMA-502 lands first.** SMA-503 writes no eslint
boundary rule. This branch rebases onto `main` after SMA-502 merges, and before the pull request
opens.

**If SMA-502 slips (Sven, 2026-09-08): block.** SMA-503's pull request does not open until
SMA-502 is merged. It does not ship with a substitute control, and it does not write the boundary
rule itself. Rebase debt against `main` is the accepted cost.

Linear records no `blockedBy` edge between the two issues. The edge is real and this section is
the record of it.

### 4.1 The rebase checklist

Three things must be re-checked after the rebase, not assumed:

1. **The console `build` task's `outputs` value.** Section 10.3's assertion reads CSS from inside
   `.next`. If SMA-502 narrows `outputs` — for example to exclude `.next/cache` — the assertion
   must still be covered by what Moon hydrates on a cache hit. Read the post-502 value and confirm
   the CSS directory is inside it. A narrowed `outputs` that drops the CSS path gives a gate that
   is green on a cold run and red on a cache hit.
2. **`next.config.ts`.** If M5 shows `transpilePackages` is needed (section 6), it must be added
   through SMA-502's factory, not beside it.
3. **`ts/pnpm-lock.yaml`.** Both branches change it. Resolve the conflict by re-running
   `pnpm install`. Never hand-merge a lockfile.

## 5. Package shape and the token layer

`@paigasus/ui` stays source-only. Its `exports` point at `./src`, as `@paigasus/sdk` and
`@paigasus/kernel` do. There is no build step and no `dist` directory.

This is not only consistency. Tailwind scans files on disk, so the package must expose real source
paths for AC 3 to work at all.

```
ts/packages/paigasus-ui/
  components.json            # shadcn monorepo configuration, committed once
  package.json
  tsconfig.json
  vitest.config.ts           # one jsdom project, plus the next/* resolver ban
  README.md
  src/
    index.ts                 # the single public barrel
    lib/cn.ts                # clsx + tailwind-merge
    nav/link.tsx             # injected navigation (section 7)
    styles/tokens.css        # the design-token layer
    components/
      table.tsx              # carries the AC-3 sentinel (section 10.2)
      dialog.tsx
      dropdown-menu.tsx
      popover.tsx
      command.tsx
      combobox.tsx
      form.tsx               # Label, Input, Field
      empty-state.tsx
      error-state.tsx
  tests/
    setup.ts
    axe.ts
    fixtures/imports-next.ts # the AC-1 negative control (section 7)
    *.test.tsx
```

Two export entries:

| Entry | Target | Purpose |
|---|---|---|
| `.` | `./src/index.ts` | every component, plus the navigation contract |
| `./styles.css` | `./src/styles/tokens.css` | the design-token layer |

`Popover` and `Command` are **internal**. They exist as separate files because `Combobox` composes
them, and because shadcn's CLI generates them as separate files. They are not re-exported from the
barrel. `Dialog`, `DropdownMenu`, `Combobox`, `Table`, the form primitives and the two state
components are the public surface.

### 5.1 The token layer

`src/styles/tokens.css` owns the theming model. It holds one `@theme` block that maps Tailwind's
colour and radius names onto CSS custom properties. Light is the bare `:root`. Dark redefines only
those properties, in two places:

- `@media (prefers-color-scheme: dark)`, guarded as `:root:not([data-theme="light"])`.
- `:root[data-theme="dark"]`, so an explicit choice wins in both directions.

**The `dark:` variant must be redefined, or the two mechanisms disagree.** Tailwind v4's built-in
`dark:` variant keys on `prefers-color-scheme` alone, so `data-theme="light"` cannot override a
`dark:` utility — and shadcn's copy-in components ship `dark:` classes. `tokens.css` therefore
declares:

```css
@custom-variant dark (&:where([data-theme='dark'], [data-theme='dark'] *));
```

Every copied-in component's `dark:` class then routes through the same mechanism as the tokens.

No component names a raw colour. Every component names a token.

### 5.2 How the application consumes it

```css
/* ts/apps/paigasus-console/app/globals.css */
@import 'tailwindcss';
@import '@paigasus/ui/styles.css';
@source '../../../packages/paigasus-ui/src';
```

`ts/apps/paigasus-console/app/layout.tsx` must add `import './globals.css';`. Without it Next
emits no CSS at all and section 10.3's assertions red immediately for a reason unrelated to
`@source`.

**M1.** Confirm the bare-specifier `@import` resolves. Tailwind v4 carries its own resolver for
CSS `@import`, but a bare package specifier under pnpm's symlinked `node_modules` is not proven
here. If it does not resolve, use a relative path to the same file and record the change in this
section.

**Why `@source` does not live inside `@paigasus/ui/styles.css`.** Two facts settle it. `@source`
paths resolve relative to the declaring stylesheet, and Tailwind excludes `node_modules` from the
scan. A package-local `@source './..'` would therefore resolve into the symlinked
`node_modules/@paigasus/ui` path and be excluded. The obligation is per-consumer by construction,
which is why section 15 records it in the package README.

## 6. Client boundaries and Next compilation

**`'use client'` placement.** Every component file that uses Radix, React state or a React context
carries `'use client'` as its first statement. `src/index.ts` does **not** — a barrel carrying the
directive would force every consumer of any export into the client bundle.

The SPDX header this repository requires is a line comment, so it precedes the directive. **M7**
confirms Next's SWC accepts a leading comment above `'use client'`. A leading comment is normally
allowed, but this repository does not ship an unmeasured claim about a build tool.

**M5 — `transpilePackages`.** Next does not compile TypeScript inside `node_modules` by default;
the documented mechanism is `transpilePackages`. pnpm resolves `@paigasus/ui` through a symlink to
`ts/packages/paigasus-ui`, a path outside `node_modules`, and Next follows symlinks by default. So
this may already work. It is not proven, and client-boundary detection for a package reached this
way is coupled to the same setting.

Measure it before writing components: build the console against a one-component `@paigasus/ui` and
record whether `transpilePackages` is needed. If it is, add it through SMA-502's `next.config`
factory during the rebase (section 4.1), not as a separate edit.

## 7. Navigation injection — AC 1

`@paigasus/ui` imports nothing from `next/*`. The package defines the contract. The application
supplies the implementation.

```ts
export type LinkComponent = ComponentType<{
  href: string;
  className?: string;
  children?: ReactNode;
}>;

export function LinkProvider(props: { link: LinkComponent; children: ReactNode }): ReactElement;
export function useLinkComponent(): LinkComponent;
export const Link: FC<LinkProps>;   // resolves the component from context
```

The context default is a plain `<a>` element. That default is what makes AC 2 work: a component
tree renders in jsdom with no provider and no Next runtime present.

The console wraps its tree in `<LinkProvider link={NextLink}>`. SMA-510 consumes the same contract
for the application shell.

### 7.1 Enforcement, stated accurately

Three controls exist, and they are not equally strong. Naming them precisely matters, because the
first revision of this spec described the weakest one with a mechanism that was simply wrong.

**Control 1 — pnpm's isolated `node_modules`. Real, but incidental.** MEASURED on this branch:
`next` is linked into `ts/apps/paigasus-console/node_modules` and **not** into
`ts/packages/paigasus-ui/node_modules`, so `import … from 'next/link'` inside the package fails to
resolve in `tsc`, in Vitest and in any consumer build. This is genuine protection today. It is
also an accident of the dependency layout: adding `next` to the package's `devDependencies` for
any reason disarms it silently, with no red anywhere.

**Control 2 — a resolver ban in the package's own Vitest config. This is SMA-503's deliverable.**
`vitest.config.ts` registers a Vite plugin whose `resolveId` throws for the id `next` and for any
id starting `next/`. This does not depend on where pnpm happens to place a package. The negative
control is a fixture, `tests/fixtures/imports-next.ts`, which imports `next/link`, and a test that
asserts importing that fixture rejects. Without the fixture the plugin is untested code that could
stop matching and never red.

The first revision of this spec claimed a jsdom environment alone would catch a `next/*` import
"because no Next runtime is present". That is false. jsdom has no bearing on module resolution,
and `next/link` renders an `<a>` against a nullable router context — so the test would have passed
with the ban violated. Control 2 replaces that claim.

**Control 3 — SMA-502's eslint boundary rule. The reviewable one, and not SMA-503's.**

**Type-only imports are banned too.** `verbatimModuleSyntax: true` in `ts/tsconfig.base.json`
makes `import type { Metadata } from 'next'` easy to write, and no runtime control can see it.
Control 2's plugin cannot catch it, because the import is erased before Vite resolves anything.
SMA-503 states the rule; SMA-502's lint rule is what can enforce it. This is a stated gap, not a
solved one.

## 8. The seeded component set

Radix supplies every accessibility-critical primitive, through the single unified `radix-ui`
package rather than ten separate `@radix-ui/react-*` packages.

| Component | Built on | Public |
|---|---|---|
| `Table` | plain semantic `<table>`, styled | yes |
| `Dialog` | Radix `Dialog` | yes |
| `DropdownMenu` | Radix `DropdownMenu` | yes |
| `Combobox` | Radix `Popover` + `cmdk` | yes |
| `Popover`, `Command` | Radix `Popover`, `cmdk` | no — internal to `Combobox` |
| `Label`, `Input`, `Field` | Radix `Label` | yes |
| `EmptyState`, `ErrorState` | plain composition | yes |

**The combobox and `cmdk` (Sven, 2026-09-08).** shadcn's combobox is a Popover wrapping a Command
list, and Command is built on `cmdk` — a third-party library, not Radix. ADR-0021 rule 5 forbids
hand-rolling a combobox, so there is no in-house option. `cmdk` is MIT-licensed.

**`cmdk` shares Radix modules with `radix-ui` today, and a future bump can split them.**

Revision 1 of this section claimed, labelled **MEASURED**, that `radix-ui` "bundles its own
pinned copies" and that "two copies of `@radix-ui/react-dialog` therefore exist in the tree".
**That was false, and it was never measured.** It is corrected here (SMA-503 fix round 2,
item 6), because in this repository MEASURED is the evidence standard and a future session
will act on the label.

**What `ts/pnpm-lock.yaml` actually says.** `grep -n 'react-dialog' ts/pnpm-lock.yaml` returns
five lines: **one** `packages:` resolution (`@radix-ui/react-dialog@1.1.23`), **one** snapshot
of it, and three consumers of that same snapshot — `radix-ui@1.6.7`, `cmdk@1.1.1` and
`@radix-ui/react-alert-dialog@1.1.23`. Exactly **one copy** exists. `radix-ui` does not bundle
the module either: it has no `bundledDependencies` key at all, and its own `package.json`
declares `"@radix-ui/react-dialog": "1.1.23"` as an ordinary dependency.

**The hazard is real but LATENT, and it comes from the two specifier styles.** `radix-ui` pins
its Radix dependencies **exactly** (`1.1.23`, no range operator), while `cmdk` uses a **caret**
(`"@radix-ui/react-dialog": "^1.1.6"`). They deduplicate today only because the exact pin
happens to satisfy the caret. A `radix-ui` bump to a `@radix-ui/react-dialog` outside cmdk's
range — the next major is the obvious case — splits them into two resolutions with no warning
from either package, and the split shows up as a silent runtime failure rather than an install
error.

**The rule therefore STAYS: nothing in this package composes across Radix copies.** A trigger
from one copy does not open content from the other, the failure is silent at runtime, and jsdom
tests may not surface it. `cmdk`'s own Radix modules are reachable only through
`Command.Dialog`, so `Command.Dialog` is **not used**. `Combobox` composes `radix-ui`'s
`Popover` with `cmdk`'s plain `Command`. `Dialog` comes from `radix-ui` only. The ban costs
nothing today and is what makes the latent split a non-event. It is recorded in
`src/components/command.tsx`'s header comment, where the next person to reach for
`Command.Dialog` will read it.

`components.json` is committed and configured for shadcn's monorepo mode, per ADR-0021's
"configured once" instruction. **It does not mean a later component arrives by CLI.** Every
component in this package was hand-written, because the CLI is not usable here: it writes to a
literal `./@/components/` directory, and — independently of that — it adds an unrelated npm
package literally named `cn` to the manifest and the lockfile. Both defects are measured, and
recorded in the package README's "The `shadcn` CLI is not usable in this package".

### 8.1 Rules for a future CLI attempt — normalising output is type-level work

**Nothing in this issue was generated.** This section states the rules that would apply IF a
future task used `shadcn add` as a starting point; it is not a description of what happened
here (corrected in SMA-503 fix round 2, item 6 — revision 1 read as though it were).

The first revision also said generated files get "an SPDX header and Prettier". That
understates it. `ts/tsconfig.base.json` sets `exactOptionalPropertyTypes`,
`verbatimModuleSyntax`, `noUncheckedIndexedAccess` and `isolatedModules`, and typed eslint
rules apply on top. shadcn output does not satisfy the first three. Every copied-in component
would need type-level rework, not only formatting.

This weakens ADR-0021's "manual merges stay cheap" argument, and the spec says so rather than
repeating it. A future upstream merge is a re-normalisation, not a patch application.

**The CLI also installs individual `@radix-ui/react-*` packages at literal versions**, which
conflicts with the unified-`radix-ui` decision and with the catalog policy in
`ts/pnpm-workspace.yaml`. Nothing catches that automatically. The rule is: after any `shadcn
add`, remove the individual packages it added, re-point the imports at `radix-ui`, and re-run
`pnpm install`. That is also the point at which the caret-versus-exact-pin split described
above stops being latent. This is recorded in the package README beside the `@source`
obligation.

## 9. Testing — AC 2 and AC 4

`@paigasus/ui` gets its own `vitest.config.ts` with one jsdom project, plus the section 7.1
resolver ban.

**Vitest configuration.** `globals: true`, because React Testing Library registers its automatic
`cleanup` only when a global `afterEach` exists, and Vitest defaults to `globals: false`. Without
it the DOM accumulates between tests and axe then reports duplicate-id and duplicate-landmark
violations belonging to an earlier test — a flake that reads as a real accessibility failure.
`tests/setup.ts` also carries `import '@testing-library/jest-dom/vitest';`.

**M2 — the jsdom polyfill list.** The expected set is `ResizeObserver`,
`Element.prototype.hasPointerCapture`, `Element.prototype.setPointerCapture`,
`Element.prototype.releasePointerCapture`, and `Element.prototype.scrollIntoView`. Confirm the
actual list by running the tests, and add only what they need. This workspace has no jsdom project
on any Vitest version — the only precedent, `ts/packages/paigasus-kernel/vitest.config.ts`, uses
`environment: 'node'` twice — and the catalog moved to Vitest 5.0.0 at commit `29c03977`. So the
whole jsdom plus React Testing Library stack on Vitest 5 is part of this measurement.

### 9.1 The axe helper

The package uses **`axe-core` directly**, behind a `tests/axe.ts` helper. It does not use
`vitest-axe`, which last published on 2025-01-22 and pulls `chalk`, `redent`, `lodash-es` and
`dom-accessibility-api` for a matcher that is about fifteen lines of code.

**The helper runs against `document.body`, not the container React Testing Library returns.**
Radix `Dialog`, `DropdownMenu` and `Popover` render their content into a portal on `document.body`.
A helper scoped to the render container would assert against a subtree holding only the trigger,
so the three most accessibility-critical components would go green for the wrong reason.

**The helper carries its own negative control.** A deliberately broken fixture — an `<input>` with
no accessible name — must make the helper red. Without that, a helper that silently stopped
running would look identical to a clean pass.

**Rule set:** `wcag2a`, `wcag2aa`, `wcag21a`, `wcag21aa`. Disabled rules are listed in
`tests/axe.ts` with a reason each. The `region` rule is disabled, because it fires on every
isolated component render and has no meaning outside a full page.

### 9.2 A stated limit

axe cannot evaluate the `color-contrast` rule in jsdom, because jsdom performs no layout. AC 4's
axe assertions cover roles, accessible names, ARIA state and labelling. They do **not** cover
contrast. Contrast stays a design responsibility, decided in the token layer. This limit is
written into `ts/packages/paigasus-ui/README.md` so a later reader does not over-read the green.

`eslint-plugin-jsx-a11y` is already configured at `ts/eslint.config.js` and applies to every
`.tsx` file. AC 4's second half needs no new configuration, only clean code.

### 9.3 The package `tsconfig.json` must change, or `ts:lint` reds

`ts/packages/paigasus-ui/tsconfig.json` sets `rootDir: "./src"` and `include: ["src/**/*"]`. The
root eslint config applies typed rules with `projectService: true` to every `**/*.{ts,tsx}`, and
`ts:lint` runs `eslint .` over the whole tree. New files at `tests/*.test.tsx` and
`vitest.config.ts` would be in no program, and the project service errors.

The fix and its precedent are already in this repository at
`ts/packages/paigasus-kernel/tsconfig.json`: `include: ["src/**/*", "tests/**/*",
"vitest.config.ts"]`, with `rootDir` removed because it rejects files outside `src/` (inert under
`noEmit`).

## 10. The `@source` proof — AC 3

### 10.1 Why the test should be honest, and what must be measured

Tailwind v4's documentation states that automatic source detection defaults to the **current
working directory**. Moon runs `next build` from `ts/apps/paigasus-console`, which would place
`ts/packages/paigasus-ui/src` outside the automatic scan and make the `@source` line load-bearing.
Tailwind also excludes `node_modules`, and pnpm resolves `@paigasus/ui` through a `node_modules`
symlink.

**M4 makes this a precondition, not a validation step.** If Tailwind instead roots the scan at the
git repository root — it walks upward to honour `.gitignore` — then `ts/packages/paigasus-ui/src`
is already scanned, the `@source` line is decorative, and AC 3 proves nothing. That outcome needs
a different design, not a fix, so it must be settled before implementation. Record the measured
scan root, the Next 16 build mode, and the actual CSS output path.

### 10.2 Two sentinels

**Sentinel A — the scan.** `src/components/table.tsx` carries the utility
`[--paigasus-ui-source-probe:1]`, which compiles to a custom-property declaration. It is a
dedicated probe rather than a real style, so the assertion cannot pass for the wrong reason. It
ships in production output, where it costs one unused custom property.

**Sentinel B — the `@import`.** Sentinel A proves Tailwind scanned a file. It says nothing about
whether `@import '@paigasus/ui/styles.css'` resolved: that import can fail, the whole token layer
can be absent from the output, and sentinel A still passes. `tokens.css` therefore declares a
custom property `--paigasus-token-probe` that appears nowhere else. M1 is a one-time check;
sentinel B is the continuous one.

### 10.3 The assertion, and where it must not live

**The script lives at `ci/tailwind-source/run.mjs`, at the repository root — never inside
`ts/apps/paigasus-console/`.** This is the sharpest trap in the design. Tailwind's automatic scan
root is the console directory (M4), and Tailwind extracts class candidates from any non-ignored
text file. A script placed under the console that contains the literal
`[--paigasus-ui-source-probe:1]` would make Tailwind generate that utility **from the script
itself**, with the `@source` line deleted. The assertion would pass for exactly the wrong reason,
and the alternative — excluding the script from its own "appears nowhere in the app" check —
reopens the same hole. The reason is repeated in the script's header comment.

The script asserts four things:

1. `--paigasus-ui-source-probe` appears in the current build's CSS.
2. `--paigasus-token-probe` appears in the current build's CSS.
3. Neither string appears anywhere in the console's own sources. The scan set is the WHOLE of
   `ts/apps/paigasus-console/`, walked recursively, minus two excluded directories: `.next`
   (build output) and `node_modules`. An allowlist of `app/**` plus four named root config
   files was an earlier draft and is not what the code does — the walk covers the whole of
   Tailwind's scan root, so a sentinel dropped in any new file or directory under the console
   is caught rather than missed (SMA-503 local review, finding G).
4. At least one CSS file was read. An empty file set fails loudly.

**Resolving "the current build's CSS" — M6.** A glob over `.next/static/css/*.css` is not safe.
Next writes content-hashed CSS, Moon hydrates `outputs: ['.next']` from a tarball on a cache hit,
and CI restores `.moon/cache` across runs. A file from an earlier build sitting beside the current
one satisfies a glob while the current build has lost the probe. Assertion 4 does not close this;
it only catches an empty set. The script therefore tries `.next/app-build-manifest.json` first and
falls back to a recursive walk of `.next/static` otherwise.

**M6, taken (Task 5): the manifest never names the CSS on this build.**
`.next/app-build-manifest.json` does not exist at all under Next 16.3.4/Turbopack — only an
unrelated `.next/build-manifest.json` (no `app-` prefix) exists, and it lists JS chunks, not CSS.
So the fallback walk is what actually runs on every real invocation, and its safety depends
entirely on `.next/static` being removed before the build that feeds it. **The fallback path,
not the manifest path, is therefore load-bearing, and the console's `build` command must be
`rm -rf .next/static && pnpm exec next build`** — the whole `static` directory, not
`.next/static/css` as an earlier draft of this section assumed; Task 1 already measured that CSS
is written under `.next/static/chunks/`, and `rm -rf .next/static` is what actually clears the
stale-chunk hazard while `.next/cache` (Turbopack's persistent cache) is left alone, so builds do
not go cold. See `docs/superpowers/specs/2026-09-08-sma-503-measurements.md`'s M6 section for the
full transcript. Task 6 owns the `moon.yml` edit that applies this.

### 10.4 The script carries its own self-test and negative control

The console's `test` task runs three invocations under an explicit `set -euo pipefail`, in the
pattern this repository already uses for its self-scheduled gates:

```
node ../../../ci/tailwind-source/run.mjs --self-test
node ../../../ci/tailwind-source/run.mjs --negative-control
node ../../../ci/tailwind-source/run.mjs
```

`--self-test` drives the verdict function over synthetic fixtures in a temporary directory.
`--negative-control` asserts the script reports red against a fixture whose CSS lacks the probes.
Moon does not enable errexit for `script:` blocks, which is why the `set -euo pipefail` line is
explicit — the same latent defect this repository documents on several other gates.

**A stated limit.** `ci/affected-graph/ci_targets.py`'s `check_self_scheduled_coverage` scans
`repo:*` tasks only. `paigasus-console-ts:test` is not a `repo:*` task, so none of the seven
registration obligations apply — and equally, no registry pins these three invocation lines.
Deleting the `--negative-control` line reds nothing. This is accepted: the alternative is a new
`repo:*` gate running a full `next build` on every affected pull request.

### 10.5 Proving the assertions bite — two distinct failures

The first revision offered one proof for two different failures. Both are needed.

**Proof 1 — the `@source` line.** Delete it, run the console's `build` and `test`, confirm red,
then restore by reverting the edit. Do not restore by moving a `.bak` file back: a rolled-back
modification time makes the next run reuse the earlier artifact, which produces a failure that
looks real and is not. Note this proof exercises only the non-cached path, because
`app/globals.css` is an input of both tasks, so the build re-runs.

**Proof 2 — the cache path, which proof 1 never reaches.** Remove sentinel A from
`src/components/table.tsx` — a file in the package, not the app. Then confirm with
`moon query tasks --affected` that both console tasks are **selected**, and that the run reds.
Parse that JSON properly: take one target per `tasks[project][task]`. A
`grep -o '"target": "[^"]*"'` counts scheduled upstreams as selections and cannot measure the
distinction this proof turns on.

## 11. Moon and CI wiring

The console's tasks are the whole of AC 3's machinery, so their inputs are stated exhaustively.

**`paigasus-console-ts:build`** — `options.merge: replace`, so it inherits nothing. Its inputs
must be, exhaustively:

- `@group(sources)` — see 11.1
- `tsconfig.json`, `package.json`, `next.config.ts`
- `postcss.config.mjs` — new, and at the console root, so no source group reaches it
- `/ts/tsconfig.base.json`
- `/ts/packages/paigasus-ui/src/**/*`
- `/ts/pnpm-lock.yaml`

**`paigasus-console-ts:test`** — replaced, `deps: ['~:build', 'repo:next-env-drift']`, running
`vitest run --passWithNoTests` and then the three script invocations of 10.4. It keeps the vitest
line so SMA-510 has a place to add real unit tests. Its inputs must be, exhaustively:

- `@group(sources)` — `scripts` is not used; the guard lives at `/ci/tailwind-source/**/*`
- `tsconfig.json`, `package.json`, `next.config.ts`, `postcss.config.mjs`
- `/ci/tailwind-source/**/*`
- `/ts/tsconfig.base.json`
- `/ts/packages/paigasus-ui/src/**/*`
- `/ts/pnpm-lock.yaml`

**`/ts/tsconfig.base.json` is on both lists because `merge: replace` drops it** (SMA-503 fix
round 2, item 7). Revision 1 of this section called these lists exhaustive while omitting it,
along with `tsconfig.json` and `/ci/tailwind-source/**/*` on the `test` list. That file sets
`exactOptionalPropertyTypes`, `verbatimModuleSyntax` and `noUncheckedIndexedAccess`, so editing
it changes what `next build` accepts — and neither task was re-keyed by it. This is the same
defect class as the dropped lockfile input in 11.1, from the same cause.

**`paigasus-console-ts:typecheck`** also gains `/ts/packages/paigasus-ui/src/**/*`. A type error
introduced in the package would otherwise leave its cache key unchanged. `next build` typechecks,
so the coverage exists in practice — but accidentally, and this makes it deliberate. This task
MERGES rather than replaces, so it keeps every inherited input, `/ts/tsconfig.base.json`
included — verified with `moon query projects --id paigasus-console-ts`, which reports the file
on all three tasks' resolved `inputFiles`.

**`ts/packages/paigasus-ui/moon.yml`** adds `vitest.config.ts` to its `test` task's `inputs`. That
file matches neither `src/**/*` nor `tests/**/*`.

`ts/apps/paigasus-console/moon.yml` also gains `dependsOn: ['paigasus-ui-ts']`. **This confers no
affectedness.** `dependsOn` schedules an upstream and never selects a downstream. It is added as
graph documentation, and because `moon query projects --affected` follows it. The `inputs` entries
above are what actually make the console tasks affected by a package change.

### 11.1 Two pre-existing defects this issue must fix

**The console's source group is dead.** MEASURED with `moon query projects --id
paigasus-console-ts`: the resolved `sources` group is exactly `ts/apps/paigasus-console/src/**/*`,
and that directory does not exist. The console's code lives in `app/`. So today **no** console
task — `build`, `typecheck` or `test` — is selected when `app/page.tsx` or `app/layout.tsx`
changes. `repo:input-liveness` cannot see this: `ci/affected-graph/task_inputs.py` keys its scan
to the `repo` project by exact project id. AC 3 rests on this being correct, so the fix is in
scope. The project gains an `app/**/*` entry in its `sources` file group. **M3** verifies the
resolved set afterwards rather than trusting that Moon merges as documented.

**The console's build does not key on the lockfile.** `options.merge: replace` drops the
`/ts/pnpm-lock.yaml` input that `.moon/tasks/typescript-project.yml` supplies. This repository
already records the consequence, in `moon.yml`'s `next-env-drift` comment: *"the lockfile is
absent from `paigasus-console-ts:build`'s own inputs, so a Next upgrade never re-keyed that
task."* Revision 1 of this spec asserted the opposite in its risks section, and was wrong. A
Dependabot bump of `tailwindcss`, `@tailwindcss/postcss` or `next` would change the installed
toolchain, leave the cache key unchanged, and let the assertion read CSS produced by a different
Tailwind version.

### 11.2 Ordering against `repo:next-env-drift`

`repo:next-env-drift` declares `deps: ['paigasus-console-ts:build']` for one stated reason:
`next typegen` writes into `.next`, which is the build's declared output, and letting the two run
concurrently would race. This design adds a **third** `.next` consumer,
`paigasus-console-ts:test`, which depends on `build` but is unordered against `next-env-drift`.
Both would then run concurrently over the same directory.

The console `test` task therefore also declares `deps: ['repo:next-env-drift']`. If measurement
shows `next typegen` writes nothing under the CSS path the assertion reads, record that instead
and drop the dep — but do not leave the ordering unstated.

### 11.3 One continuous guard on the input that makes AC 3 real

Section 10.5's proofs are one-time. Removing `/ts/packages/paigasus-ui/src/**/*` from the console's
inputs later would red nothing, and that input is the only thing making the proof real.

`ci/affected-graph/run.sh` gains **two** `run_task_case_ci` cases, each keyed on a file under
`ts/packages/paigasus-ui/src/`, each with a strict-equality expected task set. These are
reachable because `repo:affected-smoke` already lists `ts/packages/*/moon.yml` and
`ts/apps/*/moon.yml` among its own inputs, so the pull request that removes the input also
selects the gate.

**Two anchors, not one** (SMA-503 fix round 2, item 4). `ui->console` anchors on
`src/styles/tokens.css` and `ui-components->console` on `src/components/table.tsx`. One anchor
proves only that ONE PATH inside `/ts/packages/paigasus-ui/src/**/*` selects the console:
narrowing either console task's input to `src/styles/**/*` leaves a single styles-anchored case
green while a component edit silently stops selecting `paigasus-console-ts:test` — and
`src/components/table.tsx` is where sentinel A lives, so that narrowing would switch off
precisely the invalidation assertion 1 rests on. Anchors on opposite sides of the glob make the
case a proof of the glob's WIDTH. Both were measured to bite: narrowing to `src/styles/**/*`
reds only the components case, narrowing to `src/components/**/*` reds only the styles case.

Like every strict-equality case in that file, it must be re-baselined whenever the expected set
legitimately changes.

## 12. Dependencies

New entries in `ts/pnpm-workspace.yaml`'s `catalog:`. This workspace catalogs single-consumer
dependencies too, so every entry below is a catalog entry — that is also what makes Dependabot
bump them centrally. Versions are current as of 2026-09-08.

**`@paigasus/ui` runtime:**

| Package | Version | License |
|---|---|---|
| `radix-ui` | ^1.6.7 | MIT |
| `cmdk` | ^1.1.1 | MIT |
| `clsx` | ^2.1.1 | MIT |
| `tailwind-merge` | ^3.6.0 | MIT |
| `tw-animate-css` | ^1.4.0 | MIT |

**`class-variance-authority` was removed** (SMA-503 fix round 2, item 8). It was declared as a
runtime dependency and imported by nothing — no component uses `cva`, because none of the seven
has variants. Its `ts/pnpm-workspace.yaml` catalog entry is gone with it, so the workspace no
longer carries a catalog line, a manifest entry and a lockfile resolution for a package with no
call site. Re-add both if a component ever grows a variant API.

**`tw-animate-css` is a RUNTIME dependency, not a dev one.** `src/styles/tokens.css` `@import`s
it, and that file is a published `exports` entry (`./styles.css`), so a consumer resolving the
token layer resolves `tw-animate-css` through it. It was declared under `devDependencies` and
worked only because pnpm installs a workspace package's devDependencies — which stops being
true the moment this package is consumed as anything other than a workspace member. Its own
import is what decides this, so it now sits where the table above already put it.

**`@paigasus/ui` peer:** `tailwindcss` ^4.3.3, beside the existing `react` and `react-dom`.

**`@paigasus/ui` dev:** `react` and `react-dom` — **not only the peers**. The package's own Vitest
run needs them resolvable at runtime, and whether pnpm auto-installs a `catalog:` peer is not
proven here. Plus `jsdom` ^30.0.1, `@testing-library/react` ^16.3.3, `@testing-library/dom`
^10.4.1, `@testing-library/user-event` ^14.6.7, `@testing-library/jest-dom` ^7.0.1, `axe-core`
^4.13.0 (MPL-2.0), `vitest`, `@types/react`, `@types/react-dom`.

**`@paigasus/console`:** `@paigasus/ui` at `workspace:*`, plus `tailwindcss` and
`@tailwindcss/postcss`, and a new `postcss.config.mjs`.

**`tw-animate-css`, stated correctly.** It supplies `animate-in`, `fade-in-*`, `zoom-in-*` and
`slide-in-from-*`. It does **not** supply `data-[state=open]:`, which is a core Tailwind v4
arbitrary variant — revision 1 claimed otherwise. The alternative is copying its keyframes into
`tokens.css`, which removes a supply-chain edge on a small single-maintainer package and matches
the copy-in philosophy ADR-0021 already accepted for components. It is rejected for now because
the utility names are what shadcn's generated components reference, and diverging from them raises
the normalisation cost of section 8.1 on every future `shadcn add`. Revisit if the package goes
unmaintained.

**`cn` uses plain `tailwind-merge`.** It does not know about custom `@theme` scales, so it cannot
always deduplicate utilities generated from them. This is acceptable while the token layer maps
onto Tailwind's own scale names. If a genuinely new scale is added, `extendTailwindMerge` becomes
necessary, and that is the trigger to revisit.

**There is no licence gate for `ts/`.** `rs/deny.toml` is Rust-only, and `repo:osv` scans
vulnerabilities, not licences. The licence column above is documentation. It is not asserted by
anything, and this spec does not imply that it is.

## 13. SPDX headers on new file types

This change adds the repository's first `.css` files, plus `.mjs` and `.json`.

| Type | Header |
|---|---|
| `.ts`, `.tsx`, `.mjs` | `// SPDX-License-Identifier: Apache-2.0` |
| `.css` | `/* SPDX-License-Identifier: Apache-2.0 */` |
| `components.json` | none — JSON admits no comment. Stated here so its absence reads as deliberate. |

For `.tsx` files carrying `'use client'`, the SPDX comment precedes the directive, subject to M7.

## 14. Acceptance criteria mapped

| AC | How it is met | Where |
|---|---|---|
| 1 — no `next/*` import | the injected-navigation contract, plus a Vite resolver ban with a fixture-based negative control. pnpm isolation is incidental support. The lint rule is SMA-502's. Type-only imports are a stated gap. | §7, `vitest.config.ts`, `tests/fixtures/imports-next.ts` |
| 2 — renders in jsdom, no Next | one jsdom Vitest project, polyfills confirmed by M2 | §9, `vitest.config.ts` |
| 3 — `@source` proven by a build | two sentinels, a four-assertion script outside the scan root with a self-test and a negative control, both console tasks keyed on the package source, and a continuous affected-graph case | §10, §11 |
| 4 — axe clean, `jsx-a11y` clean | `axe-core` over `document.body`, with its own negative control and a named rule set; the existing eslint plugin | §9.1, `ts/eslint.config.js` |

## 15. The package README

`ts/packages/paigasus-ui/README.md` records three things a later reader would otherwise get wrong:

1. The axe contrast limit of section 9.2 — so a green is not over-read.
2. **The per-consumer `@source` obligation.** The console's green says nothing about a second zone
   app. Every new consumer must add its own `@source` line and its own probe assertion, or its
   build silently drops the package's classes.
3. The post-`shadcn add` normalisation rule of section 8.1.

## 16. Out of scope

- The eslint boundary rule. It belongs to SMA-502.
- The header, navigation and cross-zone linking. Those belong to SMA-510. This issue ships only
  the contract they consume.
- A `dist` build. The whole `ts/` workspace still exports source, and changing that is coupled to
  flipping `private: false`.
- A theme-toggle UI, and a data grid.

`next.config.ts` is **conditionally** out of scope: only M5's outcome decides, and if
`transpilePackages` is required it goes through SMA-502's factory during the rebase (§4.1).

## 17. Review findings — what changed, and what was rejected

An adversarial review of revision 1 returned NEEDS REWORK with six blockers. Revision 2 folds in
every finding except one.

**Blockers, all accepted.** The assertion script sat inside Tailwind's own scan root and would
have generated the probe it greps for (§10.3). The console build does not key on the lockfile, and
revision 1 asserted the opposite (§11.1). Nothing keyed on the assertion script itself (§11).
Tailwind's scan root was stated as fact and is now a precondition (M4). The `no-next-runtime`
control was described by a mechanism that does not exist and is replaced by a resolver ban with a
negative control (§7.1). `'use client'` and `transpilePackages` were unaddressed (§6, M5).

**Majors, all accepted.** A continuous guard on the ui-source input (§11.3); a second proof
reaching the cache path (§10.5); stale CSS in a hydrated `.next` (§10.3, M6); a second sentinel for
the `@import` (§10.2); ordering against `repo:next-env-drift` (§11.2); the SMA-502 `outputs`
coupling (§4.1); the slip rule (§4); the package `tsconfig.json` (§9.3); the missing
`globals.css` import (§5.2); `postcss.config.mjs` in no input set (§11); the duplicate Radix
copies under `cmdk` (§8); axe against `document.body` with a negative control (§9.1); React
Testing Library cleanup (§9); `react`/`react-dom` as devDependencies (§12); the `dark:` variant
conflict (§5.1); normalisation as type-level work (§8.1); the console `typecheck` key (§11).

**Minors, all accepted.** The `tw-animate-css` correction (§12); `Popover` and `Command` marked
internal (§8); catalog and versions for the dev dependencies (§12); Vitest 5 and jsdom folded into
M2; `dependsOn` documented as conferring no affectedness (§11); the `test` task keeping its vitest
line (§11); the exact scan set for assertion 3 (§10.3); the per-consumer obligation (§15); SPDX on
the new file types (§13); `tailwind-merge` and custom scales (§12).

**Rejected — one finding.** The review argued this is not one implementable unit and proposed
splitting it: part A carrying the scaffold, the harness, one probe-bearing component and every
measurement; part B carrying the remaining eight components. **Sven decided on 2026-09-08 to keep
SMA-503 as a single issue and a single pull request.** The review's reasoning stands on its own
terms — the risk really is concentrated in the part that needs one component — and the accepted
cost is a large review surface in which the measurement work and the routine component work
compete for attention.

## 18. Risks

**A wide `moon ci` selection.** New catalog entries change `ts/pnpm-lock.yaml`, which is an input
to every `ts` task except the console's `build` until §11.1's fix lands. The next `moon ci`
therefore selects the whole TypeScript graph. This is expected cost, not a fault.

**Copy-in has no upstream fixes.** ADR-0021 accepts this. Every vendored component is one this
repository now maintains, including its accessibility behaviour. Section 8.1 records that the
merge cost is higher than the ADR assumed.

**`ts:fmt` is a separate whole-tree gate**, decoupled from `ts:lint` and `tsc`. This change adds
the first `.css` files in the repository, and Prettier formats CSS. Run `moon run ts:fmt`.

**Three preconditions can invalidate the design, not merely the code.** M1, M4 and M5 are taken
before implementation for that reason.
