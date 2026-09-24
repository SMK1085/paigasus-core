# SMA-680 — measurements

Spec: `2026-09-24-sma-680-release-pr-wasm-glue-design.md`. Host: macOS (this development Mac),
2026-09-24. release-plz 0.3.158 (proto-pinned).

## P1 — the readers of `dependencies_update` (READ)

Source: release-plz tarball at tag `release-plz-v0.3.158` (`release_plz_core`, `release_plz`
crates), fetched with `gh api repos/release-plz/release-plz/tarball/release-plz-v0.3.158`.

Command: `grep -rn "dependencies_update\|update_dependencies\b|should_update_dependencies|update_all_dependencies" crates/release_plz_core/src crates/release_plz/src`

Every hit, file and line, with the matched line:

| File:line | Line |
|---|---|
| `release_plz_core/src/lock_compare.rs:13` | `pub fn are_lock_dependencies_updated(` |
| `release_plz_core/src/lock_compare.rs:21` | `    are_dependencies_updated(local_lock, registry_lock)` |
| `release_plz_core/src/lock_compare.rs:24` | `fn are_dependencies_updated(` |
| `release_plz_core/src/toml_compare.rs:6` | `pub fn are_toml_dependencies_updated(` |
| `release_plz_core/src/command/update/updater.rs:670` | `                        self.add_dependencies_update_if_any(` |
| `release_plz_core/src/command/update/updater.rs:749` | `    fn add_dependencies_update_if_any(` |
| `release_plz_core/src/command/update/updater.rs:756` | `        let are_toml_dependencies_updated = \|\| {` |
| `release_plz_core/src/command/update/updater.rs:757` | `            toml_compare::are_toml_dependencies_updated(` |
| `release_plz_core/src/command/update/updater.rs:762` | `        let are_lock_dependencies_updated = \|\| {` |
| `release_plz_core/src/command/update/updater.rs:763` | `            lock_compare::are_lock_dependencies_updated(` |
| `release_plz_core/src/command/update/updater.rs:769` | `        if are_toml_dependencies_updated() {` |
| `release_plz_core/src/command/update/updater.rs:774` | `        } else if contains_executable(package) && are_lock_dependencies_updated()? {` |
| `release_plz_core/src/command/update/update_request.rs:37` | `    dependencies_update: bool,` |
| `release_plz_core/src/command/update/update_request.rs:64` | `            dependencies_update: false,` |
| `release_plz_core/src/command/update/update_request.rs:208` | `    pub fn with_dependencies_update(self, dependencies_update: bool) -> Self {` |
| `release_plz_core/src/command/update/update_request.rs:210` | `            dependencies_update,` |
| `release_plz_core/src/command/update/update_request.rs:215` | `    pub fn should_update_dependencies(&self) -> bool {` |
| `release_plz_core/src/command/update/update_request.rs:216` | `        self.dependencies_update` |
| `release_plz_core/src/command/update/mod.rs:55` | `        update_cargo_lock(local_manifest_dir, input.should_update_dependencies())?;` |
| `release_plz_core/src/command/update/mod.rs:92` | `            update_dependencies(` |
| `release_plz_core/src/command/update/mod.rs:142` | `fn update_cargo_lock(root: &Utf8Path, update_all_dependencies: bool) -> anyhow::Result<()> {` |
| `release_plz_core/src/command/update/mod.rs:144` | `    if !update_all_dependencies {` |
| `release_plz_core/src/command/update/mod.rs:176` | `    update_dependencies(all_packages, version, &package_path, workspace_manifest)?;` |
| `release_plz_core/src/command/update/mod.rs:202` | `fn update_dependencies(` |
| `release_plz/src/config.rs:174` | `    pub dependencies_update: Option<bool>,` |
| `release_plz/src/config.rs:226` | `            dependencies_update: None,` |
| `release_plz/src/config.rs:572` | `        dependencies_update = false` |
| `release_plz/src/config.rs:593` | `                dependencies_update: Some(false),` |
| `release_plz/src/config.rs:716` | `                dependencies_update: None,` |
| `release_plz/src/args/release.rs:162` | `            dependencies_update = false` |
| `release_plz/src/args/update.rs:161` | `    fn dependencies_update(&self, config: &Config) -> bool {` |
| `release_plz/src/args/update.rs:162` | `        self.update_deps \|\| config.workspace.dependencies_update == Some(true)` |
| `release_plz/src/args/update.rs:185` | `            .with_dependencies_update(self.dependencies_update(config))` |

The hits in `lock_compare.rs` and `toml_compare.rs` are substring matches, not the config key.
`are_lock_dependencies_updated` and `are_toml_dependencies_updated` check if a lock or a toml
file already got an update. They do not read the `dependencies_update` config key.

The real path is: `config.rs:174` parses the TOML key. `args/update.rs:161-162` reads it (or the
`--update-deps` CLI flag). `args/update.rs:185` passes the result into
`UpdateRequest::with_dependencies_update`. `update_request.rs:215-216` stores and returns it as
`should_update_dependencies()`. The flag reaches exactly one functional use:
`mod.rs:55`, `update_cargo_lock(local_manifest_dir, input.should_update_dependencies())`.
No second functional use exists. P1 confirms the spec's F1.

Does `release-plz update` call `update_cargo_lock` when it proposes no version change? No. The
call at `mod.rs:55` sits inside a guard at `mod.rs:53`:
`if !packages_to_update.updates().is_empty() { ... update_cargo_lock(...) ... }`
When no package needs a version bump, `packages_to_update.updates()` is empty, the guard is
false, and `update_cargo_lock` never runs. `dependencies_update` then has no effect at all,
independent of its value.

## M1 — replay of the v0.2.0 incident (MEASURED)

Base `028cdd20`: wasm-bindgen 0.2.127, glue hash `__wbg_Error_408e67f47ca7b58b`,
`dependencies_update = true`. All three base values matched the brief exactly.

Procedure note (deviation, does not change what is measured): `moon run paigasus-kernel-ts:build`
in the warm-up step regenerated `rs/crates/bindings/paigasus-node-bindings/index.d.ts` and
`index.js` (napi glue, not wasm-bindgen glue) as a side effect, because the proto-pinned
napi-rs CLI differs from the one that produced the committed files. This is unrelated to
`paigasus-wasm` and to `Cargo.lock`. For run A, `release-plz update` on a working tree with only
`rs/release-plz.toml` edited (the two napi files were reverted with `git checkout --` first)
still refused with "the working directory ... has uncommitted changes." `release-plz update` has
a documented `--allow-dirty` flag: "Allow dirty working directories to be updated. The
uncommitted changes will be part of the update." Run A used `--allow-dirty` instead of a commit,
so the measured behavior of the `dependencies_update` key itself is unchanged; only the git
cleanliness precondition is bypassed. Run B ran on a fully clean tree
(`git reset --hard` + `git clean -qfdx -e target -e node_modules`), so it needed no such flag.

For both runs, `release-plz update` logged all three checked packages (`paigasus-kernel`,
`paigasus-proto-derive`, `paigasus-proto`) as "already up to date" — no package needed a version
bump at commit `028cdd20`. Per P1, this means the `mod.rs:53` guard was false in both runs, so
`update_cargo_lock` never ran, so `dependencies_update` had no chance to take effect in either
run.

| Run | Key | wasm-bindgen after | Workspace entries moved | Third-party entries moved | `paigasus-kernel-ts:test` |
|---|---|---|---|---|---|
| A | false | 0.2.127 (unchanged) | 0 | 0 | PASS — 11 files, 264 tests |
| B | true | 0.2.127 (unchanged) | 0 | 0 | PASS — 264 tests (no `check 1` failure, no FAIL, no ×) |

Lockdiff output, run A (`python3 lockdiff.py lock-base lock-A`), verbatim: empty, 0 lines.

Lockdiff output, run B (`python3 lockdiff.py lock-base lock-B | grep -E 'wasm-bindgen|^workspace'`
and the line count), verbatim: empty, 0 lines.

`lock-base`, `lock-A`, and `lock-B` are byte-identical (same MD5:
`f4492f95b2cf502b900b95e138d70dad`).

`paigasus-kernel-ts:test`, run A, vitest summary lines:
```
 Test Files  11 passed (11)
      Tests  264 passed (264)
```

`paigasus-kernel-ts:test`, run B, filtered lines (`grep -E "check 1|FAIL|✓|×|Tests"`):
```
paigasus-kernel-ts:test |       Tests  264 passed (264)
```
No `check 1` failure. No FAIL. No `×`. This does not match the brief's expectation for run B
(a `check 1` failure was expected).

Verdict: VOID. `wasm-bindgen` has the same version (0.2.127) in `lock-A` and `lock-B`. Step 7's
void rule applies. The direct cause is MEASURED: at commit `028cdd20`, `release-plz update`
found no package needing a version bump, in both the `false` and the `true` configurations. The
mechanism is READ (P1): `update_cargo_lock` only runs when `packages_to_update.updates()` is
non-empty, so `dependencies_update` could not have any observable effect at this base commit
regardless of its value. This base commit does not replay the incident. A different base commit
— one where `release-plz update` proposes at least one package version bump — is needed to
observe `dependencies_update` move `wasm-bindgen`.
