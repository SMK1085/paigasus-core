# SMA-680 — the release PR must not make the committed wasm glue stale

- **Issue:** [SMA-680](https://linear.app/smaschek/issue/SMA-680)
- **Date:** 2026-09-24
- **Status:** approved design (approach 1). Revised after the spec challenge. Waiting for spec review.
- **Related:** SMA-634 (the wasm glue is committed), SMA-576 (the live release-plz config),
  SMA-580 and SMA-658 (the measured tagging and processing rules)

## 1. Problem

`rs/release-plz.toml` sets `dependencies_update = true`. When release-plz makes the release PR,
it runs a full `cargo update` on `rs/Cargo.lock`. Since SMA-634, the wasm glue under
`rs/crates/bindings/paigasus-wasm/` is committed. `paigasus-kernel-ts:generate-wasm` is the only
writer of that glue, and it has `runInCI: false`. The release workflow does not regenerate it.

When `cargo update` moves `wasm-bindgen`, the committed glue becomes stale. The release PR then
fails `moon ci` in `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`. This occurred on
release v0.2.0 (PR 268, 2026-09-24): `wasm-bindgen` moved from 0.2.127 to 0.2.128. A manual
commit (`fd70820d`) fixed it. A push to `main` before the merge would have made release-plz
rebuild the branch and drop that commit.

**Acceptance (from the issue):** a release PR passes `moon ci` with no manual commit, also when
the dependency state would otherwise change the wasm output.

**Scope:** the release PR only. A dependabot cargo PR that moves `wasm-bindgen` fails the same
test. That case gets its own Linear issue (section 7). Sven chose this scope on 2026-09-24.

## 2. Facts this design stands on

F1 and F2 were READ from the release-plz source at tag `release-plz-v0.3.158`. They were not
measured. Read them again when `.prototools` moves the release-plz pin.

| # | Fact | Source |
|---|---|---|
| F1 | `dependencies_update` selects the `cargo update` arguments. `true` runs `cargo update`. `false` (the default) runs `cargo update --workspace`. **Precondition:** `--workspace` keeps every third-party entry only when the lock already satisfies the manifests. If it does not, cargo resolves the missing entries to the newest compatible versions. `main` enforces agreement: `ci/cargo-lock-integrity/run.sh` runs in `ci.yml` before `moon ci`. The in-repo measurement SMA-634 S3-2 agrees: `cargo update -p paigasus-wasm --workspace` "rewrites only that entry of `rs/Cargo.lock`". | `crates/release_plz_core/src/command/update/mod.rs:142-158` (`update_cargo_lock`). Docs: `website/docs/config.md:210-213`. The plan re-greps both `release_plz_core` and `release_plz` for every reader of the key (P1). |
| F2 | The "dependencies changed" cascade does not read `dependencies_update`. `update_dependencies()` (`mod.rs:202-232`) rewrites the dependents' version requirements. `add_dependencies_update_if_any()` (`updater.rs:749-783`) adds the synthetic commit that bumps them. The cascade reaches only the packages that release-plz processes. | Same source. |
| F3 | The cascade cannot reach the seven non-family crates in this repo. `packages_to_process()` drops a `publish = false` crate outside a group with a publishable head (SMA-658 M7, measured). A `publish = false` crate gets no tag (SMA-580, measured on the first live release). | `.github/CLAUDE.md:17-19, 43-51`; `rs/release-plz.toml:34-40, 107-113` |
| F4 | Four comments are wrong. `rs/release-plz.toml` lines 11, 17-21 and 105, and `.github/CLAUDE.md:11-14`, say that `dependencies_update = true` causes the cascade (F2 disproves it). `rs/release-plz.toml:105` and `.github/CLAUDE.md:13-14` say that a `publish = false` crate gets a tag (F3 disproves it). `rs/release-plz.toml:21-22` says that a change of the key changes the parity fixture. `ci/release-parity/ecosystems/release-plz.sh:79-97` derives only `features_always_increment_minor`, so the fixture already runs with the default `false`. | `git grep dependencies_update` |
| F5 | wasm-bindgen puts a crate version into the glue in one case only. Its macro hashes `CARGO_PKG_NAME` and `CARGO_PKG_VERSION` of the crate under compilation (`wasm-bindgen-macro-support-0.2.128/src/hash.rs:33-58`). Export names are restored to their canonical form (`wasm-bindgen-cli-support-0.2.128/src/wit/mod.rs:937`). Import shim names keep the hash (`parser.rs:1399-1400`). Today the only hashed import is `__wbg_Error_*`, which wasm-bindgen declares in its own `src/lib.rs:1234-1235`. So its hash follows the wasm-bindgen version, not a paigasus version. SMA-634 S3-2 measured this: a version-only bump of `paigasus-wasm` changed the binary and none of the four glue files (`docs/superpowers/specs/2026-09-22-sma-634-measurements.md:166-177`). | wasm-bindgen 0.2.128 source; SMA-634 S3-2 |
| F6 | **The condition that breaks F5.** If `paigasus-wasm` or the kernel gets its own `#[wasm_bindgen] extern "C"` block, `inline_js`, or `module = "..."`, that crate's version enters an import name in `paigasus_wasm_bg.js`. Then every kernel-group release PR fails check 1, also with `dependencies_update = false`. | Follows from F5 |
| F7 | The drift test compares the four glue files byte for byte. It compares only the import and export lists of the binary, and it replays the parity corpora. It does not compare the binary bytes, because they differ per host (SMA-634 spec F12). | `committed-wasm.test.ts`, checks 1-3 |
| F8 | The stamp step of `release-pr` sets `GH_TOKEN_FOR_PUSH` (a `contents: write` App token) in the environment of the WHOLE step (`release.yml:363`). The step runs `ci/version-lockstep/run.sh --write`, which runs `napi build` (`run.sh:592-594`). So every Cargo build script in that build inherits the token. With `true`, those build scripts included third-party versions that no person reviewed. | `release.yml:354-396`; `ci/version-lockstep/run.sh` |
| F9 | Dependabot security updates are off for this repository (`GET /repos/…/automated-security-fixes` returns `enabled: false`, 2026-09-24). The cargo group in `.github/dependabot.yml:17-22` is `applies-to: version-updates` only. | `gh api` |

From F1, F5 and F7: with `false`, a release PR changes only the workspace crates' versions in
`Cargo.toml` and `Cargo.lock`. That change cannot reach the glue files while F6 does not apply,
and it cannot change the binary's interface. So the drift test stays green with no regeneration.

## 3. Options considered

1. **Set `dependencies_update = false`. CHOSEN.** It removes the cause. It also removes a security
   exposure (F8): unreviewed third-party build scripts no longer run in a job with a write-capable
   token. Reviewed dependencies' build scripts still do. Section 7 tracks that. Cost: section 6, R1.
2. **Regenerate the glue in the stamp step of `release-pr`.** Rejected. The job would need
   wasm-pack and a wasm32 build. That build compiles third-party crates that `cargo update` just
   moved. The stamp step already runs such build scripts with the token in their environment (F8),
   so option 2 keeps that exposure and makes it larger. It does not break the rule "no job
   downstream of `release` may build anything", because `release-pr` is not downstream of
   `release`.
3. **A job with no secrets builds the glue, and a second job commits it.** Rejected. It is secure,
   but it adds two jobs and new `release_guard.py` classifications for a cause that option 1
   removes.

## 4. Design

### 4.1 `rs/release-plz.toml`

- Set `dependencies_update = false`. Keep the key explicit, so that a reader sees the decision.
- Move the key out of the "Conventional-Commit -> semver classification" block (line 14). The file
  header (lines 3-5) says that the parity harness derives "the classification keys below". The
  harness does not read this key (F4), so it must not sit under that heading.
- The new comment on the key states:
  - what the key does (F1), with the source path, the lock precondition and the integrity gate;
  - that F1 and F2 were read from the 0.3.158 source, and must be read again on a release-plz bump;
  - that the cascade does not depend on the key (F2);
  - why it is `false`: SMA-680 (a full `cargo update` can make the committed wasm glue stale, and
    the release workflow does not regenerate it) and F8 (unreviewed build scripts with a write
    token);
  - that setting it to `true` again CAN make a release PR fail `committed-wasm.test.ts`, when the
    registry has a newer compatible wasm-bindgen. The test's message then asks for a manual
    `generate-wasm` commit. The real cause is this key;
  - that F6 breaks the design, with a pointer to the note in 4.3.
- Rewrite, as whole sentences, the comments at line 11 and lines 104-106. Say that the cascade is
  unconditional (F2), that it reaches only processed packages, and that `release = false` keeps a
  processed crate out of the proposal. Remove "permanently tagging": F3 disproves it.

### 4.2 `.github/CLAUDE.md`

Rewrite the release-plz bullet at lines 9-14 as whole sentences:

- `[workspace] release = false` makes release-plz hard-error. Keep that fact.
- The cascade is unconditional per the 0.3.158 source (F2, read, not measured). It reaches only
  the packages that release-plz processes.
- A `publish = false` crate outside a group with a publishable head is not processed (M7), and a
  `publish = false` crate is not tagged (SMA-580). Remove "but **not tagging**".
- Keep the fixture measurement that `rs/release-plz.toml` records ("a crate neither in the
  version group nor touched by the commit was still bumped"), and give its scope: that fixture
  had publishable crates. No issue ID is recorded for it; do not add one.
- Add: `dependencies_update` is `false` since SMA-680, with the two reasons from 4.1.

### 4.3 `ts/packages/paigasus-kernel/moon.yml`

Add a short note above the `generate-wasm` task. It records F6: the condition under which a crate
version enters the glue, and that a release PR would then fail check 1 on every kernel-group bump.
The `rs/release-plz.toml` comment points here.

### 4.4 What does not change

- `.github/workflows/release.yml`. No new step and no new tool in `release-pr`.
- The drift test, the `generate-wasm` script, and the parity harness.
- **No new gate.** A regression to `true` is one config line. It fails only when a newer
  wasm-bindgen exists, and the comment in 4.1 names the cause. A gate would need a home: the
  natural parsers (`ci/release-plan/release_plan.py`, the parity harness) have other purposes.
  Sven can ask for one at the spec gate.

## 5. Verification

The acceptance criterion names a release PR, and a real one runs only after the merge. So the proof
is a replay of the v0.2.0 incident plus a forced kernel-group bump, both with the pinned
release-plz. Each measurement has a precondition that must hold, or the measurement is void.

**P1 — the key's readers.** Grep `release_plz_core` and `release_plz` at `release-plz-v0.3.158` for
`dependencies_update` and its accessor. Expected: the only reader is `update_cargo_lock`. Also
answer: does `release-plz update` call `update_cargo_lock` when it proposes no version change?

**M1 — replay the v0.2.0 incident.** Base: `028cdd20`, the last `main` commit before PR 268. Its lock
holds `wasm-bindgen` 0.2.127, and its glue has the matching hash `__wbg_Error_408e67f47ca7b58b`
(both checked on 2026-09-24).

- Use a new scratch clone for each run, or `git reset --hard && git clean -fdx` between runs.
  `release-plz update` writes in place. Pass no `--update-deps` flag.
- **Run A (`false`):** set the key to `false`, run `release-plz update` from `rs/`. Expected:
  `wasm-bindgen` stays at 0.2.127, and only workspace entries of `Cargo.lock` change. Then run
  `moon run paigasus-kernel-ts:test` on the committed glue. Expected: pass.
- **Run B (`true`, the negative control):** the same, with `true`. Expected: `wasm-bindgen` moves,
  and `paigasus-kernel-ts:test` fails check 1.
- **Void if:** `wasm-bindgen` is the same after run A and run B. Then the control did not bite, and
  M1 proves nothing.

**M2 — force a kernel-group bump.** On a scratch clone of this branch, add a scratch `fix(rs):`
commit that edits `rs/crates/libs/paigasus-kernel/README.md`. That file is in the crate's `include`
list (`Cargo.toml:20`) and does not reach the wasm. Run `release-plz update` with `false`.

- **Void if:** no kernel-group crate version changed.
- Run `moon run paigasus-kernel-ts:test` FIRST, on the committed glue. Expected: pass. (CI tests
  the committed files, so a regeneration must not run before the test.)
- Then run `generate-wasm` and diff the four glue files. Expected: no change. This repeats SMA-634
  S3-2 on wasm-bindgen 0.2.128 with all four kernel-group crates bumped.

**M3 — the first live release after the merge.** Record on SMA-680: which version groups moved,
whether the `Cargo.lock` diff touches only workspace entries, and whether `moon ci` passed with no
manual commit. A proto-only release PR does not prove the kernel path. So record the FIRST
kernel-group release PR as a separate check. M3 is not a merge condition.

**The repo gates.** Run the `ci-targets` command from the root `CLAUDE.md`.
`repo:release-parity` must stay green (F4).

## 6. Risks

- **R1 — third-party updates stop in release PRs.** Intended, with two concrete effects.
  (1) A new RustSec advisory against a locked crate makes `repo:deny` fail on every release PR,
  because the release PR changes `rs/Cargo.lock`. Before, the full update could move past the
  advisory. Now a normal PR must bump the crate first. Dependabot security updates are off (F9), so
  a person must make that PR. (2) The weekly `cargo-minor-patch` group is the only path for
  third-party Rust updates. A wasm-bindgen patch in that group stops the whole group until a person
  commits new glue. After a commit from another author, dependabot does not rebase the branch.
- **R2 — a crate version could enter the glue (F6).** Not true today (F5). M2 checks it on
  0.2.128. The notes in 4.1 and 4.3 record the condition.
- **R3 — the host-determinism question in the issue.** With this design, no job regenerates the
  glue, so the determinism of the wasm build across hosts does not affect this fix. The glue files
  are byte-identical per host (SMA-634 spec F15). That fact is not re-measured here.

## 7. Follow-ups

- **Linear issue A — the dependabot case.** A dependabot cargo PR that moves `wasm-bindgen` fails
  `committed-wasm.test.ts` and needs a manual `generate-wasm` commit. It also stops the whole
  `cargo-minor-patch` group (R1 (2)).
- **Linear issue B — the token scope in the stamp step (F8).** Give `GH_TOKEN_FOR_PUSH` only to the
  `git fetch` and `git push` commands, not to the whole step, so no build script inherits it.
- Set the project, the milestone ("CI & Tooling") and the priority on both.
- **After the merge:** update the auto-memory entry `release-pr-wasm-glue-drift.md`. Its advice
  "regenerate and push" is the workaround that this change removes. The new first step is: check
  `dependencies_update` in `rs/release-plz.toml`.
