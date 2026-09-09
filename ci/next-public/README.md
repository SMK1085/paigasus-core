<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:next-public-free`

Bans `NEXT_PUBLIC_` across `ts/`, and requires every app's Next config to build
through `@paigasus/next-config`'s factory (SMA-502).

## Why

Next inlines every `NEXT_PUBLIC_*` value at `next build` time. Once a value is
inlined, it is baked into the compiled JavaScript that ships in the OCI image. So one
image cannot serve two self-hosted customers with different IdP issuers or API URLs —
the second customer would see the first customer's values, compiled in and
unchangeable at deploy time. That single fact is the whole reason
`@paigasus/next-config` exists: every deployment-varying value must be read at request
time, through `@paigasus/next-config/runtime`, never baked in at build time. A grep is
enough to enforce the ban mechanically, which beats relying on code review forever.

## What it asserts

Two independent checks, both against **tracked** files only (`git ls-files`, so
`node_modules` and any other untracked path are out of scope by construction):

1. **`check_prefix`** — no tracked `ts/` file (minus the pnpm lockfile and any
   Markdown) contains the literal string `NEXT_PUBLIC_`.
2. **`check_factory`** — every tracked `ts/apps/*/next.config.{ts,js,mjs,cjs}` calls
   `createNextConfig`, and there is at least `APP_CONFIG_FLOOR` of them.

Both checks run on every invocation with no flags (`bash ci/next-public/run.sh`).
Nothing else in `ts/` is inspected.

**The match requires an identifier character after the prefix (fix round 1, SMA-502).**
`check_prefix` scans for `NEXT_PUBLIC_[A-Za-z0-9]` (`grep -E`), not the bare literal
`NEXT_PUBLIC_`. The hazard this gate exists to catch is always a real environment
variable — `NEXT_PUBLIC_API_URL`, say — and the bare prefix on its own is not a valid
env var name; nothing can read it. An earlier version of this gate matched the bare
prefix, which meant the one place in the codebase where naming `NEXT_PUBLIC_`
explicitly is most useful — the error message explaining why `extend.env` is refused —
tripped the same rule it was written to describe, and had to be reworded around it.
That was a defect in the gate, not in the source file: a check that forbids its own
subject from being named in the message explaining it is mis-specified. The refinement
closes that without widening the evasion surface — see L2 below, unchanged by this
fix.

## Exit codes, and why 1 and 2 must not collapse into each other

`0` pass, `1` the repo is wrong, `2` infrastructure failed — the repo's usual
contract. A `git ls-files` failure while deriving either corpus is routed to
`die_infra` (rc 2), not treated as an empty corpus that then trips the floor at rc 1:
those are different facts, and collapsing them would let an environmental fault read
as "a file uses the banned prefix," or the reverse, let a real finding read as merely
inconclusive. An unknown flag is also rc 2: passing `--nonsense-flag` must not be
readable as "the repo is clean" or "the repo is wrong," since the gate never actually
ran the check it was asked to run.

## Why check 2 exists

Check 1 alone is not sufficient. `env:` in a `next.config` is a build-time inlining
channel exactly equivalent to `NEXT_PUBLIC_` — Next compiles whatever it names into
the bundle at `next build` time, the same as the banned prefix does.
`createNextConfig()` refuses an `extend.env` key, so any app that goes through the
factory cannot reintroduce this channel. But a hand-written Next config that
never calls the factory at all contains no `NEXT_PUBLIC_` literal — it passes check 1
cleanly — and can still bake a deployment-varying value into the image through its own
`env:` block. Check 2 closes that gap by requiring every app config to route through
the one factory that already refuses it.

## Corpus derivation

`next_public_corpus` is `git -C "$root" ls-files -- 'ts/'`, filtered to drop
`ts/pnpm-lock.yaml` and any `*.md` file. Markdown is excluded so a document — this
README included — can name the string it bans; a document compiles into nothing, so
naming it in prose creates no image-inlining hazard. The lockfile is excluded because
a third-party dependency's *name* can legitimately contain the prefix, and a lockfile
is not compiled either.

**The corpus floor.** `check_prefix` refuses to proceed if the derived file list has
fewer than `CORPUS_FLOOR` (48) entries. This is what stops a moved or renamed `ts/`
from silently emptying the gate — the SMA-553 class, which `repo:input-liveness`
cannot reach here: it proves a *declared* Moon task input glob is live, never that
this script's own runtime scan still sees anything. 48 was set as two thirds of 72,
the corpus size measured when this gate was first written — the same proportion
`ci/ruff/run.sh`'s floor uses against its own corpus. **The floor is a collapse
detector, not a proportion of the current corpus** (fix round 2): it answers "does
`ts/` still exist," not "is `ts/` still N-sized," and stays fixed at 48 as the corpus
grows. Re-measured at fix round 2, the real corpus is **90** tracked files, grown from
72 as later tasks in this issue added files — 48 is comfortably below that, and is
left unchanged rather than re-tied to a ratio that would need re-deriving on every
future addition. A floor set far below the real count would let `ts/` collapse most of
the way before the gate noticed, which defeats the point of having one.
**`check_factory` carries its OWN floor, `APP_CONFIG_FLOOR` (1 today).** It did not, and
that was a hole: the check iterated app configs and printed its success line when the
list was **empty**, so renaming `ts/apps` — or adding an app that used
`next.config.mjs`, which the old `.ts`-only glob did not match — silently dropped that
app from the check while `CORPUS_FLOOR` stayed satisfied, because the corpus counts
`ts/` **files** and not app configs. Two fixes, together: the glob now matches all four
valid Next config extensions (`ts`, `js`, `mjs`, `cjs`), and the floor asserts the list
is non-empty before the loop runs. Re-baseline `APP_CONFIG_FLOOR` deliberately when a
second console zone app lands, in the same style as `CORPUS_FLOOR`.

## Limitations

**L1 — the ban is scoped to `ts/`.** A `NEXT_PUBLIC_` in `rs/Dockerfile`, `ops/`, or a
Helm template is equally harmful — Next reads the prefix from its own process
environment regardless of where that value was set — and this gate does not scan any
of them. That scope is a decision, not an oversight: `ts/` is where a Next build
actually reads the prefix from, so it is where a source-level ban has something to
check.

**L2 — a split literal evades any text scan.** `'NEXT_' + 'PUBLIC_'`, or any other
construction that assembles the string at runtime instead of writing it as one
literal, defeats `grep -qE -- "$BANNED_RE"` entirely. No text gate closes this; it
would need something that reasons about the built output, not the source text. The
fix-round-1 identifier-character refinement (above) does not change this: it narrows
what counts as a match among *literal* occurrences, and a computed name was never a
literal occurrence in the first place.

**L3 — the `**/*.md` exclusion is safe only while no app compiles Markdown.** The
exclusion rests on the fact that a Markdown file compiles into nothing today. An MDX
app would make that premise false and turn the exclusion into a bypass: an MDX source
file ends in `.md` (or `.mdx`, which this gate does not even list) and would be
skipped by the same rule that lets this README name the banned string safely.

**L4 — `repo:input-liveness` catches a dead input glob, not a too-narrow one.** A
future edit that narrowed this gate's own declared Moon task `inputs` would switch the
gate off for whatever it stopped watching, and nothing but `CORPUS_FLOOR` would notice
— and only for the `ts/`-rename case, not a narrowing that still leaves 48-plus files
in view. This is not hypothetical: the gate SHIPPED narrow. Its first `inputs` were
`ts/apps/**/*` and `ts/packages/**/*`, which left twelve tracked, scanned files
(`ts/eslint.config.js`, `ts/package.json`, `ts/moon.yml`, `ts/scripts/*.mjs`,
`ts/tooling/*.mjs` among them) outside the declared set, so a PR touching only one of
them never scheduled the gate at all. The inputs are now `ts/**/*` with the built
`.next` tree negated — the same shape as the scanned corpus — and
`SELF_TASK_EXPECTED_GLOBS["next-public-free"]` in `ci/affected-graph/ci_targets.py`
pins that by strict equality. The alternative, an unconditional `ci.yml`
step that always runs regardless of Moon's affected-graph, closes that residual but
loses local `moon ci` coverage for the gate. That trade was made deliberately (spec
§ 6.5), the same way the codegen-drift gate is deliberately unconditional and this one
is not.

**L4b — neither read uses a here-string, deliberately.** `check_prefix` and `check_factory`
both fill their arrays with `mapfile -t … < <(printf …)`, not `mapfile -t … <<< "$var"`.
On bash 5.3.15 (macOS/homebrew) the here-string form **deadlocks**: bash writes the
here-string into a pipe from `do_redirections` *before* the builtin that would drain it
starts, so anything past the pipe's capacity blocks on `write()` forever. Measured on
that host: 10 lines (330 bytes) passes, 20 lines (670 bytes) hangs; `/bin/bash` 3.2 never
hangs, and process substitution — which forks a writer — passes at 200 lines. It is
**intermittent**, because macOS can hand back a 512-byte pipe under memory pressure
instead of the usual 16K, so the same command can pass and then hang minutes later in the
same session. The real corpus is 90 paths, well past the small-pipe threshold, and the
symptom is `moon run repo:next-public-free` hanging with no output and no failure. Do not
simplify either read back to `<<<`. `ci/ruff/run.sh:114` and `:239` carry the same
pattern and are not changed here — that gate is outside this issue's scope, but the same
hang applies to it.

**L5 — check 2 proves the call is present, not that it is load-bearing.** It asserts
an app's Next config *mentions* `createNextConfig`; it does not prove the call's
result reaches the file's default export. A config that imports the factory, calls it,
and then exports something else entirely — a literal object, say — passes check 2
while shipping a config the factory never actually built.

**L6 — the build-phase guard was measured to fire in prerender workers, so this
residual is closed rather than latent.** Task 1 measured
`NEXT_PHASE_REACHES_WORKERS = true`: a module-scope read of a deployment-varying value
fails `next build` outright (rc 1, the throw logged twice), including inside a
prerender worker, not only in the main build process. So the failure mode this
limitation would otherwise describe — a module-scope config read inside a worker
silently escaping the guard — does not occur on this repo's measured Next version.
`await connection()` remains the documented discipline for code that must defer a read
to request time regardless, but it is not required merely to make a module-scope
mistake visible; the build already fails loudly on its own.

**L7 — the identifier-character requirement lets prose name the bare prefix, and
still cannot see a computed name (fix rounds 1-2, SMA-502).** `check_prefix` matches
`NEXT_PUBLIC_[A-Za-z0-9_]`, not the bare `NEXT_PUBLIC_` literal, so a comment or string
that names only the prefix itself, followed by `.`, a space, or a backtick — none of
which are in the class — passes: `"...exactly as NEXT_PUBLIC_ does"` is not a match.
That is the entire purpose of the refinement, and nothing more: the earlier
bare-literal match caught real usages and this kind of prose in the same net, and the
one non-Markdown place where naming the prefix is most useful — the `extend.env`
refusal message in `ts/packages/paigasus-next-config/src/index.ts` — was exactly the
casualty.

**The character class itself went through two attempts, and the first was wrong in
the unsafe direction.** Fix round 1 shipped `[A-Za-z0-9]`, which was verified to pass
prose but was never checked against every valid env-var character —
`echo 'NEXT_PUBLIC__FOO' | grep -qE 'NEXT_PUBLIC_[A-Za-z0-9]'` finds nothing, so a
double-underscore name such as `NEXT_PUBLIC__DEBUG` (a perfectly valid env var) was
silently missed. That traded a false positive (prose naming the bare prefix) for a
false negative (a real banned variable going undetected) — the wrong direction for a
gate whose only job is to catch this prefix. Fix round 2 corrected the class to
`[A-Za-z0-9_]`, which catches the underscore case again while still not matching the
bare prefix followed by `.`, space, or backtick, so the original fix-round-1 goal
still holds.

**This refinement exists only to let prose name the bare prefix. It does not narrow
the evasion surface recorded in L2, in either of its forms.** A computed name such as
`process.env['NEXT_PUBLIC_' + suffix]` defeats the scan exactly as it did before any
of this — no literal `NEXT_PUBLIC_<identifier char>` run ever appears in the source
text either way, so neither character class changes what a computed name can evade.
