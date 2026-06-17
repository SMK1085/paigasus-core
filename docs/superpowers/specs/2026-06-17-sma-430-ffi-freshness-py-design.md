# SMA-430 — FFI freshness: symmetric py wheel-guard fix

**Date:** 2026-06-17
**Issue:** SMA-430 (follow-up of SMA-420)
**Related:** SMA-419 (PyO3 wheel guard), SMA-420 (ts/napi guard — the fix template)
**Scope decision:** py symmetric fix only. The CI `rs/target` cache-invalidation policy
is explicitly deferred (see "Out of scope").

## Problem

cargo / `napi build` / maturin incrementality is **mtime-based**. After a warm `rs/target/`
plus a git op that leaves a Rust source file's mtime OLDER than its existing artifact
(checkout / rebase / stash → mtime inversion), cargo reports "up to date" and does NOT
recompile — so the FFI build re-links a **stale** artifact and the runtime freshness guard
asserts against the wrong kernel. SMA-420's review reproduced `sum(2,3) → 6` against an
`a + b` kernel: a silent false red/green that defeats the guard's whole purpose.

SMA-420 fixed this on the **ts/napi** side. `paigasus-kernel-ts`'s `build` + `test` tasks
prepend a host-agnostic `touch` of the kernel + binding sources before `napi build`, forcing a
content-correct recompile from current source (`ts/packages/paigasus-kernel/moon.yml:33,73`).

The **py/maturin** guard is latently exposed to the identical bug. `paigasus-kernel-py:test`
runs `uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests`
(`py/packages/paigasus-kernel/moon.yml:23-24`). `--reinstall-package` forces a fresh *wheel*
build, but maturin's underlying cargo is still mtime-incremental — the same inversion can serve
a **stale wheel**.

### Second, related gap (found during brainstorming)

The py `test` task's `inputs:` list does **not** include the Rust sources. The ts side added
them (review "F4") so a kernel-only edit re-keys the task's Moon cache. Without them, Moon hashes
only the listed inputs, so a kernel-only edit can serve the py `test` from Moon cache and the
guard never even runs. Closing this belongs with the same fix.

## Design

### Mechanism: `touch`, not `cargo clean -p`

The spike doc's original sketch used
`cargo clean -p paigasus-kernel -p paigasus-py-bindings --target <triple>`, but that requires
knowing the host triple (maturin builds into the per-triple subdir; a bare `cargo clean -p` only
touches the host dir). The ts side rejected this for a host-agnostic `touch` of the sources
(CI is Linux, dev is macOS). We mirror that exact choice on py: proven, host-agnostic, and keeps
the two FFI guards symmetric.

### The change — `py/packages/paigasus-kernel/moon.yml`, `test` task only

**1. Prepend the touch** to the `test` script:

```yaml
script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-py-bindings/src/lib.rs && uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests'
```

Touch **both** crates' sources (not just the kernel): the inversion can hit either crate's
artifact, and maturin's cargo recompiles both. This bumps the (tiny, single-file) crates' source
mtime ahead of any warm artifact, forcing a content-correct recompile before the assertion — the
py analog of the ts `touch`.

**2. Add the missing Rust-source + binding-manifest inputs** so a kernel-only edit re-keys the
task's Moon cache (closes the F4 gap):

```yaml
inputs:
  - 'tests/**/*'
  - 'src/**/*'
  - 'pyproject.toml'
  - '/py/uv.lock'
  - '/rs/crates/libs/paigasus-kernel/src/**/*'
  - '/rs/crates/bindings/paigasus-py-bindings/src/**/*'
  - '/rs/crates/bindings/paigasus-py-bindings/Cargo.toml'
  - '/rs/crates/bindings/paigasus-py-bindings/pyproject.toml'
```

The bindings-crate `pyproject.toml` is the **maturin** manifest (the py analog of the
`package.json` the ts side added), so a maturin-config/dependency change also re-keys the guard
even without a Rust source diff.

### Why `test` only (not `build`)

Unlike the ts `build` task, the py `build` task (`deps: ['^:build']`, no script) does not build
or assert against the wheel. Wheel materialization (`uv sync --reinstall-package`) and the runtime
FFI assertion both live solely in `test` — so `test` is the only task that can assert against a
stale artifact. This intentional asymmetry with the ts side warrants an inline comment in the task.

### Comments

Update the existing `test`-task comment to record the freshness mechanism (mirroring the ts
moon.yml comments): why the `touch` is needed, why it's host-agnostic (vs a per-triple
`cargo clean`), and why only `test` (not `build`) carries it.

## Verification (the trap test, mirroring ts)

1. **Trap (false-green proof):** build a warm `rs/target` from a tampered kernel (`a + b + 1`),
   revert the source to `a + b` with an mtime OLDER than the artifact. Confirm the **unpatched**
   task serves a stale wheel (false green) while the **patched** task recompiles and passes.
2. **Real regression:** a genuine `a + b → a + b + 1` kernel edit still **fails** the patched
   guard (proves it is not merely always-passing).
3. Run `moon run paigasus-kernel-py:test` clean to confirm no breakage.

Restore the kernel to `a + b` (git-clean) afterward.

## Out of scope (deferred)

The CI `rs/target` cache-invalidation policy. The `touch` runs inside the `moon ci` graph, so
the FFI guards are already protected on CI once the py touch lands. CI's checkout-then-restore
ordering (sources written at "now"; `actions/cache`/tar preserves the older restored-artifact
mtimes) means the inversion is a local-demonstrated risk, not a natural CI one. Protecting the
*non-FFI* Rust tasks (cargo build / nextest / clippy) from inversion would be a separate, broader
follow-up — not required to close the FFI-guard gap this issue is about.

## Files touched

- `py/packages/paigasus-kernel/moon.yml` — `test` task: prepend `touch`, extend `inputs`, update
  comment.
- (verification only, reverted) `rs/crates/libs/paigasus-kernel/src/lib.rs`.
