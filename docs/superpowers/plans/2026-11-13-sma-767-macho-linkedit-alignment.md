# SMA-767 Mach-O LINKEDIT alignment: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Release builds of the macOS FFI bindings load under dyld on macOS 27, and CI fails when a darwin binding has a mis-aligned string pool.

**Architecture:** A macOS-only rustflag `-C strip=none` in `rs/.cargo/config.toml` stops rustc from running `rust-objcopy` on apple-darwin output. A crate-local copy of the flag ships in the py sdist. An `otool` guard (`stroff mod 8`) in `wheels.yml` and `prebuild.yml` catches a regression.

**Tech Stack:** cargo config, maturin 1.9.6 (`[tool.maturin] include`), Moon task inputs, GitHub Actions bash steps, `otool`.

**Spec:** `docs/superpowers/specs/2026-11-13-sma-767-macho-linkedit-alignment-design.md` (approved at GATE 1; §7 holds Sven's decisions).

## Global Constraints

- Every source file opens with an SPDX header (`#` for TOML/YAML). Keep the existing headers.
- Commits: conventional, scope `rs`, `py` or `ci`; end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` line in a commit body. Never `--no-verify`.
- Do not use `git commit --amend` followed by `git reset`. Do not use bare `git stash`.
- Do not install host software (no `brew`, no global pip/npm). Use `docker run` for a Linux check.
- Run every command in the foreground. Do not start a background job.
- Bash PATH: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text`.
- Scratch files go in a new directory under `$TMPDIR`, never in the repo.
- Write all prose (comments, docs) in ASD-STE100 Simplified Technical English. Keep technical names exact.
- The guard parse must NOT pipe into a reader that exits early (`repo:actionlint` check 13). Use exactly: `otool -l "$f" | awk '/^ *stroff /&&!d{v=$2; d=1} END{print v}'`.
- The guard error message names `SMA-767`.
- Measured reference values on this Mac (Rust 1.95.0, SDK 27.2): py dylib default strip `stroff 402708` (mis-aligned); `strip=none` `stroff 435672`; napi default `470128`; napi `strip=none` `503056`.

## Review Focus

1. A file with no `LC_SYMTAB` (or a non-Mach-O file) given to the guard: it must fail, not pass with an empty value. Pinned in Task 3 Step 1 (case `no-symtab`).
2. The x86_64 slices (`x86_64-apple-darwin`) get the flag too: the config has two target tables. Pinned in Task 1 Step 3 (`grep -c` = 2) and by the CI guard on both arches.
3. A cargo run with the crate as cwd reads BOTH config files; the flag appears twice. That must build without a warning. Pinned in Task 2 Step 5 (warning count 0).
4. The sdist built by `pip install` (build isolation, temp dir) must find the shipped config. Pinned by the `sdist-verify` macOS guard in Task 3.
5. A stale `py/.venv` binding after the pull: the README command must refresh it. Pinned in Task 1 Step 6.

---

### Task 1: macOS-only `-C strip=none` rustflag, and its records

**Files:**
- Modify: `rs/.cargo/config.toml` (both `[target.*-apple-darwin]` tables, and the header comment)
- Modify: `rs/CLAUDE.md` (one new bullet in "Cargo, the lockfile and nextest")
- Modify: `py/README.md` ("After a Rust change" section: a note for SMA-767)

**Interfaces:**
- Produces: each apple-darwin `rustflags` list ends with `"-C", "strip=none"`. Task 2 copies this flag; Task 3's guard relies on it.

- [ ] **Step 1: Record the failing state.** Make a scratch dir. Run `cargo build --release -p paigasus-py-bindings` in `rs/crates/bindings/paigasus-py-bindings` with `CARGO_TARGET_DIR=<scratch>/before`. Read `stroff` of `<scratch>/before/release/libpaigasus_py_bindings.dylib` with the Global Constraints awk. Expected: `402708` (mod 8 = 4). Keep this dylib: Task 3 uses it as the unfixed case.
- [ ] **Step 2: Edit `rs/.cargo/config.toml`.** Append `"-C", "strip=none"` to both apple-darwin `rustflags` lists. Add a comment block (STE) that states: rustc 1.95 strips release Mach-O output with `rust-objcopy`; it does not pad after the indirect symbol table; with an odd count the string pool is 4-byte aligned; dyld on macOS 27 rejects that for a binary linked with SDK 27; the flag is scoped to darwin because `strip = "none"` grows the Linux `.so` 7.7 times; removal condition = spec D6 (`stroff > indirectsymoff + 4 × nindirectsyms` on an odd-count dylib with the default strip); the crate-local copy in `rs/crates/bindings/paigasus-py-bindings/.cargo/config.toml` must stay in step (Task 2); SMA-767.
- [ ] **Step 3: Verify.** `grep -c 'strip=none' rs/.cargo/config.toml` → `2`. Rebuild as Step 1 into `<scratch>/after`. Expected `stroff 435672`, mod 8 = 0. Copy it to `<scratch>/pkg/paigasus_py_bindings.abi3.so` and run `uv run --no-project --python 3.12 python -c "import sys; sys.path.insert(0,'<scratch>/pkg'); import paigasus_py_bindings"` → no error.
- [ ] **Step 4: napi.** `cargo build --release -p paigasus-node-bindings` (cwd `rs/crates/bindings/paigasus-node-bindings`, fresh `CARGO_TARGET_DIR`). Expected `stroff 503056` (the unstripped value, not 470128).
- [ ] **Step 5: Moon task.** `moon run paigasus-kernel-py:test --force` → rc 0 (151 passed at the time of writing).
- [ ] **Step 6: Shared venv.** Run the "After a Rust change" command from `py/README.md` from the repo root, then `moon run py:test --force` → rc 0.
- [ ] **Step 7: Records.** `rs/CLAUDE.md`: one bullet (STE): do not remove the darwin `-C strip=none` until spec D6 holds; do not add maturin `--strip`/`strip = true` or napi `--strip` (each runs `rust-objcopy` again); "mis-aligned LINKEDIT" on macOS = a `rust-objcopy`-stripped Mach-O linked with SDK 27; the two config files must agree (SMA-767). `py/README.md`: in "After a Rust change", add: after you pull SMA-767, run the command once, because `py/.venv` keeps the old, unloadable binding; point to the `rs/CLAUDE.md` entry.
- [ ] **Step 8: Commit.** `fix(rs): stop rust-objcopy stripping macOS release binaries (SMA-767)`.

### Task 2: ship the flag in the py sdist

**Files:**
- Create: `rs/crates/bindings/paigasus-py-bindings/.cargo/config.toml` (SPDX header; both apple-darwin tables with only `rustflags = ["-C", "strip=none"]`; an STE comment: it exists so that `maturin sdist` ships the flag at the sdist root, because maturin cannot include a path with `..`; keep in step with `rs/.cargo/config.toml`; SMA-767)
- Modify: `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` (`[tool.maturin]`: `include = [{ path = ".cargo/config.toml", format = "sdist" }]`, with a comment)
- Modify: `rs/crates/bindings/paigasus-py-bindings/moon.yml` (add `.cargo/config.toml` as an input of `build`, `build-release`, `test`, `lint`; inherited inputs must stay — check Moon's merge strategy with `moon task paigasus-py-bindings-rs:build --json`)
- Modify: `py/packages/paigasus-kernel/moon.yml` (`test` inputs: `/rs/crates/bindings/paigasus-py-bindings/.cargo/config.toml`, with a comment)
- Modify: `.github/workflows/wheels.yml` (`pull_request.paths`: add `rs/crates/bindings/paigasus-py-bindings/.cargo/config.toml`; sdist job "Assert the sdist ships nothing repo-internal": add `.cargo/config.toml` to the `required` list, anchored `/.cargo/config.toml$` at the archive root)

**Interfaces:**
- Consumes: the flag from Task 1.
- Produces: the sdist root holds `.cargo/config.toml`. Task 3's `sdist-verify` guard relies on it.

- [ ] **Step 1: Failing check.** `maturin sdist -o <scratch>/sd0` from the crate dir. `tar tzf` the result: no `.cargo/config.toml` entry. Run the sdist job's required-file loop (copied into a scratch script) with the new entry added → it must fail with `sdist is missing .cargo/config.toml`.
- [ ] **Step 2: Create the crate-local config and the include.**
- [ ] **Step 3: Verify the listing.** Rebuild the sdist. The loop from Step 1 → passes. `moon.yml` still absent.
- [ ] **Step 4: Build from the sdist outside the repo.** Extract to scratch. Run `RUSTUP_TOOLCHAIN=1.95.0 CARGO_TARGET_DIR=<scratch>/t $HOME/.proto/tools/maturin/1.9.6/maturin build --release -o <scratch>/w` in the extracted root (the proto shim does not resolve maturin outside the repo). Unzip, read `stroff` → mod 8 = 0, import → OK.
- [ ] **Step 5: Double-config check.** `cargo build --release -p paigasus-py-bindings -v` with the crate as cwd: build log has 0 lines that start with `warning`, and rc 0.
- [ ] **Step 6: Moon inputs.** `moon task paigasus-py-bindings-rs:build --json` lists both `/rs/.cargo/config.toml` and the crate file. Run `/bin/bash ci/affected-graph/run.sh` (system bash 3.2, see root CLAUDE.md) and `moon run repo:input-liveness` → pass.
- [ ] **Step 7: Commit.** `fix(py): ship the macOS strip flag in the paigasus-py-bindings sdist (SMA-767)`.

### Task 3: CI alignment guard, with a proof that it bites

**Files:**
- Modify: `.github/workflows/wheels.yml` (step "Assert Mach-O arch and minimum macOS (darwin only)": the guard for each `$so`, both arches; a NEW step after "Install from source into a clean venv, import and call" in `sdist-verify`, `if: runner.os == 'macOS'`, that locates the installed `*.abi3.so` via `"$vpy" -c 'import paigasus_py_bindings, os; print(os.path.dirname(paigasus_py_bindings.__file__))'` and runs the guard; the venv path must match the earlier step: `venv/bin/python`)
- Modify: `.github/workflows/prebuild.yml` (step "Verify Mach-O architecture (darwin only)": the guard for both `.node` files)
- Modify: `.github/CLAUDE.md` (wheels bullet: one sentence that the darwin steps assert `stroff mod 8 = 0`, and a pointer to SMA-767 / `rs/CLAUDE.md`)

**Interfaces:**
- Consumes: Task 1 (CI darwin builds are aligned), Task 2 (sdist config).
- Guard text (same in every copy, `$f` = the file):

```bash
stroff="$(otool -l "$f" | awk '/^ *stroff /&&!d{v=$2; d=1} END{print v}')"
case "$stroff" in ''|*[!0-9]*) echo "::error::no numeric LC_SYMTAB stroff in $f [$stroff] (SMA-767)"; exit 1;; esac
[ $((stroff % 8)) -eq 0 ] || { echo "::error::$f string pool at stroff $stroff is not 8-byte aligned; dyld on macOS 27 rejects it when linked with SDK 27 (SMA-767)"; exit 1; }
echo "$f stroff $stroff: 8-byte aligned"
```

- [ ] **Step 1: Bite proof (before the edit).** Write a scratch harness that extracts each guard copy's text from the workflow file (after Step 2) and runs it under `bash -euo pipefail` with `f` set. Cases and expected results:
  - `unfixed`: Task 1 Step 1 dylib (`402708`) → FAIL (alignment message)
  - `pypi-arm64`, `pypi-x86_64`: the PyPI 0.2.0 wheels' `.so` (download with `curl` into a new empty dir, inspect statically, never import) → FAIL
  - `npm-arm64`: npm `@paigasus/node-bindings-darwin-arm64` 0.2.0 `.node` → PASS
  - `no-symtab`: a text file → FAIL (rc ≠ 0)
  - `fixed`: Task 1 Step 3 dylib (`435672`) → PASS
- [ ] **Step 2: Add the guard to the three places.**
- [ ] **Step 3: Run the harness for every copy** (wheels Mach-O step, wheels sdist-verify step, prebuild step). All six cases match the expected column. Save the output for the PR body.
- [ ] **Step 4: Gates.** `repo:actionlint` full gate under `/opt/homebrew/bin/bash ci/actionlint/run.sh`; read its pipe-capacity preflight line first. If the preflight reports a small pipe (rc 2), say so and rely on CI. Then `moon run repo:workflow-credentials`.
- [ ] **Step 5: Commit.** `ci(rs): assert 8-byte string pool alignment on darwin bindings (SMA-767)`.

### Task 4: whole-branch verification

- [ ] **Step 1:** Update the spec §6/§8 only where a measured value differs from the plan. No other spec edits.
- [ ] **Step 2:** Run the full gate graph from the root `CLAUDE.md` (`moon ci … --base origin/main --include-relations`). Re-run the bash-4+ gates (`repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`) with `/opt/homebrew/bin/bash ci/<gate>/run.sh` and `repo:affected-smoke` with `/bin/bash`, per the root CLAUDE.md. Record any gate that cannot run locally.
- [ ] **Step 3:** Confirm D7: the changed files under `rs/crates/bindings/paigasus-py-bindings/` belong to a `publish = false` crate, so release-plz does not version it. Record it.
