# SMA-685 — release-plz `version_group` and the `publish = false` binding crates

Linear: SMA-685. Related: SMA-680 (the measurements that found both behaviours), SMA-658 (M7),
SMA-407 / SMA-576 (the kernel floor and `repo:version-lockstep`). release-plz 0.3.158
(proto-pinned; `release_plz_core` 0.36.14). Evidence labels: READ (from source), MEASURED (a
command ran), INFERRED.

Source citations below are relative to the release-plz 0.3.158 tarball's `crates/` directory
(`gh api repos/release-plz/release-plz/tarball/release-plz-v0.3.158`).

## 1. Problem

SMA-680 measured two `release-plz update` behaviours and did not explain them
(`2026-09-24-sma-680-measurements.md`, M1 attempt 2 and M2 attempt 1).

- **B1.** A kernel `fix(rs):` commit bumped `paigasus-kernel` 0.1.0 -> 0.1.1. The three
  `publish = false` members of `version_group = "kernel"` stayed at 0.1.0. This contradicts
  `rs/release-plz.toml`, `.github/CLAUDE.md` and other comments (§6), which say that
  `version_group` writes the version of a Cargo `publish = false` crate in a group with a
  publishable head.
- **B2.** At the SMA-680 branch head `6093e2e9`, the same maneuver gave
  `paigasus-kernel: already up to date` and `Diff { commits: [] … }`.

Acceptance (Linear): the cause of each behaviour is known and recorded; the comments are correct.
Scope decision (Sven, 2026-09-25): also fix the gap that B1 opens, and confirm both causes by
measurement.

## 2. Causes

### 2.1 B1 — `version_group` never sees a Cargo `publish = false` crate (READ; M1 confirms)

1. `release_plz_core/src/command/update/updater.rs:283-302`, `packages_to_process()`, takes
   `self.project.publishable_packages()` (line 289), plus every workspace package for which
   `should_use_git_only` is true (line 297).
2. `project.rs:110-114`, `publishable_packages()`, filters on `is_publishable()`.
   `next_ver.rs:456-471` reads **Cargo's own `[package] publish` field**. The release-plz.toml
   keys `release`, `publish` and `version_group` are not read there.
3. `updater.rs:162-189`, `get_version_groups`, iterates the diffs of the processed set only
   (`get_packages_diffs`, `updater.rs:219-238`). `updater.rs:169-171` reads a package's
   `version_group` only for a processed package. A crate outside that set is never in a group.
4. `command/update/mod.rs:161-178`, `set_version`, is the only writer of `[package] version`.
   It is called only for `packages_to_update.updates()`, which come from the processed set.
   `update_dependencies` (`mod.rs:202-232`) touches every workspace manifest, but it rewrites
   only dependency requirement strings, never a crate's own `[package] version`.
5. `command/release_pr/mod.rs:144` calls the same `update` function. So `release-plz release-pr`
   behaves like `release-plz update` here.

So a Cargo `publish = false` crate gets its version written only if it has `git_only = true`, or if
its Cargo manifest does not say `publish = false`. On the three binding crates, the keys
`version_group = "kernel"` and `release = true` are therefore **inert today**. They stay in
`rs/release-plz.toml` as the correct declaration if a later pin changes the filter (§7).

(SMA-658 §3.1 cites `updater.rs:361-366` for the same filter. Those lines are wrong for 0.3.158;
the correct lines are `:283-302`. Do not copy the old citation.)

**Why the old claim said "measured".** Two facts:

- The binding crates reached 0.1.0 by hand, in the floor commit `e2007d55` (SMA-407 / SMA-576).
  release-plz has never bumped the kernel group: the only kernel tag is `paigasus-kernel-v0.1.0`.
  So no live release is evidence for the claim. (MEASURED: `git log --follow` on the binding
  manifests; `git tag -l`.)
- SMA-407 §4 item 2 (`2026-08-22-sma-407-release-activation-design.md:83-85`) recorded a fixture
  in which a group member "whose Cargo manifest says `publish = false`" moved 0.1.0 -> 0.2.0, and
  release-plz logged it "already up to date". That log line comes only from `updater.rs:780`,
  inside `get_package_diff`, which runs only for processed packages. With Cargo
  `publish = false`, a package is processed only with `git_only = true`. The repo's own
  release-plz fixture has exactly that shape: `ci/release-parity/ecosystems/release-plz.sh:92`
  sets `git_only = true` and `:113` sets Cargo `publish = false`. So the SMA-407 fixture most
  probably ran with `git_only = true` (INFERRED; the fixture is not kept). M1c tests it.

### 2.2 B2 — release-plz walks from the upstream's branch name, not from HEAD (READ; M2 confirms)

1. `git_cmd/src/lib.rs:44-60`, `get_current_remote_and_branch` (called by `Repo::new` at
   `lib.rs:30`), runs `git rev-parse --abbrev-ref --symbolic-full-name @{upstream}` and keeps the
   part after the first `/` (`split_once`, `lib.rs:57`) as `original_branch`. With no upstream
   it logs `no upstream configured` and falls back to the current branch name (`lib.rs:61-65`).
2. `lib.rs:177-179`, `checkout_head`, runs `git checkout <original_branch>` in the temporary
   copy (`updater.rs:552-560`). `get_package_diff` (`updater.rs:611-719`) then walks the
   path-filtered history from there.
3. The SMA-680 M2 clone made its branch with
   `git checkout -q -B m2 origin/feature/sma-680-release-wasm-glue`
   (`docs/superpowers/plans/2026-09-24-sma-680-release-pr-wasm-glue.md:361`). Under the default
   `branch.autoSetupMerge`, that sets the upstream to `origin/feature/sma-680-release-wasm-glue`
   (READ). So release-plz checked out the clone's local branch
   `feature/sma-680-release-wasm-glue`, which git made at clone time at `6093e2e9`. That branch
   does not contain the scratch commit. The newest kernel commit from there is the tag commit
   `64c96242`, the walk stops at once, and `commits` is empty. This matches the log line
   `next version calculated starting from commits after 64c96242…`.
4. Rejected: a README-blind or whitespace-tolerant comparison (READ: `are_files_equal` and
   `file_hash`, `package_compare.rs:252-266`, hash raw bytes); an early stop on an equal package
   (MEASURED: no kernel commit exists between the tag and either base); a non-ancestor boundary
   (MEASURED: `64c96242` is an ancestor of both); the `dependencies_update` flip (READ: used only
   after versions are computed, `update/mod.rs:53-55`).

**Consequence.** B2 is an artifact of how the scratch branch was made. For CI: the release-PR
job checks out `main`, which tracks `origin/main`, so the names agree (INFERRED; the plan tries
to make it MEASURED from the `actions/checkout` log of a past `release-pr` run, and the proto
v0.2.0 release PR is evidence that the CI walk sees `main`). For a LOCAL `release-plz`
measurement, the rule is: the branch needs no upstream, or an upstream with the same branch
name. The "same name" half is READ (`lib.rs:57`); M2 tests only the "no upstream" half and the
"different name" failure.

## 3. The gap B1 opens, and the fix

**Gap (READ; M3 confirms).** The next kernel release PR works like this:
`release-plz release-pr` bumps only `paigasus-kernel`. The stamp step in `release.yml` runs
`ci/version-lockstep/run.sh --write`. `run_write` (`run.sh:570`) writes only the `pyproject`,
`pyproject-dep` and `packagejson` sites. `run_check` compares every `cargo-package` site with
the head. So `repo:version-lockstep` fails on the release PR. The expected failing rows after the
old `--write` are four: the three binding `cargo-package` rows and the kernel `cargo-lock` row
(the lock row is non-uniform because `cargo update -w` reads the bindings at the old version).

**Fix.** `--write` also writes the `cargo-package` kind, for **Cargo `publish = false`
non-head sites only**.

- **Which sites.** For every `cargo-package` site whose path is not the group's
  `SOURCE_OF_TRUTH`, and whose Cargo manifest says `publish = false`, `--write` sets
  `[package] version` to the head's version. The `publish` flag is read at run time with
  `tomllib`, not from a fixed list. Today that is the three kernel bindings.
  `paigasus-proto-derive` is Cargo-publishable, so `--write` never touches it: release-plz owns
  its version, and `--check` still catches a proto `version_group` fault
  (`ci/version-lockstep/README.md:25-28`). `--write` then owns nine sites, not six.
- **Structure.** `run_write` is split into `stamp_sites` (the per-site loop, taking the tree
  root as an argument) and the regeneration tail (`cargo update -w`, `uv lock`, `napi build`).
  `run_write` calls both, in that order. The three regeneration lines keep their text
  byte-identical: `ci/affected-graph/cargo_moon_parity.py:450-482` keys its A8 waivers on that
  exact text.
- **The writer.** It edits the file in place, like the `pyproject` writer, so it makes no
  unrelated churn. It changes exactly the value of the `version = "<x>"` line inside the
  `[package]` table, before the next table header. It keeps a trailing comment on that line and
  the file's line endings (it reads and writes bytes, or uses `newline=''`).
  - It writes nothing, and keeps the mtime, when the value already matches (cargo rebuilds on
    mtime). It prints `1` when the file changed and `0` when not.
  - After the edit, it parses the old and the new text with `tomllib` and asserts that they
    differ only in `package.version`. Any other difference is rc 2.
  - It fails closed with rc 2 when: the file has no `[package]` table; `[package]` has no
    literal `version = "…"` line (for example `version.workspace = true`); `[package]` has more
    than one `version` line; or the version is not plain `X.Y.Z`.
  - It refuses to lower a version: if the site is higher than the head, it exits rc 1 with a
    `FAIL` line naming the site. A higher binding version is drift for a human to decide.
  - It never touches `rust-version`, a dependency's `version` key, or a `version` key in another
    table such as `[package.metadata.x]`.
- **Counts.** `EXPECTED_SITE_COUNT` does not change: no site is added. `SELF_TEST_COUNT` goes up
  by the number of new self-test tables (§5). The plan must check whether that anchor counts
  tables that ran or tables that are defined, and prefer the definitions count that
  `ci/actionlint/run.sh:5382` uses.
- **After the loop.** The existing `cargo update -w`, `uv lock` and `napi build` then bring the
  lock and the napi glue in line with the new binding versions. No new step.
- **`release.yml`.** No logic change. Change the step name and the commit message from
  "non-Cargo manifests" to "lockstep manifests" (`git grep` finds no pin on that text outside
  `release.yml` and an old plan). The plan re-checks `ci/actionlint/release_guard.py` and
  `ci/actionlint/run.sh` for a pin on the step name first; if one exists, keep the name and
  change only the commit message. Correct the comments at `release.yml:318-321` and `:634`.

**Constraints on new `run.sh` text.** A8 does not strip quoted strings
(`cargo_moon_parity.py:369-370`), so no new line, diagnostic text included, may contain a cargo
invocation shape (for example `cargo package`, `cargo update`). `repo:actionlint` check 13 bans
`| grep -q`, `| head` and similar pipe readers in every `*.sh`.

**Rejected alternatives.**

- `git_only = true` on the three bindings. SMA-658 M7 measured `git_only` failing
  `cargo package` when a crate has an unpublished workspace dependency. At release-PR time the
  new kernel version is unpublished, so the same failure is probable.
- Remove Cargo `publish = false` from the bindings. They then fall under `repo:publish-metadata`'s
  crates.io checks, and `release-plz release` would try to publish them.
- `version.workspace = true` for the kernel family, with a `[workspace.package] version`.
  release-plz writes the workspace version for processed packages that inherit it
  (`command/update/mod.rs:83-88`, `updater.rs:70-87`), so the bindings would follow with no new
  writer. Rejected: the `cargo-package` reader must then resolve inheritance; any new crate that
  opts in joins the kernel version with no warning; and it is a larger change to four manifests.
- `release-plz set-version` for the bindings in the stamp step. It rewrites a `CHANGELOG.md` in
  each package (`command/set_version.rs:136-152`), and the bindings have none.
- Writing every non-head `cargo-package` site, `paigasus-proto-derive` included. Rejected: it
  would hide a proto `version_group` fault from `--check`, and `release-plz release` would then
  publish `paigasus-proto-derive` at a version release-plz never proposed.
- Docs only. It leaves a known red on the next kernel release PR.

**Out of scope.** The committed `paigasus_wasm_bg.wasm` carries the crate version (SMA-634
S3-2), so a binding version bump makes its bytes stale. `committed-wasm.test.ts` checks the glue
and the interface, not the bytes (SMA-634 F16), and SMA-680 M2 part B measured it green after a
manual four-crate bump. M3 re-measures it.

## 4. Measurements

All runs use one **full** throwaway clone (`git clone`, never a worktree:
`release_plz_core/src/copy_dir.rs:50-93` copies a worktree's `.git` gitlink as a plain file, so
release-plz's `git checkout` would move the worktree's HEAD) at `cb772393`, under the session
scratchpad. Nothing is pushed. `release-plz` reads crates.io. Results go to
`docs/superpowers/specs/2026-09-25-sma-685-measurements.md`, each labelled MEASURED with the
command and the verbatim output lines. That file must not name Moon's CI report file by name
(`repo:actionlint` check 12; see the "Correction" paragraph of the SMA-680 measurements file).

**Host preconditions, recorded before any run.** `run.sh` needs bash 4+ (`declare -A`); use
Homebrew bash 5. Under bash 5, a heredoc over 512 bytes hangs when the host pipe holds 512 bytes
(root `CLAUDE.md`, SMA-612). Read the pipe capacity first (the preflight in `ci/actionlint/run.sh`
shows how). If it is below 8192 bytes, stop and report: do not run M3 or the mutations, and never
read a hang as a result. Export `PROTO_REPORTER=text` for every step that captures `release-plz`
or `proto` output.

**Baseline facts, recorded once.** `git rev-parse HEAD origin/main`; `grep '^version'` on the
six `cargo-package` manifests; `git log paigasus-kernel-v0.1.0..HEAD --
rs/crates/libs/paigasus-kernel` (expected empty); `git log paigasus-proto-v0.2.0..HEAD --
rs/crates/libs/paigasus-proto` (expected: `1fd87a15`, an unreleased `feat(rs)`, so every run
below also bumps proto 0.2.0 -> 0.3.0).

**Log settings.** `RUST_LOG=release_plz_core=debug,git_cmd=trace`. With
`RUST_LOG=release_plz=debug` the `git_cmd` events are off (`release_plz/src/log.rs:17-21`), so
the `no upstream configured` warning (`git_cmd/src/lib.rs:65`) and the checkout argument
(`trace!`, `lib.rs:413`) would be missing. Also set `GIT_TRACE=<absolute file>` and record every
`checkout` line from it.

**Reset rule.** `release-plz update` refuses a dirty tree (`next_ver.rs:268-270`). Before each
run: `git reset -q --hard <recorded sha> && git clean -fdq -e target -e node_modules`, then
`git status --short` must be empty.

- **M1 (B1 symptom).** Branch `m1` from `cb772393` with no upstream (assert that
  `git rev-parse --abbrev-ref --symbolic-full-name @{upstream}` fails). Append a blank line to
  `rs/crates/libs/paigasus-kernel/README.md`, commit `fix(rs): …`. Run `release-plz update` in
  `rs/`. Expected: kernel 0.1.1; the three bindings 0.1.0; both proto crates 0.3.0.
- **M1b (B1 cause, control).** From M1's scratch commit, remove `publish = false` from
  `rs/crates/bindings/paigasus-wasm/Cargo.toml` and commit. First check crates.io for a crate
  named `paigasus-wasm` (`cargo search` or the crates.io API): release-plz uses any such crate as
  the baseline, so if one exists, record it and pick another binding. Run `release-plz update`.
  Expected (READ: `version.rs:14-15`, `updater.rs:814-820`): `paigasus-wasm` is processed and
  takes the group maximum 0.1.1; the other two bindings stay at 0.1.0. If it stays at 0.1.0,
  record B1's cause as OPEN.
- **M1c (the SMA-407 fixture cause).** From M1's scratch commit, add `git_only = true` to the
  `paigasus-wasm` entry in `rs/release-plz.toml` and commit. Run `release-plz update`. Expected:
  `paigasus-wasm` moves to 0.1.1. If `release-plz` errors (for example the M7 `cargo package`
  failure), record the error verbatim: it then also confirms the first rejected alternative in §3.
  In both cases, record whether the §2.1 inference holds.
- **M2 (B2).** From M1's scratch commit (reset). Run A: no upstream -> expected kernel bump and a
  `no upstream configured` warning. Run B: `git branch -u origin/main` (a different name) ->
  expected `paigasus-kernel: already up to date` and a checkout of `main`. Before run B, record
  `git rev-parse main origin/main` (the clone's local `main` must exist and equal `cb772393`, or
  git makes it from `origin/main`). If run B bumps, B2's cause is wrong: record it as OPEN and
  stop; do not guess.
- **M3 (the gap and the fix).** On M1's result tree (committed as a scratch commit, so the tree
  is clean):
  1. Old `run.sh` (the one at `cb772393`): `--write`, then `--check`. Expected: `--check` exits
     1 with exactly four FAIL rows — the three binding `cargo-package` rows and the kernel
     `cargo-lock` row.
  2. Reset to the same scratch commit. New `run.sh` (copied in from the worktree): `--write`,
     then `--check`. Expected: exit 0.
  3. Lockdiff of `rs/Cargo.lock` against the scratch-commit baseline (M1's lock before any
     `--write`): expected three entries, the three binding crates, because M1's release-plz run
     already moved the kernel entry (corrected after M3 measured three); zero third-party
     entries. And
     against `cb772393`: four kernel-family and two proto workspace entries; zero third-party.
  4. `moon run paigasus-kernel-ts:test --force`: expected pass.
- **Mutations.** Apply each as a marked insert (`# MUTATION-SMA-685`), and restore by deleting
  the marked line, never with `git checkout --` (that also reverts the fix). Each mutation must
  still parse under bash.
  - Mutation 1: an early `cargo-package) printf '0' ;;` arm in the writer. Expected: `--self-test`
    red, and M3 step 2's `--check` red.
  - Mutation 2: remove `cargo-package` from `stamp_sites`' kind filter. Expected: `--self-test`
    red (this is the production call site), and M3 step 2's `--check` red.

## 5. Tests

- **Writer self-test** (`--self-test`), on fixture Cargo manifests in a temp dir. The fixtures
  must not share one layout:
  - writes `[package] version` and leaves byte-identical every other line, including
    `rust-version`, a `[dependencies]` entry with `version = "…"`, an inline-table dependency,
    and a `[package.metadata.x]` sub-table with its own `version` key;
  - spacing variants (`version="x"`, `version   =  "x"`), a trailing comment on the `version`
    line, a `[package] # comment` header, comment lines between `[package]` and `version`, and a
    `[package]` table that is not the first table;
  - a CRLF file keeps CRLF;
  - is idempotent: a second run prints `0`, changes no byte and keeps the mtime;
  - rc 2 on `version.workspace = true`, on a missing `[package]` table, on a missing
    `[package] version`, on two `version` lines in `[package]`, and on a non-`X.Y.Z` version;
  - rc 1 when the site is higher than the head.
- **Production-call-site self-test.** Stage the real tree with `stage_pristine_tree`, set the
  kernel head to a sentinel version (for example `9.9.9`) in the copy, run `stamp_sites` with
  that copy as the root, and read every site back with `read_version`. Expected: the three
  binding `cargo-package` sites, the pyproject, pyproject-dep and packagejson kernel sites all
  read `9.9.9`; `paigasus-proto-derive` is unchanged. This runs the writer on the real manifest
  shapes (for example the comment lines at `rs/crates/libs/paigasus-proto-derive/Cargo.toml:3-8`
  and the kernel's `[package]` comments). It does not run the regeneration tail.
- **Negative control.** Add a third drift of the `cargo-package` kind: one binding manifest at
  `99.99.99` in its own pristine tree. `run_check` must report red. (README L1 records that this
  kind has no end-to-end drift today, and B1 has exactly that shape.)
- The existing `--negative-control` drifts must still pass unchanged.

## 6. Documentation changes

All in Simplified Technical English, with the evidence label on each claim. The acceptance
criterion "the comments are correct" covers every site below.

- `rs/release-plz.toml`: the kernel-family block (`:56-64`), the binding-crates note
  (`:87-94`, "`release = true` … keeps these crates in the version group"), and the gateway/iam
  note (`:144-150`). State the READ cause of B1; that `version_group` and `release = true` are
  inert on the bindings today and stay as the declaration; the hand-set floor; and that
  `version-lockstep --write` now writes the binding versions. Remove the `CONFLICT (SMA-685)`
  lines. Keep the TAGS note: it stays true.
- `rs/crates/libs/paigasus-kernel/Cargo.toml:5-8`. Correct the claim. **Side effect:**
  release-plz compares the local `Cargo.toml` with the registry's `Cargo.toml.orig` byte for
  byte (`package_compare.rs:178-186`), so this edit makes the next `release-pr` run propose a
  kernel 0.1.1 release PR (READ). That release PR is the first live test of this fix. This is a
  decision for Sven at GATE 1: fix the comment and accept that release PR (recommended), or
  leave the comment and record why.
- `.github/CLAUDE.md`: `:21-27` (release-plz entry), `:50-58` (the standing rule that
  "`release = true` keeps a `publish = false` crate in the version group"), `:146-156`
  (kernel-family lockstep entry: "release-plz owns every Cargo `[package] version`" becomes "the
  head's"; `--write` now owns nine sites, not six), `:275-278` (service versions). Add one short
  bullet for B2: a local `release-plz` measurement needs a branch with no upstream or an upstream
  with the same branch name.
- `.github/workflows/release.yml:318-321` and `:634`.
- `ci/version-lockstep/run.sh`: the comments that say release-plz owns every `cargo-package`
  site and that `--write` skips them.
- `ci/version-lockstep/README.md`: `:25-28` (keep the rationale; add that `--write` writes only
  the `publish = false` non-head sites so that it cannot hide a proto fault), `:40` (the Modes
  table), and the L1 / L2 entries.
- `ci/affected-graph/cargo_moon_parity.py:454` ("the six non-Cargo version sites"): a comment
  only. Do not change any waiver key text.
- `docs/superpowers/specs/2026-09-24-sma-680-measurements.md`: add one pointer line under M1
  attempt 2 and M2 attempt 1 to the SMA-685 measurements. Do not rewrite the old text.

## 7. Risks

- **The writer lands a wrong edit on a release PR.** The stamp step runs with a write token and
  pushes. The `tomllib` diff assertion, the fail-closed rules and the byte-identical self-test
  cover the known shapes. The existing `--check` runs after the stamp in the release PR's own CI.
- **A future release-plz pin changes the filter.** Then release-plz and the script both write the
  same value, which is harmless. The comment in `rs/release-plz.toml` says to re-read the source
  on a pin bump.
- **B2 in CI.** Not expected (§2.2). If a release PR ever fails to open after a kernel commit,
  check `@{upstream}` in the job first.
- **The kernel manifest comment edit starts a release PR** (§6). Merging that release PR stays a
  human decision, and `approve-release` still gates the publish.
