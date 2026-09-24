# SMA-680 — measurements

Spec: `2026-09-24-sma-680-release-pr-wasm-glue-design.md`. Host: macOS (this development Mac),
2026-09-24. release-plz 0.3.158 (proto-pinned).

## P1 — the readers of `dependencies_update` (READ)

Source: release-plz tarball at tag `release-plz-v0.3.158` (`release_plz_core`, `release_plz`
crates), fetched with `gh api repos/release-plz/release-plz/tarball/release-plz-v0.3.158`.

Command: `grep -rn "dependencies_update\|update_dependencies\b\|should_update_dependencies\|update_all_dependencies" crates/release_plz_core/src crates/release_plz/src`

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

## M1 attempt 1 — VOID (MEASURED)

Base `028cdd20`: wasm-bindgen 0.2.127, glue hash `__wbg_Error_408e67f47ca7b58b`,
`dependencies_update = true`. All three base values matched the brief exactly. Run A (`false`)
and run B (`true`) both left `wasm-bindgen` at 0.2.127; `lock-base`, `lock-A`, and `lock-B` were
byte-identical (MD5 `f4492f95b2cf502b900b95e138d70dad`). Root cause: `release-plz update` at
`028cdd20` logged all three checked packages (`paigasus-kernel`, `paigasus-proto-derive`,
`paigasus-proto`) as "already up to date" — no package needed a version bump — so (per P1) the
`mod.rs:53` guard around `update_cargo_lock` was false in both runs, and `dependencies_update`
had no chance to act either way. Run A used `release-plz update --allow-dirty` after reverting
the napi `index.js`/`index.d.ts` churn from the build warm-up, since only the intentional
`rs/release-plz.toml` edit was left uncommitted. Verdict: VOID per step 7. Controller ruling:
re-run M1 with a forced kernel-group version bump (see attempt 2).

## M1 attempt 2 — forced kernel-group bump (MEASURED)

Base `028cdd20`, branch `m1-forced`. An extra scratch commit forced a version bump: a blank
line appended to `rs/crates/libs/paigasus-kernel/README.md` (in the crate's `include` list,
does not reach the wasm), committed as `fix(rs): scratch readme edit for the sma-680 m1
measurement` (`8ef4bf04`). All work stayed in the throwaway scratch clone `$S/m1`; nothing in
this paragraph touched the worktree.

Procedure notes (deviations from the original brief text, ordered by the controller):

- Run A (`false`): `dependencies_update = false` was set and COMMITTED in the scratch clone
  (`chore(rs): scratch false for m1`, `37bf5139`), instead of using `--allow-dirty`. The
  commit-msg hook (commitlint) ran normally and passed for both scratch commits; the
  `core.hooksPath=/dev/null` fallback was not needed.
- Before each `release-plz update`, the tree was confirmed clean. The napi glue regenerated by
  the `moon run paigasus-kernel-ts:build` warm-up (`index.d.ts`/`index.js`, unrelated to
  wasm-bindgen) did not reappear as dirty churn this time; `git status --short` was empty before
  both runs.
- Run B (`true`): `git reset -q --hard HEAD~1` in the scratch clone dropped only the "false"
  scratch commit. Confirmed with `git log --oneline -3`: HEAD was `8ef4bf04` (the README
  commit), and `grep -n '^dependencies_update' rs/release-plz.toml` read `true`.

The kernel version group (`rs/release-plz.toml`) has four members: `paigasus-kernel`,
`paigasus-py-bindings`, `paigasus-node-bindings`, `paigasus-wasm`. Kernel-group versions, before
and after each run (all four started at 0.1.0):

| Run | paigasus-kernel | paigasus-py-bindings | paigasus-node-bindings | paigasus-wasm |
|---|---|---|---|---|
| A (false), after | 0.1.1 | 0.1.0 | 0.1.0 | 0.1.0 |
| B (true), after | 0.1.1 | 0.1.0 | 0.1.0 | 0.1.0 |

`release-plz update` proposed `paigasus-kernel: 0.1.0 -> 0.1.1` in both runs — a kernel-group
bump was proposed in both, so the new void condition (the controller's fix-round-1 ruling,
"Finding 2 ... New void condition") does not apply. MEASURED: `version_group = "kernel"` did not
carry `paigasus-kernel`'s bump to the three `publish = false` members
(`paigasus-py-bindings`, `paigasus-node-bindings`, `paigasus-wasm`) at this base, with this
forced bump, under `dependencies_update = false` AND under `dependencies_update = true`. Only
the group head's own manifest was rewritten in either run. This conflicts with the comment at
`rs/release-plz.toml:25-28`, which states `version_group` "DOES apply to crates whose Cargo
manifest says `publish = false` (measured)." Cause: OPEN. The two measurements disagree and
this file does not resolve why.

| Run | Key | wasm-bindgen after | Workspace entries moved | Third-party entries moved | `paigasus-kernel-ts:test` |
|---|---|---|---|---|---|
| A | false | 0.2.127 (unchanged) | 1 (`paigasus-kernel`) | 0 | PASS — 11 files, 264 tests |
| B | true | 0.2.128 (moved) | 1 (`paigasus-kernel`) | 53 | FAIL — 2 failed, 262 passed |

Lockdiff output, run A (`python3 lockdiff.py lock-base2 lock-A2`), verbatim:
```
workspace	paigasus-kernel	['0.1.0'] -> ['0.1.1']
```

Lockdiff output, run B (`python3 lockdiff.py lock-base3 lock-B2`), verbatim:
```
third-party	bitflags	['1.3.2', '2.13.1'] -> ['1.3.2', '2.13.2']
third-party	cc	['1.4.4'] -> ['1.4.7']
third-party	cfg-if	['1.0.4'] -> ['1.0.5']
third-party	crossbeam-epoch	['0.9.20'] -> ['0.9.21']
third-party	crossbeam-queue	['0.3.13'] -> ['0.3.14']
third-party	crossbeam-utils	['0.8.22'] -> ['0.8.23']
third-party	darling	['0.20.11', '0.23.0'] -> ['0.20.11', '0.24.1']
third-party	darling_core	['0.20.11', '0.23.0'] -> ['0.20.11', '0.24.1']
third-party	darling_macro	['0.20.11', '0.23.0'] -> ['0.20.11', '0.24.1']
third-party	der	['0.7.10', '0.8.1'] -> ['0.7.10', '0.8.2']
third-party	derive-where	['1.6.1'] -> ['1.7.0']
third-party	find-msvc-tools	['0.1.11'] -> ['0.1.13']
third-party	generator	['0.8.9'] -> ['0.8.10']
third-party	hybrid-array	['0.4.14'] -> ['0.4.15']
third-party	hyper-rustls	['0.27.9'] -> ['0.27.10']
third-party	hyper-util	['0.1.20'] -> ['0.1.21']
third-party	indexmap	['1.9.3', '2.14.1'] -> ['1.9.3', '2.14.2']
third-party	ipnet	['2.12.1'] -> ['2.12.2']
third-party	jiff	['0.2.35'] -> ['0.2.37']
third-party	jiff-core	['0.1.0'] -> ['0.1.1']
third-party	jiff-static	['0.2.35'] -> ['0.2.37']
third-party	js-sys	['0.3.104'] -> ['0.3.105']
third-party	libsqlite3-sys	['0.30.1'] -> ['0.37.0']
third-party	lru-slab	['0.1.2'] -> ['0.1.3']
third-party	mio	['1.2.2'] -> ['1.2.3']
workspace	paigasus-kernel	['0.1.0'] -> ['0.1.1']
third-party	portable-atomic-util	['0.2.7'] -> ['0.2.8']
third-party	quinn	['0.11.11'] -> ['0.11.12']
third-party	quinn-proto	['0.11.17'] -> ['0.11.18']
third-party	rand	['0.10.2', '0.8.8', '0.9.5'] -> ['0.10.3', '0.8.8', '0.9.5']
third-party	rustix	['1.1.4'] -> ['1.1.5']
third-party	serde_with	['3.22.0'] -> ['3.23.0']
third-party	serde_with_macros	['3.22.0'] -> ['3.23.0']
third-party	smallvec	['1.15.2'] -> ['1.16.1']
third-party	synstructure	['0.13.2'] -> ['0.13.2', '0.14.0']
third-party	thiserror	['2.0.20'] -> ['2.0.21']
third-party	thiserror-impl	['2.0.20'] -> ['2.0.21']
third-party	tinyvec	['1.12.0'] -> ['1.13.3']
third-party	tinyvec_macros	['0.1.1'] -> None
third-party	tokio-rustls	['0.26.4'] -> ['0.26.5']
third-party	toml_edit	['0.22.27', '0.25.13+spec-1.1.0'] -> ['0.22.27', '0.25.15+spec-1.1.0']
third-party	unicode-ident	['1.0.24'] -> ['1.0.26']
third-party	ureq	['3.4.0'] -> ['3.4.2']
third-party	ureq-proto	['0.6.1'] -> ['0.6.4']
third-party	wasm-bindgen	['0.2.127'] -> ['0.2.128']
third-party	wasm-bindgen-futures	['0.4.77'] -> ['0.4.78']
third-party	wasm-bindgen-macro	['0.2.127'] -> ['0.2.128']
third-party	wasm-bindgen-macro-support	['0.2.127'] -> ['0.2.128']
third-party	wasm-bindgen-shared	['0.2.127'] -> ['0.2.128']
third-party	web-sys	['0.3.104'] -> ['0.3.105']
third-party	yoke-derive	['0.8.2'] -> ['0.8.3']
third-party	zerocopy	['0.8.56'] -> ['0.8.58']
third-party	zerocopy-derive	['0.8.56'] -> ['0.8.58']
third-party	zerofrom-derive	['0.1.7'] -> ['0.1.8']
```
54 lines total (1 workspace, 53 third-party). `lock-A2` and `lock-B2` are NOT identical
(MD5 `43e5d68f670fe87601cc6742b1967750` vs `4e20fcfb46dea728f76b4b02ce1c0003`).

`paigasus-kernel-ts:test`, run A, vitest summary lines:
```
 Test Files  11 passed (11)
      Tests  264 passed (264)
```
Fix round 1 correction: the first run A test above ran without `--force`, while run B's ran with
it, so run A's PASS was not proven uncached. Re-measured: in the scratch clone, run A's state was
recreated exactly (README commit `8ef4bf04` + the committed `false` edit, then
`release-plz update`), and the resulting `Cargo.lock` MD5 (`43e5d68f670fe87601cc6742b1967750`)
matched `lock-A2` exactly. `moon run paigasus-kernel-ts:test --force` was then run, forcing a
fresh, uncached execution. Result: unchanged — `Test Files 11 passed (11)`,
`Tests 264 passed (264)`. The original PASS was not a stale cache hit.

`paigasus-kernel-ts:test`, run B, failing test names (verbatim):
```
 FAIL  |node| tests/committed-wasm.test.ts > the committed wasm artifacts agree with the Rust source > check 1: the committed paigasus_wasm_bg.js equals the fresh build
 FAIL  |node| tests/committed-wasm.test.ts > the committed wasm artifacts agree with the Rust source > check 2: the committed binary has the fresh interface
      Tests  2 failed | 262 passed (264)
```
Check 1 diffed the committed glue's wbindgen hash against the fresh build's:
`- export function __wbg_Error_67e7344beaa85059(arg0, arg1) {` (fresh) vs
`+ export function __wbg_Error_408e67f47ca7b58b(arg0, arg1) {` (committed, unchanged). Check 2
failed for the same reason (the fresh binary's import name differs from the committed one's).

Verdict: VALID, not void. `wasm-bindgen` differs between `lock-A2` (0.2.127, unchanged) and
`lock-B2` (0.2.128, moved). This replays the wasm-bindgen drift of the v0.2.0 incident: with
`dependencies_update = false`, a kernel-group version bump moves only the workspace
`paigasus-kernel` entry in `Cargo.lock` and leaves every third-party crate, including
wasm-bindgen, untouched, so the committed wasm glue stays valid (`paigasus-kernel-ts:test`
PASS). With `dependencies_update = true`, the same kernel-group bump also runs a full
dependency update: 53 third-party entries move, including wasm-bindgen 0.2.127 -> 0.2.128, and
the committed glue (built against wasm-bindgen 0.2.127) no longer matches a fresh build
(`paigasus-kernel-ts:test` FAIL, checks 1 and 2). MEASURED: only the kernel-group head
(`paigasus-kernel`) had its own manifest version rewritten in either run; the three
`publish = false` binding crates in the same version group did not, under `false` AND under
`true`. This conflicts with the comment at `rs/release-plz.toml:25-28`, which states that
`version_group` "DOES apply to crates whose Cargo manifest says `publish = false` (measured)."
Cause: OPEN.
