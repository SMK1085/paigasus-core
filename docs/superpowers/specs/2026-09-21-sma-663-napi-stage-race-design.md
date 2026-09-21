# SMA-663 — `moon ci` fails when a napi staging directory is a Cargo workspace member

- **Issue:** SMA-663 (related: SMA-604, SMA-658)
- **Date:** 2026-09-21
- **Status:** Revision 2, after the adversarial challenge. For approval.

## 1. Problem

`moon ci` fails at random with this error:

```
error: failed to load manifest for workspace member
`…/rs/crates/bindings/.paigasus-node-bindings.napi-stage-NgjPXr`
Caused by:
  failed to read `…/.paigasus-node-bindings.napi-stage-NgjPXr/Cargo.toml`
```

The rate on the SMA-658 branches was 9 of 9 `moon ci` runs on the GitHub Linux runners. Re-runs
did not clear it. A change to `rs/Cargo.lock` schedules both napi tasks and the Rust crate tasks in
the same wave. Each manual service version bump changes `rs/Cargo.lock`, so each image release hits
this failure.

## 2. Mechanism (read from the installed source)

These facts come from `@napi-rs/cli` 3.10.3. That is the version that `ts/pnpm-lock.yaml:30-32`
resolves for `@paigasus/kernel`. The catalog specifier is `^3.7.2` (`ts/pnpm-workspace.yaml:169`).
napi 3.7.2 has no staging, so the staging behaviour came with a lockfile update inside that range.

| # | Fact | Source |
|---|---|---|
| M1 | napi finds the crate from `--cwd`, `--manifest-path` or `-p`. The output directory defaults to the crate directory. | `dist/cli.js:9749`, `:9757`, `:9768-9771`, `:9937-9938` |
| M2 | `napi build` stages every output in `mkdtemp(join(dirname(outputDir), "." + basename(outputDir) + ".napi-stage-"))`. | `dist/cli.js:10350` |
| M3 | The staging directory is removed in a `finally` block. A SIGINT or SIGKILL during that phase leaves it on disk. | `dist/cli.js:10384-10392` |
| M4 | Staging is unconditional. No flag or env var turns it off or moves it by itself. `TMPDIR` has no effect on this call. Only `--output-dir` moves it, together with all outputs. | `dist/cli.js:10027`, `:14418` |
| M5 | napi also writes lock files beside the crate (`.napi-rs-filesystem-reconciliation.<sha256>.swp` and variants). They are regular files, and Cargo skips a glob match that is not a directory. So they are harmless. | `dist/cli.js:1019`, `:1683`, `:1698-1701`, `:1762-1776` |
| M6 | `rs/Cargo.toml` declares `members = ["crates/bindings/*", "crates/libs/*", "crates/services/*"]`. Cargo's glob matches a name that starts with a dot. | `rs/Cargo.toml:10`, observed error |

So `napi build --cwd rs/crates/bindings/paigasus-node-bindings` makes
`rs/crates/bindings/.paigasus-node-bindings.napi-stage-XXXXXX`. That directory never contains a
`Cargo.toml`. For its full life, **every** concurrent Cargo command that loads the workspace fails
with exit code 101. The failure is not limited to the teardown moment. The window is only the
post-build phase (`dist/cli.js:10339-10392`), after cargo exits.

**Which tasks make the directory.** Only `paigasus-kernel-ts:build` and `paigasus-kernel-ts:test`
run `napi build` in a Moon task (`ts/packages/paigasus-kernel/moon.yml:47`, `:139`). Both target the
same crate, and `moon ci` runs them in parallel. `.github/workflows/prebuild.yml:147`, `:157` and
the crate's `package.json` scripts also run `napi build`, outside Moon.

**Correction to the issue.** The issue says `paigasus-kernel-py:test` also makes a staging
directory. That is not correct. That task runs `uv sync`, then maturin, then `cargo metadata`
(`py/packages/paigasus-kernel/moon.yml:36`). It is a victim of the race, like
`paigasus-service-info-rs:lint`. It is not a cause.

## 3. Decision

**Make every `members` entry a literal path.** The list has 13 entries, one for each crate
directory under `rs/crates/`:

```toml
members = [
  "crates/bindings/paigasus-node-bindings",
  "crates/bindings/paigasus-py-bindings",
  "crates/bindings/paigasus-wasm",
  "crates/libs/paigasus-iam-core",
  "crates/libs/paigasus-kernel",
  "crates/libs/paigasus-kernel-parity",
  "crates/libs/paigasus-logging",
  "crates/libs/paigasus-observability",
  "crates/libs/paigasus-proto",
  "crates/libs/paigasus-proto-derive",
  "crates/libs/paigasus-service-info",
  "crates/services/paigasus-gateway",
  "crates/services/paigasus-iam",
]
```

A staging directory is then not a member. The race cannot occur, because Cargo never reads the
directory. The same is true for any future tool that stages a dot-sibling beside a crate in any of
the three directories.

**Cost.** A new crate needs one new line in `members`. `cargo new` inside the workspace adds that
line automatically. If a person forgets it, A9 reds (see below).

### Why this option

| Option | Result | Decision |
|---|---|---|
| 1a. All 13 entries literal | Removes the race for napi and for any other dot-sibling stager. The guard is one trivial rule (section 4). | **Chosen** |
| 1b. Literal `bindings` entries only, globs for `libs`/`services` | Removes the napi race. But the guard must then model napi's crate and output resolution (M1), which has many flag shapes. A wrong model passes in silence. | Rejected: the guard is fragile |
| 2. `napi build --output-dir <outside rs/crates/bindings>` | Removes the race. But M4 moves the `.node` file and the committed `index.js`/`index.d.ts` too. That changes the `file:` dependency, the vitest alias, the moon `outputs` and `.gitignore`. | Rejected: many files change, no advantage |
| 3. Run the napi tasks one after the other | Only makes the race less frequent. It costs parallelism. | Rejected |
| `[workspace] exclude` | Cargo's `exclude` is a path-prefix match. It cannot name a random suffix. | Not possible |

### Compatibility with SMA-604, A9 and other readers

- **Dependabot.** `expand_workspaces` returns a literal entry as it is. The SMA-604 rule is "at most
  one wildcard level per entry". A literal entry has zero levels, so it obeys the rule.
- **A9** (`check_member_globs`, `ci/affected-graph/cargo_moon_parity.py:1716`) transcribes
  Dependabot's expander. That expander returns a literal entry verbatim (`:1703-1704`). A self-test
  row already covers literal entries (`:3068-3071`). So A9 passes.
- **Drift is guarded.** A9 finds crate directories with a filesystem `rglob("Cargo.toml")`
  (`cargo_crates`, `:2097`), not with the `members` list. If a new crate is not listed, A9 reds with
  a "never reaches" row (`:1759-1762`).
- **`ci/release-plan/release_plan.py`** `crate_manifests()` takes a literal entry as a direct path
  (`:205-209`). No change.
- **`rs/Cargo.lock`** must stay byte-identical, because the member set does not change.

## 4. Regression guard: assertion A11

A future edit can put a glob back, and nothing would red until the race returns in CI. So
`repo:affected-smoke` gets a new assertion, **A11** (findings key `a11`), in
`ci/affected-graph/cargo_moon_parity.py`, next to A9.

**Rule.** No `[workspace] members` entry in `rs/Cargo.toml` may contain a glob character (`*`, `?`
or `[`). Each violating entry gives one row. The row names the entry and says why: a glob matches a
dot-sibling staging directory, such as napi's `.napi-stage-*`, and a concurrent `cargo metadata`
then fails (SMA-663).

**Reuse.** A11 reads the `members` list with the same code as A9. Extract A9's read of
`rs/Cargo.toml` (`:1728-1739`) into a helper, if A9 does not already have one, so that both
assertions share the same infrastructure errors for a missing file or an empty list.

**No floor is necessary.** A9 already raises `MoonOutputError` when `members` is absent or empty.
An empty list therefore cannot make A11 pass in silence.

**Reachability.** `repo:affected-smoke` declares `rs/**/Cargo.toml` as an input (root `moon.yml`).
A PR that puts a glob back therefore schedules the gate.

**Self-test rows** (in `self_test()`, next to the A9 rows):

| Row | `members` input | Expected |
|---|---|---|
| a | `["crates/bindings/*"]` | one row |
| b | 13 literal entries | no row |
| c | `["crates/libs/paigasus-kernel", "crates/services/*", "crates/x/[ab]"]` | two rows (the `*` entry and the `[` entry) |
| d | `["crates/libs/paigasus-?"]` | one row |

**Gate plumbing.** Add `a11` to `EXPECTED_FINDING_KEYS`, add A11 to `collect_findings` with a
title, and update the PASS text in `main()`. The negative control of `repo:affected-smoke` already
runs `--self-test`, so the new rows run in CI.

## 5. Proof for acceptance criterion 1

There are four parts. Parts P1 and P2 are local measurements. Part P3 is a fallback. Part P4 is the
evidence in the form that the AC names.

**P1. Deterministic probe.** This part does not depend on timing.

1. On `origin/main`: `mkdir rs/crates/bindings/.paigasus-node-bindings.napi-stage-PROBE`, then run
   `cargo metadata --format-version=1 --locked` in `rs/`. Expect rc 101 and the exact error from
   section 1.
2. With the probe directory still present, run `moon query projects`. Record whether Moon lists the
   probe directory as a project or fails. `.moon/workspace.yml:7` globs `rs/crates/bindings/*`.
3. Remove the probe directory.
4. Repeat steps 1 to 3 on the branch. Expect rc 0 from `cargo metadata`.

If step 2 shows that Moon lists or fails on a dot-directory, the Moon project globs have the same
defect. Then this PR also makes the three `rs/crates/*/*` globs in `.moon/workspace.yml` literal,
and the spec records that. If Moon skips dot-directories, no Moon change is made.

**P2. Forced-overlap race.** This part uses the real `napi build`.

- Loop A: `pnpm -C ts/packages/paigasus-kernel exec napi build --platform --cwd
  ../../../rs/crates/bindings/paigasus-node-bindings`, repeated **N = 200** times. A warm build
  stages each time (M4).
- Loops B1 to B3: three parallel loops of `cargo metadata --format-version=1 --locked` in `rs/`.
  Each loop records every exit code and the stderr of each non-zero exit.
- Run the same script with the same N on `origin/main` and on the branch.
- Record: the total number of B calls, the number of non-zero exits, and the number of those that
  name `napi-stage`.

**Pass condition.** On `origin/main`, at least 10 `napi-stage` failures. On the branch, zero
non-zero exits of any kind. If `origin/main` gives fewer than 10 failures, the local host does not
force the overlap well enough. Then use P3.

**P3. Fallback on Linux.** Run the P2 script in a Linux container (Docker 29.8.0 is available on
the development Mac). The CI failures came from Linux runners.

**P4. The PR's own `moon ci` run.** This PR edits `rs/Cargo.toml`. That file is an input of each
crate `lint` task and of both `paigasus-kernel-ts` tasks (`ts/packages/paigasus-kernel/moon.yml:100`,
`:174`). So the run schedules the same shape that failed 9 of 9. Record its run ID.

A green P4 alone is not proof. P1 and P2 (or P3) carry the proof. P4 shows the AC's form.

## 6. Other changes

- **`.gitignore`.** Add a rule for a leftover staging directory (M3): `.*.napi-stage-*/`. After
  the fix a leftover directory is harmless to Cargo, but without the rule `git add -A` can commit
  it. Do not use a bare directory name that ignores more than intended (compare the `build/` trap in
  the root `CLAUDE.md`).
- **`rs/Cargo.toml`.** Replace the comment above `members`. Keep the SMA-604 reason (one wildcard
  level at most for Dependabot). Add that each entry is now literal because of SMA-663, and name A11.

## 7. Documentation (acceptance criterion 2)

- **Root `CLAUDE.md`,** the bullet in "Diagnosing an unattributed `moon ci` failure". Replace the
  symptom-only text and the "a re-run clears it" advice. State the mechanism (M2, M6), the fix and
  the guard (A11) in a few sentences. State that this error must not occur after the change, so a
  recurrence is a new defect. Remove the wrong `paigasus-kernel-py:test` attribution. Stay outside
  the two gated marker blocks.
- **`rs/CLAUDE.md`,** the `members` bullet. Add the SMA-663 rule and A11. Correct the sentence
  "adding a crate inside an existing one does not [need a new entry]". It is false after this
  change.
- **`ci/affected-graph/README.md`.** Add an A11 section next to A9. Correct the same sentence at
  `:242-244`. Change "A1-A10" to "A1-A11" at `:340`.
- **Memory.** Update `moon-ci-napi-stage-race.md` in the user's auto-memory: the race is fixed, and
  here is the new diagnosis.

## 8. Acceptance criteria mapping

| AC | How it is met |
|---|---|
| 1. No missing `.napi-stage-*` path under forced overlap | Section 5: P1 red then green, P2 (or P3) at least 10 failures then zero, P4 recorded |
| 2. Mechanism and fix in `CLAUDE.md` | Section 7 |
| 3. Dependabot still expands `members`; A9 passes | Literal entries (section 3). `repo:affected-smoke` passes, with its A9 rows and its negative control |

## 9. Measurements

To be filled during implementation: the P1 results, the P2 (or P3) counts, the P4 run ID, and the
full text of the P2 script, so that a reviewer can run it again.

## 10. Out of scope

- The `.wasmpack-out` and `.wasmpack-test-out` directories. They are inside the `paigasus-wasm`
  crate directory, and no entry matches them.
- The napi builds in `prebuild.yml` and in the crate's `package.json` scripts. They do not run in
  parallel with other cargo calls in the same checkout.
- A change upstream in `@napi-rs/cli` to stage under `os.tmpdir()`.
- **Follow-up issue (to be filed).** The challenger read that napi 3.10.3 passes arguments through
  to cargo (`dist/cli.js:14494`, `:10275`) and forwards `--locked` to its own `cargo metadata`
  (`:4171`). `rs/CLAUDE.md` and the `ALLOW_UNLOCKED_CARGO` reason (`cargo_moon_parity.py:413`) say
  that `napi build` has no `--locked` and no cargo passthrough. That text may be stale after the
  lockfile moved to 3.10.3. Nobody has measured it. Measure it in a separate issue. This PR does not change that text.
