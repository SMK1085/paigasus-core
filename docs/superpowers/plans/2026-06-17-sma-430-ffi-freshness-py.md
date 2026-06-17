# SMA-430 — FFI freshness: symmetric py wheel-guard fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `paigasus-kernel-py:test` runtime FFI guard assert against the *current* kernel source — never a stale, mtime-inverted `rs/target` artifact — mirroring the fix SMA-420 applied to the ts/napi guard.

**Architecture:** Two-layer fix to one Moon task (`py/packages/paigasus-kernel/moon.yml`, `test` only). Layer 1 — extend the task `inputs:` with the Rust sources + binding manifests so Moon's content hash re-runs the task on a kernel edit (closes the F4 cache-input gap). Layer 2 — prepend a host-agnostic `touch` of the kernel + py-bindings sources before `uv sync --reinstall-package`, so cargo's mtime-based incrementality recompiles from current source instead of re-linking a warm stale artifact. No production code changes; the kernel/binding/test files are untouched.

**Tech Stack:** Moon 2.3.2 task config (YAML), uv + maturin (PyO3 wheel build), cargo, pytest.

**Spec:** `docs/superpowers/specs/2026-06-17-sma-430-ffi-freshness-py-design.md` (+ its review).

---

## Prerequisites (read before starting)

- **PATH:** moon/uv/cargo are proto-managed and off the default Bash PATH. Before any command below, ensure:
  ```bash
  export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
  ```
  (shims FIRST = repo-pinned versions). Verify: `moon --version` → `2.3.2`, `uv --version` and `cargo --version` resolve.
- **Host:** dev host is macOS arm64. The trap commands use `perl -i` (portable) and `touch -t` (BSD/GNU compatible for the `[[CC]YY]MMDDhhmm` form used here).
- **Working dir:** all paths are relative to the repo root `/Users/smaschek/dev/paigasus/paigasus-core` unless a step says otherwise.
- **Branch:** already on `feature/sma-430-ffi-freshness-fix-the-cargo-mtime-stale-artifact-caveat`.
- **State note:** the trap mutates `rs/target/` (gitignored) and back-dates a source file's mtime. Every task that sets a trap also restores git-clean state. Confirm `git status` is clean before starting.

---

## Task 1: Reproduce the staleness bug on the *current* (unpatched) guard — RED

Prove that, with a warm mtime-inverted `rs/target`, the **current** `test` script serves a STALE wheel even though it runs `uv sync --reinstall-package`. This isolates the cargo/maturin mtime layer (raw script, no Moon cache involved).

**Files:**
- Trap target (reverted after): `rs/crates/libs/paigasus-kernel/src/lib.rs:14`
- Guard under test: `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py` (asserts `sum_as_string(2, 3) == "5"`)

- [ ] **Step 1: Tamper the kernel to `a + b + 1`**

```bash
perl -i -pe 's/^    a \+ b$/    a + b + 1/' rs/crates/libs/paigasus-kernel/src/lib.rs
grep -n 'a + b' rs/crates/libs/paigasus-kernel/src/lib.rs
```
Expected: line 14 now reads `    a + b + 1`.

- [ ] **Step 2: Build the tampered artifact into the warm `rs/target` (populate the stale `.so`)**

```bash
( cd py/packages/paigasus-kernel && uv sync --reinstall-package paigasus-py-bindings )
```
Expected: maturin compiles `paigasus-kernel` + `paigasus-py-bindings` from the tampered (`+1`) source; `rs/target/` now holds a `+1` kernel artifact. (No pytest yet.)

- [ ] **Step 3: Revert the source to `a + b` but back-date its mtime (simulate the inversion)**

```bash
git checkout -- rs/crates/libs/paigasus-kernel/src/lib.rs
touch -t 200001010000 rs/crates/libs/paigasus-kernel/src/lib.rs
grep -n 'a + b' rs/crates/libs/paigasus-kernel/src/lib.rs   # back to `a + b`
```
Expected: source content is the correct `a + b`, but its mtime (2000-01-01) is now OLDER than the `+1` artifact built in Step 2 — the exact mtime inversion a git checkout/rebase/stash can produce on a warm cache.

- [ ] **Step 4: Run the CURRENT unpatched script and observe the false result**

```bash
( cd py/packages/paigasus-kernel && uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests -v )
```
Expected: **FAIL** — `test_sum_crosses_ffi_boundary` raises `AssertionError: assert '6' == '5'`. Despite `--reinstall-package`, cargo saw the source as older than its artifact, skipped the recompile, and maturin re-linked the **stale `+1`** kernel into the fresh wheel. This is the defeated guard (it asserts against the wrong kernel). **Leave the trap in place** — the next task must overcome this same warm-stale state.

---

## Task 2: Apply the fix to `py/packages/paigasus-kernel/moon.yml`

**Files:**
- Modify: `py/packages/paigasus-kernel/moon.yml` (the `test` task only)

- [ ] **Step 1: Replace the `test` task block**

Replace the existing comment + `test:` block (everything from the `# Dedicated runtime FFI smoke test.` comment through the end of the file) with:

```yaml
  # Dedicated runtime FFI smoke test. It lives here (not only in the whole-tree py:test) so the
  # kernel→bindings→py cascade actually re-runs it on a Rust edit (review F4). uv serves a CACHED
  # maturin wheel and ignores Rust source changes (spike S4), so we MUST force a rebuild with
  # --reinstall-package before pytest — otherwise this would assert against a stale wheel.
  #
  # `touch` the kernel + binding sources first: maturin's underlying cargo is mtime-incremental, so
  # after a git op that leaves a Rust source OLDER than an existing rs/target/ artifact (warm cache +
  # checkout/rebase/stash → mtime inversion), cargo reports "up to date" and maturin re-links a STALE
  # .so into the freshly reinstalled wheel — the runtime guard then asserts against the wrong kernel
  # (SMA-420's review reproduced sum(2,3)→6 against an `a + b` kernel). Bumping the two (tiny) crates'
  # source mtime forces a content-correct recompile from current source — the py analog of the ts
  # `touch` (SMA-430). `touch` is host-agnostic (CI is Linux, dev is macOS); a per-triple
  # `cargo clean --target <triple>` would have to know the host triple (maturin builds into the
  # per-triple subdir), so we avoid it. Only `test` carries the touch — the `build` task above
  # neither materializes nor asserts the wheel, so it cannot serve a stale artifact (intentional
  # asymmetry with the ts side, whose `build` produces the .node).
  test:
    script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-py-bindings/src/lib.rs && uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests'
    deps: ['^:build']
    inputs:
      - 'tests/**/*'
      - 'src/**/*'
      - 'pyproject.toml'
      - '/py/uv.lock'
      # Both Rust sources: maturin recompiles the binding AND the kernel it calls, so a kernel-source
      # edit must bust this task's Moon cache too (review F4) — Moon hashes only listed inputs, and an
      # upstream `^:build` recompile alone does not re-key this task.
      - '/rs/crates/libs/paigasus-kernel/src/**/*'
      - '/rs/crates/bindings/paigasus-py-bindings/src/**/*'
      # ...and the binding's manifests, so a maturin-config (pyproject.toml) or dependency
      # (Cargo.toml) change re-keys the guard even without a Rust src diff (cache-input completeness).
      - '/rs/crates/bindings/paigasus-py-bindings/Cargo.toml'
      - '/rs/crates/bindings/paigasus-py-bindings/pyproject.toml'
```

The `build` task (`build:` / `deps: ['^:build']`) and everything above it (schema, id, layer, language, the `dependsOn` block + its comment) are unchanged.

- [ ] **Step 2: Sanity-check the YAML parses**

```bash
moon project paigasus-kernel-py >/dev/null && echo "moon parsed the project OK"
```
Expected: prints `moon parsed the project OK` (no schema/parse error).

---

## Task 3: Verify the fix defeats the trap — GREEN

Re-run against the **same** warm-stale state left by Task 1, this time through the patched script's `touch`. (Raw script again, to isolate the cargo-mtime layer from Moon's cache.)

- [ ] **Step 1: Confirm the trap is still in place**

```bash
grep -n 'a + b' rs/crates/libs/paigasus-kernel/src/lib.rs          # correct source: `a + b`
stat -f '%m %N' rs/crates/libs/paigasus-kernel/src/lib.rs          # mtime still ~946684800 (2000-01-01)
```
Expected: source is `a + b`; its mtime is the back-dated 2000 value (older than the Task-1 artifact).

- [ ] **Step 2: Run the patched task's exact script**

```bash
( cd py/packages/paigasus-kernel && touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-py-bindings/src/lib.rs && uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests -v )
```
Expected: **PASS** — `test_sum_crosses_ffi_boundary PASSED`. The `touch` bumped the kernel + binding source mtimes ahead of the warm artifact, so cargo recompiled the kernel from the correct `a + b` source and the wheel returned `"5"`. The fix overcomes the exact state that false-failed in Task 1.

- [ ] **Step 3: Reset to a clean state**

```bash
git checkout -- rs/crates/libs/paigasus-kernel/src/lib.rs
git status --short
```
Expected: `git status` shows only the modified `py/packages/paigasus-kernel/moon.yml` (the back-dated mtime is irrelevant to git; `rs/target/` is gitignored).

---

## Task 4: Verify a real kernel edit still fails through Moon — real-regression + F4 cache re-key

Prove the patched guard is not merely always-passing, and that Moon re-runs it (not a cache hit) on a kernel content change — exercising the full stack including Moon's input hash.

- [ ] **Step 1: Establish a passing Moon cache entry**

```bash
moon run paigasus-kernel-py:test
```
Expected: **PASS**. (May run fresh or report cached; either is fine — this just seeds the cache.)

- [ ] **Step 2: Make a real kernel edit (normal mtime, no back-dating)**

```bash
perl -i -pe 's/^    a \+ b$/    a + b + 1/' rs/crates/libs/paigasus-kernel/src/lib.rs
grep -n 'a + b' rs/crates/libs/paigasus-kernel/src/lib.rs   # now `a + b + 1`
```

- [ ] **Step 3: Re-run through Moon — must re-run AND fail**

```bash
moon run paigasus-kernel-py:test
```
Expected: **FAIL**, and the task is **NOT** reported as `(cached)` — Moon's content hash changed because the kernel source is now a listed input (F4), so it re-ran; the `touch` + recompile produced the `+1` kernel; pytest fails with `assert '6' == '5'`. (If it had reported `(cached)` or passed, the inputs fix is wrong — stop and investigate.)

- [ ] **Step 4: Revert the kernel and confirm clean**

```bash
git checkout -- rs/crates/libs/paigasus-kernel/src/lib.rs
grep -n 'a + b' rs/crates/libs/paigasus-kernel/src/lib.rs   # back to `a + b`
git status --short
```
Expected: only `py/packages/paigasus-kernel/moon.yml` modified.

---

## Task 5: Final clean run + commit

- [ ] **Step 1: Clean green run through Moon**

```bash
moon run paigasus-kernel-py:test
```
Expected: **PASS** against the real `a + b` kernel (`"5"`).

- [ ] **Step 2: Commit the fix**

```bash
git add py/packages/paigasus-kernel/moon.yml
git commit -m "$(cat <<'EOF'
fix(py): force content-correct recompile in the paigasus-kernel-py FFI guard

uv sync --reinstall-package rebuilds the wheel but maturin's cargo is
mtime-incremental, so a warm rs/target + a git mtime-inversion served a
STALE .so to the runtime guard. Prepend a host-agnostic touch of the
kernel + py-bindings sources before the rebuild (the py analog of the ts
napi fix, SMA-420), and add the Rust sources + binding manifests to the
task inputs so a kernel edit re-keys Moon's cache (F4). test-only: the
build task neither materializes nor asserts the wheel.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
Expected: commitlint (lefthook commit-msg) passes; commit is created and SSH-signed. (If signing fails with "failed to fill whole buffer", 1Password is locked — ask the user to unlock and retry the commit.)

- [ ] **Step 3: Confirm clean tree**

```bash
git status --short
```
Expected: empty (nothing to commit, working tree clean).

---

## Verification summary (what proves the fix)

| Check | Task | Expected |
| --- | --- | --- |
| Bug reproduced on unpatched guard (stale artifact served despite `--reinstall-package`) | 1 | FAIL `'6' == '5'` |
| Patched `touch` defeats the same warm-stale state | 3 | PASS `"5"` |
| Real kernel edit still fails (no false-green) **and** Moon re-keys on kernel input (F4) | 4 | re-runs (not cached), FAIL `'6' == '5'` |
| Clean run against real kernel | 5 | PASS `"5"` |

## Out of scope (per spec — do NOT implement here)

- CI `rs/target` cache-invalidation policy / protecting non-FFI cargo tasks from inversion (deferred; see spec "Out of scope" F2/F3).
- Adding `rs/Cargo.lock` to the guard inputs to catch FFI-crate version bumps (spec "Known limitation" F1 — would break ts/py symmetry; tighten both guards or neither).
- Any change to the `build` task, the kernel, the binding, or the test files.
