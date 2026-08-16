# SMA-525 — Gate workflow YAML with actionlint

**Status:** approved (2026-08-16)
**Linear:** [SMA-525](https://linear.app/smaschek/issue/SMA-525/repo-gate-workflow-yaml-with-actionlint-a-malformed-paths-filter-fails)
**Related:** SMA-520 (added the `paths:` filter that surfaced this), SMA-375 (proto-pinned CLI gates), SMA-448 (the same silent-failure class)

## 1. Problem

Nothing in this repo lints `.github/workflows/**`. No Moon task takes a workflow file as an
input, `:affected-smoke` scopes to `ci.yml` only, and `actionlint` is neither pinned nor run.

The failure this leaves open is **silent and permanent**. `prebuild.yml` triggers on
push-to-`main`, `workflow_dispatch`, and a narrow `pull_request` filter. If its `paths:` filter
breaks — a mistyped glob, a path that matches nothing, an indentation slip nesting `paths:`
under `workflow_dispatch:` — GitHub raises no error. The workflow simply stops running, forever.
There is no red check, no notification, and no per-PR status on the `push` path, so the
7-platform prebuild verification would silently cease and nobody would learn of it.

This is the SMA-448 class (a Windows-reserved filename passed the Linux-only gate and reddened
`main` only after merge), except worse: here `main` would not go red at all. It would go quiet.

SMA-520 mitigated its *own* change by arranging a positive test — that PR's merge commit touches
`prebuild.yml`, which is on the `push` allowlist, so prebuild had to run. That is a property of
one change, not a standing guard for the next edit.

## 2. Evidence

Measured against `actionlint` 1.7.12 before any design decisions were made.

**The current tree is clean.** `ci.yml`, `prebuild.yml` and `security-scan.yml` all pass, with and
without shellcheck. Adding the gate requires no cleanup wave.

**What actionlint catches**, tested case by case:

| Mutation | Verdict |
|---|---|
| `paths:` nested under `workflow_dispatch:` | caught, `[syntax-check]` |
| `path:` instead of `paths:` | caught, `[syntax-check]` |
| Malformed glob `'rs/[**'` | caught, `[glob]` |
| Unknown runner label `ubunut-latest` | caught, `[runner-label]` |
| Undefined `steps.nope.outputs.x` | caught, `[expression]` |
| **Typo'd but valid glob** `'rz/**'`, `.github/workflow/prebuild.yml` | **NOT caught — exit 0** |

The last row matters: it is precisely the "mistyped glob, a path that never matches" failure the
issue names as its motivation, and actionlint is structurally unable to see it. actionlint
validates *syntax*; it has no view of the file tree. A gate built on actionlint alone would close
this issue while leaving its headline scenario unguarded.

**Two further behaviours**, both load-bearing for the design below:

- `actionlint -` lints stdin and applies the workflow schema regardless of `-stdin-filename`.
  A permanent proof-that-it-bites therefore needs no fixture files on disk, and nothing broken
  ever lands in `.github/workflows/` where GitHub would try to parse it.
- `actionlint` exits **3** when the workflows directory is missing or empty. Check 1 below
  therefore needs no separate "did it actually lint anything" control.

## 3. Decisions

**D1 — Scope: actionlint *plus* a path-existence control.** One Moon task running both checks.
actionlint covers syntax, keys, runner labels and expressions; a companion control covers the
never-matching glob it cannot see. This matches how every other gate in this repo carries a
control that proves it is guarding something (`ci/osv/run.sh` asserts a per-lockfile package
count; `repo:redis-connect-single-site` asserts its `expected` set is non-empty).

**D2 — shellcheck integration disabled explicitly (`-shellcheck= -pyflakes=`).** actionlint
shells out to `shellcheck` for `run:` blocks when it finds one on PATH. That makes the gate's
strictness a property of the host: shellcheck 0.11.0 here via homebrew, whatever `ubuntu-latest`
currently ships in CI, possibly nothing on a fresh dev box. Pinning shellcheck via proto was
considered and rejected: upstream publishes **no checksums file** with its GitHub release, so
`.proto/plugins/shellcheck.toml` would be the first vendored tool in this repo downloaded without
integrity verification — a posture regression in a repo that pins action SHAs, runs osv-scanner
and enforces cargo-deny. An issue that exists to make a silent failure loud should not ship a
sub-check whose strictness varies by machine. Inline-bash linting gets its own follow-up issue
where the checksum question can be decided on its merits.

**D3 — Proof by permanent self-test, not by one-off demonstration.** AC-3 requires a broken
`paths:` filter to be *demonstrated* to fail, "not merely assumed to". A demonstration performed
once during implementation decays the moment the invocation changes — someone adds an `-ignore`
regex that swallows everything and nothing notices. Using stdin (see §2) the proof runs on every
CI invocation at negligible cost and leaves no artefacts in the tree.

**D4 — Glob matching via `git ls-files -- ':(glob)<pattern>'`.** Prototyped against all 18 real
globs in the tree: every one resolves (`rs/**`→320 files, `.moon/**`→14, dotfiles such as
`rs/.cargo/config.toml` included), and both typo cases return 0. Rejected alternatives: a
hand-rolled GitHub-filter-pattern-to-regex translator (a second thing needing its own tests, and
a source of subtle false positives), and a literal-prefix heuristic (weaker, and no simpler than
this).

## 4. Architecture

Five files.

| File | Change |
|---|---|
| `.proto/plugins/actionlint.toml` | new — vendored proto TOML plugin |
| `.prototools` | `actionlint = "1.7.12"` + `[plugins]` entry |
| `ci/actionlint/run.sh` | new — the gate and its controls |
| `moon.yml` | new `repo:actionlint` task |
| `.github/workflows/ci.yml` | `:actionlint` into `T=(…)`; step-name update |

### 4.1 The proto plugin

Verified against the v1.7.12 release rather than assumed by analogy:

- Assets are `actionlint_{version}_{os}_{arch}.tar.gz` with the binary at **archive root**, so
  **no `exe-path`** is needed on Linux or macOS — unlike `promtool` and `cargo-deny`, whose
  binaries nest one directory deep. Windows ships a `.zip` with `actionlint.exe` at root.
- A combined `actionlint_{version}_checksums.txt` exists, so `checksum-file` is set on every
  platform. Every vendored tool in this repo stays integrity-verified.
- Asset names use Go's `amd64`/`arm64` uniformly across all three OSes, so a single global
  `[install.arch]` remap suffices and no `[platform.*.arch]` override is required — the
  `osv-scanner` shape, not the `buf`/`lefthook` shape.
- Tags are `v`-prefixed (`v1.7.12`) while filenames embed the bare version — the `promtool` shape.

### 4.2 The Moon task

`repo:actionlint`, hosted on the root `repo` project, `toolchain: 'system'`, `script:
'ci/actionlint/run.sh'`, with narrow `inputs:` — `.github/workflows/**/*`, `ci/actionlint/**/*`,
`.prototools`, `.proto/plugins/actionlint.toml`. `repo` owns the whole tree, so without narrow
inputs the gate would run on every change.

`run.sh` opens with `set -euo pipefail`. Moon does not enable errexit for `script:` blocks, so a
failing command followed by a succeeding one still exits the task 0 — the latent defect CodeRabbit
found in `nats-permissions`, and fatal here because this script runs several checks in sequence.

### 4.3 What `run.sh` checks

| # | Check | Catches |
|---|---|---|
| 1 | `actionlint -shellcheck= -pyflakes=` over the discovered workflow set | syntax, wrong key, malformed glob, unknown runner label, bad expressions |
| 2 | A malformed workflow piped to `actionlint -` **must fail** | the gate having been neutered — a bad flag, a stray `-ignore` |
| 3 | A healthy workflow piped to `actionlint -` **must pass** | control for 2: a globally broken invocation would otherwise read as "malformed input correctly rejected" |
| 4 | Every `paths:`/`paths-ignore:` glob matches ≥1 tracked file | the typo'd-but-valid glob of §2 |
| 5 | Total extracted globs > 0 | the extractor silently yielding nothing |
| 6 | Any workflow containing a `paths:` key yields ≥1 glob | an unsupported YAML form becoming a silent skip |

Check 1 invokes `actionlint` **bare**, with no file arguments, relying on its repository
auto-discovery of `.github/workflows`. Two reasons: a `*.yml` argument list would silently miss a
`.yaml`-suffixed workflow, and the exit-3 behaviour of §2 only applies to the auto-discovery path
— an explicit glob that expands to nothing would pass, vacuously, as "no errors found".

Checks 5 and 6 are what stop check 4 from going vacuous. The extractor is a zero-dependency `awk`
pass over the block-sequence form (`paths:` followed by `- 'glob'` items), chosen over PyYAML
because PyYAML's presence on a GitHub runner's system `python3` is not something to bet a gate on.
Check 6 converts the extractor's known blind spot — the inline flow form `paths: [a, b]`, which
this repo does not currently use but plausibly could — from a silent skip into a loud failure
naming the file. That is the difference between a limitation and a hole.

`!`-negated patterns are skipped by design: they are exclusions, so requiring them to match a
tracked file would be wrong.

### 4.4 CI wiring

`:actionlint` joins the `T=(…)` array in `ci.yml`'s `moon ci` step, and the "Install pinned CLIs"
step name gains `actionlint`. `ci/affected-graph/run.sh` asserts only that every `moon ci`
invocation carries `--include-relations`, not the contents of `T`, so this addition does not
disturb it — though editing `ci.yml` does make `:affected-smoke` itself affected, which is
expected.

## 5. Verification

- `moon run repo:actionlint` passes on the tree as committed.
- The gate is verified with the **full graph the way CI runs it** — `moon ci … --base origin/main
  --include-relations` with the complete target array — not just the single task. Per-project
  tasks do not run repo-level gates, and this task is new to the array.
- Checks 2 and 3 self-verify on every run; no manual demonstration is required, and none is
  relied upon.
- Additionally, once during implementation: break a real `paths:` glob in `prebuild.yml`, confirm
  check 4 fails and names the offending pattern, then revert. This proves the *path-existence*
  control specifically (checks 2 and 3 only prove the actionlint invocation), and its output is
  recorded in the PR body. AC-3 asks for a demonstration; §D3 adds the standing guard on top.

## 6. Limitations

Stated deliberately, so nothing here reads as a stronger guarantee than it is.

- **L1 — Cache keying.** Moon keys this task on the workflow files and the tool pins. A tree
  rename that orphans an existing glob (`rs/` → `rust/`) does not re-key it, so such a break is
  caught at the next workflow edit rather than immediately. Broadening `inputs:` to the whole tree
  would run this gate on every change; naming the limit is the better trade.
- **L2 — No inline-bash linting**, per D2. `ci.yml` and `prebuild.yml` carry roughly 120 lines of
  inline bash that remain unlinted. Follow-up issue.
- **L3 — Runner labels are baked into the pinned binary.** A genuinely new GitHub label (a future
  `ubuntu-26.04`, say) reds the gate until the pin is bumped. The escape hatch is a
  `.github/actionlint.yaml` with `self-hosted-runner.labels`; not created now, since inventing
  configuration for a situation that does not exist is how config rots.
- **L4 — The gate cannot prove a workflow *ran*.** It proves a filter is well-formed and its globs
  name real files. A filter whose globs all exist but which collectively never match a real change
  set is still possible.
- **L5 — Meta, out of scope.** Nothing asserts that a `repo:*` gate is actually wired into
  `ci.yml`'s `T=(…)`. That is the same silent-omission class one level up: a future gate could be
  added and never run. Worth a follow-up; not folded in here.

## 7. Non-goals

- Pinning shellcheck (D2).
- `dependabot.yml` — actionlint does not lint it.
- Composite actions — none exist in this repo.
- Post-merge verification that `prebuild` actually ran (L4).
