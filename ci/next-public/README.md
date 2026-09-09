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
   Markdown) uses `NEXT_PUBLIC_` in an executable or configuration context. The four
   matched forms are tabulated below; naming the prefix in prose is not one of them.
2. **`check_factory`** — every tracked `ts/apps/*/next.config.{ts,js,mjs,cjs}` calls
   `createNextConfig`, and there is at least `APP_CONFIG_FLOOR` of them.

Both checks run on every invocation with no flags (`bash ci/next-public/run.sh`).
Nothing else in `ts/` is inspected.

**The match is CONTEXT-AWARE, not a bare literal and not a suffix rule (fix round 3,
SMA-502).** `check_prefix` runs one `grep -E` alternation. It matches `NEXT_PUBLIC_` in
four forms, and each form is a way the string reaches an executable or configuration
position:

| # | Form | Matched by | Example |
|---|------|-----------|---------|
| 1 | followed by an identifier character | `NEXT_PUBLIC_[A-Za-z0-9_]` | `NEXT_PUBLIC_API_URL`, `NEXT_PUBLIC__DEBUG` |
| 2 | property access | `env\.NEXT_PUBLIC_` | `process.env.NEXT_PUBLIC_` |
| 3 | quoted string key | `['"]NEXT_PUBLIC_['"]` | `process.env["NEXT_PUBLIC_"]` |
| 4 | dotenv-style assignment | `NEXT_PUBLIC_=` | `NEXT_PUBLIC_=https://issuer.example` |

Anything else — the bare prefix in prose, in a comment, or inside a longer sentence in
an error message — passes. **A backtick is deliberately NOT a quote in form 3**, so a
doc comment may write `` `NEXT_PUBLIC_` `` freely. That is the one thing form 3 is
carefully NOT allowed to do, and a self-test row holds it: adding the backtick to the
quote class reds both the synthetic doc-comment row and the row that copies the real
`ts/packages/paigasus-next-config/src/index.ts` into a fixture (measured).

**Why the rule has this shape, in three corrections.** The history matters, because the
trade has now been made wrong in both directions.

- **Round 0** matched the bare literal `NEXT_PUBLIC_`. That banned the prefix from being
  *named*, and the casualty was the one non-Markdown place where naming it is most
  useful: the `extend.env` refusal message in `createNextConfig`. A check that forbids
  its own subject from being named in the message explaining it is mis-specified.
- **Round 1** required an identifier character, `[A-Za-z0-9]`. That freed the prose and
  bought a false *negative*: `NEXT_PUBLIC__DEBUG` is a valid env var and the class
  without `_` does not match the character following the prefix there.
- **Round 2** restored the `_`. That closed the double-underscore miss and left a
  different false negative, which round 3 fixes: **the exact key `NEXT_PUBLIC_` is
  itself inlinable.** Next collects every environment key for which
  `key.startsWith('NEXT_PUBLIC_')` holds
  (`packages/next/src/lib/static-env.ts`, `getNextPublicEnvironmentVariables`), and a
  prefix is a prefix of itself — so `process.env.NEXT_PUBLIC_` is a live read of an
  inlined value, and it sailed past a rule demanding one more identifier character.

Reverting to the bare literal to close that would simply re-break the prose, which is
why round 3 separates the two by **context** rather than by suffix. The evasion surface
recorded in L2 is unchanged: a *computed* name still defeats the scan entirely.

## Exit codes, and why 1 and 2 must not collapse into each other

`0` pass, `1` the repo is wrong, `2` infrastructure failed — the repo's usual
contract. A `git ls-files` failure while deriving either corpus is routed to
`die_infra` (rc 2), not treated as an empty corpus that then trips the floor at rc 1:
those are different facts, and collapsing them would let an environmental fault read
as "a file uses the banned prefix," or the reverse, let a real finding read as merely
inconclusive. An unknown flag is also rc 2: passing `--nonsense-flag` must not be
readable as "the repo is clean" or "the repo is wrong," since the gate never actually
ran the check it was asked to run.

**A fully collapsed `ts/` is rc 1, and it was rc 2 (fix round 3, SMA-502).** The
separation above was stated but not implemented. `next_public_corpus` piped
`git ls-files` through `grep -v`, and `grep -v` exits **1** when it selects no lines;
`set -o pipefail` then handed that status to the whole pipeline. So a `ts/` that had
moved or been renamed — precisely the case `CORPUS_FLOOR` exists to catch — returned 1
from the corpus function, `check_prefix` read any non-zero as a broken `git`, and the
gate exited **2** instead of tripping the floor at **1**. The floor could report every
collapse except the total one. The fix keeps both facts distinct at the source rather
than swallowing errors: `git ls-files` is measured on its own and a real failure still
returns 2, while the exclusions are applied with `sed`, which exits 0 whether it
deletes every line or none. Two self-test rows hold the pair — a zero-`ts/`-file
repository must be rc 1, and a directory that is not a git repository at all must be
rc 2.

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

`next_public_corpus` is `git -C "$root" ls-files -- 'ts/'`, filtered with `sed` to drop
`ts/pnpm-lock.yaml` and any `*.md` file. (`sed`, not `grep -v`, and not in the same
pipeline as `git` — see the exit-codes section above for why that is load-bearing
rather than stylistic.) Markdown is excluded so a document — this
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

**L2 — a computed or split name evades any text scan.** `'NEXT_' + 'PUBLIC_'`,
`process.env['NEXT_PUBLIC_' + suffix]`, or any other construction that assembles the
string at runtime instead of writing it as one literal, defeats
`grep -qE -- "$BANNED_RE"` entirely. No text gate closes this; it would need something
that reasons about the built output, not the source text. **This residual is unchanged
by fix round 3, and by rounds 1 and 2 before it.** Every one of those rounds moved the
boundary between *literal* occurrences — which ones count as a match — and a computed
name was never a literal occurrence to begin with. Round 3's context forms add three
new literal shapes to the matched set; they take nothing away from what a computed name
could already evade.

The backtick carve-out in form 3 is a narrower case of the same thing: a TypeScript
template literal is a quote, so `` process.env[`NEXT_PUBLIC_`] `` is a real read this
gate does not match. That is a deliberate trade against false-positiving on every doc
comment that names the prefix — and a template literal with no interpolation is not a
form anyone writes by accident.

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

**L7 — prose may name the bare prefix, and the rule that allows it went through three
attempts (SMA-502).** Recorded here because each attempt looked correct and two were
wrong in a way no row caught at the time.

Round 0 matched the bare literal and false-positived on the `extend.env` refusal
message. Round 1 required `[A-Za-z0-9]` after the prefix and silently stopped matching
`NEXT_PUBLIC__DEBUG`, a valid env var — a false positive traded for a false negative,
the wrong direction for a gate whose only job is to catch this prefix. Round 2 restored
the `_` to the class. Round 3 found that the **exact key** `NEXT_PUBLIC_` is inlinable
in its own right, because Next selects keys with `startsWith('NEXT_PUBLIC_')` and a
prefix is a prefix of itself, and replaced the suffix-only rule with the four context
forms tabulated above.

**What holds the shape now is rows, not the comment.** Each context form has a
self-test row that must FAIL the gate, the backtick carve-out has two rows that must
PASS it — one synthetic, one a copy of the real
`ts/packages/paigasus-next-config/src/index.ts` — and reverting `BANNED_RE` to the
round-2 suffix rule reds the property-access row (measured). Before round 3 none of
that existed: the two prose rows passed and no row exercised the bare key at all, so
the miss was invisible.
