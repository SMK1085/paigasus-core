# SMA-525 — Gate workflow YAML with actionlint

**Status:** revised after adversarial review (2026-08-16)
**Linear:** [SMA-525](https://linear.app/smaschek/issue/SMA-525/repo-gate-workflow-yaml-with-actionlint-a-malformed-paths-filter-fails)
**Related:** SMA-520 (added the `paths:` filter that surfaced this), SMA-375 (proto-pinned CLI gates), SMA-448 (the same silent-failure class)

## 1. Problem

Nothing in this repo lints `.github/workflows/**`. No Moon task takes
`.github/workflows/prebuild.yml` as an input (`repo:affected-smoke` takes `ci.yml`, and only to
assert every `moon ci` invocation carries `--include-relations`), and `actionlint` is neither
pinned in `.prototools` nor run anywhere in CI.

The failure this leaves open is **silent and permanent**. `prebuild.yml` triggers on
push-to-`main`, `workflow_dispatch`, and a narrow `pull_request` filter. If its `paths:` filter
comes to match nothing — a mistyped glob, a dropped `/**`, a renamed directory — GitHub raises no
error. The workflow simply stops running, forever. There is no red check, no notification, and no
per-PR status on the `push` path, so the 7-platform prebuild verification would silently cease.

This is the SMA-448 class (a Windows-reserved filename passed the Linux-only gate and reddened
`main` only after merge), except worse: here `main` would not go red at all. It would go quiet.

SMA-520 mitigated its *own* change by arranging a positive test — that PR's merge commit touches
`prebuild.yml`, which is on the `push` allowlist, so prebuild had to run. That is a property of
one change, not a standing guard for the next edit.

## 2. Evidence

Measured against `actionlint` 1.7.12 on 2026-08-16, before any design decisions were made.
Everything in this section was observed, not reasoned about.

**The current tree is clean.** `ci.yml`, `prebuild.yml` and `security-scan.yml` all pass, with and
without shellcheck. Adding the gate requires no cleanup wave.

**What actionlint catches:**

| Mutation | Verdict |
|---|---|
| `paths:` nested under `workflow_dispatch:` | caught, `[syntax-check]` |
| `path:` instead of `paths:` | caught, `[syntax-check]` |
| Malformed glob `'rs/[**'` | caught, `[glob]` |
| Unknown runner label `ubunut-latest` | caught, `[runner-label]` |
| Undefined `steps.nope.outputs.x` | caught, `[expression]` |
| **Typo'd but valid glob** `'rz/**'`, `.github/workflow/prebuild.yml` | **NOT caught — exit 0** |

The last row is why this spec is not simply "pin actionlint". It is precisely the "mistyped glob,
a path that never matches" failure the issue names as its motivation, and actionlint is
structurally unable to see it: it validates *syntax* and has no view of the file tree. A gate
built on actionlint alone would close this issue while leaving its headline scenario unguarded.

**Three further behaviours**, all load-bearing below:

- `actionlint -` lints stdin and applies the workflow schema regardless of `-stdin-filename`, so a
  permanent proof-that-it-bites needs no fixture files on disk and nothing broken ever lands in
  `.github/workflows/` where GitHub would try to parse it.
- `actionlint` exits **3** when the workflows directory is missing or empty, so check 1 needs no
  separate "did it lint anything" control.
- A `.github/actionlint.yaml` carrying `paths: {"…": {ignore: [".*"]}}` makes actionlint exit **0**
  on a workflow with an unknown runner label. Critically, **stdin fixtures are not suppressed by
  that config** even when `-stdin-filename` names a matching path — verified. So the self-tests of
  checks 3 and 4 cannot detect a neutering config; only an explicit assertion can (check 2).

**Not verified:** §1 asserts GitHub itself raises no error for a nested `paths:` key. That claim
comes from the issue and was not tested here — testing it means pushing a broken workflow to
`main`. If GitHub does reject it, that one mutation is already loud and the motivation narrows to
the typo'd-glob case, which *raises* the importance of check 5 rather than lowering it. Either way
the design is unchanged, so the claim was not worth an experiment on `main`.

## 3. Decisions

**D1 — Scope: actionlint *plus* a path-existence control.** One Moon task running both. actionlint
covers syntax, keys, runner labels and expressions; a companion control covers the never-matching
glob it cannot see. This matches how every other gate here carries a control proving it guards
something (`ci/osv/run.sh` asserts a per-lockfile package count; `repo:redis-connect-single-site`
asserts its `expected` set is non-empty).

**D2 — shellcheck integration disabled explicitly (`-shellcheck= -pyflakes=`).** actionlint shells
out to `shellcheck` for `run:` blocks when it finds one on PATH, making the gate's strictness a
property of the host: 0.11.0 here via homebrew, whatever `ubuntu-latest` ships in CI, possibly
nothing on a fresh box. Pinning shellcheck via proto was considered and rejected: as of 2026-08-16
the `v0.11.0` release publishes 13 archives and **no checksums asset**, so
`.proto/plugins/shellcheck.toml` would be the first vendored tool here downloaded without
integrity verification — a posture regression in a repo that pins action SHAs, runs osv-scanner
and enforces cargo-deny. An issue that exists to make a silent failure loud should not ship a
sub-check whose strictness varies by machine. Inline-bash linting gets its own follow-up.

**D3 — Proof by permanent self-test, not one-off demonstration.** AC-3 requires a broken `paths:`
filter to be *demonstrated* to fail, "not merely assumed to". A demonstration performed once
decays the moment the invocation changes. Using stdin (§2) the proof runs on every invocation at
negligible cost. Precedent: `--negative-control` in `ci/affected-graph/run.sh`.

**D4 — Glob matching splits on wildcard presence.** `git ls-files -- ':(glob)P'` alone is **not**
a sound model of GitHub's filter patterns, and the naive version of this check would have
false-greened the exact scenario the issue was filed for. Two measured divergences drive the
design:

- *Wildcard-free patterns take directory-prefix semantics under git.* `':(glob)rs'` matches 320
  tracked files and `':(glob)rs/'` likewise; GitHub matches **nothing**, because filter patterns
  match file paths and no file is named `rs`. Dropping a `/**` is among the likeliest hand-edits,
  and `prebuild.yml` lists `.moon/**` twice. A wildcard-free pattern is therefore required to be an
  **exact tracked file path** — it must appear verbatim in `git ls-files` output — never a prefix.
- *`**` means different things.* GitHub defines `**` as "zero or more of any character",
  slash-crossing anywhere in the string; git's wildmatch under `WM_PATHNAME` only crosses `/` when
  `**` is a whole path component. GitHub documents `'**.js'` as "all .js files in the repository";
  under `:(glob)` it yields **0** (measured; `**.yml`→2 root-level files, vs `**/*.yml`→57). A
  pattern like that would false-red the *only* required check.

So check 5 accepts a restricted vocabulary where the two matchers provably agree — literals,
`dir/**`, `**/name`, and `*` within a single segment — and **rejects loudly**, naming the
divergence, on `?`, `+`, `[...]`, or any `**` that is not a whole path component. An unsupported
pattern produces a clear failure telling the author to use the documented skip list, never a
silently wrong verdict in either direction.

**D5 — Zero-dependency `awk` extraction, with a specified contract and self-tests.** The
alternative is a real YAML parser. `ci/osv/run.sh` does invoke system `python3`, but only for
`import json` (stdlib) — it is not a precedent for PyYAML, whose presence on a GitHub runner's
system interpreter is not something to bet a required check on. The strongest alternative is
`uv run --with pyyaml==<pin>` (uv is pinned in `.prototools`), which is deterministic but adds a
resolve/network step to a gate that must also run offline. `awk` is kept, and the two things that
make hand-rolled YAML parsing dangerous are addressed head-on: §4.3.1 specifies the contract
exactly, and check 7 tests the extractor against a fixture table rather than trusting it.

**D6 — Broad `inputs:`, because the cost was measured.** The first draft keyed this task on
`.github/workflows/**` only and filed the consequence as an accepted limitation. That was wrong:
a directory rename is the *dominant* real-world way a glob comes to match nothing, and Moon would
have kept serving a cached pass indefinitely — reproducing §1's "silent and permanent" inside the
fix. Concretely, `security-scan.yml` filters on `ci/osv/**`, and this very PR adds a sibling
`ci/actionlint/`, making a future `ci/` reshuffle plausible. The trade was asserted but never
quantified; measured, the whole gate — actionlint over three workflows plus 14 `git ls-files`
calls — runs in **109 ms**. There is no trade. See §4.2 for the one open cost.

## 4. Architecture

Six files.

| File | Change |
|---|---|
| `.proto/plugins/actionlint.toml` | new — vendored proto TOML plugin |
| `.prototools` | `actionlint = "1.7.12"` + `[plugins]` entry |
| `ci/actionlint/run.sh` | new — the gate and its controls |
| `ci/actionlint/README.md` | new — matching `ci/affected-graph/`, `ci/release-parity/` |
| `moon.yml` | new `repo:actionlint` task |
| `.github/workflows/ci.yml` | `:actionlint` into `T=(…)`; step-name update |

`CLAUDE.md`'s "run the full graph like CI does" command enumerates every gate target and must gain
`:actionlint` in the same PR, or the documented pre-push command silently omits the new gate.

### 4.1 The proto plugin

Verified against the real v1.7.12 release rather than assumed by analogy:

- Assets are `actionlint_{version}_{os}_{arch}.tar.gz` with the binary at **archive root**, so
  **no `exe-path`** is needed on Linux or macOS — unlike `promtool` and `cargo-deny`, whose
  binaries nest one directory deep. Windows ships a `.zip` with `actionlint.exe` at root.
- `actionlint_{version}_checksums.txt` exists, so `checksum-file` is set on every platform and
  every vendored tool here stays integrity-verified. The `{version}` interpolation in a checksum
  filename is precedented by `cargo-deny.toml`, `cargo-machete.toml` and `cargo-nextest.toml`
  (`promtool`/`osv-scanner`, cited elsewhere as the model, happen to use static names).
- Asset names use Go's `amd64`/`arm64` uniformly across all three OSes, so one global
  `[install.arch]` remap suffices and no `[platform.*.arch]` override is required — the
  `osv-scanner` shape, not the `buf`/`lefthook` shape. The SMA-411 `proto = "0.58.1"` floor is
  unaffected either way.
- Tags are `v`-prefixed (`v1.7.12`) while filenames embed the bare version — the `promtool` shape.
- `[resolve] git-url` is set, as on all nine existing plugins.

`.prototools` is covered by none of dependabot's four ecosystems (cargo `/rs`, npm `/ts`, uv `/py`,
github-actions `/`), so this pin will not receive bump PRs and can rot. That matters more than
usual here because L2 makes pin rot merge-blocking; noted in §7.

### 4.2 The Moon task

`repo:actionlint`, hosted on the root `repo` project, `toolchain: 'system'`, `script:
'ci/actionlint/run.sh'`, with a `description:` like every other `repo:*` task. `run.sh` opens with
the SPDX header (`ci/osv/run.sh` precedent).

`inputs:` is deliberately broad — `**/*` — per D6, because check 5 is a statement about the whole
file tree, not about the workflow files. The narrow-inputs convention used by the other `repo:*`
gates exists because those gates are expensive (`cargo nextest`, `next typegen`); this one is
109 ms.

**Open cost, to be measured in implementation:** broad `inputs:` makes Moon hash the whole tree for
this task's cache key, and that hashing cost — unlike the runtime — has not been measured. If total
wall time (hash + run) exceeds ~2 s, split into `repo:actionlint` (narrow inputs, the linter) and
`repo:workflow-path-filters` (broad inputs, checks 5–7), which isolates the hashing to the cheap
half. Both targets then go into `T=(…)`. The fallback is defined now so the decision is not made
under pressure later.

**Shell settings.** `set -uo pipefail`, *not* `set -e` — matching `ci/osv/run.sh`. Moon does not
enable errexit for `script:` blocks, which is why several gates here need explicit care, but `-e`
is the wrong cure for *this* script: check 3 deliberately *expects* non-zero exits and grep-based
extraction legitimately finds nothing. Every check therefore captures status explicitly
(`if ! cmd`, `rc=$?`, `|| true`) and the script tracks a failure flag, so one failing check cannot
be masked by a later passing one.

**Exit codes** follow the `ci/` convention: **1** = assertion failure (a real finding), **2** =
infrastructure error (actionlint missing, `git ls-files` failing, `awk` error, not a git repo).
Without the split, a broken tool reads as a lint failure — or worse, as a pass.

### 4.3 What `run.sh` checks

| # | Check | Catches |
|---|---|---|
| 1 | `actionlint "${ARGS[@]}"` over the auto-discovered workflow set | syntax, wrong key, malformed glob, unknown runner label, bad expressions |
| 2 | No `.github/actionlint.{yaml,yml}` carrying an `ignore:` key | a config that neuters check 1 invisibly (§2) |
| 3 | One malformed fixture per AC-1 class via `actionlint -`, each **must fail with its expected rule tag** | the gate having been neutered by a targeted `-ignore` or a narrowed rule set |
| 4 | A healthy fixture via `actionlint -` **must pass** | control for 3: a globally broken invocation would otherwise read as "malformed input correctly rejected" |
| 5 | Every `paths:` glob is in the supported vocabulary and matches the tree (D4) | the typo'd-but-valid glob of §2; a dropped `/**` |
| 6 | Any workflow containing a `paths:`/`paths-ignore:` key yields ≥1 extracted sequence item | an unsupported YAML form becoming a silent skip |
| 7 | Extractor self-test against a fixture table | the extractor silently mis-parsing real files |

Check 1 invokes `actionlint` **bare**, with no file arguments, relying on repository
auto-discovery. A `*.yml` argument list would silently miss a `.yaml`-suffixed workflow, and the
exit-3 behaviour of §2 only applies to auto-discovery — an explicit glob expanding to nothing
would pass vacuously as "no errors found". Checks 5–7 discover files the same way, over
`.github/workflows/*.{yml,yaml}`, non-recursive, matching GitHub's own execution semantics.

Checks 1, 3 and 4 share **one** `ARGS=(-shellcheck= -pyflakes=)` array. Written twice, an
`-ignore` added to check 1 would be invisible to check 3 by construction and the control would be
decorative. Check 3 asserts the *rule tag* (`[syntax-check]`, `[glob]`, `[runner-label]`,
`[expression]`) appears in output, not merely that the exit status was non-zero — otherwise a
fixture that fails at YAML parse satisfies the check while proving nothing about runner labels,
and a targeted `-ignore 'label .* is unknown'` on check 1 leaves it green. Check 3 is the standing
version of §2's evidence table.

Check 5 applies to `paths:` only. `paths-ignore:` is deliberately **excluded**: for `paths:`,
matching nothing kills the workflow, but for `paths-ignore:` matching nothing is a no-op and the
dangerous direction is matching *everything*. Requiring `paths-ignore` globs to match ≥1 file
would add false-red surface while guarding the wrong end; the real invariant there (a
`paths-ignore` set must not cover every tracked file) is out of scope and listed in §7.

Entries beginning with `!` are excluded from check 5's matching — they are exclusions, so
requiring them to match is wrong — but are still **counted by check 6**, which counts raw sequence
items *before* any filtering. Otherwise a legal all-negated block would grep positive, yield zero
post-filter globs, and hard-fail the required check.

The first draft carried an additional "total globs > 0" control. It is dropped: check 6 already
covers the real vacuity risk per-file, while a repo-wide count would false-red the legitimate
change of removing every path filter — which makes workflows run *more*, the fail-safe direction.

Failures print the offending file, pattern and match count. With `.moon/tasks.yml`'s
`outputStyle: 'buffer-only-failure'` a passing gate prints nothing, so the diagnostic table must be
on the failure path, as in `ci/osv/run.sh`.

Extracted patterns are validated against a conservative character class before use. A `paths:`
entry beginning with `:` would otherwise be read by git as pathspec magic; the `--` separator and
quoting are necessary but not sufficient.

#### 4.3.1 Extractor contract

Specified rather than left to the implementer, because the naive reading breaks on files already
committed here — `prebuild.yml`'s `pull_request.paths` block has three interior comment lines
mid-sequence and trailing `#` comments on four entries:

- A `paths:`/`paths-ignore:` key at any indentation opens a block; the block ends at the first
  content line whose indentation is **≤ the key's** (dedent), **not** at the first line that is not
  a `- ` item. Terminating early extracts 7 of 9 globs from `prebuild.yml` — and check 6 still
  passes, 7 being ≥ 1. That is exactly the silent partial extraction check 6 exists to prevent, so
  the contract, not the control, has to carry it.
- Whole-line comments inside a block are skipped **without** closing it.
- A trailing ` #` comment outside quotes is stripped; a `#` inside a quoted scalar is not.
- Unquoted, single-quoted and double-quoted scalars are all accepted; surrounding quotes stripped.
- The inline flow form (`paths: [a, b]`) is **not** parsed. Check 6 turns it into a loud failure
  naming the file, which is the difference between a limitation and a hole.

Check 7 tests each clause above against a fixture table of YAML strings with expected glob sets,
run on every invocation — the `--negative-control` pattern from `ci/affected-graph/run.sh`.

### 4.4 CI wiring

`:actionlint` joins the `T=(…)` array in `ci.yml`'s `moon ci` step, and the "Install pinned CLIs"
step name gains `actionlint`. `ci/affected-graph/run.sh` asserts only that every `moon ci`
invocation carries `--include-relations`, not the contents of `T`, so this addition does not
disturb the strict-equality guard — though editing `ci.yml` does make `:affected-smoke` affected,
which is expected.

AC-2 ("runs on every PR touching `.github/workflows/**`") is satisfied a fortiori by D6's broad
`inputs:`: the task is affected by any change at all, so it certainly runs on workflow edits.

## 5. Verification

- `moon run repo:actionlint` passes on the tree as committed.
- Verified with the **full graph the way CI runs it** — `moon ci … --base origin/main
  --include-relations` with the complete target array — not just the single task. Per-project tasks
  do not run repo-level gates, and this target is new to the array.
- Checks 2–4, 6 and 7 self-verify on every run.
- **AC-1 through the Moon target, not just the binary.** §2 proves the *binary* catches the three
  classes; that is not the same as the *gate* catching them. For each of invalid syntax, unknown
  runner label and bad expression: break `ci.yml`, confirm `moon ci :actionlint --base origin/main
  --include-relations` reds and names the rule, revert. Note that `moon ci :actionlint` exits 0
  having run nothing when the task is unaffected relative to the base, so a demonstration that does
  not actually touch a keyed input passes vacuously — the mutation must be committed for the check
  to be meaningful.
- **AC-3 for the path-existence control specifically.** Break a real `paths:` glob in
  `prebuild.yml` two ways — a typo (`rz/**`) and a dropped suffix (`rs`, the D4 false-green case) —
  confirm check 5 fails and names the pattern in both, then revert. Output recorded in the PR body.
- Confirm the §4.2 hashing cost against the ~2 s threshold, and split the task if it is exceeded.

## 6. Rollout and rollback

This lands inside `CI / moon ci`, which the `Protect main` ruleset makes the **only** required
check. A false red therefore wedges every merge in the repo, including the PR that would fix it,
so the escape hatches are defined up front rather than improvised:

- **A new GitHub runner label** the pinned binary does not know (L2): add it to
  `self-hosted-runner.labels` in `.github/actionlint.yaml`. Check 2 permits that file — it bans
  only an `ignore:` key — so this hatch and that control do not collide.
- **A GitHub-valid pattern outside check 5's supported vocabulary** (D4): add it to the
  commented, justification-required skip list in `run.sh`, the same shape as `deny.toml`'s license
  exceptions and cargo-machete's `ignored` allowlist.
- **Anything worse:** drop `:actionlint` from `T=(…)` in `ci.yml`. One line, no revert of the
  plugin or the script, and the gate can be re-enabled once fixed.

## 7. Limitations

Stated deliberately, so nothing reads as a stronger guarantee than it is.

- **L1 — `git ls-files` sees tracked files only.** A legitimately generated or gitignored target,
  or a new directory not yet `git add`ed, makes check 5 false-red for a developer with a dirty
  tree. Correct for CI, occasionally surprising locally.
- **L2 — Runner labels are baked into the pinned binary.** A genuinely new GitHub label reds the
  gate until the pin is bumped, and §4.1 notes dependabot will not send that bump. Escape hatch in
  §6.
- **L3 — No inline-bash linting** (D2). `ci.yml` and `prebuild.yml` carry ~120 lines of inline
  bash — including `sudo rm -rf` of preinstalled toolchains — that stay unlinted. Follow-up issue,
  to be filed with the PR.
- **L4 — The gate cannot prove a workflow *ran*.** It proves a filter is well-formed and its globs
  name real files. A filter whose globs all exist but which collectively never match a real change
  set is still possible.
- **L5 — `branches:` filters are not checked.** `branches: [mian]` silently kills a workflow in
  exactly the way §1 describes, and actionlint will not catch it. Excluded here to keep the blast
  radius of a change to the only required check contained: branch-existence checking has real
  false-red surface (a workflow may legitimately filter on a `release/**` branch that does not
  exist yet). Same follow-up as L3.
- **L6 — Meta, out of scope.** Nothing asserts that a `repo:*` gate is actually wired into
  `ci.yml`'s `T=(…)`; a future gate could be added and never run. Same silent-omission class one
  level up. Follow-up.

## 8. Non-goals

- Pinning shellcheck (D2).
- `dependabot.yml` — actionlint does not lint it.
- Composite actions — none exist in this repo.
- Post-merge verification that `prebuild` actually ran (L4).
- The `paths-ignore`-covers-everything invariant (§4.3).
