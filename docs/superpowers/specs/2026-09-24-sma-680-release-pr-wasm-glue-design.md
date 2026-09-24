# SMA-680 — the release PR must not make the committed wasm glue stale

- **Issue:** [SMA-680](https://linear.app/smaschek/issue/SMA-680)
- **Date:** 2026-09-24
- **Status:** approved design (approach 1), waiting for spec review
- **Related:** SMA-634 (the wasm glue is committed), SMA-576 (the live release-plz config)

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

| # | Fact | Source |
|---|---|---|
| F1 | `dependencies_update` has one effect. `true` runs `cargo update`. `false` runs `cargo update --workspace`, which changes only the lockfile entries of workspace members. `false` is the default. | `release_plz_core` at tag `release-plz-v0.3.158`, `crates/release_plz_core/src/command/update/mod.rs:142-158` (`update_cargo_lock`). Docs: `website/docs/config.md:210-213`. |
| F2 | The "dependencies changed" cascade does not read `dependencies_update`. `update_dependencies()` (`mod.rs:202-232`) rewrites the dependents' version requirements, and `add_dependencies_update_if_any()` (`updater.rs:749-783`) adds the synthetic commit that bumps them. Neither function reads the key. | Same source. A grep of `updater.rs` for the key and for `should_update_dependencies` finds no hit. |
| F3 | The comments in `rs/release-plz.toml` (lines 11, 16-22, 105) and `.github/CLAUDE.md:11` say that `dependencies_update = true` causes the cascade. F2 shows that this is wrong. The comment at line 21-22 also says that a change of the key changes what the parity fixture derives. That is wrong too: `ci/release-parity/` does not read the key (`git grep` finds no hit). | `git grep dependencies_update` |
| F4 | The drift test compares the four glue files byte for byte. It compares only the import and export lists of `paigasus_wasm_bg.wasm`, and it replays the parity corpora. It does not compare the binary bytes, because they differ per host (SMA-634 spec F12). | `committed-wasm.test.ts`, checks 1-3 |
| F5 | No crate version reaches the glue. The kernel and wasm sources hold no `env!` and no `CARGO_PKG_VERSION`. The committed glue files hold no version string. | `grep`, 2026-09-24 |

From F1, F4 and F5: with `false`, a release PR changes the workspace crates' versions in
`Cargo.toml` and `Cargo.lock` only. That change cannot reach the glue files, and it cannot change
the binary's interface. So the drift test stays green with no regeneration.

## 3. Options considered

1. **Set `dependencies_update = false`. CHOSEN.** It removes the cause. As a side effect, a
   release commit no longer carries third-party updates that no person reviewed. Cost: third-party
   updates come only from the weekly dependabot PRs. That is already the main path, and each
   dependabot PR runs `moon ci` and gets a review.
2. **Regenerate the glue in the stamp step of `release-pr`.** Rejected. The job needs wasm-pack and
   the Rust toolchain. It also compiles the build scripts of crates that `cargo update` just moved,
   in a job that holds a write-capable App token. This does not break the rule "no job downstream
   of `release` may build anything", because `release-pr` is not downstream of `release`. But it
   makes the credential surface wider and the job longer.
3. **A job with no secrets builds the glue, and a second job commits it.** Rejected. It is secure,
   but it adds two jobs and new `release_guard.py` classifications for a cause that option 1
   removes.

## 4. Design

### 4.1 `rs/release-plz.toml`

- Set `dependencies_update = false`. Keep the key explicit, not absent, so that the comment has a
  line to attach to and a reader sees the decision.
- Replace the comment above the key (lines 16-21). The new comment states:
  - what the key does (F1), with the source path and line;
  - that the cascade does not depend on it (F2);
  - why it is `false`: SMA-680, a full `cargo update` in the release PR can make the committed wasm
    glue stale, and the release workflow does not regenerate it;
  - that setting it back to `true` makes a release PR fail `committed-wasm.test.ts` again.
- Correct line 11 ("which is also what stops `dependencies_update` from cascading tags"). The
  per-package `release = false` stops the cascade from turning into bumps and tags. The cascade
  itself is unconditional (F2).
- Correct line 105 ("the dependencies_update cascade") to "the dependencies-changed cascade".

### 4.2 `.github/CLAUDE.md`

Correct the claim at line 11 the same way. The claim is: deleting the workspace-level
`release = false` is worse, because the dependencies-changed cascade bumps every transitive
dependent. That cascade is unconditional (F2), and it does not come from `dependencies_update`.
Add one sentence: `dependencies_update` is `false` since SMA-680, and why.

### 4.3 What does not change

- `.github/workflows/release.yml`. No new step, no new tool in `release-pr`.
- The drift test, the `generate-wasm` task, and the parity harness.
- No new gate. If someone sets the key back to `true`, the next release PR fails loudly in
  `committed-wasm.test.ts`. The comment in 4.1 names the cause. A gate would add a check for a
  one-line config change that already has a loud failure.

## 5. Verification

The acceptance criterion names a release PR. A real release PR runs only after the merge. So the
proof is a local measurement of the same release-plz code path, plus the first release after the
merge.

- **M1 — the lockfile diff, before and after.** In a scratch clone of this branch, run the pinned
  `release-plz update` from `rs/` twice: once with `dependencies_update = true`, once with `false`.
  `release-plz update` is the local form of the version and lockfile work that `release-pr` does.
  Record the `Cargo.lock` diff of each run. Expected: with `false`, only `[[package]]` entries of
  workspace members change. With `true`, third-party entries also change, if the registry has
  newer compatible versions on that day. If the registry has none, record that, and M1 then proves
  only the `false` half.
- **M2 — the glue after a `false` run.** On the `false` result of M1, run
  `moon run paigasus-kernel-ts:generate-wasm`. Expected: `git diff` shows no change in the four
  glue files. Then run `moon run paigasus-kernel-ts:test`. Expected: it passes.
- **M3 — the first live release after the merge.** The next release PR must pass `moon ci` with no
  manual commit, and its `Cargo.lock` diff must touch only workspace entries. This is recorded on
  SMA-680 when it occurs. It is not a merge condition for this PR.
- **The repo gates.** The change touches `rs/release-plz.toml` and `.github/CLAUDE.md`. Run the
  `ci-targets` command from the root `CLAUDE.md`. `repo:release-parity` must stay green: it derives
  its fixture from `rs/release-plz.toml`, but not from this key (F3).

## 6. Risks

- **R1 — third-party updates stop in release PRs.** Intended. If a security advisory needs a fast
  update, the path is a normal PR or a dependabot security PR, as it is for any other crate.
- **R2 — `cargo update --workspace` could still change the wasm output** if a workspace version
  reached the glue. F5 says it does not today. M2 checks it.
- **R3 — the host-determinism question in the issue.** With this design, no job regenerates the
  glue, so the determinism of the wasm build across hosts does not affect this fix. The glue files
  are byte-identical per host (SMA-634 spec F15). That fact is not re-measured here.

## 7. Follow-up

Make a Linear issue: a dependabot cargo PR that moves `wasm-bindgen` (or another crate that
changes the wasm output) fails `committed-wasm.test.ts`, and needs a manual `generate-wasm`
commit. Set the project, the milestone ("CI & Tooling") and the priority.
