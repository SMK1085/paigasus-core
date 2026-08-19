# SMA-553 — Assert every `repo:*` task's inputs still match a tracked file

**Status:** revised after adversarial review (2026-08-19)
**Linear:** [SMA-553](https://linear.app/smaschek/issue/SMA-553/repo-assert-every-repo-tasks-inputs-still-match-a-file-a-gate-can-be)
**Related:** SMA-541 (limitation L3, which filed this), SMA-525 (`repo:actionlint`, whose `inputs:
['**/*']` comment reasons about exactly this hazard), SMA-524 / SMA-534 / SMA-546 (the sibling
assertions in `ci/affected-graph/`), SMA-378 (the `uv_build` license gotcha this spec's §7 finding
belongs to)

## 1. Problem

SMA-541 proves a `repo:*` gate is **wired into CI**: present in `ci.yml`'s `T=(…)` array, resolving
to a CI-eligible task, mirrored in CLAUDE.md. This is the layer below — a gate that is wired,
resolvable, and **stops firing on the changes it exists to catch**, because its `inputs` no longer
match anything.

`repo:promtool` declares three inputs (`moon.yml:422-425`): the glob
`ops/observability/prometheus/**/*`, plus `.prototools` and `.proto/plugins/promtool.toml`. Move or
rename `ops/` and that glob matches zero files. Moon still schedules the task — on a toolchain-pin
change — but **never again on a change to the Prometheus config it exists to validate**. `moon ci`
stays green, the target stays in `T`, SMA-541's C1-C5 all pass, and the gate has silently stopped
testing its subject. *(The first draft said "Moon never schedules the task again", which is wrong
and understates how quiet this is: a partially-dead input set still produces occasional green runs,
so the gate looks alive.)*

The surface is wide because `repo` owns the whole tree, so almost every `repo:*` gate carries a
narrow hand-written glob precisely to avoid running on every change: `osv`, `machete`, `promtool`,
`observability-drift`, `nats-permissions`, `redis-connect-single-site`,
`iam-docker-policy-single-site`, `parity-corpus-drift`, `next-env-drift`, `release-parity*`,
`publish-metadata`, `wasm-getrandom-free`.

Nothing checks this today, and SMA-541's design doc already corrects an earlier draft that claimed
otherwise: `cargo_moon_parity.py`'s A4 iterates Cargo crates, A5 derives FFI tasks from command
markers, and `lockfile->all-lint` is a Rust-task case. None looks at a `repo:*` task's inputs.

## 2. Evidence

Measured on 2026-08-19 against the pinned moon **2.3.2**, before any design decisions were made.
Everything here was observed, not reasoned about. E1, E2, E6 and E8 each contradict a premise in the
issue text or in an obvious first design; E11-E14 were added after adversarial review, two of them
to **refute** a proposed finding rather than to fold it in.

**E1 — Moon reports inputs VERBATIM, unresolved.** The finding the whole design turns on. A throwaway
`repo` task declaring inputs that do not exist:

```yaml
  ghost-probe:
    inputs:
      - 'nonexistent-dir/**/*'
      - 'nonexistent-file.txt'
```

reports them back unchanged:

```
inputFiles: {'nonexistent-file.txt': {}}
inputGlobs: ['nonexistent-dir/**/*', '.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}']
```

`moon query projects`' `inputFiles` / `inputGlobs` are **normalised**, not **resolved**: a
path-keyed dict of what the author declared, with the leading `/` of a workspace-relative entry
stripped and a bare directory expanded (E6). Moon never touches the filesystem to produce them. So
Moon's own output can never tell you a gate has gone dead — *(this corrects the issue text's step 1,
which says "Read `moon query projects`' **resolved** `inputFiles` / `inputGlobs`")*. The gate must
match against tracked files itself.

`moon task repo:promtool` is no help either: it pretty-prints the same four declared patterns.

**E2 — every task carries a Moon-injected glob, so a resolved input set is NEVER empty** *(this
makes the issue's AC #2 unreachable as literally worded)*. All **119** tasks across all **28**
projects carry `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}`. A task declaring literally `inputs: []`
still reports it:

```
has inputFiles key: False
inputGlobs: ['.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}']
raw inputs: [{'glob': '/.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}', 'cache': True}]
```

"Fail if a task's resolved input set is empty" therefore asserts nothing until that injected glob is
subtracted. With the subtraction it becomes a real check: a `repo` task with no authored inputs keys
only on `.moon/` config edits, which is a broken gate.

**E3 — `repo` is 100% clean; nothing else is.** Matching every declared pattern against
`git ls-files -- ":(glob)…"` across the whole graph:

| | count |
|---|---|
| dead globs on `repo:*` | **0** |
| dead globs on the other 27 projects | **98** |
| declared file inputs not tracked | **4** (all `py`, see §7) |
| tasks with no authored inputs at all | **2** (`ts:check-config-only`, `ts:commitlint`) |

The 98 are essentially all *speculative convention globs* inherited from
`.moon/tasks/{rust,typescript,python}.yml` — `**/*_test.rs`, `tests/**/*`, `**/*.spec.ts`,
`ts/src/**/*`, `py/src/**/*`. `paigasus-kernel-rs` has no `tests/` directory, so its inherited
`tests/**/*` is legitimately dead. A whole-graph check would red on day one, which is SMA-541's E4
finding reproduced one layer down. Spot-checked during review: `py:lint`'s dead `py/src/**/*` comes
from `.moon/tasks/python.yml:16-18`'s `sources` file group, and `py/moon.yml:11-15` adds the
`packages/*/src/**/*` that actually matches — documented at `py/moon.yml:6-10`. So `py:lint` is
**not** under-keyed; the dead glob is genuinely harmless.

**E4 — `git ls-files -- ":(glob)P"` agrees with wax on every pattern in use, except braces.**

| pattern | tracked matches |
|---|---|
| `rs/**/Cargo.toml` | 14 — **including `rs/Cargo.toml`**, so `**` spans zero segments as wax does |
| `**/*` | 714 — every tracked file, including the 9 at top level and all dotfiles |
| `ops/observability/prometheus/**/*` | 7 |
| `rs/crates/*/*/moon.yml` | 13 |
| `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}` | **0** — git pathspec has no brace expansion |

714 is also the repo's total tracked-file count, so `**/*` is exactly the whole tree. Graph-wide
there are **zero** negated (`!`-prefixed) globs and **zero** brace globs other than the injected one.
This is five measured patterns, not a proof of general equivalence — E12 and E14 probe two specific
divergence hypotheses raised in review, and L9 records what remains untested.

**E5 — `repo` inherits no task config, so its injected input set is a single known string.**
`moon query projects --id repo` reports `inherited.configs: []` and `layers: {}`. Every other stack
inherits one: `paigasus-kernel-rs:lint` carries `.moon/tasks/rust.yml` as an additional global
`inputFile`, and `paigasus-ml-py:build` carries `globalInputs: [{"file":
"/.moon/tasks/python-project.yml"}]`. Moon's *merged* task object records no per-input provenance —
`inherited.configs[…].tasks[…].inputs` holds the unmerged, project-relative, unexpanded form
(`{"file": "pyproject.toml"}`, `@group(sources)`), so recovering "authored vs inherited" graph-wide
means re-implementing file-group expansion and path resolution.

**E6 — a bare directory input arrives as a glob** *(so I2 needs no directory-prefix fallback)*.
`inputs: ['ops/nats', 'ops/observability/prometheus/rules']` reports:

```
raw:        [{'file': 'ops/nats'}, {'file': 'ops/observability/prometheus/rules'}, …]
inputFiles: (absent)
inputGlobs: ['.moon/*.{…}', 'ops/nats/**/*', 'ops/observability/prometheus/rules/**/*']
```

Moon classifies the raw entry as a `file` but expands it into `inputGlobs` as `dir/**/*`, leaving
`inputFiles` empty.

**E7 — `moon query`'s `--id` filter is an unanchored regex.** `--id epo` returns `repo`; `--id
paigasus-kernel` returns `paigasus-kernel-parity-rs`, `paigasus-kernel-py`, `paigasus-kernel-rs`,
`paigasus-kernel-ts`. SMA-541's D8 warned about this for `--project`; it is measured true for `--id`
too. `moon query tasks` sidesteps it entirely — its output is `d['tasks'][<exact project id>]`.

**E8 — an absent `inputFiles` key is LEGITIMATE here** *(this corrects the issue text's step 1,
which says "Treat an absent key as a violation, never a skip — the convention A4 already follows")*.
Seven tasks graph-wide have no file inputs because every input they declare is a glob — five of them
on `repo`: `actionlint`, `iam-docker-policy-single-site`, `machete`, `observability-drift`,
`redis-connect-single-site`. Applying A4's rule verbatim would red five clean gates on day one. A4
can hold that rule because it asserts specific named files must be present on a `lint` task; this
gate iterates whatever is declared, so absence means "no file inputs", not "Moon told us nothing".

**E9 — timings**, on this machine, warm. The first five are measured; the last is SMA-525's figure
for a *different* task, quoted as an **order-of-magnitude reference point, not a measurement of the
task this spec adds** (see D2).

| | wall | |
|---|---|---|
| `ci/affected-graph/run.sh` (real suite) | 29.3s | measured |
| `ci/affected-graph/run.sh --negative-control` | 5.2s | measured |
| ⇒ `repo:affected-smoke` | ~35s + Moon's per-task floor | derived from the two above |
| `moon query projects` (one call) | ~2.5s | measured |
| `repo:input-liveness` — `inputs: ['**/*']` + script | ~6.0s | measured (median of 3 alternating `moon run repo:input-liveness --force` runs, warm — Task 7) |
| `repo:promtool` — Moon's narrow per-task floor | ~8.7s | SMA-525's figure |

**E10 — `.moon/workspace.yml` states this issue's invariant in prose, unguarded.** Lines 41-43,
verbatim:

> AND DO NOT ADD ONE EITHER: no task may declare an `inputs:` path under these trees. A path
> excluded here contributes nothing to any cache key, so the task that names it would never be
> invalidated by a change to it — a permanently stale cached pass, with nothing red to notice.

`ignorePatterns` is `['**/node_modules/**', '**/target/**', '**/.venv/**']`, and the block above it
records the compliance check as "verified" by hand. That is SMA-553's failure class, written down,
with no gate behind it.

**E11 — wax globs DO match dot-paths** *(added in review; the reviewer flagged the opposite as the
single highest-value unmeasured assumption, on the grounds that if wax's `*` skipped a leading `.`
then `repo:actionlint`'s `**/*` would never key on `.github/` and D2 would collapse)*. Measured by
feeding a dot-path to Moon's own affected query:

```
$ printf '.github/dependabot.yml\n' | moon query tasks --affected --downstream deep
repo tasks affected: ['actionlint']
```

`repo:actionlint` declares only `**/*` and the injected glob (`moon.yml:477-478`), and
`.github/dependabot.yml` is not an input of any other repo task — so `**/*` matched it. D2 holds.

**E12 — git does NOT case-fold a `:(glob)` pathspec, even with `core.ignoreCase=true`** *(added in
review, to refute a proposed false-green)*. The concern was that git case-folds on APFS while wax is
case-sensitive, so `inputs: ['OPS/…']` would read live to the gate and dead to Moon. Measured on this
macOS checkout:

```
$ git config --get core.ignoreCase
true
$ git -c core.quotePath=false ls-files -- ":(glob)OPS/observability/prometheus/**/*" | wc -l
0
```

Case-sensitive, agreeing with wax. The hypothesis does not reproduce. I2 is case-sensitive by
construction anyway (D5: exact set membership against `git ls-files` output, not a pathspec call).

**E13 — `.moon/tasks.yml` declares a seven-entry `implicitInputs` block, and it does not reach
`repo`** *(added in review; E5's stated reason was correct but incomplete)*. `.moon/tasks.yml:16-23`
lists `/.moon/toolchains.yml`, `/.moon/tasks.yml` and the five `/.moon/tasks/*.yml` files, under a
comment reading "Inserted into every **inherited** task's inputs". E2's probe is the confirmation
that it does not apply to `repo`'s locally-defined tasks: a `repo` task with `inputs: []` reported
exactly one glob and no `inputFiles` at all. So D4's subtraction is correct today — but it is correct
because of *this* measurement, not because `inherited.configs` is empty, and D4 is hardened
accordingly.

**E14 — a wildcard-free pathspec prefix-matches a directory.** `git ls-files -- rs` and
`git ls-files -- ":(glob)rs"` both return **330**. This is why `pattern_verdict` needs a separate
`tracked_exact` helper (`ci/actionlint/run.sh:886-892`) and a distinct `not-exact` verdict, and it is
why I2 is specified as exact set membership rather than as a non-empty `ls-files` result — the naive
form would pass for any directory path.

## 3. Design decisions

**D1 — the check lives in a new `ci/affected-graph/task_inputs.py`.** A third file rather than an
extension of `cargo_moon_parity.py` (29k, Cargo-centric, though its A4 is precedent for an inputs
assertion living there) or `ci_targets.py` (51k). Own concern, own fixtures, own README bullet,
matching SMA-541 D1's reasoning for `ci_targets.py`. It keeps `ci/affected-graph/`'s established
shape: pure functions with fixture tables, a `--self-test` flag, 0/1/2 exit codes.

Hosting it inside `ci/actionlint/run.sh` instead — which already has `inputs: ['**/*']`, the verdict
vocabulary and a fixture harness — was raised in review and rejected: that gate's subject is GitHub
workflow files, its matcher answers a *different question* (see D5), and folding a Moon-graph
assertion into it would put two unrelated failure domains behind one red.

**D2 — but it is scheduled by its OWN `repo:input-liveness` task, not by `run.sh`.** This is forced
by E1 combined with `repo:affected-smoke`'s narrow inputs. The gate's verdict depends on the
**entire tracked file tree** — a glob dies when files move — while `affected-smoke` keys on
`moon.yml`, `ci/affected-graph/**/*`, `.moon/**/*`, the manifests, `CLAUDE.md` and `.prototools`
(`moon.yml:130-155`, checked: no `ops/` path among them). Rename `ops/` and `repo:promtool`'s glob
dies, but nothing in that list changed, so `affected-smoke` serves a **cached PASS** and the headline
acceptance criterion silently does not hold. That is the same vacuity trap SMA-541's D9 had to add
`CLAUDE.md` to close.

The only honest input for this check is `inputs: ['**/*']` — the conclusion `repo:actionlint`
already reached, and which the issue cites. E11 confirms `**/*` reaches dot-paths, so the input is
genuinely whole-tree. Of the two ways to get there, both are now measured: broadening `affected-smoke`
to `**/*` makes its **measured ~35s** suite run on every PR, while the standalone task is **measured**
at **~6.0s** — the median of three alternating `moon run repo:input-liveness --force` runs, warm
(Task 7, §5 Step 2; recorded alongside the mutation battery in the README). That is comfortably below
the ~35s alternative, an order of magnitude down, confirming the decision this section makes.

Being independently scheduled is the other reason: it is the vehicle SMA-541's L6 names for
eventually closing the gate-inside-the-thing-it-guards hole. Cost: the task must be added to `T` and
to CLAUDE.md's marker region — which is SMA-541's own rule, so this change self-exercises C1 and C3.

**D3 — the negative control runs FIRST, inside the task's own script**, mirroring
`repo:affected-smoke` and `repo:publish-metadata`. Without it CI runs only the real check, so the
self-test that proves the assertions can FIRE is never executed and a rotted control ships green —
the failure SMA-526 hit. Moon does not enable errexit for `script:` blocks, so `set -euo pipefail`
is explicit, as every other multi-line `repo:*` script in this file already documents.

**D4 — "authored inputs" is the declared set minus Moon's injected inputs, and the subtraction is
guarded by COMPOSITION, not presence.** Forced by E2: without the subtraction, I3 asserts nothing.

The first draft asserted only that the injected glob is *present* on every repo task. Review showed
that guards the weaker half: it catches the glob disappearing or being renamed, but not a **change in
composition**. If a future Moon injected a second, *live* input — `.moon/**/*`, say, or the
`implicitInputs` of E13 starting to apply to locally-defined tasks — then every repo task would
satisfy I3 with **zero** authored inputs, and I3 would pass vacuously forever while the presence
check still passed. That is a false green, which is the one outcome worse than nothing.

So I5 asserts the **intersection** of `inputGlobs ∪ inputFiles` across *all* repo tasks equals exactly
`{INJECTED_GLOB}`, → rc 2 otherwise. Exact on today's tree: `install-hooks` (`moon.yml:11-13`) and
`actionlint` (`:477-478`) share nothing else, so the intersection is that one string. It catches an
added member, a rename, and a re-enabled `implicitInputs` alike. E13 is why this matters concretely
rather than hypothetically — the `implicitInputs` block already exists in the tree and is one
Moon-behaviour change away from applying.

**Ordering is load-bearing:** the injected glob contains braces, which D6 rejects. The subtraction
must happen strictly *before* classification, or every repo task reds.

**D5 — the matcher is `git ls-files`, "tracked" is the deliberate predicate, and this is a
REIMPLEMENTATION of `pattern_verdict`, not a reuse of it.** The first draft said "reuses
`pattern_verdict`"; that is not possible and the spec now says so plainly. `pattern_verdict` is a
bash function (`ci/actionlint/run.sh:919-973`) depending on three bash helpers in the same file
(`globstars_are_components:874`, `tracked_exact:886`, `has_dotty_segment:899`), and D1 puts this
check in Python.

Shipping a second copy of a policy is a real cost in a repo that runs two gates specifically to
prevent hand-rolled duplication (`repo:redis-connect-single-site`,
`repo:iam-docker-policy-single-site`). The justification is that the two matchers deliberately answer
**different questions**, so a shared implementation would be wrong rather than merely awkward:
`pattern_verdict` decides whether a pattern is legal and live under **GitHub Actions filter
semantics**, which is why it rejects `?`, `+` and `[]` outright (`run.sh:939-941`: "'?' is 'zero or
one of the PRECEDING character' on GitHub but 'any single character' in git") and why
`has_dotty_segment` exists ("GitHub filter patterns match the literal path text and do not"
normalise). This gate decides liveness under **Moon/wax** semantics, where those rationales do not
apply. What is shared is the *shape* — the verdict-token vocabulary and the fixture discipline — and
D6 keeps the token names identical so a reader moving between the two files is not learning a second
vocabulary.

Two corrections to the first draft's attributions, both verified:

- The "do not anchor on `^\./` — GNU grep emits the prefix, ugrep strips it" lesson is **not** in
  `pattern_verdict`. It lives in `moon.yml:315-319` (repeated at `:385-388`), in the
  `repo:redis-connect-single-site` comment, and it is about `grep -rn` over a directory tree — a
  matcher this gate does not use. Cited honestly or not at all. (`grep -c ugrep`: **0** in
  `ci/actionlint/run.sh`, **2** in `moon.yml`.)
- `rejected-dotty` is kept, but justified on this gate's own grounds — whether Moon normalises a
  dotty segment is unmeasured, so the gate refuses to guess — not on GitHub-Actions divergence.

**Tracked, not on-disk**, is the deliberate predicate, and the reason is stronger than the first
draft gave. Moon's input collection demonstrably does **not** honour `.gitignore`: that is the entire
reason `hasher.ignorePatterns` exists, and `.moon/workspace.yml:35-57` records that removing it makes
`repo:actionlint` ~8x slower "because the walk descends into pnpm's symlinked content-addressable
store", `node_modules/` being `.gitignore:15`. So a path under an ignored tree is collected by Moon
but excluded from hashing — it contributes nothing to any cache key — and `git ls-files` is the
cheapest available proxy for "can this path ever invalidate the task".

**Matcher failure polarity is pinned:** a non-zero rc from `git ls-files` is **rc 2** (infrastructure),
never a skip and never "no matches". rc 0 with empty stdout is `dead`. The first draft left this
unspecified, which is the difference between a false red and a permanently vacuous check on every
pattern. Every `git` call passes `cwd=<workspace root>` explicitly and `-c core.quotePath=false`:
`ls-files` only lists below its working directory, so running the script from inside
`ci/affected-graph/` would otherwise make every pattern read `dead`, and a quoted non-ASCII path
would read as a false `not-exact`.

**D6 — the pattern validator is default-deny, and a rejected pattern is rc 1, not a skip.** Before
any pattern reaches git it is classified. The vocabulary is `pattern_verdict`'s, extended by one
token:

- `!`-prefixed → **negated** → **skipped, not failed**. An exclusion must not be required to match
  anything; requiring it would be simply wrong. `pattern_verdict:928` has this verdict for the same
  reason, and the first draft dropped it — under that draft's charset rule, a legitimate
  `!ops/scratch/**` would have failed as `rejected-charset` with a message naming the wrong problem
  and offering no fix. E4 measured zero negated globs today, which is why the omission was invisible.
- `{` or `}` → **rejected-braces**, its own message. git pathspec has no brace expansion (E4), and
  expanding braces here is the hand-rolled parsing `ci/actionlint/run.sh:263-266` warns about *(the
  first draft cited that line as being about brace expansion; it is about hand-rolled YAML parsing —
  an apt analogy, cited accurately now)*. Zero authored brace globs exist.
- `?`, `+`, `[`, `]` → **rejected-charclass**, its own message, restored from `pattern_verdict:939`.
  The first draft folded these into `rejected-charset`, which would have told an author their pattern
  "contains characters this gate will not pass to git" — true, unactionable, and not the reason.
  Unlike `pattern_verdict`, this gate rejects them because git-vs-wax equivalence for them is
  **unmeasured** (E4 covers none), not because GitHub semantics differ; the message says so, and
  measuring them is the sanctioned way to lift the restriction.
- anything else outside `[A-Za-z0-9._/*-]` → **rejected-charset**. Doubles as the pathspec-injection
  guard: a pattern starting with `:` would otherwise be read by git as pathspec magic, and `--` plus
  quoting is necessary but not sufficient.
- a `**` that is not a whole path component → **rejected-globstar**.
- a `.`, `..` or empty path segment → **rejected-dotty** (D5's rationale).

A skip would be the silent-hole failure this whole issue is about. Failing means the gate says what
it will not evaluate, and the author extends the validator deliberately — D10's stance in SMA-541.

**D7 — two live-fire canaries run on EVERY real invocation**, following `ci/actionlint/run.sh`'s
"Check 7 — the self-tests, invoked for real" (`:1449-1459`), which calls `path_filter_self_test`
unconditionally so its fixture tables "guard the gate on every real run — without them the tables are
dead code in CI". *(The first draft claimed this "promotes it from the self-test into the production
path"; that novelty claim was false — actionlint already does exactly this. The design follows the
precedent rather than extending it.)*

A known-dead pattern must verdict `dead`, and a known-live one must verdict `ok`. This is the one
failure the fixture table cannot catch: a matcher stuck returning "live" passes I1, I2 and I4
vacuously while every check prints PASS. It costs one extra `git` call.

**D8 — one `moon query tasks` call, keyed by exact project id.** Not `moon query projects --id repo`
(E7: unanchored regex — a future project named `paigasus-repo-ts` would silently join the set).
`moon query tasks` returns `d['tasks']['repo']`, an exact key lookup with no filter to get wrong, and
carries everything needed: `inputFiles`, `inputGlobs`, `options`, and the resolved `script` that D10
uses.

E1, E2 and E6 were measured through `moon query projects` because that is the command the issue text
names. The **task object is identical under both** — verified: `moon query tasks`' entry for
`repo:affected-smoke` carries the same twelve keys (`command`, `description`, `id`, `inputFiles`,
`inputGlobs`, `inputs`, `options`, `script`, `state`, `target`, `toolchains`, `type`). So those three
findings transfer unchanged; only the envelope around the task object differs.

**D9 — `runInCI: false` tasks ARE checked.** SMA-541's C1 excludes `install-hooks` because it asks
"does CI run this". I1-I3 ask "can this task ever be scheduled at all", which matters for a local
`moon run` too. Its inputs (`lefthook.yml`, `.lefthook/**/*`) are live today, so this costs nothing
and closes a hole by default rather than by luck.

**D10 — the self-guard extends SMA-541's C4, via a SEPARATE extractor, with whole-line anchoring.**
D2 puts the gate in its own task, so its **existence** is guarded by C1: deleting `repo:input-liveness`
from `moon.yml` while leaving `:input-liveness` in `T` reds C1, and deleting both reds C3 against
CLAUDE.md. What remains unguarded is the task's **script**: rewriting it to drop the `--self-test`
line, or to swallow a failure, leaves everything green.

Three corrections to the first draft, all confirmed against the code:

- **The script is not available where the first draft assumed.** `ci_targets.py`'s `_eligibility`
  (`:270-330`) returns `{pid: {task: bool}}` — line `:320` is
  `row[name] = (options or {}).get("runInCI") is not False`. It discards `script`. Reshaping its
  return would break eight self-test fixtures including the exact-equality `want_polarity`
  (`:597-608`). So C4's new half is fed by a **second pure extractor**, `_scripts(projects)`, reading
  the same raw JSON `moon_tasks()` already fetches. `_eligibility` and its fixtures are untouched.
- **The two new call sites are prefix-contained.** `python3 ci/affected-graph/task_inputs.py` is a
  substring of `python3 ci/affected-graph/task_inputs.py --self-test`, and `check_self_invocation`
  (`:474-476`) is a plain `site in text` test — so deleting the real run would leave C4 green. Each
  required site is therefore matched as a **whole line** (stripped), not as a substring. This is the
  same class of hole the existing constant's comment records being burned by (`:169-175`: "Matching
  the prefix alone left `--self-test || true` looking identical to a wired call site").
- **The two texts are checked separately.** `run.sh`'s sites are looked for in `run.sh`; the task's
  sites in the resolved `script`. Concatenating them would let a call site in the wrong file satisfy
  the check. The constant is renamed accordingly, since it no longer describes `run.sh` alone.

**The claim that this closes L6 is withdrawn.** The first draft said "Neither can suppress the other,
so this has none of L6's circularity". That is false in one direction: C1 lives in `ci_targets.py`,
which runs inside `repo:affected-smoke`, and SMA-541's L6 records that removing `:affected-smoke`
from `T` and CLAUDE.md switches C1-C5 off. So the new gate's existence guard sits downstream of an
open hole. Recorded as L8 rather than claimed as closed; D13 mitigates the part of it that is cheap
to mitigate.

**D11 — `ALLOW_DEAD_INPUT` ships empty, with a required non-empty reason.** `repo` is measured 100%
clean (E3), so unlike `T_EXEMPT` there is not even a hypothetical entry on day one. It mirrors
`ALLOW_NO_CARGO_BACKING` (`cargo_moon_parity.py:53-61`): a `{(task, pattern): reason}` map where an
empty reason is itself an assertion failure. The key's `pattern` may name **either** an `inputGlobs`
or an `inputFiles` entry — the allowlist covers I1 and I2 alike, and the staleness rule below checks
both fields. Two staleness rules, from SMA-541 D5's leftover-exemption lesson: an entry naming a task
that does not exist is rc 1, and so is one naming a pattern that task does not declare in either
field. A typo is loud either way — the real pattern shows up as a violation — but a leftover entry
exempts nothing, forever, and is silent.

**D12 — exit codes 0/1/2, with rc 2 reserved for genuine tool failure**, exactly as SMA-541 D2.
`moon` or `git` failing (D5), output that will not parse as JSON, or a shape lacking a key the gate
needs → rc 2. Every authorial mistake — a dead glob, an untracked file, an empty authored input set,
a stale allowlist entry — is rc 1 with a message naming what to edit. Unlike `ci_targets.py`, an rc 2
here aborts only this task, not a suite of eight other assertions, because D2 gave the gate its own
task; the split is kept anyway so the two siblings triage identically.

**D13 — the gate asserts its OWN inputs, in both places.** *(New, from review — the sharpest finding.)*
D2 makes `inputs: ['**/*']` load-bearing, but nothing in the first draft asserted the task keeps it.
Narrowed to `ops/**/*` "for cost", `repo:input-liveness` is still live under I1, still has ≥1 authored
input under I3, still passes C1/C2/C3/C5 — and stops firing on exactly the renames it exists to
catch. The headline AC becomes silently false with nothing red: the issue's own failure class,
reproduced inside the fix.

So the authored glob set of `repo:input-liveness` must equal exactly `('**/*',)`. The assertion is
placed **twice**: in `task_inputs.py` as an I5 floor, and in `ci_targets.py` so it also runs inside
`repo:affected-smoke` — a gate judging only its own inputs is the circularity D10 is otherwise
avoiding. This is `EXPECTED_MOON_CI_INVOCATIONS` (`ci_targets.py:110-117`) and `REQUIRED_FFI_TASKS`
(`cargo_moon_parity.py:94-103`) applied to this gate's own configuration.

## 4. Components

### `ci/affected-graph/task_inputs.py`

**Inputs** — two:

| # | Source | Used for |
|---|---|---|
| 1 | `moon query tasks` (one call, D8) | `repo`'s `{task → inputFiles, inputGlobs, options}` |
| 2 | `git ls-files` / `git ls-files -- ":(glob)P"` | the tracked set, and per-pattern liveness |

**Structure**, mirroring `ci_targets.py`'s pure/subprocess split so every rc-2 raise is fixturable:

| function | kind | role |
|---|---|---|
| `moon_tasks()` | subprocess | runs `moon query tasks`, `json.loads`, delegates to `_repo_tasks` |
| `_repo_tasks(projects)` | **pure** | shape checks + the D4 intersection; raises `MoonOutputError` |
| `tracked_files()` / `glob_matches(p)` | subprocess | `git ls-files`, `cwd=root`, non-zero rc → rc 2 |
| `classify(pattern)` | **pure** | D6's verdict for one pattern |
| `check(tasks, tracked, matcher)` | **pure** | I1-I5, returns violation rows |
| `self_test()` | — | drives the three pure functions against fixtures |

**Checks:**

- **I1 — no dead glob.** Every authored glob must match ≥1 tracked file. Failure names the task and
  the pattern, and says a rename or move is the likely cause.
- **I2 — no dead file input.** Every authored `inputFiles` entry must be a member of the tracked set,
  by **exact set membership** — not a non-empty `ls-files` result, which per E14 passes for any
  directory path. Separate verdict from I1 (`not-exact` vs `dead`) because the fix differs.
- **I3 — no task without authored inputs.** After the D4 subtraction, each task must declare ≥1.
- **I4 — every authored pattern is evaluable.** Any `rejected-*` verdict from D6 is a failure naming
  the pattern and the specific reason. A `negated` verdict is skipped, not failed.
- **I5 — anti-vacuity floors.** rc 1 unless noted:
  - `d['tasks']['repo']` absent or empty → **rc 2**
  - the tracked-file set empty → **rc 2**
  - the D4 **intersection** across all repo tasks ≠ exactly `{INJECTED_GLOB}` → **rc 2**
  - `repo:input-liveness`'s authored glob set ≠ exactly `('**/*',)` → **rc 1** (D13)
  - `REQUIRED_TASKS = ('affected-smoke', 'input-liveness', 'promtool', 'publish-metadata')` must all
    be present — the `REQUIRED_FFI_TASKS` precedent. *(Note the honest scope: this catches a task
    RENAMED or made `internal: true` while the gate still runs. It does **not** catch
    `repo:input-liveness` itself vanishing — if the task is gone, nothing executes this file. C1 is
    what catches that, per D10. The first draft claimed the self-naming entry made a vanished task
    red, which is unreachable.)*
  - the two D7 canaries
  - every `ALLOW_DEAD_INPUT` entry carries a non-empty reason, names an existing task, and names a
    pattern that task declares in `inputGlobs` or `inputFiles` (D11)

### `moon.yml`

A new `repo:input-liveness` task, written out here because D10 pins its script text exactly:

```yaml
  input-liveness:
    description: 'Assert every repo:* task input still matches a tracked file, so a wired gate cannot silently stop firing (SMA-553).'
    # inputs MUST stay ['**/*'] — asserted by I5 (D13) and by ci_targets.py. This gate's verdict
    # depends on the whole tracked tree: a glob dies when files MOVE, and no narrow input list can
    # observe that. Same reasoning repo:actionlint records, and the reason this is not folded into
    # repo:affected-smoke, whose narrow inputs would serve a cached PASS on exactly the rename that
    # kills a gate (D2).
    #
    # Negative control FIRST, mirroring repo:affected-smoke and repo:publish-metadata: without it CI
    # runs only the real check and a rotted self-test ships green (SMA-526). Moon does not enable
    # errexit for `script:` blocks, hence the explicit `set -euo pipefail`.
    script: |
      set -euo pipefail
      python3 ci/affected-graph/task_inputs.py --self-test
      python3 ci/affected-graph/task_inputs.py
    toolchain: 'system'
    inputs:
      - '**/*'
```

### `ci/affected-graph/ci_targets.py`

`RUN_SH_CALL_SITES` grows from two entries to four and is renamed, since it no longer describes
`run.sh` alone; the two texts are matched separately and by whole line (D10). A new `_scripts`
extractor feeds the task-script half. The D13 assertion on `repo:input-liveness`'s inputs is added
here as well as in `task_inputs.py`. Its existing C4 fixture row is extended to cover the two new
entries, including a row proving the prefix-contained pair is distinguished.

### `.github/workflows/ci.yml`

`:input-liveness` appended to the single-line `T=(…)` array at line 215.

### `CLAUDE.md`

`:input-liveness` added inside the `ci-targets` marker region **at the same ordinal position** as in
`T` — C3 is an *ordered*, token-for-token mirror (`ci_targets.py:399-410`), so an off-by-one reds the
PR that adds the gate. Plus a gotcha line recording that a `repo:*` task's inputs are now asserted
live.

### `ci/affected-graph/README.md`

A bullet describing I1-I5, the `ALLOW_DEAD_INPUT` contract, and the measured cost.

## 5. Testing

`--self-test` drives the **pure functions** against in-memory fixtures. Every row below names a
fixture that **exists** in `self_test()` — a documented-but-absent control is the same drift class
this issue exists to close, and SMA-541's §9 changelog records shipping exactly that defect.

| Fixture | Expected |
|---|---|
| a task with a glob matching nothing | I1 **red**, naming task + pattern |
| a task with an untracked file input | I2 **red**, naming task + path |
| a file input naming a tracked *directory* (E14 shape) | I2 **red** — exact membership, not prefix |
| a task whose only glob is the injected one | I3 **red** |
| a bare directory arriving as `dir/**/*` (E6 shape) | **green** |
| a `!`-negated glob | **skipped**, not failed (D6) |
| brace / charclass / charset / non-component-`**` / dotty | I4 **red**, one row each, distinct messages |
| an allowlisted `(task, pattern)` with a reason, on each of `inputGlobs` and `inputFiles` | **green** |
| an allowlist entry with an empty reason | **red** |
| an allowlist entry naming no repo task | **red** — the exemption outlived its task |
| an allowlist entry naming a pattern the task declares in neither field | **red** |
| a task set missing a `REQUIRED_TASKS` member | **red** |
| `input-liveness` with inputs narrowed to `ops/**/*` | **red** (D13) |
| a stub matcher stuck returning "live" | **red** via the dead canary (D7) |
| an empty `d['tasks']['repo']` | **rc 2** |
| an empty tracked set | **rc 2** |
| an intersection with a second member, and one with none | **rc 2** each (D4) |
| a non-zero rc from the matcher | **rc 2**, not "no matches" (D5) |
| everything aligned | **green** — catches a permanently-red harness |

Two rows the fixture table **cannot** carry, because they assert wiring rather than logic, and which
are therefore §5 mutations instead:

- **AC #4 — `repo:actionlint`'s `**/*` does not trip the gate.** Asserting `**/*` verdicts `ok`
  against a *stub* matcher proves nothing about the real task. The control is the D7 live canary plus
  the unmutated real run being green, with `actionlint`'s row confirmed present in the real output.
- **D7's canaries are actually wired.** The fixture proves the canary function works; it does not
  prove `main()` calls it. Mutation 8 below covers that.

Verification by **mutation against the real tree**, run and recorded rather than assumed:

1. point `repo:promtool`'s glob at a nonexistent directory → the gate names the task and the glob
2. `git mv ops/observability ops/obs` → the same, for `promtool` **and** `observability-drift`
3. set a `repo` task to `inputs: []` → I3 fires
4. narrow `repo:input-liveness` to `ops/**/*` → D13 fires, in `task_inputs.py` **and** in
   `ci_targets.py`
5. add an `ALLOW_DEAD_INPUT` entry with an empty reason → red
6. delete the real-run line from the task's script, leaving `--self-test` → C4 fires (the
   prefix-containment case)
7. remove `:input-liveness` from `T` → C1 fires; remove it from CLAUDE.md → C3 fires
8. delete a canary call from `main()` → the run must red
9. the unmutated tree is green

The real wall-clock cost of `repo:input-liveness` is measured and recorded in the README and in E9,
replacing D2's estimate.

## 6. Limitations

- **L1 — `repo` only.** E3 measured 98 dead globs across the other 27 projects, essentially all
  speculative convention globs inherited from `.moon/tasks/{rust,typescript,python}.yml`, and the
  `py:lint` spot check confirms they are harmless rather than symptoms. Covering them needs
  authored-vs-inherited provenance, which E5 shows Moon's merged output does not record.
  Widening is **not** free even if those globs are pruned: E3's other row — the four untracked
  `README.md`/`LICENSE` inputs of §7 — are genuine defects, so they would red first. Pruning the
  templates is necessary but not sufficient; fixing §7 is a named precondition of any widening.
- **L2 — liveness, not sufficiency.** A glob matching one file when it should match twenty is
  invisible. This gate asserts a task *can* be scheduled, not that its inputs are complete. The
  complementary assertion — "this edit makes that task affected" — is what `run_task_case` in
  `ci/affected-graph/run.sh` does, case by case.
- **L3 — untracked-but-real files read as dead.** Correct for cache-key purposes (D5), but a
  generated, gitignored input would be reported as a violation. None exists on `repo` today, and the
  intended resolution if one appears is to fix the input, not to allowlist it: a gitignored path
  under `hasher.ignorePatterns` genuinely cannot invalidate the task, so a red there is right.
- **L4 — E10's invariant is enforced incidentally, and only for `repo`.** Two separate gaps. The
  invariant says "**no task** may declare an `inputs:` path under these trees"; this gate scopes to
  `repo`'s 18 of 119 tasks. And the enforcement is a coincidence of those trees being gitignored —
  `.gitignore:15` for `node_modules/` — rather than of anything asserting the `hasher.ignorePatterns`
  list itself. Asserting it directly means reading `.moon/workspace.yml`, which is YAML parsing this
  file avoids. *(The first draft said removing an entry from `.gitignore` would end the coverage;
  that is wrong — the files would also have to be `git add`ed. The real fragility is that the two
  lists are maintained independently with nothing comparing them.)*
- **L5 — bare-directory handling rests on E6.** Because Moon expands `inputs: ['ops/nats']` into a
  glob, I2 needs no directory-prefix fallback. A future Moon that stopped doing that would make a
  directory input read as an untracked file — a false red, not a false green, and no floor catches
  it. Recorded rather than pre-solved, because unreachable defensive code is its own drift.
- **L6 — a `repo` gate defined outside the root `moon.yml`** — a future `ci/newgate/moon.yml`
  declaring its own project — is invisible to this gate, exactly as it is to SMA-541's C1 (its L9).
  The fix, if `ci/` ever grows a second project, is the same: widen the partition to an explicit list
  of gate-owning project ids.
- **L7 — the gate cannot see a task switched off with `internal: true`.** Per SMA-541's E3,
  `moon query tasks` omits an internal task entirely, so I1-I3 never consider it. `REQUIRED_TASKS`
  catches it for the four named tasks only.
- **L8 — this gate's existence guard sits downstream of SMA-541's open L6.** C1 is what makes
  deleting `repo:input-liveness` red, and C1 runs inside `repo:affected-smoke`. SMA-541's L6 records
  that removing `:affected-smoke` from `T` and from CLAUDE.md — two edits every check would accept as
  mutually consistent — switches C1-C5 off entirely. So the sequence "delete `:affected-smoke`, then
  delete `repo:input-liveness`" is green at every step. D13's second placement narrows this (an
  input-narrowing is caught by two independently-scheduled tasks) but does not close it. Closing it
  needs an assertion in a gate neither can suppress; `repo:actionlint` remains the natural host, as
  SMA-541 already notes.
- **L9 — E4's equivalence is five measured patterns plus two refuted hypotheses, not a proof.**
  Case-folding (E12) and dot-paths (E11) were the two divergences raised in review and both were
  measured. Untested and knowingly so: sparse checkouts (`git ls-files` reads the index, so a path
  absent from the working tree still counts as live — a false green), submodule contents (absent from
  the superproject index — a false red), and `?`/`[]`/`+`, which D6 rejects outright rather than
  guess at. None occurs in this repo today.
- **L10 — break-glass.** The fix path for a red is always "fix the input, or record an
  `ALLOW_DEAD_INPUT` entry with a reason". There is no warn-only mode by design.

## 7. Finding outside this issue's scope

`paigasus-ml-py:build` and `paigasus-workflows-py:build` declare `README.md` and `LICENSE` as inputs,
inherited from `.moon/tasks/python-project.yml:27`. **None of the four files exists** — not
gitignored, absent (`ls` shows only `moon.yml`, `pyproject.toml`, `src/`). Both packages are
PyPI-bound with `uv_build`, which is the SMA-378 gotcha: `uv_build` does not auto-glob license files,
so a wheel built today would ship no license text.

Real, latent (no release workflow is wired — the `python-semantic-release` config is dormant), and in
a different workspace than this issue's subject. Filed as **SMA-556** rather than fixed here, to keep
this PR's diff to the gate. Per L1 it is also a precondition of ever widening this gate's scope, so
SMA-556 is a dependency, not a detached note.

## 8. Acceptance criteria

| Issue AC | Covered by |
|---|---|
| Renaming a directory a `repo:*` task's `inputs` depend on fails the gate, naming the task and the dead glob | I1, made reachable by D2's `inputs: ['**/*']` (E11) and kept reachable by D13; §5 mutation 2 |
| A task whose resolved input set is empty fails the gate | I3 — reinterpreted per E2 as the **authored** set, since the resolved set is never empty (D4) |
| The check carries its own control proving it can fire, verified by mutation rather than asserted | `--self-test` run first by the task itself (D3), D7's canaries on every real run, and the nine §5 mutations |
| `repo:actionlint`'s `inputs: ['**/*']` does not trip it | E4 (714 tracked matches) + the real-run control in §5 — deliberately *not* a stub-matcher fixture row |
| Whatever remains uncovered is recorded honestly, in the manner of SMA-541's §6 | §6, L1-L10 |

## 9. Changelog — adversarial review (2026-08-19)

**Folded in.** D13, an entirely new decision: the gate must assert its own `inputs: ['**/*']`, in
both `task_inputs.py` and `ci_targets.py`, or the headline AC is disableable by a one-line edit the
gate itself calls live — the issue's failure class reproduced inside the fix. D4 strengthened from a
presence check to a composition (intersection) check, after E13 showed a seven-entry `implicitInputs`
block already exists in the tree one Moon-behaviour change away from making I3 vacuous. D10 rebuilt
three times over: `_eligibility` does not carry `script` so a separate `_scripts` extractor is needed
(the first draft's plan was unbuildable); the two new call sites are prefix-contained so whole-line
anchoring is required (the first draft's plan was undetectable); and the two texts must be checked
separately. D5's "reuses `pattern_verdict`" corrected to "reimplements", with the justification that
the two matchers answer different questions, plus the misattributed ugrep lesson removed and the
matcher's failure polarity pinned to rc 2. D6 gained `negated` (skip) and `rejected-charclass`
(restored from `pattern_verdict`), both dropped by the first draft. I2 specified as exact set
membership after E14 showed a wildcard-free pathspec prefix-matches a directory. §4 now writes the
`moon.yml` task out verbatim and names the pure/subprocess split; the CLAUDE.md edit is pinned to the
same ordinal position as `T`, since C3 is an ordered mirror. §5's AC #4 row demoted from fixture to
real-run control, and a mutation added for canary wiring. §1's motivating example corrected —
`repo:promtool` has three inputs, so an `ops/` rename is partial input death, not total. E9's two
quoted figures marked as reference points rather than measurements, and D2's cost basis relabelled an
estimate with a revisit trigger. L1, L4 and I5's self-naming rationale each corrected; L8 (SMA-541's
L6 inherited) and L9 (untested glob divergences) added; D10's "none of L6's circularity" claim
withdrawn.

**Rejected, with reason.** *Case-folding false-green* — the hypothesis was that git case-folds
pathspecs on APFS while wax does not; measured and refuted (E12), `core.ignoreCase=true` yet
`:(glob)OPS/…` returns 0. *Wax may not match dot-paths* — refuted (E11); flagged as the highest-value
unmeasured assumption and it holds. *Host the check in `ci/actionlint/run.sh`* — rejected in D1: that
gate's subject is GitHub workflow files and its matcher answers a different question. *A shared
pattern/verdict corpus driving both implementations* — rejected for the same reason; forcing the two
to agree would make one of them wrong, since `pattern_verdict` encodes GitHub Actions semantics
deliberately. The shape is shared instead: identical verdict tokens, identical fixture discipline.
