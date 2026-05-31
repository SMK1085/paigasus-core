# SMA-396 — Config-only TS packages as a first-class scaffold shape

**Status:** Design approved
**Date:** 2026-05-31
**Linear:** SMA-396
**Branch:** `feature/sma-396-ts-make-config-only-ts-packages-a-first-class-shape`
**Related:** SMA-395 (special-cased `commitlint-config-ts`, flagged this follow-up), SMA-394 (ts-root exclude + app-build invariant), SMA-381 (scaffold field-order / `layer` semantics)

## Problem

The per-project `build` and `typecheck` tasks inherited from `.moon/tasks/typescript.yml` are
both `pnpm exec tsc -p tsconfig.json --noEmit` (lines 23 and 54). This bakes in the assumption
that **every** `language: typescript` project is a `tsc` compilation unit with a `tsconfig.json`.
A **config-only** package violates that: it is CommonJS/JSON only (no `.ts`, no `tsconfig.json`),
so the inherited task fails `TS5058` ("The specified path does not exist: 'tsconfig.json'") on a
full `moon run :build` / `moon ci :build`.

SMA-395 fixed the one such package that exists today (`ts/packages/commitlint-config`,
`commitlint-config-ts`) with a per-project opt-out:

```yaml
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

But this is a **class, not a one-off**. A future `@paigasus/eslint-config`,
`@paigasus/prettier-config`, or a shared `tsconfig` package would each be config-only, hit the
identical `TS5058`, and need the same block — rediscovered per package. SMA-394 does **not** cover
this: it dropped the *root* aggregators and excluded tasks on the `ts` **root** project, leaving
the *per-project* inherited `tsc -p tsconfig.json` task — the actual failure — unchanged.

## Why option (a), not (b)

The issue offers two directions (recorded decision: **(a)**):

- **(a) Scaffold archetype + documented convention.** Treat "config-only TS package" as a named
  shape: add a `config` archetype to the TS scaffold template that emits the
  `inheritedTasks.exclude: ['build', 'typecheck']` block, and document the shape in CONTRIBUTING.
- **(b) Harden the inherited task** to no-op when no `tsconfig.json` is present (one fix for all).

(b) is rejected. It would replace the canonical `tsc -p tsconfig.json --noEmit` with a wrapper
that runs `tsc` only if `tsconfig.json` exists — but **file presence alone cannot distinguish** a
legitimately config-only package (skip is correct) from a real TS package that accidentally lost
its `tsconfig.json` (must fail). So (b) would silently stop type-checking a genuine regression. It
also adds wrapper-script indirection to the hot path of *every* TS project. (a) is honest, opt-in,
lower-risk, and consistent with how the repo already special-cases the `app` shape via the
template (SMA-394's app-build invariant). The SMA-395 spec rejected dishonest no-ops in the same
spirit. A hybrid — (a) plus a CI guard that catches *hand-written* config packages missing the
block — is noted as a possible future, but is YAGNI now with a single config package.

## The `layer` is `library`, not `configuration`

CONTRIBUTING's documented `layer` semantics reserve `configuration` for the **workspace-root
project that aggregates child projects** (e.g. `py/moon.yml`, `ts/moon.yml`). A shared config
*package* like `commitlint-config` is importable/published code (`@paigasus/commitlint-config`),
which the same doc classifies as `library`. So the `config` archetype keeps `layer: library`,
matching the existing `commitlint-config-ts`. **No `layer` is reclassified anywhere.**

## Decision

### A. Scaffold template — `.moon/templates/typescript/`

**`template.yml`** — extend the `archetype` enum and document the three shapes:

```yaml
variables:
  name:
    type: 'string'
    default: ''
    required: true
    prompt: 'Project name (e.g. sdk)?'
  archetype:
    type: 'enum'
    values: ['library', 'app', 'config']
    default: 'library'
    prompt: 'Archetype?'
```

The `description` gains a sentence on the `config` archetype: a config-only package (CJS/JSON, no
`tsconfig.json`) that excludes the inherited `build`/`typecheck` so it does not fail `TS5058`.

**`moon.yml`** — add a `config` branch. `config` falls through to `layer: library` (the existing
`{% else %}`), so only the rendered body differs from `library`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-ts'
layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
language: 'typescript'
{%- if archetype == "app" %}
# App-build invariant (SMA-394): an app that emits a build artifact MUST define its own
# `build` task with `outputs:`. The ts root excludes the inherited build/typecheck, so an
# app that only inherited the default would run `tsc --noEmit` and silently emit nothing.
tasks:
  build:
    command: 'pnpm exec next build'
    # next.config.{ts,js,mjs} are all supported by Next; list each so the config
    # file edits invalidate cache regardless of which extension a future project uses.
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json', 'next.config.ts', 'next.config.js', 'next.config.mjs']
    outputs: ['.next']
    options:
      merge: replace
{%- elif archetype == "config" %}
# Config-only TS package (CJS/JSON, no tsconfig.json — e.g. an eslint/prettier/commitlint
# config). Stays language: typescript so lint/fmt/test still attach; excludes the inherited
# per-project build/typecheck (.moon/tasks/typescript.yml runs `tsc -p tsconfig.json --noEmit`,
# which fails TS5058 with no tsconfig.json). See CONTRIBUTING "Moon project files". (SMA-396)
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
{%- endif %}
```

Field order stays `$schema`, `id`, `layer`, `language`, … with `workspace` trailing (CONTRIBUTING
rule; matches `commitlint-config-ts` and the `ts`/`py` roots).

### B. Documentation — `CONTRIBUTING.md`

Add a **"Config-only TS packages:"** paragraph immediately after the existing app-build-invariant
paragraph (the one ending at the `archetype app` sentence, ~line 164), in parallel structure:

> **Config-only TS packages:** a TypeScript *package* that ships no `tsconfig.json` (a CommonJS/
> JSON config such as a shared `eslint`/`prettier`/`commitlint` config — `commitlint-config` is
> the one today) MUST exclude the inherited per-project `build`/`typecheck`:
> `workspace.inheritedTasks.exclude: ['build', 'typecheck']`. Those tasks run
> `tsc -p tsconfig.json --noEmit`, which fails `TS5058` with no `tsconfig.json`. It stays
> `language: typescript` (so `lint`/`fmt`/`test` still attach) and `layer: library` (importable/
> published code). The TypeScript scaffold (`.moon/templates/typescript/`, archetype `config`)
> emits this block for you.

The Notion Development Guidelines should mirror this (external — flagged, not edited here).

### C. Existing `commitlint-config-ts` — realign comment

`ts/packages/commitlint-config/moon.yml` already carries the correct `exclude` block. Update only
its comment: it currently says *"First use of this field in the repo; SMA-394 will later apply the
same field to the `ts` root."* — both now historical. Reword to mark it as the reference instance
of the documented config-only shape (point at CONTRIBUTING / SMA-396). **Comment-only; the
`exclude` block and all other fields are unchanged.**

### D. Verification — prove the AC with a throwaway generate

The load-bearing AC is *"a second config-only package added after this issue does not fail a full
`moon ci :build` / `:typecheck` out of the box."* Demonstrate it by generating a throwaway config
package from the new archetype, asserting its resolved task list, then removing it (no cruft left
in the tree). See the Verification plan.

## What deliberately stays

- The inherited `lint`/`fmt`/`test` on config-only packages (they pass on CJS/JSON; only
  `build`/`typecheck` are the `TS5058` problem). `language: typescript` is kept for exactly this.
- The `library` and `app` archetypes — unchanged in behavior; only the enum and description grow.

## Alternatives considered

- **(b) Harden the inherited `build`/`typecheck` task.** Rejected — see "Why option (a), not (b)":
  file-presence can't distinguish config-only from a regressed TS package, so it risks silently
  skipping a real type-check; plus indirection on every TS task.
- **Hybrid (a) + a CI/Moon guard** that fails fast when a `language: typescript` project has no
  `tsconfig.json` and no exclude. Deferred — YAGNI with one config package; revisit if config-only
  packages proliferate or a hand-written one slips the convention.
- **A separate `language: javascript` for config packages.** Rejected (same as SMA-395): the
  TS task file is scoped `inheritedBy.languages: ['typescript']`, so flipping language drops the
  wanted `lint`/`fmt`/`test` and changes affected-graph language semantics.

## Out of scope / non-goals

- **Adding a real second config package** (`eslint-config`, `prettier-config`, …). None is needed
  yet (YAGNI). The throwaway-generate verification proves the scaffold without leaving one behind.
- **The Notion Development Guidelines edit** — external; mirror the CONTRIBUTING note there
  manually.
- **Any `layer` reclassification** — `config` = `library` per the repo's documented semantics.

## Acceptance criteria

- [ ] The decision is recorded as **(a)** (this spec).
- [ ] `.moon/templates/typescript/` gains a `config` archetype that emits the
      `inheritedTasks.exclude: ['build', 'typecheck']` block (+ comment); `template.yml` documents
      it.
- [ ] The config-only convention is documented in `CONTRIBUTING.md`.
- [ ] A config-only package generated from the `config` archetype has **no** `build`/`typecheck`
      task and does **not** fail a full `moon ci :build` / `:typecheck` (`TS5058`-free) out of the
      box.
- [ ] `commitlint-config-ts` still resolves with no `build`/`typecheck` and keeps `lint`/`fmt`/
      `test` (no regression from the comment realign).

## Verification plan

`moon` is proto-managed; if a shell reports `moon: command not found`, prefix with
`export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`.

1. **Generate a throwaway config package from the new archetype:**
   ```bash
   moon generate typescript ts/packages/scratch-config -- --name scratch-config --archetype config
   cat ts/packages/scratch-config/moon.yml
   ```
   Expect: a `moon.yml` with `layer: library`, `language: typescript`, and the
   `workspace.inheritedTasks.exclude: ['build', 'typecheck']` block. (Add a minimal
   `package.json` with `"name": "@paigasus/scratch-config"` if Moon/pnpm needs the project to
   resolve.)
2. **Resolved task list omits build/typecheck:**
   ```bash
   moon project scratch-config-ts | grep -iE 'build|typecheck' || echo "no build/typecheck ✓"
   ```
3. **Full graph is TS5058-free with the new package present:**
   ```bash
   moon run :build :typecheck 2>&1 | grep -iE 'TS5058|failed' || echo "no TS5058, 0 failed ✓"
   ```
4. **Remove the throwaway package — leave no cruft:**
   ```bash
   rm -rf ts/packages/scratch-config
   git status --short ts/   # expect empty
   ```
5. **commitlint-config-ts unchanged in behavior:**
   ```bash
   moon project commitlint-config-ts | grep -iE 'build|typecheck' || echo "still excluded ✓"
   ```
   Expect: no `build`/`typecheck`; `lint`/`fmt`/`test` still present.

## Files touched

- `.moon/templates/typescript/template.yml` — add `config` to the `archetype` enum; document it
  in `description`.
- `.moon/templates/typescript/moon.yml` — add the `{% elif archetype == "config" %}` branch
  emitting the exclude block + comment.
- `CONTRIBUTING.md` — add the "Config-only TS packages" paragraph after the app-build invariant.
- `ts/packages/commitlint-config/moon.yml` — comment realign only (exclude block unchanged).
