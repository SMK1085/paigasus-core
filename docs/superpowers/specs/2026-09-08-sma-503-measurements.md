# SMA-503 — `@paigasus/ui` foundation measurements

Gathered 2026-09-08 on branch `feature/sma-503-ts-paigasus-ui-tailwind-radix`, worktree
`.claude/worktrees/sma-503`, against `fd3ecf6c` plus the uncommitted Task 1 scaffold.

The measurement table is defined in
`2026-09-08-sma-503-paigasus-ui-foundation-design.md` §4. This file records the exact
command, the verbatim output, and the verdict for each one.

**All seven of M1–M7 are taken.** Taken in Task 1: M1, M4, M5, M7. Taken in Task 5: M6. Taken
in Task 6: M3. Taken in Task 9: M2 (initial), updated in Task 10 (`ResizeObserver` added).
Revision 1 of this line said M3 and M6 were "not yet taken" while both already carried verdicts
further down this same file; corrected in SMA-503 fix round 2, item 5.

## Environment

| Item | Value |
|---|---|
| node | `v24.16.0` |
| pnpm | `11.3.0` |
| next | `16.3.4` (Turbopack) |
| tailwindcss | `4.3.3` |
| `@tailwindcss/postcss` | `4.3.3` |
| host | darwin 25.6.0 (arm64) |

---

## M1 — does `@import '@paigasus/ui/styles.css'` resolve under pnpm's symlinked `node_modules`?

Design §5. `ts/apps/paigasus-console/app/globals.css` carries the bare specifier
`@import '@paigasus/ui/styles.css'`, which resolves through the package's
`exports["./styles.css"]` entry to `src/styles/tokens.css`.

**Command**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build
```

**Output (verbatim)**

```
▲ Next.js 16.3.4 (Turbopack)
✓ Running next.config.ts took 2.0s

  Creating an optimized production build ...
✓ Compiled successfully in 3.5s
  Running TypeScript ...
  Finished TypeScript in 2.2s ...
  Collecting page data using 4 workers ...
  Generating static pages using 4 workers (0/3) ...
✓ Generating static pages using 4 workers (3/3) in 679ms
  Finalizing page optimization ...

Route (app)
┌ ○ /
└ ○ /_not-found


○  (Static)  prerendered as static content

rc=0
```

The build alone does not prove the `@import` resolved — Tailwind could have silently
dropped it. Sentinel B is what proves it. It lives only in `tokens.css`:

```bash
grep -o -- '[^{};]*--paigasus-token-probe[^{};]*' ts/apps/paigasus-console/.next/static/chunks/*.css
```

```
--paigasus-token-probe:1
```

**Verdict: RESOLVES.** The bare specifier works. **No fallback to the relative path
`../../../packages/paigasus-ui/src/styles/tokens.css` was needed**, so design §5.2 stands
unchanged.

---

## M4 — Tailwind's scan root, Next's build mode, and where the CSS is written

Design §10.1, §10.3. This is the measurement that could have invalidated the design: if
Tailwind's automatic source detection already reached `ts/packages/paigasus-ui/src`, the
`@source` line in `globals.css` would be decorative and AC 3 would prove nothing.

### M4a — build mode

From the M1 output above: `▲ Next.js 16.3.4 (Turbopack)`.

**Verdict: Turbopack**, not webpack. Next 16 uses Turbopack for `next build` with the
unmodified `next.config.ts`.

### M4b — where the CSS is written

**Command**

```bash
find ts/apps/paigasus-console/.next -name '*.css' -not -path '*/cache/*' | sort
```

**Output (verbatim)**

```
ts/apps/paigasus-console/.next/static/chunks/1islji-dt854e.css
```

**Verdict: `.next/static/chunks/`, NOT `.next/static/css/`.** The task brief's grep path
`ts/apps/paigasus-console/.next/static/css/*.css` does not exist under Turbopack. Run
verbatim, it fails at glob expansion rather than at the assertion:

```bash
grep -c -- '--paigasus-ui-source-probe' ts/apps/paigasus-console/.next/static/css/*.css
```

```
(eval):1: no matches found: ts/apps/paigasus-console/.next/static/css/*.css
```

Exactly **one** CSS file is written, and its basename is content-hashed: the same source
tree produced `1islji-dt854e.css` with the `@source` line and `381sj-zkskhir.css` without
it. **`ci/tailwind-source/run.mjs` must therefore discover the CSS files by walking
`.next/static/` for `*.css` and must assert it found at least one** — a hardcoded path or a
bare shell glob is a gate that can pass vacuously or abort at expansion instead of
reporting red.

### M4c — is the `@source` line load-bearing? (the gate)

Both sentinels present **with** the `@source` line:

```bash
grep -c -- '--paigasus-ui-source-probe' ts/apps/paigasus-console/.next/static/chunks/*.css
grep -c -- '--paigasus-token-probe'     ts/apps/paigasus-console/.next/static/chunks/*.css
```

```
1
1
```

The generated declaration, verbatim:

```
.\[--paigasus-ui-source-probe\:1\]
--paigasus-ui-source-probe:1
```

The `@source '../../../packages/paigasus-ui/src';` line was then **deleted** from
`globals.css`, `.next` was removed, and the build re-run (`rc=0`). Re-grep:

```
--- source-probe ---
0
rc=1
--- token-probe ---
1
rc=0
```

Corroborating evidence — every utility that exists only inside the package disappeared,
while an app-owned utility survived:

```
--- no-@source build: package-only utilities ---
caption-bottom: 0
overflow-x-auto: 0
align-middle: 0
--- app-owned utility (page.tsx) ---
text-2xl: 1
```

The `@source` line was then restored and the build re-run from a removed `.next`; both
sentinels return to `1`.

**Verdict: LOAD-BEARING. The design holds.** Tailwind v4's automatic source detection
roots at the console app's own directory and does **not** reach
`ts/packages/paigasus-ui/src`. Without the `@source` line the package's classes — the probe
included — are silently absent from a production build, and the build still exits 0. AC 3
therefore asserts something real, and Task 2 onward may proceed.

Note that sentinel B stayed at `1` throughout. The two sentinels are genuinely independent:
B proves the `@import` resolved, A proves the scan reached the source. A gate asserting only
one of them would pass for the wrong reason.

---

## M5 — is `transpilePackages: ['@paigasus/ui']` needed?

Design §6. pnpm resolves `@paigasus/ui` through a `node_modules` symlink to a path outside
`node_modules`, and Next does not compile TypeScript inside `node_modules` by default.

The M1/M4 builds ran against the **unmodified** `ts/apps/paigasus-console/next.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { NextConfig } from 'next';

const nextConfig: NextConfig = {};

export default nextConfig;
```

```bash
git diff --stat -- ts/apps/paigasus-console/next.config.ts
```

produces no output — the file is untouched. The build compiled `table.tsx` (a `.tsx`
source file under `ts/packages/paigasus-ui/src/`) and both `next build` runs exited 0 with
no parse or loader error naming any file under that directory. `app/page.tsx` imports the
components from the barrel and the page prerendered successfully.

**Verdict: NOT NEEDED.** Next 16 / Turbopack compiles the package's TypeScript through the
pnpm symlink without `transpilePackages`. No temporary edit to `next.config.ts` was made,
so nothing has to be migrated into SMA-502's `next.config` factory during the rebase.

---

## M7 — does SWC accept an SPDX line comment above `'use client'`?

Design §6. `ts/packages/paigasus-ui/src/components/table.tsx` opens:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';
```

Both `next build` runs compiled that file and exited 0. Searching both captured build logs
for a directive warning finds nothing:

```bash
grep -in 'use client' /tmp/next-build-1.log /tmp/next-build-3.log       # rc=1, no output
grep -in -E 'warn|directive|ignored' /tmp/next-build-1.log /tmp/next-build-3.log  # rc=1, no output
```

Absence of a warning is weak evidence on its own, so the client boundary was confirmed
positively — the file appears in the page's client reference manifest, which only happens
when the directive was honoured:

```bash
grep -o 'paigasus-ui[^"]*table[^"]*' ts/apps/paigasus-console/.next/server/app/page_client-reference-manifest.js
```

```
paigasus-ui/src/components/table.tsx
paigasus-ui_src_components_table_tsx_1nz7ycm._.js
```

**Verdict: ACCEPTED.** A leading SPDX line comment does not suppress the `'use client'`
directive. The repo's SPDX convention and the directive convention can both be honoured in
their documented order; no component file needs the comment moved below the directive.

---

## Incidental — `tailwind-merge` does not strip the probe (controller ruling R2)

`table.tsx` routes the sentinel through `cn`, which runs `twMerge`. Tailwind's scanner reads
source text, so the CSS is generated regardless — but the runtime `className` was checked
anyway, in the prerendered HTML:

```bash
grep -o 'class="[^"]*paigasus-ui-source-probe[^"]*"' ts/apps/paigasus-console/.next/server/app/index.html
```

```
class="[--paigasus-ui-source-probe:1] w-full caption-bottom text-sm"
```

`twMerge` passes the unrecognised arbitrary custom-property utility through unchanged. The
probe survives to the DOM, so ruling R2's fallback (moving the literal into the JSX
`className`) is **not** required.

---

## M6 — does `.next/app-build-manifest.json` name the current build's CSS files?

Taken in Task 5 (`ci/tailwind-source/run.mjs`), against `ts/apps/paigasus-console`, Next
16.3.4, Turbopack. `.next/static` was removed and the app rebuilt from a clean slate first
(`rm -rf .next/static && pnpm exec next build`), so this reads a genuinely current build.

```bash
ls -la ts/apps/paigasus-console/.next/app-build-manifest.json
```

```
ls: ts/apps/paigasus-console/.next/app-build-manifest.json: No such file or directory
```

**The file does not exist at all** on this Turbopack build. A different, unrelated manifest
with a similar name — `.next/build-manifest.json` (no `app-` prefix) — does exist, but it
lists no CSS entries either; its `pages` key holds only `"/_app": []`, and its `polyfillFiles`
/ `rootMainFiles` name JS chunks only:

```bash
cat ts/apps/paigasus-console/.next/build-manifest.json
```

```json
{
  "pages": { "/_app": [] },
  "devFiles": [],
  "polyfillFiles": ["static/chunks/0cz1d0mv5g_q7.js"],
  "lowPriorityFiles": [
    "static/9vsEQWdyjnmmf4290hkCH/_buildManifest.js",
    "static/9vsEQWdyjnmmf4290hkCH/_ssgManifest.js",
    "static/9vsEQWdyjnmmf4290hkCH/_clientMiddlewareManifest.js"
  ],
  "rootMainFiles": [
    "static/chunks/0b2h9zpqqv42-.js",
    "static/chunks/1mmj1r9civq93.js",
    "static/chunks/3ufb2-57w81-y.js",
    "static/chunks/3vwy08us48hdy.js",
    "static/chunks/turbopack-33o88_rv57fyk.js"
  ],
  "rootMainFilesTree": {},
  "pagesChunkGroupBootstrapParams": {},
  "chunkLoadingGlobal": "TURBOPACK"
}
```

Task 1's finding holds: the CSS is written under `.next/static/chunks/`, not
`.next/static/css/`. Exactly one file on this build,
`.next/static/chunks/0bu_ozx_n57eq.css`, and it carries both sentinels.

**Verdict: the manifest resolver never fires on this build.** `run.mjs`'s
`currentBuildCssFiles` falls through to the recursive `walkCss(.next/static)` fallback on
every real run against this app. That fallback cannot, by itself, distinguish a file the
current build wrote from a stale one a cache hydration left behind — so its safety depends
entirely on `.next/static` being removed before every build.

**Consequence for Task 6:** `paigasus-console-ts:build`'s command must be
`rm -rf .next/static && pnpm exec next build`, not bare `pnpm exec next build`. This has been
applied to spec §10.3's build-command note; Task 6 still owns the actual `moon.yml` edit.

---

## M3 — does Moon merge a project's `fileGroups` with the inherited ones, or override them?

Taken in Task 6, against `paigasus-console-ts` after adding the project-level `sources:
['app/**/*']` file group (the inherited group from `.moon/tasks/typescript-project.yml` is
`src/**/*`, and this app has no `src/`).

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --id paigasus-console-ts | grep -A8 '"sources"'
```

```
          "sources": [
            {
              "glob": "app/**/*",
              "cache": true
            }
          ]
        },
        "id": "paigasus-console-ts",
        "language": "typescript",
--
        "sources": {
          "env": [],
          "files": [],
          "globs": [
            "ts/apps/paigasus-console/src/**/*",
            "ts/apps/paigasus-console/app/**/*"
          ],
          "id": "sources"
        },
```

**Verdict: Moon MERGES.** The resolved `sources` group carries both
`ts/apps/paigasus-console/src/**/*` (inherited) and `ts/apps/paigasus-console/app/**/*`
(project-level), matching the documented behaviour. This is harmless for this project — `src/`
does not exist here, so the inherited half of the merge matches nothing — but it means a
project that both HAS a `src/` directory and wants to narrow its `sources` group cannot do so
by declaring a project-level `fileGroups.sources`; that entry adds to the inherited glob, it
does not replace it. No other project in this repo currently relies on the merge behaving
otherwise (checked: `paigasus-ui-ts` and `paigasus-kernel-ts` both keep the inherited
`src/**/*` group unmodified).

---

## Not yet taken

Nothing. Every measurement the design's §4 table defines has been taken and recorded above.
This section listed M2 as outstanding after it had already been recorded, at "M2 — jsdom
polyfill list for Radix (Task 9)" below; corrected in SMA-503 fix round 2, item 5.

| Measurement | Recorded in |
|---|---|
| M1 | "M1 — does `@import '@paigasus/ui/styles.css'` resolve …" |
| M2 | "M2 — jsdom polyfill list for Radix (Task 9)" |
| M3 | "M3 — does Moon merge a project's `fileGroups` …" |
| M4 (and M4b) | "M4 — Tailwind's scan root, Next's build mode …" |
| M5 | "M5 — is `transpilePackages: ['@paigasus/ui']` needed?" |
| M6 | "M6 — does `.next/app-build-manifest.json` name …" |
| M7 | "M7 — does SWC accept an SPDX line comment above `'use client'`?" |

---

## Proofs (Task 7 — the AC-3 assertions bite)

Taken 2026-09-08 on the same branch/worktree as above. Every proof edits, measures, then
restores with `git checkout --`; `git status --short` was confirmed empty after each restore
before the next proof started. NOTE: the `7 checks passed` figures quoted verbatim below are
what the runs printed AT THE TIME. SMA-503 fix round 2, item 3 added three walk-level cases, so
`--self-test` now reports 10 checks; the quoted output is left as recorded rather than rewritten.
Before these proofs ran, `ci/tailwind-source/run.mjs`'s
`verdict()` was fixed (separate commit) to fail on an empty `appFiles` set — its self-test went
from 6 to 7 checks — so assertion 3 no longer passes vacuously if `ts/apps/paigasus-console/app`
is ever renamed or emptied.

### Proof 1 — the `@source` line

Deleted `@source '../../../packages/paigasus-ui/src';` from
`ts/apps/paigasus-console/app/globals.css`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:test --force; echo "rc=$?"
```

Result: `rc=1`. Verbatim:

```
paigasus-console-ts:test | FAIL: sentinel A (--paigasus-ui-source-probe) is absent from the built CSS — the app's Tailwind `@source` line no longer covers ts/packages/paigasus-ui/src
```

Sentinel B was not reported — unaffected, as expected. Restored with
`git checkout -- ts/apps/paigasus-console/app/globals.css`; `git status --short` confirmed empty.

### Proof 1b — the token `@import`

Deleted `@import '@paigasus/ui/styles.css';` from the same file, re-ran the identical command.

Result: `rc=1`. Verbatim:

```
paigasus-console-ts:test | FAIL: sentinel B (--paigasus-token-probe) is absent from the built CSS — the app's `@import '@paigasus/ui/styles.css'` did not resolve
```

Restored with `git checkout -- ts/apps/paigasus-console/app/globals.css`; `git status --short`
confirmed empty.

### Proof 2 — the cache path (a package-source edit, not an app-source edit)

First attempt (as the brief's literal instruction reads): set
`const SOURCE_PROBE = '[--paigasus-ui-source-probe:1]';` in
`ts/packages/paigasus-ui/src/components/table.tsx` to `const SOURCE_PROBE = '';`, leaving the
surrounding JSDoc comment — which itself quotes the literal `` `[--paigasus-ui-source-probe:1]` ``
— untouched. **This did not bite**: `moon run paigasus-console-ts:test` exited 0 with
`tailwind-source guard: both sentinels present across 1 CSS file(s)`. Cause: Tailwind's
candidate scanner reads the raw text of every scanned file, not the JS AST, so the literal
sitting in the comment was still picked up as a utility candidate and compiled into the CSS
even though the `SOURCE_PROBE` binding itself was blanked. Confirmed via
`grep -rn paigasus-ui-source-probe ts/packages/paigasus-ui/src/` still matching the comment
line after the first edit. Reverted with `git checkout -- ts/packages/paigasus-ui/src/components/table.tsx`.

Second attempt: blanked `SOURCE_PROBE` to `''` **and** rewrote the comment so no occurrence of
the string `paigasus-ui-source-probe` remains anywhere in the file (confirmed via the same
grep, zero matches).

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

Output included `paigasus-console-ts:build`, `paigasus-console-ts:test` and
`paigasus-console-ts:typecheck` (confirmed by inspecting the JSON: `tasks[project]` is a dict
keyed by task name, so iterating its keys yields genuine selections, not the `deps[].target`
entries a `grep -o '"target"'` would also count as selections).

```bash
moon run paigasus-console-ts:test; echo "rc=$?"
```

Result: `rc=1`. Verbatim:

```
paigasus-console-ts:test | FAIL: sentinel A (--paigasus-ui-source-probe) is absent from the built CSS — the app's Tailwind `@source` line no longer covers ts/packages/paigasus-ui/src
```

This is the cache path proof 1 never reaches: `app/globals.css` was untouched, so the console
build only re-ran because the task's `/ts/packages/paigasus-ui/src/**/*` input changed.
Restored with `git checkout -- ts/packages/paigasus-ui/src/components/table.tsx`; re-ran
`moon run paigasus-console-ts:test`, which passed (`rc=0`, `Tasks: 3 completed (2 cached)`).
`git status --short` confirmed empty.

### Proof 3 — the script's own inputs (Ruling R1: edits `run.mjs`, not `moon.yml`)

Established a fully-cached baseline first: `moon run paigasus-console-ts:test` →
`Tasks: 3 completed (3 cached)`. Then appended one comment line to the end of
`ci/tailwind-source/run.mjs`:

```
// SMA-503 task 7 proof 3: cache-busting comment, restored via git checkout --
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:test; echo "rc=$?"
```

Result: `rc=0`, `Tasks: 3 completed (2 cached)` — `paigasus-console-ts:build` and
`repo:next-env-drift` were served from cache (their log lines carry a `(cached, …)` marker);
`paigasus-console-ts:test` carries no such marker, ran the guard for real (`tailwind-source
self-test: 7 checks passed`, `tailwind-source negative control: reported red as expected`,
`tailwind-source guard: both sentinels present across 2 CSS file(s)`, 648ms), and its task hash
(`f4f8dd18`) differed from the baseline run's. This proves `/ci/tailwind-source/**/*` is
correctly keyed into the task's inputs — editing the guard script alone forces a genuine
re-run rather than serving a cached pass. Restored with
`git checkout -- ci/tailwind-source/run.mjs`; `git status --short` confirmed empty, and
`node ci/tailwind-source/run.mjs --self-test` still reported 7 checks passed (the R11 fix,
committed separately, survived the restore).

### Proof 2, amended — fix round 1: the comment fix makes the honest edit bite

Code review (fix round 1/5) flagged that Proof 2's finding above pointed at a real defect, not
only a scanning quirk: the sentinel literal `[--paigasus-ui-source-probe:1]` appeared twice in
`ts/packages/paigasus-ui/src/components/table.tsx` — once as `SOURCE_PROBE`'s value, once
verbatim in the JSDoc comment above it, which claimed the utility "exists ONLY here". That claim
was false as written. A future refactor that drops `SOURCE_PROBE` from the JSX but leaves the
comment would leave the guard passing on comment text alone, proving nothing.

Fix: reworded the comment to describe `SOURCE_PROBE` by name instead of writing out the
bracketed literal, mirroring how `ci/tailwind-source/run.mjs` already assembles its own probe
names from string fragments for the same reason. Confirmed
`grep -n paigasus-ui-source-probe ts/packages/paigasus-ui/src/components/table.tsx` matches
only the `const SOURCE_PROBE = '[--paigasus-ui-source-probe:1]';` line — one occurrence, not two.

Verification, in order:

1. `cd ts/packages/paigasus-ui && pnpm exec vitest run` — 4 test files, 11 tests, all passed;
   `tests/table.test.tsx:30`'s `expect(screen.getByRole('table').className).toContain(...)`
   assertion is unweakened and still checks the literal.
2. `moon run paigasus-console-ts:test --force` — `rc=0`,
   `tailwind-source guard: both sentinels present across 1 CSS file(s)`.
3. Re-ran Proof 2 the honest way — blanked `SOURCE_PROBE`'s value ALONE this time
   (`const SOURCE_PROBE = '';`), leaving the (now literal-free) comment untouched. Confirmed
   via the same grep that the literal was now fully absent from the file (zero matches, where
   the pre-fix version would still have matched the comment). Rebuilt:

   ```bash
   export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
   moon run paigasus-console-ts:test --force; echo "rc=$?"
   ```

   Result: **`rc=1`**. Verbatim:

   ```
   paigasus-console-ts:test | FAIL: sentinel A (--paigasus-ui-source-probe) is absent from the built CSS — the app's Tailwind `@source` line no longer covers ts/packages/paigasus-ui/src
   ```

   This is what the original Proof 2 should have produced on its first attempt and did not —
   the comment fix is precisely what makes the honest, minimal edit (blanking the constant
   alone) bite. Restored the blanked value back to the literal by hand (`const SOURCE_PROBE =
   '[--paigasus-ui-source-probe:1]';`) — **not** `git checkout --`, since that would also have
   discarded the comment fix itself, which is the change this commit keeps. `git diff` after
   the restore showed only the comment rewrite, confirmed by rebuilding once more:
   `moon run paigasus-console-ts:test --force` → `rc=0`,
   `tailwind-source guard: both sentinels present across 1 CSS file(s)`.
4. `moon run ts:fmt` — `rc=0`, "All matched files use Prettier code style!".
5. `moon run ts:lint` — `rc=0`, 0 errors (2 pre-existing warnings in `nav/link.tsx`, unrelated
   to this file).

### Verdict

All four proofs bite as expected. Proof 2's first attempt is recorded above as a finding — not
a scanning quirk in the gate but a real defect in the fixture file, since fixed (fix round 1)
and reverified by the amended Proof 2 immediately above: blanking `SOURCE_PROBE` alone now
correctly reds the guard, where before the fix it did not.

---

## M2 — jsdom polyfill list for Radix (Task 9)

Design §9, §14 row 2. The expected set going in, from design §9.1, was `ResizeObserver`,
`Element.prototype.hasPointerCapture`, `Element.prototype.setPointerCapture`,
`Element.prototype.releasePointerCapture` and `Element.prototype.scrollIntoView`. This task is
the first to render a Radix component (`Dialog`, `DropdownMenu`) in jsdom, so it is the first
real measurement.

**Command**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run
```

**Output (verbatim, trimmed to the summary)**

```
Not implemented: Window's getComputedStyle() method: with pseudo-elements
Not implemented: HTMLCanvasElement's getContext() method: without installing the canvas npm package

 Test Files  6 passed (6)
      Tests  15 passed (15)
```

**Verdict: none of the five candidate polyfills were needed.** All 15 tests passed on the
first run, against unmodified `tests/setup.ts`, including both `expectNoAxeViolations()`
assertions taken with the Dialog and DropdownMenu content open, and both open/close
role-visibility assertions driven by `userEvent`. `tests/setup.ts` is therefore **unchanged** by
this task — no line was added.

The `getComputedStyle`/`getContext` lines above are jsdom's own stderr noise (from Radix
probing layout APIs jsdom stubs rather than omits) and do not fail a test or need a polyfill;
jsdom returns benign defaults for both. This narrows, but does not close, design §9.1's
question: this task's Dialog/DropdownMenu fixtures render no `ResizeObserver`-consuming layout
and no pointer-capture-driven drag (no `Select`, `Slider`, or scroll-locking `ScrollArea`), so a
later task exercising one of those primitives may still need to add one of the five back —
per `tests/setup.ts`'s own instruction, with a comment naming the failing test.

### M2 update — `ResizeObserver` needed (Task 10)

Task 10's `tests/combobox.test.tsx` closes part of the question above: `ResizeObserver` **was**
needed, the first time this suite renders `cmdk`'s `CommandList` (composed under `Combobox`).

**Command**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-ui && pnpm exec vitest run tests/combobox.test.tsx
```

**Output (verbatim, before the polyfill was added)**

```
ReferenceError: ResizeObserver is not defined
 ❯ .../node_modules/cmdk/dist/index.mjs:1:8384
...
 Test Files  1 failed (1)
      Tests  3 failed (3)
```

All three tests failed the same way, including the ones that never open the popover — cmdk's
`CommandList` calls `new ResizeObserver(...)` in a mount effect regardless of open state, so the
failure fires on render, not on interaction. A minimal stub class
(`observe`/`unobserve`/`disconnect` as no-ops) was added to `tests/setup.ts`, assigned via
`globalThis.ResizeObserver ??= ...` so it only takes effect where jsdom has none. Re-run:

```
Not implemented: HTMLCanvasElement's getContext() method: without installing the canvas npm package

 Test Files  1 passed (1)
      Tests  3 passed (3)
```

**Verdict: `ResizeObserver` is now polyfilled in `tests/setup.ts`.** The other four design §9.1
candidates (`hasPointerCapture`/`setPointerCapture`/`releasePointerCapture`/`scrollIntoView`)
remain untested by this repository's suite — `Combobox` needs none of them — and stay out per
the same add-only-what's-measured rule. Full-suite re-run after the change:
`Test Files  7 passed (7)` / `Tests  18 passed (18)`.
