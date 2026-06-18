# Cross-binding Behavioral Parity Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a parity harness where one Rust-generated corpus (`{a, b, expected}`, `expected` computed from `paigasus_kernel::sum`) is replayed against every binding (Python/PyO3, Node/napi, browser/wasm) and the Rust impl, plus a proptest suite on the kernel and a drift guard keeping the committed corpus in lockstep — so cross-language drift fails red before real domain logic lands.

**Architecture:** Kernel-as-oracle golden corpus (committed, deterministic, no PRNG). A new `paigasus-kernel-parity` Rust crate owns the corpus schema, a deterministic generator bin, and a Rust replay test. Python/TS replay the same JSON file. A `repo`-level Moon task regenerates and `git diff --exit-code`s the corpus; the strict-equality affected-graph guard is extended for the new crate. Spec: `docs/superpowers/specs/2026-06-18-sma-433-cross-binding-parity-harness-design.md`.

**Tech Stack:** Rust (edition 2024, 1.95), `proptest`, `serde`/`serde_json`; Python (`pytest`); TypeScript (`vitest`); Moon 2.3.2 orchestration; bash CI guards.

---

## Conventions for every task

- **PATH:** before any `moon`/`cargo`/`uv`/`pnpm` command, ensure proto shims resolve:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"` (shims FIRST = repo-pinned versions). The Bash tool runs `zsh`; there is no macOS `timeout`.
- **SPDX header:** every new source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python/YAML).
- **Commits:** Conventional, lowercase subject, workspace scope, `(SMA-433)` at the end — e.g. `test(rs): … (SMA-433)`. Commits are SSH-signed via 1Password; if a commit fails with "failed to fill whole buffer", ask the user to unlock 1Password and retry.
- **Expected-red window:** `ci/affected-graph/run.sh` (`repo:affected-smoke`) goes red the moment Task 5 registers the parity Moon project, and Task 5 is also where it goes green again. Do **not** run `repo:affected-smoke` in Tasks 1–4; those tasks verify with direct `cargo` commands.

## File Structure

**Create:**
- `rs/crates/libs/paigasus-kernel/tests/props.rs` — kernel property tests (proptest); the "against the Rust impl" half.
- `rs/crates/libs/paigasus-kernel-parity/Cargo.toml` — parity crate manifest (lib + bin).
- `rs/crates/libs/paigasus-kernel-parity/src/lib.rs` — `Case`, `build_corpus`, `serialize`, `corpus_path`, `load_corpus`.
- `rs/crates/libs/paigasus-kernel-parity/src/bin/gen-parity-vectors.rs` — deterministic corpus generator.
- `rs/crates/libs/paigasus-kernel-parity/tests/replay.rs` — corpus-integrity + Rust replay.
- `rs/crates/libs/paigasus-kernel-parity/vectors/sum.json` — committed corpus (generated, byte-stable).
- `rs/crates/libs/paigasus-kernel-parity/moon.yml` — Moon project (`id: paigasus-kernel-parity-rs`, `dependsOn: paigasus-kernel-rs`).
- `rs/crates/libs/paigasus-kernel-parity/README.md` — crate + drift-guard doc.
- `py/packages/paigasus-kernel/tests/test_parity.py` — Python parity replay.
- `ts/packages/paigasus-kernel/tests/corpus.ts` — shared corpus loader (single path constant).

**Modify:**
- `rs/Cargo.toml` — add `proptest` to `[workspace.dependencies]`.
- `rs/crates/libs/paigasus-kernel/Cargo.toml` — add `proptest` dev-dependency.
- `ci/affected-graph/run.sh` — add `paigasus-kernel-parity-rs` to the `kernel->bindings` case; add a `parity-oneway` case.
- `ci/affected-graph/README.md` — document both.
- `moon.yml` (repo project) — add the `parity-corpus-drift` task.
- `.github/workflows/ci.yml` — add `:parity-corpus-drift` to the `moon ci` target array.
- `py/packages/paigasus-kernel/moon.yml` — add the corpus to `test` inputs.
- `ts/packages/paigasus-kernel/moon.yml` — add the corpus to `build` + `test` inputs.
- `ts/packages/paigasus-kernel/tests/sum.test.ts` — rewrite as a corpus replay (napi).
- `ts/packages/paigasus-kernel/tests/sum.wasm.test.ts` — rewrite as a corpus replay (wasm).

**Delete:**
- `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py` — superseded by `test_parity.py`.

---

### Task 1: Kernel property-based suite (proptest)

**Files:**
- Modify: `rs/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `rs/crates/libs/paigasus-kernel/Cargo.toml`
- Create: `rs/crates/libs/paigasus-kernel/tests/props.rs`

- [ ] **Step 1: Add `proptest` to the workspace dependency table**

In `rs/Cargo.toml`, add this entry to `[workspace.dependencies]` immediately after the `wasm-bindgen = "0.2"` line (before the `# In-tree path dep:` comment):

```toml
# proptest — property-based coverage of the kernel's behavioral properties (the "against the
# Rust impl" half of the ADR-0005 parity harness, SMA-433). Dev-dependency only — never enters a
# published artifact.
proptest = "1"
```

- [ ] **Step 2: Declare it as a kernel dev-dependency**

In `rs/crates/libs/paigasus-kernel/Cargo.toml`, add a `[dev-dependencies]` section after the `publish = false` line (before `[lints]`):

```toml
[dev-dependencies]
proptest.workspace = true
```

- [ ] **Step 3: Write the property test**

Create `rs/crates/libs/paigasus-kernel/tests/props.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Property-based coverage of the kernel itself — the "property-based suite against the Rust
//! impl" half of ADR-0005's safety net (SMA-433). Randomized, fresh each run; proptest persists
//! any failing seed to `proptest-regressions/` for reproduction. Inputs are drawn as `i32` and
//! widened to the kernel's `i64`, so `a + b` can never overflow `i64` — and the range mirrors the
//! committed corpus's i32-safe parity domain.

use paigasus_kernel::sum;
use proptest::prelude::*;

proptest! {
    #[test]
    fn matches_integer_addition(a: i32, b: i32) {
        prop_assert_eq!(sum(a as i64, b as i64), a as i64 + b as i64);
    }

    #[test]
    fn is_commutative(a: i32, b: i32) {
        prop_assert_eq!(sum(a as i64, b as i64), sum(b as i64, a as i64));
    }

    #[test]
    fn zero_is_identity(a: i64) {
        prop_assert_eq!(sum(a, 0), a);
    }
}
```

- [ ] **Step 4: Run the property test**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"; cargo nextest run --manifest-path rs/Cargo.toml -p paigasus-kernel`
Expected: PASS — the existing `sums_two_integers` unit test plus three proptest cases (`matches_integer_addition`, `is_commutative`, `zero_is_identity`).

- [ ] **Step 5: Verify lint/fmt are clean on the new test**

Run: `cargo fmt --manifest-path rs/Cargo.toml --check && cargo clippy --manifest-path rs/Cargo.toml -p paigasus-kernel --all-targets -- -D warnings`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/libs/paigasus-kernel/tests/props.rs
git commit -m "test(rs): property-based suite for the kernel (SMA-433)"
```

---

### Task 2: The `paigasus-kernel-parity` crate (corpus schema + generator logic)

**Files:**
- Create: `rs/crates/libs/paigasus-kernel-parity/Cargo.toml`
- Create: `rs/crates/libs/paigasus-kernel-parity/src/lib.rs`

No `moon.yml` yet — this task verifies with direct `cargo` so the affected-graph guard stays green until Task 5.

- [ ] **Step 1: Create the crate manifest**

Create `rs/crates/libs/paigasus-kernel-parity/Cargo.toml`:

```toml
[package]
name = "paigasus-kernel-parity"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
# Test-support crate: owns the cross-binding parity corpus + its generator. Never published.
publish = false

[dependencies]
paigasus-kernel.workspace = true
serde = { workspace = true }
serde_json.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Write the failing unit tests for the library API**

Create `rs/crates/libs/paigasus-kernel-parity/src/lib.rs` with ONLY the tests first (so they fail to compile against absent items):

```rust
// SPDX-License-Identifier: Apache-2.0
//! Cross-binding behavioral-parity corpus for the Paigasus kernel (ADR-0005, SMA-433).
//!
//! The corpus is a set of `{a, b, expected}` cases where `expected = paigasus_kernel::sum(a, b)` —
//! the kernel is the single oracle. Every binding (Python/PyO3, Node/napi, browser/wasm) and the
//! Rust impl replay this same file and must reproduce `expected`. The sample is a deterministic
//! enumeration (no PRNG) over the i32-safe parity domain, so regeneration is byte-stable and the
//! drift guard's `git diff` is meaningful. See the design spec for why the domain is i32-safe.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_is_non_empty_and_in_domain() {
        let cases = build_corpus();
        assert!(!cases.is_empty(), "generated corpus is empty");
        for c in &cases {
            // Every case lies in the parity domain: `expected` fits i32 (so napi/wasm don't wrap)…
            assert!(i32::try_from(c.expected).is_ok(), "case out of i32 parity domain: {c:?}");
            // …and equals the kernel for that input (the corpus is kernel-derived by construction).
            assert_eq!(c.expected, paigasus_kernel::sum(c.a as i64, c.b as i64));
        }
    }

    #[test]
    fn serialize_is_stable_and_round_trips() {
        let cases = build_corpus();
        let text = serialize(&cases);
        // Byte-stability contract: one case per line, trailing newline.
        assert!(text.ends_with("]\n"), "corpus must end with a newline");
        assert_eq!(text.matches('\n').count(), cases.len() + 2, "expect one line per case + brackets");
        let parsed: Vec<Case> = serde_json::from_str(&text).expect("round-trips");
        assert_eq!(parsed, cases);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --manifest-path rs/Cargo.toml -p paigasus-kernel-parity 2>&1 | head -20`
Expected: FAIL — compile errors `cannot find function build_corpus` / `cannot find function serialize` / `cannot find type Case`.

- [ ] **Step 4: Implement the library**

Prepend the implementation to `rs/crates/libs/paigasus-kernel-parity/src/lib.rs`, above the `#[cfg(test)]` module (keep the file's `//!` header at the very top):

```rust
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One parity case: inputs `a`, `b` and the kernel-computed `expected`. `a`/`b` are `i32` (the
/// narrowest binding surface — napi/wasm); `expected` is the kernel's `i64` result, constrained
/// to the i32 range by [`build_corpus`] so every binding agrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub a: i32,
    pub b: i32,
    pub expected: i64,
}

/// Representative i32 values, including the boundaries. Pairs are taken across the full lattice and
/// filtered to the parity domain; the order is fixed, so the generated corpus is deterministic.
const SAMPLE_VALUES: [i32; 9] = [
    i32::MIN, -1_000_000, -1_000, -1, 0, 1, 1_000, 1_000_000, i32::MAX,
];

/// Build the deterministic corpus: every `(a, b)` from the lattice whose kernel sum stays within
/// `i32` (the parity domain — outside it napi/wasm would wrap while Python would not). `expected`
/// is computed FROM the kernel, making the kernel the single oracle.
#[must_use]
pub fn build_corpus() -> Vec<Case> {
    let mut cases = Vec::new();
    for &a in &SAMPLE_VALUES {
        for &b in &SAMPLE_VALUES {
            let expected = paigasus_kernel::sum(a as i64, b as i64);
            if i32::try_from(expected).is_ok() {
                cases.push(Case { a, b, expected });
            }
        }
    }
    cases
}

/// Serialize the corpus byte-stably: a JSON array with exactly one compact case object per line and
/// a trailing newline. Stable across runs/toolchains so the drift guard's `git diff` is meaningful
/// (`serde_json::to_string_pretty` would also be stable, but one-line-per-case gives cleaner diffs).
#[must_use]
pub fn serialize(cases: &[Case]) -> String {
    let mut out = String::from("[\n");
    for (i, c) in cases.iter().enumerate() {
        let comma = if i + 1 < cases.len() { "," } else { "" };
        out.push_str("  ");
        out.push_str(&serde_json::to_string(c).expect("Case serializes infallibly"));
        out.push_str(comma);
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

/// Absolute path to the committed corpus, resolved against this crate's directory so it is
/// independent of the process working directory (the generator and the replay both use it).
#[must_use]
pub fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors/sum.json")
}

/// Load and parse the committed corpus. Panics (i.e. fails a test red) if it is missing or invalid.
#[must_use]
pub fn load_corpus() -> Vec<Case> {
    let text = std::fs::read_to_string(corpus_path()).expect("read committed corpus");
    serde_json::from_str(&text).expect("parse committed corpus")
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path rs/Cargo.toml -p paigasus-kernel-parity`
Expected: PASS — `corpus_is_non_empty_and_in_domain` and `serialize_is_stable_and_round_trips`. (`load_corpus` is exercised in Task 4, after the corpus file exists.)

- [ ] **Step 6: Lint/fmt clean**

Run: `cargo fmt --manifest-path rs/Cargo.toml --check && cargo clippy --manifest-path rs/Cargo.toml -p paigasus-kernel-parity --all-targets -- -D warnings`
Expected: no output, exit 0.

- [ ] **Step 7: Commit**

```bash
git add rs/Cargo.lock rs/crates/libs/paigasus-kernel-parity/Cargo.toml rs/crates/libs/paigasus-kernel-parity/src/lib.rs
git commit -m "feat(rs): paigasus-kernel-parity crate + corpus builder (SMA-433)"
```

---

### Task 3: Corpus generator bin + commit the generated corpus

**Files:**
- Create: `rs/crates/libs/paigasus-kernel-parity/src/bin/gen-parity-vectors.rs`
- Create (generated): `rs/crates/libs/paigasus-kernel-parity/vectors/sum.json`

- [ ] **Step 1: Write the generator bin**

Create `rs/crates/libs/paigasus-kernel-parity/src/bin/gen-parity-vectors.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Regenerates the committed parity corpus from the current kernel. Run via the
//! `repo:parity-corpus-drift` Moon task, which then `git diff --exit-code`s the result — so a
//! kernel edit landed without regenerating the corpus fails CI red (SMA-433).

use std::fs;

use paigasus_kernel_parity::{build_corpus, corpus_path, serialize};

fn main() -> std::io::Result<()> {
    let cases = build_corpus();
    let path = corpus_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serialize(&cases))?;
    eprintln!("wrote {} parity cases to {}", cases.len(), path.display());
    Ok(())
}
```

- [ ] **Step 2: Run the generator to produce the corpus**

Run: `( cd rs && cargo run -p paigasus-kernel-parity --bin gen-parity-vectors )`
Expected: stderr like `wrote 69 parity cases to …/vectors/sum.json` and the file now exists.

- [ ] **Step 3: Eyeball the generated corpus**

Run: `head -5 rs/crates/libs/paigasus-kernel-parity/vectors/sum.json && echo … && tail -3 rs/crates/libs/paigasus-kernel-parity/vectors/sum.json`
Expected: a JSON array, one compact object per line, e.g.:

```json
[
  {"a":-2147483648,"b":0,"expected":-2147483648},
  {"a":-2147483648,"b":1,"expected":-2147483647},
  {"a":-2147483648,"b":1000,"expected":-2147482648},
  {"a":-2147483648,"b":1000000,"expected":-2146483648},
…
  {"a":2147483647,"b":-1,"expected":2147483646},
  {"a":2147483647,"b":0,"expected":2147483647}
]
```

- [ ] **Step 4: Verify regeneration is byte-stable (no diff on re-run)**

Run: `( cd rs && cargo run -p paigasus-kernel-parity --bin gen-parity-vectors ) && git diff --exit-code rs/crates/libs/paigasus-kernel-parity/vectors/sum.json`
Expected: the file is staged/untracked but a second generation produces no change — `git diff` exits 0 (this is exactly what the Task 8 drift guard will assert).

- [ ] **Step 5: Lint/fmt clean (bin is covered by `--all-targets`)**

Run: `cargo fmt --manifest-path rs/Cargo.toml --check && cargo clippy --manifest-path rs/Cargo.toml -p paigasus-kernel-parity --all-targets -- -D warnings`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-kernel-parity/src/bin/gen-parity-vectors.rs rs/crates/libs/paigasus-kernel-parity/vectors/sum.json
git commit -m "test(rs): generated cross-binding parity corpus (SMA-433)"
```

---

### Task 4: Rust replay + corpus-integrity test

**Files:**
- Create: `rs/crates/libs/paigasus-kernel-parity/tests/replay.rs`

- [ ] **Step 1: Write the replay/integrity test**

Create `rs/crates/libs/paigasus-kernel-parity/tests/replay.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! The Rust side of the 4-way replay (the kernel as its own "binding") + the corpus-integrity
//! guard shared by every language replay (SMA-433). The integrity guard is the load-bearing part:
//! a replay that only iterates-and-asserts goes GREEN if the corpus fails to load or is empty.

use paigasus_kernel_parity::{build_corpus, load_corpus};

#[test]
fn committed_corpus_is_non_empty() {
    // Integrity guard: a missing/empty corpus must fail RED, not pass having compared nothing.
    assert!(!load_corpus().is_empty(), "committed parity corpus is empty");
}

#[test]
fn committed_corpus_matches_a_fresh_generation() {
    // The committed vectors must equal what the current kernel produces. Catches a kernel edit
    // landed without regenerating the corpus (an in-process complement to repo:parity-corpus-drift),
    // and self-validates the file against a hand-edit. Full Vec equality subsumes count + per-case
    // value + domain.
    assert_eq!(load_corpus(), build_corpus());
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo nextest run --manifest-path rs/Cargo.toml -p paigasus-kernel-parity`
Expected: PASS — the two lib unit tests plus `committed_corpus_is_non_empty` and `committed_corpus_matches_a_fresh_generation`.

- [ ] **Step 3: Prove the integrity guard bites (temporary tamper)**

Run:
```bash
printf '[]\n' > /tmp/sma433-backup.json
cp rs/crates/libs/paigasus-kernel-parity/vectors/sum.json /tmp/sma433-backup.json
printf '[]\n' > rs/crates/libs/paigasus-kernel-parity/vectors/sum.json
cargo nextest run --manifest-path rs/Cargo.toml -p paigasus-kernel-parity 2>&1 | tail -20
cp /tmp/sma433-backup.json rs/crates/libs/paigasus-kernel-parity/vectors/sum.json
```
Expected: with an emptied corpus the run FAILS (`committed_corpus_is_non_empty` + `committed_corpus_matches_a_fresh_generation`), then the file is restored. Confirm `git diff --exit-code rs/crates/libs/paigasus-kernel-parity/vectors/sum.json` is clean after restore.

- [ ] **Step 4: Lint/fmt clean**

Run: `cargo fmt --manifest-path rs/Cargo.toml --check && cargo clippy --manifest-path rs/Cargo.toml -p paigasus-kernel-parity --all-targets -- -D warnings`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-kernel-parity/tests/replay.rs
git commit -m "test(rs): corpus integrity + rust parity replay (SMA-433)"
```

---

### Task 5: Register the Moon project + extend the affected-graph guard

This is where `paigasus-kernel-parity-rs` enters the Moon graph; the strict-equality guard forces the matching update in the same task.

**Files:**
- Create: `rs/crates/libs/paigasus-kernel-parity/moon.yml`
- Create: `rs/crates/libs/paigasus-kernel-parity/README.md`
- Modify: `ci/affected-graph/run.sh`
- Modify: `ci/affected-graph/README.md`

- [ ] **Step 1: Create the Moon project file**

Create `rs/crates/libs/paigasus-kernel-parity/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-parity-rs'
layer: 'library'
language: 'rust'

# Cross-language edge to the kernel (ADR-0005): a kernel change must rebuild this crate so its
# replay/drift tests re-run against the new kernel. The task-level `^:build` is what propagates
# `affected` under `moon ci --include-relations` — a project dependsOn alone does not (SMA-389 D3).
# Mirrors the binding crates.
dependsOn:
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

- [ ] **Step 2: Create the crate README (documents the drift guard, per spec decision #4)**

Create `rs/crates/libs/paigasus-kernel-parity/README.md`:

```markdown
# paigasus-kernel-parity

Cross-binding behavioral-parity corpus for the Paigasus kernel (ADR-0005, SMA-433).

`vectors/sum.json` is a committed, kernel-derived corpus of `{a, b, expected}` cases over the
i32-safe parity domain. Every binding (Python/PyO3, Node/napi, browser/wasm) and the Rust impl
replay it and must reproduce `expected` — the kernel is the single oracle.

- **Regenerate:** `cargo run -p paigasus-kernel-parity --bin gen-parity-vectors` (run from `rs/`).
  The sample is a deterministic enumeration (no PRNG), so output is byte-stable.
- **Drift guard:** the `repo:parity-corpus-drift` Moon task regenerates the corpus and
  `git diff --exit-code`s it, so a kernel edit landed without regenerating fails CI red. The
  in-crate `tests/replay.rs` asserts the same thing in `cargo nextest`.

Scope note: parity here is *decoded-value* equality on the i32-safe domain, not *surface*
identity — the Python binding returns a stringified i64 (`sum_as_string`), napi/wasm a `number`.
Surface unification + the full i64 range are deferred (spec § Out of scope, L5).
```

- [ ] **Step 3: Add `paigasus-kernel-parity-rs` to the `kernel->bindings` case**

In `ci/affected-graph/run.sh`, find the `kernel->bindings` case and append the new project to its expected CSV. Replace:

```bash
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts,paigasus-wasm-rs"
```

with:

```bash
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts,paigasus-wasm-rs,paigasus-kernel-parity-rs"
```

- [ ] **Step 4: Add the `parity-oneway` case**

In `ci/affected-graph/run.sh`, immediately after the `binding-oneway-wasm` `run_case` block (and before the `assert_include_relations || SUITE_RC=1` line), add:

```bash
  # parity crate edit -> only the parity crate. One-directional w.r.t. the kernel: a parity edit
  # must NOT rebuild the kernel (paigasus-kernel-rs deliberately absent), now enforced implicitly by
  # strict equality. Confirms Moon treats the cross-project corpus `inputs` of the py/ts test tasks
  # as task-hash keys, NOT as project-affected edges (so py/ts do not appear here) — SMA-433.
  run_case "parity-oneway" "rs/crates/libs/paigasus-kernel-parity/src/lib.rs" \
    "paigasus-kernel-parity-rs"
```

- [ ] **Step 5: Update the guard README**

In `ci/affected-graph/README.md`, find the `**kernel edit**` bullet and append `+ paigasus-kernel-parity-rs` to its project list (after `paigasus-kernel-ts`). Then add a new bullet immediately after the `**wasm binding edit**` bullet:

```markdown
- **parity-crate edit** → `paigasus-kernel-parity-rs`; one-directional w.r.t. the kernel (a parity
  edit must not rebuild the kernel). The py/ts parity tests list the corpus as a task `input`
  (cache-keying), which does not make them project-affected by a corpus-only edit.
```

- [ ] **Step 6: Run the affected-graph guard (now green again)**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"; moon run repo:affected-smoke`
Expected: PASS — all cases including `kernel->bindings` (now listing `paigasus-kernel-parity-rs`) and the new `parity-oneway`. If `parity-oneway` reports `unexpected paigasus-... ` (e.g. py/ts appear), Moon's cross-project-input semantics differ from the spec's M2 assumption — set the case's expected set to exactly what it reports and note it in the README.

- [ ] **Step 7: Run the negative control**

Run: `ci/affected-graph/run.sh --negative-control`
Expected: PASS — "negative-control OK: harness reported red on all wrong expectations".

- [ ] **Step 8: Commit**

```bash
git add rs/crates/libs/paigasus-kernel-parity/moon.yml rs/crates/libs/paigasus-kernel-parity/README.md ci/affected-graph/run.sh ci/affected-graph/README.md
git commit -m "build(rs): register parity project + extend affected-graph guard (SMA-433)"
```

---

### Task 6: Python parity replay

**Files:**
- Delete: `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py`
- Create: `py/packages/paigasus-kernel/tests/test_parity.py`
- Modify: `py/packages/paigasus-kernel/moon.yml` (`test` task `inputs`)

- [ ] **Step 1: Replace the smoke test with the corpus replay**

Delete `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py` and create `py/packages/paigasus-kernel/tests/test_parity.py`:

```python
# SPDX-License-Identifier: Apache-2.0
"""Cross-binding parity: the PyO3 wheel must reproduce the kernel-computed corpus (SMA-433).

The corpus is generated once from the Rust kernel (the single oracle) and committed under the
`paigasus-kernel-parity` crate; here we replay it through `sum_as_string`. Parity is decoded-value
equality — the PyO3 surface returns a stringified i64, so we compare against `str(expected)`.
"""

import json
from pathlib import Path

import pytest

from paigasus_kernel import sum_as_string

# Single resolved path constant (the committed corpus lives in the Rust parity crate). From this
# file: tests -> paigasus-kernel -> packages -> py -> repo root == parents[4].
CORPUS_PATH = Path(__file__).resolve().parents[4] / "rs/crates/libs/paigasus-kernel-parity/vectors/sum.json"
CASES: list[dict[str, int]] = json.loads(CORPUS_PATH.read_text())


def test_corpus_is_present_and_non_empty() -> None:
    # Integrity guard: a wrong path / empty corpus must fail RED. An empty `parametrize` set is
    # reported by pytest as a *skipped* test (`got empty parameter set`), i.e. a green run that
    # compared nothing — the worst failure mode for a safety net.
    assert CORPUS_PATH.exists(), f"parity corpus not found at {CORPUS_PATH}"
    assert len(CASES) > 0


@pytest.mark.parametrize("case", CASES, ids=[f"{c['a']}+{c['b']}" for c in CASES])
def test_sum_as_string_matches_corpus(case: dict[str, int]) -> None:
    assert sum_as_string(case["a"], case["b"]) == str(case["expected"])
```

- [ ] **Step 2: Add the corpus to the `test` task inputs**

In `py/packages/paigasus-kernel/moon.yml`, add this line to the end of the `test` task's `inputs:` list (after `'/rs/crates/bindings/paigasus-py-bindings/pyproject.toml'`):

```yaml
      # Parity corpus (SMA-433): a corpus change must re-run the py replay. This is a task-hash
      # input only — it does NOT make this project affected by a corpus-only edit (see affected-smoke
      # parity-oneway).
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/sum.json'
```

- [ ] **Step 3: Run the Python replay via Moon**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"; moon run paigasus-kernel-py:test`
Expected: PASS — `test_corpus_is_present_and_non_empty` plus one parametrized case per corpus row (the task rebuilds the wheel via `uv sync --reinstall-package` first).

- [ ] **Step 4: Typecheck/lint the new test (basedpyright + ruff run in the py graph)**

Run: `moon run py:typecheck py:lint 2>&1 | tail -20` (or the repo's py check tasks if named differently — confirm with `moon query tasks --affected` if unsure)
Expected: PASS — no basedpyright/ruff errors on `test_parity.py`. (If `py:typecheck`/`py:lint` are not the task ids, run `cd py && uv run basedpyright packages/paigasus-kernel && uv run ruff check packages/paigasus-kernel`.)

- [ ] **Step 5: Commit**

```bash
git add py/packages/paigasus-kernel/tests/test_parity.py py/packages/paigasus-kernel/moon.yml
git rm py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py
git commit -m "test(py): parity replay against the kernel corpus (SMA-433)"
```

---

### Task 7: TypeScript parity replay (napi + wasm)

**Files:**
- Create: `ts/packages/paigasus-kernel/tests/corpus.ts`
- Modify: `ts/packages/paigasus-kernel/tests/sum.test.ts`
- Modify: `ts/packages/paigasus-kernel/tests/sum.wasm.test.ts`
- Modify: `ts/packages/paigasus-kernel/moon.yml` (`build` + `test` inputs)

- [ ] **Step 1: Create the shared corpus loader**

Create `ts/packages/paigasus-kernel/tests/corpus.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export interface ParityCase {
  a: number;
  b: number;
  expected: number;
}

// Single resolved path constant: the committed corpus lives in the Rust parity crate. From this
// file: tests -> paigasus-kernel -> packages -> ts -> repo root == four `../`.
const corpusPath = fileURLToPath(
  new URL('../../../../rs/crates/libs/paigasus-kernel-parity/vectors/sum.json', import.meta.url),
);

export const cases: ParityCase[] = JSON.parse(readFileSync(corpusPath, 'utf8')) as ParityCase[];
```

- [ ] **Step 2: Rewrite the napi test as a corpus replay**

Replace the entire contents of `ts/packages/paigasus-kernel/tests/sum.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sum } from '@paigasus/kernel';
import { cases } from './corpus';

describe('kernel FFI parity (napi)', () => {
  it('corpus is present and non-empty', () => {
    // Integrity guard: an empty corpus (a bad path) registers zero `it.each` cases below, so
    // without this assertion the file would pass green having compared nothing.
    expect(cases.length).toBeGreaterThan(0);
  });

  it.each(cases)('sum($a, $b) === $expected', ({ a, b, expected }) => {
    expect(sum(a, b)).toBe(expected);
  });
});
```

- [ ] **Step 3: Rewrite the wasm test as a corpus replay**

Replace the entire contents of `ts/packages/paigasus-kernel/tests/sum.wasm.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sum } from '@paigasus/kernel';
import { cases } from './corpus';

describe('kernel FFI parity (wasm)', () => {
  it('corpus is present and non-empty', () => {
    // Integrity guard: an empty corpus (a bad path) registers zero `it.each` cases below, so
    // without this assertion the file would pass green having compared nothing.
    expect(cases.length).toBeGreaterThan(0);
  });

  it.each(cases)('sum($a, $b) === $expected', ({ a, b, expected }) => {
    expect(sum(a, b)).toBe(expected);
  });
});
```

- [ ] **Step 4: Add the corpus to the `build` and `test` task inputs**

In `ts/packages/paigasus-kernel/moon.yml`, add this line to the end of BOTH the `build` task's and the `test` task's `inputs:` lists (after each `'/rs/crates/bindings/paigasus-wasm/package.json'`):

```yaml
      # Parity corpus (SMA-433): a corpus change must re-key the ts replay. Task-hash input only —
      # it does not make this project affected by a corpus-only edit (see affected-smoke parity-oneway).
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/sum.json'
```

- [ ] **Step 5: Run the TS replay via Moon (builds .node + wasm, runs both vitest projects)**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"; moon run paigasus-kernel-ts:test`
Expected: PASS — both the `node` and `browser` vitest projects: each shows a `corpus is present and non-empty` test plus one `sum(a, b) === expected` case per corpus row.

- [ ] **Step 6: Typecheck (the kernel build task runs `tsc --noEmit` over `tests/**`)**

Run: `moon run paigasus-kernel-ts:build`
Expected: PASS — `tsc` typechecks `corpus.ts` and both test files (the tsconfig already includes `tests/**/*`); `binding-parity.types.ts` still compiles.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-kernel/tests/corpus.ts ts/packages/paigasus-kernel/tests/sum.test.ts ts/packages/paigasus-kernel/tests/sum.wasm.test.ts ts/packages/paigasus-kernel/moon.yml
git commit -m "test(ts): parity replay against the kernel corpus (SMA-433)"
```

---

### Task 8: Corpus drift guard (Moon task + CI wiring)

**Files:**
- Modify: `moon.yml` (repo project)
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the drift-guard task to the repo project**

In `moon.yml` (repo root), add this task to the `tasks:` map (after the `affected-smoke` task):

```yaml
  parity-corpus-drift:
    description: 'Regenerate the parity corpus from the kernel; assert it matches the committed vectors (SMA-433).'
    # Run the generator crate-scoped from rs/ so rs/.cargo/config.toml is in scope, then assert the
    # committed corpus is unchanged. NEVER broaden to --workspace: the parity crate is a plain
    # lib+bin (no cdylib), so it needs none of the apple-darwin link flags the FFI cdylibs do.
    script: '( cd rs && cargo run -p paigasus-kernel-parity --bin gen-parity-vectors ) && git diff --exit-code rs/crates/libs/paigasus-kernel-parity/vectors/sum.json'
    toolchain: 'system'
    # Narrow inputs — `repo` owns the whole tree, so without these the guard would run on every change.
    inputs:
      - 'rs/crates/libs/paigasus-kernel/src/**/*'
      - 'rs/crates/libs/paigasus-kernel-parity/src/**/*'
      - 'rs/crates/libs/paigasus-kernel-parity/Cargo.toml'
      - 'rs/crates/libs/paigasus-kernel-parity/vectors/sum.json'
```

- [ ] **Step 2: Add the task to the CI target array**

In `.github/workflows/ci.yml`, find the `T=(...)` line and add `:parity-corpus-drift`. Replace:

```bash
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :release-parity :release-parity-py :release-parity-ts)
```

with:

```bash
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 3: Run the drift guard (clean tree → pass)**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"; moon run repo:parity-corpus-drift`
Expected: PASS — regeneration produces no change, `git diff --exit-code` exits 0.

- [ ] **Step 4: Prove the guard catches kernel drift (temporary tamper)**

Run:
```bash
sed -i.bak 's/a + b/a + b + 1/' rs/crates/libs/paigasus-kernel/src/lib.rs
moon run repo:parity-corpus-drift; echo "exit=$?"
git checkout rs/crates/libs/paigasus-kernel/src/lib.rs rs/crates/libs/paigasus-kernel-parity/vectors/sum.json
rm -f rs/crates/libs/paigasus-kernel/src/lib.rs.bak
```
Expected: with the kernel changed but the corpus not regenerated-and-committed, the guard FAILS (`git diff --exit-code` reports the now-stale `sum.json`), `exit=1`. The `git checkout` restores both files; confirm `git status --short` is clean afterward.

- [ ] **Step 5: Commit**

```bash
git add moon.yml .github/workflows/ci.yml
git commit -m "ci(rs): parity-corpus drift guard (SMA-433)"
```

---

### Task 9: Full-suite verification

No new files — a final green sweep across all four stacks and the guards, mirroring the CI target array.

- [ ] **Step 1: Rust — tests, fmt, clippy across the workspace**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"; cargo nextest run --manifest-path rs/Cargo.toml --workspace --no-tests=pass && cargo fmt --manifest-path rs/Cargo.toml --check && cargo clippy --manifest-path rs/Cargo.toml --workspace --all-targets -- -D warnings`
Expected: PASS — kernel proptest, parity lib/replay tests, all existing crates; no fmt/clippy findings.

- [ ] **Step 2: Supply-chain gates (proptest's new transitive deps)**

Run: `moon run repo:deny repo:machete`
Expected: PASS. If `cargo machete` flags `proptest` as unused (it scans `tests/`, but has had edge cases with integration-test-only dev-deps), add to `rs/crates/libs/paigasus-kernel/Cargo.toml`:
```toml
[package.metadata.cargo-machete]
ignored = ["proptest"]
```
and re-run. If `cargo deny` flags a license from a proptest transitive dep, confirm it is Apache/MIT-compatible and add it to the `allow` list in `rs/deny.toml` with a comment (do not add blanket exceptions).

- [ ] **Step 3: Affected-graph guard + negative control**

Run: `moon run repo:affected-smoke && ci/affected-graph/run.sh --negative-control`
Expected: PASS both.

- [ ] **Step 4: Drift guard + per-stack parity replays**

Run: `moon run repo:parity-corpus-drift paigasus-kernel-rs:test paigasus-kernel-parity-rs:test paigasus-kernel-py:test paigasus-kernel-ts:test`
Expected: PASS — drift clean; Rust/Python/TS replays all green.

- [ ] **Step 5: Confirm a kernel edit cascades to every replay (affected graph end-to-end)**

Run:
```bash
moon query projects --affected --downstream deep <<< "rs/crates/libs/paigasus-kernel/src/lib.rs" \
  | python3 -c 'import sys,json; print("\n".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"])))'
```
Expected: the set includes `paigasus-kernel-parity-rs`, `paigasus-kernel-py`, `paigasus-kernel-ts`, the three binding crates, the gateway, and `repo` — confirming a kernel change re-runs the parity suite across all bindings (scope bullet 2).

- [ ] **Step 6: Confirm the working tree is clean**

Run: `git status --short`
Expected: empty (every task committed its own work; the tamper steps were reverted).

---

## Self-Review

**Spec coverage** (against `2026-06-18-sma-433-cross-binding-parity-harness-design.md`):
- Decision #1 kernel-as-oracle corpus → Tasks 2–3 (`build_corpus` computes `expected` from the kernel). ✓
- Decision #2 committed + drift guard + proptest, no PRNG → Task 1 (proptest), Task 2/3 (deterministic `SAMPLE_VALUES`, no `rand`), Task 8 (drift guard). ✓
- Decision #3 i32-safe parity domain + per-binding comparison (py `str`, napi/wasm number, rust value) → `build_corpus` filter (Task 2), Task 6 `str(expected)`, Task 7 number, Task 4 value. ✓
- Decision #4 co-located corpus, one file per fn → `…/paigasus-kernel-parity/vectors/sum.json` (Task 3). ✓
- Decision #5 dedicated crate, kernel pure, proptest dev-dep → Tasks 1–2. ✓
- Decision #6 minimal Rust replay → Task 4. ✓
- Decision #7 corpus-integrity guard in every replay → Task 4 (rust), Task 6 (py), Task 7 (ts). ✓
- Moon wiring + affected-graph guard update (kernel case += parity-rs, `parity-oneway`) → Task 5; corpus added to py/ts test inputs → Tasks 6–7. ✓
- Verification mapping (deny/machete green; affected-smoke + negative control; drift fails on stale corpus; cross-stack isolation) → Task 9 + Task 5/8 tamper steps. ✓

**Placeholder scan:** no TBD/TODO/"handle edge cases"; every code step shows complete content. The corpus file content (Task 3 Step 3) is illustrative output, not something to type — it is generated. ✓

**Type/name consistency:** `Case { a: i32, b: i32, expected: i64 }`, `build_corpus`, `serialize`, `corpus_path`, `load_corpus` are defined in Task 2 and used identically in Tasks 3–4. The `gen-parity-vectors` bin name matches `--bin gen-parity-vectors` and the drift guard. `paigasus-kernel-parity-rs` is used identically across moon.yml, the guard CSV, and Task 9. The ts `ParityCase`/`cases` exports in Task 7 Step 1 match their imports in Steps 2–3. ✓

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-06-18-sma-433-cross-binding-parity-harness.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
