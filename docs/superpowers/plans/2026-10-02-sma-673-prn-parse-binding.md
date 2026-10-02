# SMA-673: one `prnParse` kernel binding — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the kernel one function that parses a PRN once and returns every field, bind it to wasm and napi, and make `@paigasus/console-core`'s `prn-tenancy.ts` read a PRN with ONE kernel call instead of up to six.

**Architecture:** A hidden `paigasus_kernel::wire::prn_parse_fields(&str) -> Vec<String>` returns the fixed six-string wire form `[errorKind, service, region, org, resourceType, resourceId]`. The wasm and napi bindings export it as `prnParseFields` (one-line wrappers). `@paigasus/kernel` wraps it in ONE typed adapter, `prnParse(prn): PrnParseResult`, and does not re-export the raw function. A new parity corpus `prn_parse.json` (expected values from the typed `Prn` accessors, not from the wire function) is replayed by Rust, napi, wasm (vitest and `wasm-probe.mjs`) and Python (through its six existing accessors). `readKernelFields` in `prn-tenancy.ts` makes one `prnParse` call inside its `try`, and its `catch` logs a redacted `prn.kernel_call_failed` event.

**Tech Stack:** Rust 1.95 (edition 2024), wasm-bindgen 0.2 through wasm-pack, napi-rs 3, TypeScript, vitest, pytest, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-10-02-sma-673-prn-parse-binding-design.md` (APPROVED by Sven on 2026-10-02, with the answers to Q1-Q7 in its section 10). Executors read the spec and this plan.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-673-prn-parse-binding` on the branch `feature/sma-673-prn-parse-binding`. Start every Bash command with `cd <worktree> &&` and `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" &&`.
- Before each commit, `git -C <worktree> branch --show-current` must print `feature/sma-673-prn-parse-binding`. Never commit, check out or reset in the main checkout.
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset`, or a force push. Stage files BY NAME (`git add <path>`), never `git add -A` or `git add .`. If a commit fails with "failed to fill whole buffer" or "communication with agent failed", 1Password is locked: stop and report a blocker.
- Conventional commits with a workspace scope (`feat(rs)`, `feat(ts)`, `test(py)`, `fix(ci)`). Each commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` line or a `token: value` line in the commit body (commitlint `footer-leading-blank`).
- Every new source file opens with the SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- The wire order is FIXED: `[errorKind, service, region, org, resourceType, resourceId]`, always six strings. A valid PRN has `errorKind === ""`. An invalid PRN has the `PrnError::kind()` token and five `""` fields (D1).
- Rust names: `#[doc(hidden)] pub mod wire;` in `paigasus-kernel`, `pub fn prn_parse_fields(s: &str) -> Vec<String>`. NO `PrnFields` struct, NO `Prn::fields` method, NO new re-export at the crate root (D2, Q7).
- Binding names: wasm `#[wasm_bindgen(js_name = prnParseFields)]`, napi `#[napi(js_name = "prnParseFields")]`, signature `(s: String) -> Vec<String>`. No PyO3 binding, no `.pyi` change, no `ci/pyo3-stub/check.py` change (D3, Q1).
- `@paigasus/kernel` exports `prnParse` and the type `PrnParseResult` from BOTH `src/index.ts` and `src/wasm.ts`. It does NOT re-export `prnParseFields` (D2).
- The six existing single-field accessors stay unchanged in every binding (A7). Their deprecation is SMA-725.
- `prn-tenancy.ts`: the public surface does not change. `MAX_LEN` stays before the kernel call. IAM's tenancy rule (`TENANCY_KINDS`, `'principal'`) stays OUTSIDE the `try`. The `try` stays (D4).
- The `catch` logs `logger.appEvent('prn.kernel_call_failed', { error })`, where `error` is the thrown value's `name` when it is an `Error`, else `'unknown'`. It NEVER logs the PRN or the error message (D5, Q6).
- Committed generated glue stays in sync: the five files under `rs/crates/bindings/paigasus-wasm/` (`moon run paigasus-kernel-ts:generate-wasm`) and the napi `index.js` / `index.d.ts` (written by `napi build` in `paigasus-kernel-ts:build`). After `generate-wasm`, run `rm -rf ts/node_modules && pnpm -C ts install`, or the console `setupFiles` check 4 reds.
- No measurement file is committed (5.5). Stop rule (Q5): if the median `parseTenancyPrn` time for the team PRN does not fall by at least 30%, stop before the PR, post the numbers on Linear and report.
- Mutation runs are authorized (Sven, 2026-09-28). Each mutation must COMPILE. Restore by reverting your exact edit with the Edit tool (NOT `git checkout --`, which also drops uncommitted work), then confirm with `git diff`. Never commit a mutation. If the permission system refuses a mutation run, restore the file, write the exact manual steps under "Mutation proof pending" in the PR-notes file, and continue.
- PR-notes file: `$(git -C <worktree> rev-parse --git-dir)/sma-673-pr-notes.md`. It is outside the work tree and is never committed. Append every mutation result, every measurement and every link that the PR body needs.
- Known CI condition: `repo:deny` fails on every PR that selects it, because `yoke-derive` 0.8.3 in `rs/Cargo.lock` is yanked. Do not fix it here. Report it if it is the only red.
- New repo text (comments, docs) is written in ASD-STE100 Simplified Technical English.
- Do not name the moon CI report file in any new doc or comment (actionlint check 12 requires a `moon-diagnosis` marker for that).
- Do not install host software (no brew, no global npm). Do not leave background jobs running.

## Review Focus

1. **Arbitrary input to the wire function** (non-ASCII, control characters, 10 000 characters, only colons). Expected: always exactly six strings; on an error, five empty fields. Pinned by the proptest `wire_props.rs` in Task 1.
2. **A position swap that empty fields hide.** Most valid rows have `region === ""`, so a swap of `region` and `org` is invisible on them. Expected: a replay row where `region` and `org` are both non-empty and different. Pinned by the region-ful corpus row (Task 2) and the distinct-values adapter test (Task 4).
3. **A non-`Error` value thrown inside the `try`** (a string, `undefined`). Expected: the parser returns `null`, does not throw, and logs `error: 'unknown'`. Pinned in Task 7.
4. **The PRN or the error message reaching the log.** The PRN is attacker-controlled. Expected: the logged fields hold only `{ error: <name> }`. Pinned in Task 7 with an error message that contains the PRN.
5. **An uncommitted new vector file.** `git diff --exit-code` ignores untracked files. Expected: `repo:parity-corpus-drift` reds while `prn_parse.json` is untracked. Proved in Task 2.

---

### Task 1: Kernel `wire` module

**Files:**
- Create: `rs/crates/libs/paigasus-kernel/src/wire.rs`
- Modify: `rs/crates/libs/paigasus-kernel/src/lib.rs:3-19` (module doc and module list)
- Create: `rs/crates/libs/paigasus-kernel/tests/wire_props.rs`

**Interfaces:**
- Consumes: `paigasus_kernel::Prn::parse`, `Prn::{service, region, org, resource_type, resource_id}`, `PrnError::kind`.
- Produces: `paigasus_kernel::wire::prn_parse_fields(s: &str) -> Vec<String>` (six elements, D1 order).

- [ ] **Step 1: Write the module with its unit tests, and a stub body that fails**

Create `rs/crates/libs/paigasus-kernel/src/wire.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! FFI wire forms of kernel values, FOR THE BINDINGS UNDER `rs/crates/bindings/` ONLY (SMA-673).
//!
//! A consumer of this crate must use [`crate::Prn`]. The shapes here are a binding convention, not
//! a domain API: a binding returns them across an FFI boundary, and each language wraps them in ONE
//! typed adapter. The module is `#[doc(hidden)]`, but it is `pub` in a published crate, so a change
//! to a shape here is a breaking change for the crate.
//!
//! This module has no FFI dependency and does no I/O. It fixes one marshalling convention in one
//! place, so that the wasm and napi bindings do not each copy it.

use crate::Prn;

/// Parse `s` ONCE and return `[error_kind, service, region, org, resource_type, resource_id]`.
///
/// The function is total: it never panics and always returns six strings. A valid PRN gives
/// `error_kind == ""` and the five canonical fields (`org` is `""` when the PRN has no org; the
/// UUIDs are lower-case and hyphenated). An invalid PRN gives the `PrnError::kind()` token and five
/// empty fields.
#[must_use]
pub fn prn_parse_fields(s: &str) -> Vec<String> {
    let _ = s;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::prn_parse_fields;

    const ORG: &str = "0190a100-0000-7000-8000-0000000000aa";
    const TEAM: &str = "0190a1b2-0000-7000-8000-000000000001";

    fn row(values: [&str; 6]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn a_valid_prn_with_an_org_gives_every_field() {
        let prn = format!("prn:pgs:iam::{ORG}:team/{TEAM}");
        assert_eq!(prn_parse_fields(&prn), row(["", "iam", "", ORG, "team", TEAM]));
    }

    #[test]
    fn a_valid_prn_without_an_org_gives_an_empty_org() {
        let id = "0190a1e5-0000-7000-8000-000000000000";
        let prn = format!("prn:pgs:iam:::organization/{id}");
        assert_eq!(prn_parse_fields(&prn), row(["", "iam", "", "", "organization", id]));
    }

    #[test]
    fn a_valid_prn_with_a_region_gives_the_region_in_position_two() {
        let prn = format!("prn:pgs:iam:eu-central-1:{ORG}:team/{TEAM}");
        assert_eq!(prn_parse_fields(&prn), row(["", "iam", "eu-central-1", ORG, "team", TEAM]));
    }

    #[test]
    fn an_upper_case_uuid_comes_back_lower_case() {
        let prn = "prn:pgs:iam:::user/0190A1E5-0000-7000-8000-00000000ABCD";
        assert_eq!(prn_parse_fields(prn), row(["", "iam", "", "", "user", "0190a1e5-0000-7000-8000-00000000abcd"]));
    }

    #[test]
    fn each_error_kind_gives_its_token_and_five_empty_fields() {
        let too_long = format!("prn:pgs:iam:::user/{}", "a".repeat(600));
        let cases: [(&str, &str); 11] = [
            ("", "empty"),
            (&too_long, "too-long"),
            ("xrn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004", "bad-scheme"),
            ("prn:pgz:iam:::user/0190a1e5-0000-7000-8000-000000000004", "bad-partition"),
            ("prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004:extra", "wrong-field-count"),
            ("prn:pgs:IAM:::user/0190a1e5-0000-7000-8000-000000000004", "bad-service"),
            ("prn:pgs:iam:US-EAST:0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001", "bad-region"),
            ("prn:pgs:iam::not-a-uuid:team/0190a1b2-0000-7000-8000-000000000001", "bad-org"),
            ("prn:pgs:iam:::userwithoutslash", "bad-resource-path"),
            ("prn:pgs:iam:::/0190a1e5-0000-7000-8000-000000000004", "bad-resource-type"),
            ("prn:pgs:iam:::user/not-a-uuid", "bad-resource-id"),
        ];
        for (input, kind) in cases {
            assert_eq!(prn_parse_fields(input), row([kind, "", "", "", "", ""]), "input {input:?}");
        }
    }
}
```

In `rs/crates/libs/paigasus-kernel/src/lib.rs`, replace lines 10-19 (the "No I/O" paragraph and the module list) with:

```rust
//! No I/O, no FFI, and no adapter dependencies live here. The Python, Node and browser
//! bindings under `rs/crates/bindings/` call into this crate rather than reimplementing
//! it (ADR-0005). The hidden [`wire`] module fixes the shape of a multi-value FFI return in one
//! place for those bindings (SMA-673). It has no FFI dependency, and only the bindings use it.

pub mod cedar;
// The PRN value type lives in `resource_name`, NOT `prn`: `prn` (PRN) is a Windows reserved device
// name, so a `prn.rs` file cannot be checked out on Windows (git fails with "invalid path"). Do not
// rename this back to `prn`. The public type is still `Prn` (re-exported below).
pub mod resource_name;
pub mod uuid7;
// Binding-only wire forms (SMA-673, Sven accepted it on 2026-10-02 as Q7). Hidden from the docs,
// and NOT re-exported at the crate root: a consumer uses `Prn`.
#[doc(hidden)]
pub mod wire;
```

(Keep the existing `pub use` lines and everything below them unchanged.)

- [ ] **Step 2: Run the unit tests and confirm that they fail**

Run: `cd rs && cargo nextest run --locked -p paigasus-kernel wire::`
Expected: 5 tests FAIL (the stub returns an empty `Vec`).

- [ ] **Step 3: Write the real body**

Replace the stub body in `wire.rs` with:

```rust
pub fn prn_parse_fields(s: &str) -> Vec<String> {
    match Prn::parse(s) {
        Ok(p) => vec![
            String::new(),
            p.service().to_string(),
            p.region().to_string(),
            p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default(),
            p.resource_type().to_string(),
            p.resource_id().as_hyphenated().to_string(),
        ],
        Err(e) => vec![e.kind().to_string(), String::new(), String::new(), String::new(), String::new(), String::new()],
    }
}
```

- [ ] **Step 4: Run the unit tests and confirm that they pass**

Run: `cd rs && cargo nextest run --locked -p paigasus-kernel wire::`
Expected: 5 tests PASS.

- [ ] **Step 5: Add the property test (Review Focus 1)**

Create `rs/crates/libs/paigasus-kernel/tests/wire_props.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! The wire form of `prn_parse_fields` holds for ANY input, not only for the corpus rows (SMA-673):
//! always six strings, the error kind agrees with `Prn::parse`, and an error row has five empty
//! fields.

use paigasus_kernel::Prn;
use paigasus_kernel::wire::prn_parse_fields;
use proptest::prelude::*;

fn check(s: &str) -> Result<(), TestCaseError> {
    let wire = prn_parse_fields(s);
    prop_assert_eq!(wire.len(), 6);
    match Prn::parse(s) {
        Ok(_) => {
            prop_assert_eq!(&wire[0], "");
            // service, resource type and resource id are never empty on a valid PRN.
            prop_assert!(!wire[1].is_empty() && !wire[4].is_empty() && !wire[5].is_empty());
        }
        Err(e) => {
            prop_assert_eq!(&wire[0], e.kind());
            prop_assert!(wire[1..].iter().all(String::is_empty), "error row with a non-empty field: {:?}", wire);
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn any_string_gives_six_consistent_strings(s in "\\PC{0,700}") {
        check(&s)?;
    }

    #[test]
    fn prn_shaped_strings_give_six_consistent_strings(
        service in "[a-zA-Z0-9-]{0,6}",
        region in "[a-zA-Z0-9-]{0,6}",
        org in "[0-9a-fA-F-]{0,36}",
        rtype in "[a-z0-9/-]{0,8}",
        rid in "[0-9a-fA-F-]{0,36}",
    ) {
        check(&format!("prn:pgs:{service}:{region}:{org}:{rtype}/{rid}"))?;
    }
}

#[test]
fn a_very_long_and_a_colon_only_input_give_six_strings() {
    check(&"x".repeat(10_000)).unwrap();
    check(":::::").unwrap();
    check("prn:pgs:iam:::user/\u{0}").unwrap();
}
```

- [ ] **Step 6: Run the whole kernel crate, clippy and fmt**

Run: `cd rs && cargo nextest run --locked -p paigasus-kernel && cargo clippy --locked -p paigasus-kernel --all-targets -- -D warnings && cargo fmt --check -p paigasus-kernel`
Expected: all PASS, no warning, no fmt diff. If `cargo fmt --check` reports a diff, run `cargo fmt -p paigasus-kernel` and re-run.

- [ ] **Step 7: Commit**

```bash
git -C <worktree> branch --show-current   # must print feature/sma-673-prn-parse-binding
git add rs/crates/libs/paigasus-kernel/src/wire.rs rs/crates/libs/paigasus-kernel/src/lib.rs rs/crates/libs/paigasus-kernel/tests/wire_props.rs
git commit -m "feat(rs): add the hidden kernel wire module with prn_parse_fields (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The `prn_parse` parity corpus and the untracked-file drift check

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel-parity/src/lib.rs` (new case type, shared helper, builder, unit test, `corpus_path` doc at line 60)
- Modify: `rs/crates/libs/paigasus-kernel-parity/src/bin/gen-parity-vectors.rs:8,25`
- Modify: `rs/crates/libs/paigasus-kernel-parity/tests/replay.rs`
- Create (generated): `rs/crates/libs/paigasus-kernel-parity/vectors/prn_parse.json`
- Modify: `rs/crates/libs/paigasus-kernel-parity/README.md`
- Modify: `moon.yml:280` (`parity-corpus-drift` script)

**Interfaces:**
- Consumes: `paigasus_kernel::wire::prn_parse_fields` (Task 1), only in `tests/replay.rs`.
- Produces: `PrnParseCase { input, error_kind, service, region, org, resource_type, resource_id }` (all `String`), `build_prn_parse_corpus() -> Vec<PrnParseCase>`, and the JSON file `vectors/prn_parse.json` with those seven snake_case keys per row. Tasks 3, 4 and 5 read this file.

- [ ] **Step 1: Write the failing replay tests**

In `tests/replay.rs`, change the `use` line to:

```rust
use paigasus_kernel_parity::{
    Case, PrnCanonicalCase, PrnCedarCase, PrnFieldsCase, PrnParseCase, Uuid7Case, build_corpus, build_prn_canonical_corpus, build_prn_cedar_corpus, build_prn_fields_corpus, build_prn_parse_corpus,
    build_uuid7_corpus, load_corpus,
};
```

Append:

```rust
#[test]
fn prn_parse_corpus_present_and_fresh() {
    let committed = load_corpus::<PrnParseCase>("prn_parse");
    assert!(!committed.is_empty(), "prn_parse corpus is empty");
    assert_eq!(committed, build_prn_parse_corpus());
}

/// The Rust "binding" of the one-call parse: every row through the kernel's wire function. The
/// expected values come from the typed accessors (`build_prn_parse_corpus`), not from the function
/// under test, so a reorder in `wire::prn_parse_fields` reds here (SMA-673 L2).
#[test]
fn prn_parse_corpus_replays_through_the_wire_function() {
    let committed = load_corpus::<PrnParseCase>("prn_parse");
    assert!(!committed.is_empty(), "prn_parse corpus is empty");
    for c in &committed {
        let expected = vec![c.error_kind.clone(), c.service.clone(), c.region.clone(), c.org.clone(), c.resource_type.clone(), c.resource_id.clone()];
        assert_eq!(paigasus_kernel::wire::prn_parse_fields(&c.input), expected, "input {:?}", c.input);
    }
}

/// A marshalling defect shared by the corpus helper and the wire function is invisible to the
/// replay above. The older `prn_fields` corpus is a second oracle for the PRNs both corpora hold.
#[test]
fn prn_parse_rows_agree_with_prn_fields_rows() {
    let parse = load_corpus::<PrnParseCase>("prn_parse");
    let fields = load_corpus::<PrnFieldsCase>("prn_fields");
    assert!(!fields.is_empty(), "prn_fields corpus is empty");
    for f in &fields {
        let p = parse.iter().find(|p| p.input == f.prn).unwrap_or_else(|| panic!("prn_parse has no row for the prn_fields PRN {:?}", f.prn));
        assert_eq!(p.error_kind, "", "{:?}", f.prn);
        assert_eq!((&p.service, &p.region, &p.org, &p.resource_type, &p.resource_id), (&f.service, &f.region, &f.org, &f.resource_type, &f.resource_id), "{:?}", f.prn);
    }
}
```

- [ ] **Step 2: Run them and confirm that they fail**

Run: `cd rs && cargo nextest run --locked -p paigasus-kernel-parity`
Expected: FAIL to COMPILE (`PrnParseCase` and `build_prn_parse_corpus` do not exist). This is the red state.

- [ ] **Step 3: Add the case type, the shared helper and the builder**

In `src/lib.rs`:

1. Change the `corpus_path` doc (line 60) to: `/// Absolute path to a committed corpus by stem (`sum`, `uuid7`, `prn_canonical`, `prn_cedar`, `prn_fields`, `prn_parse`).`

2. Add, directly above `build_prn_fields_corpus` (after the `PrnFieldsCase` struct):

```rust
/// The five canonical fields of a parsed PRN, marshalled the way every binding marshals them: `org`
/// is `""` when the PRN has no org, and the UUIDs are lower-case and hyphenated. The `prn_fields`
/// and `prn_parse` corpora share it, so they cannot marshal differently. It is deliberately NOT
/// `paigasus_kernel::wire::prn_parse_fields`: the `prn_parse` corpus is the oracle for that
/// function, so it must not be computed by it (SMA-673).
struct Fields {
    service: String,
    region: String,
    org: String,
    resource_type: String,
    resource_id: String,
}

fn fields_of(p: &paigasus_kernel::Prn) -> Fields {
    Fields {
        service: p.service().to_string(),
        region: p.region().to_string(),
        org: p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default(),
        resource_type: p.resource_type().to_string(),
        resource_id: p.resource_id().as_hyphenated().to_string(),
    }
}
```

3. Change the body of `build_prn_fields_corpus`'s `.map` closure to use the helper (the output must not change; the drift gate proves it):

```rust
        .map(|s| {
            let p = paigasus_kernel::Prn::parse(s).expect("prn_fields corpus PRN parses");
            let f = fields_of(&p);
            PrnFieldsCase {
                prn: (*s).to_string(),
                service: f.service,
                region: f.region,
                org: f.org,
                resource_type: f.resource_type,
                resource_id: f.resource_id,
            }
        })
```

4. Add, after `build_prn_fields_corpus`:

```rust
/// One case of the one-call PRN parse (SMA-673): the input and the six values of the binding wire
/// form `[error_kind, service, region, org, resource_type, resource_id]`. A valid input has
/// `error_kind == ""`. An invalid input has the kind token and five empty fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrnParseCase {
    pub input: String,
    pub error_kind: String,
    pub service: String,
    pub region: String,
    pub org: String,
    pub resource_type: String,
    pub resource_id: String,
}

/// A valid PRN WITH a region. Neither older corpus holds one, and the console branches on region.
/// Its region and org are both non-empty and different, so a swap of those two positions is
/// visible (SMA-673 M2).
const REGIONFUL_PRN: &str = "prn:pgs:iam:eu-central-1:0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001";

/// Deterministic one-call parse corpus: the `prn_canonical` inputs (valid rows, an upper-case UUID
/// row and one row per `PrnError` kind), then the `prn_fields` PRNs, then [`REGIONFUL_PRN`], in that
/// order, with duplicates removed (first occurrence wins).
#[must_use]
pub fn build_prn_parse_corpus() -> Vec<PrnParseCase> {
    let candidates = build_prn_canonical_corpus()
        .into_iter()
        .map(|c| c.input)
        .chain(build_prn_fields_corpus().into_iter().map(|c| c.prn))
        .chain(std::iter::once(REGIONFUL_PRN.to_string()));
    let mut inputs: Vec<String> = Vec::new();
    for input in candidates {
        if !inputs.contains(&input) {
            inputs.push(input);
        }
    }
    inputs
        .into_iter()
        .map(|input| match paigasus_kernel::Prn::parse(&input) {
            Ok(p) => {
                let f = fields_of(&p);
                PrnParseCase {
                    input,
                    error_kind: String::new(),
                    service: f.service,
                    region: f.region,
                    org: f.org,
                    resource_type: f.resource_type,
                    resource_id: f.resource_id,
                }
            }
            Err(e) => PrnParseCase {
                input,
                error_kind: e.kind().to_string(),
                service: String::new(),
                region: String::new(),
                org: String::new(),
                resource_type: String::new(),
                resource_id: String::new(),
            },
        })
        .collect()
}
```

5. Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn prn_parse_corpus_covers_every_branch_the_console_takes() {
        let cases = build_prn_parse_corpus();
        let mut kinds: Vec<&str> = cases.iter().map(|c| c.error_kind.as_str()).filter(|k| !k.is_empty()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        // One row per PrnError kind (there are 11).
        assert_eq!(kinds.len(), 11, "error kinds in the corpus: {kinds:?}");
        assert!(cases.iter().any(|c| c.error_kind.is_empty() && c.org.is_empty()), "no valid row without an org");
        assert!(cases.iter().any(|c| c.error_kind.is_empty() && !c.org.is_empty()), "no valid row with an org");
        assert!(cases.iter().any(|c| c.error_kind.is_empty() && !c.region.is_empty() && !c.org.is_empty() && c.region != c.org), "no valid row with a region and an org");
        assert!(cases.iter().any(|c| c.error_kind.is_empty() && c.service != "iam"), "no valid row of another service");
        let mut inputs: Vec<&str> = cases.iter().map(|c| c.input.as_str()).collect();
        inputs.sort_unstable();
        let before = inputs.len();
        inputs.dedup();
        assert_eq!(inputs.len(), before, "duplicate input rows");
    }
```

- [ ] **Step 4: Write the generator line and generate the file**

In `src/bin/gen-parity-vectors.rs`, add `build_prn_parse_corpus` to the `use` list (line 8), and add after line 25:

```rust
    write("prn_parse", &serialize(&build_prn_parse_corpus()))?;
```

Run: `cd rs && cargo run --locked -p paigasus-kernel-parity --bin gen-parity-vectors`
Expected: `wrote …/vectors/prn_parse.json` and the five other lines. Then `git status --short rs/crates/libs/paigasus-kernel-parity/vectors/` must show ONLY `?? …/prn_parse.json` (the five older files are byte-identical; this proves that the `fields_of` refactor changed no output).

- [ ] **Step 5: Run the crate tests and confirm that they pass**

Run: `cd rs && cargo nextest run --locked -p paigasus-kernel-parity && cargo clippy --locked -p paigasus-kernel-parity --all-targets -- -D warnings && cargo fmt --check -p paigasus-kernel-parity`
Expected: all PASS (9 replay tests, the lib unit tests).

- [ ] **Step 6: Make `repo:parity-corpus-drift` red on an untracked vector file**

In `moon.yml` line 280, replace the `script:` line of `parity-corpus-drift` with:

```yaml
    # `git diff --exit-code` ignores UNTRACKED files, so a new corpus that was generated but never
    # added passed this gate (SMA-673 4.4). The `git ls-files --others` check closes that gap.
    script: '( cd rs && cargo run --locked -p paigasus-kernel-parity --bin gen-parity-vectors ) && git diff --exit-code rs/crates/libs/paigasus-kernel-parity/vectors/ && test -z "$(git ls-files --others --exclude-standard -- rs/crates/libs/paigasus-kernel-parity/vectors/)"'
```

Deviation from the spec text, on purpose: the spec writes `git status --porcelain`, and also says the gate is green "after `git add`". `git status --porcelain` lists a STAGED file too (`A  …/prn_parse.json`), so with it the gate stays red after `git add` until the commit. `git ls-files --others --exclude-standard` lists untracked files only, which is the gap the spec names, and it makes the spec's own proof (red untracked, green after `git add`) true. Record this deviation in the PR-notes file.

- [ ] **Step 7: Prove the drift check (Review Focus 5)**

With `prn_parse.json` still untracked:
Run: `moon run repo:parity-corpus-drift --force; echo "rc=$?"`
Expected: rc != 0 (the `test -z` fails).

Then: `git add rs/crates/libs/paigasus-kernel-parity/vectors/prn_parse.json && moon run repo:parity-corpus-drift --force; echo "rc=$?"`
Expected: rc = 0.

Append both results (command, rc) to the PR-notes file under "Drift gate proof".

- [ ] **Step 8: Update the parity README**

In `rs/crates/libs/paigasus-kernel-parity/README.md`, replace the first paragraph after the title line (lines 5-7) with:

```markdown
The committed, kernel-derived corpora live in `vectors/`. Every binding (Python/PyO3, Node/napi,
browser/wasm) and the Rust impl replay each of them, and the kernel is the single oracle:

- `sum.json`: `{a, b, expected}` over the i32-safe parity domain.
- `uuid7.json`: UUIDv7 minting from injected time and random bytes.
- `prn_canonical.json`: PRN parse and canonical form, with one row per error kind.
- `prn_cedar.json`: PRN to Cedar entity type and id.
- `prn_fields.json`: the five PRN field accessors and the `prn_build` round trip.
- `prn_parse.json`: the one-call parse wire form `[error_kind, service, region, org,
  resource_type, resource_id]` (SMA-673). Python has no one-call binding, so it replays this file
  through its six single-field accessors.
```

and in the "Drift guard" bullet, after "runs `git diff --exit-code`", add: ", then fails on any untracked or uncommitted file under `vectors/`".

- [ ] **Step 9: Commit (two commits)**

```bash
git -C <worktree> branch --show-current   # must print feature/sma-673-prn-parse-binding
git add rs/crates/libs/paigasus-kernel-parity/src/lib.rs rs/crates/libs/paigasus-kernel-parity/src/bin/gen-parity-vectors.rs rs/crates/libs/paigasus-kernel-parity/tests/replay.rs rs/crates/libs/paigasus-kernel-parity/vectors/prn_parse.json rs/crates/libs/paigasus-kernel-parity/README.md
git commit -m "feat(rs): add the prn_parse parity corpus and its Rust replay (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add moon.yml
git commit -m "fix(ci): fail repo:parity-corpus-drift on an uncommitted vector file (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The wasm and napi bindings, their committed glue, and the wasm drift gate

**Files:**
- Modify: `rs/crates/bindings/paigasus-wasm/src/lib.rs` (after `prn_resource_id`, line 83)
- Modify: `rs/crates/bindings/paigasus-node-bindings/src/lib.rs` (after `prn_resource_id`, line 86)
- Regenerate and commit: `rs/crates/bindings/paigasus-wasm/{paigasus_wasm_bg.wasm, paigasus_wasm_bg.js, paigasus_wasm.js, paigasus_wasm.d.ts, paigasus_wasm_bg.wasm.d.ts}`
- Regenerate and commit: `rs/crates/bindings/paigasus-node-bindings/index.js`, `index.d.ts`
- Modify: `ts/packages/paigasus-kernel/src/binding-parity.types.ts`
- Modify: `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts:9,32,34-56,119-120,131`
- Modify: `ts/packages/paigasus-kernel/tests/wasm-probe.mjs:10,77-99`
- Modify: `ts/CLAUDE.md:127`

**Interfaces:**
- Consumes: `paigasus_kernel::wire::prn_parse_fields` (Task 1); `vectors/prn_parse.json` (Task 2).
- Produces: wasm export `prnParseFields(s: string): string[]` and napi export `prnParseFields(s: string): Array<string>`, in `@paigasus/wasm` and `@paigasus/node-bindings`.

- [ ] **Step 1: Write the failing guards first**

1. In `src/binding-parity.types.ts`, add after line 30:

```ts
const _prnParseFields: Exact<NapiApi['prnParseFields'], WasmApi['prnParseFields']> = true;
```

and after line 43: `void _prnParseFields;`

2. In `tests/wasm-probe.mjs`: change line 10 to `//   node tests/wasm-probe.mjs --corpus <dir>       replay all six parity corpora through <dir>`. Insert before line 87 (`if (failures.length > 0)`):

```js
  // SMA-673: the one-call parse. Compared as JSON, because the binding returns an array.
  const parse = vectors('prn_parse');
  for (const row of parse) {
    const expected = [row.error_kind, row.service, row.region, row.org, row.resource_type, row.resource_id];
    same(`prnParseFields(${row.input})`, JSON.stringify(api.prnParseFields(row.input)), JSON.stringify(expected));
  }
```

and add `prn_parse: parse.length,` to the `checked` object after `prn_fields: fields.length,`.

3. In `tests/committed-wasm.test.ts`:
   - line 9: `//   3. the committed glue and binary instantiate together and replay all six parity corpora.`
   - line 131: `expect(Object.keys(result.checked).sort()).toEqual(['prn_canonical', 'prn_cedar', 'prn_fields', 'prn_parse', 'sum', 'uuid7']);`
   - line 119: make the message count derive from the list, so it cannot go stale again: `` `the ${label} binary does not export the kernel's ${EXPECTED_EXPORTS.length} names. ${SURFACE_CHANGED}` ``
   - line 120: `` `the ${label} binary does not import the glue's ${EXPECTED_IMPORTS.length} callbacks. ${SURFACE_CHANGED}` ``
   - Leave `EXPECTED_EXPORTS`, `EXPECTED_IMPORTS` and the "Twelve" comment unchanged for now. Step 5 writes the MEASURED values.

- [ ] **Step 2: Confirm the red state**

Run: `cd ts/packages/paigasus-kernel && pnpm exec tsc -p tsconfig.json --noEmit`
Expected: FAIL — `Property 'prnParseFields' does not exist` on both binding types.

- [ ] **Step 3: Add the two one-line wrappers**

In `rs/crates/bindings/paigasus-wasm/src/lib.rs`, after `prn_resource_id` (line 83):

```rust
/// Parse `s` ONCE and return `[errorKind, service, region, org, resourceType, resourceId]`: always
/// six strings, never a throw. A valid PRN has `errorKind === ""`; an invalid one has the
/// `kind()` token and five empty fields. Use `prnParse` from `@paigasus/kernel`, not this raw form.
#[wasm_bindgen(js_name = prnParseFields)]
pub fn prn_parse_fields(s: String) -> Vec<String> {
    paigasus_kernel::wire::prn_parse_fields(&s)
}
```

In `rs/crates/bindings/paigasus-node-bindings/src/lib.rs`, after `prn_resource_id` (line 86):

```rust
/// Parse `s` ONCE and return `[errorKind, service, region, org, resourceType, resourceId]`: always
/// six strings, never a throw. A valid PRN has `errorKind === ""`; an invalid one has the
/// `kind()` token and five empty fields. Use `prnParse` from `@paigasus/kernel`, not this raw form.
#[napi(js_name = "prnParseFields")]
pub fn prn_parse_fields(s: String) -> Vec<String> {
    paigasus_kernel::wire::prn_parse_fields(&s)
}
```

- [ ] **Step 4: Regenerate the committed glue and reinstall**

Run, in order:

```bash
moon run paigasus-kernel-ts:generate-wasm
moon run paigasus-kernel-ts:build --force
rm -rf ts/node_modules && pnpm -C ts install
git status --short rs/crates/bindings/
```

Expected: `generate-wasm` and `build` succeed. `git status` shows the five `paigasus-wasm/paigasus_wasm*` files and `paigasus-node-bindings/index.js`, `index.d.ts` as modified. Check: `grep -n prnParseFields rs/crates/bindings/paigasus-node-bindings/index.d.ts rs/crates/bindings/paigasus-node-bindings/index.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts` shows `prnParseFields(s: string): Array<string>` in the napi `index.d.ts`, `module.exports.prnParseFields` in `index.js`, and `prnParseFields(s: string): string[]` in `paigasus_wasm.d.ts`.

Then run `moon run paigasus-kernel-ts:build --force` a SECOND time and confirm that `git diff --stat rs/crates/bindings/paigasus-node-bindings/` does not change between the two runs (the napi glue is stable, so the committed copy is the one CI regenerates).

- [ ] **Step 5: Measure the wasm interface and write it into the drift gate**

Run: `node ts/packages/paigasus-kernel/tests/wasm-probe.mjs --interfaces rs/crates/bindings/paigasus-wasm`
Write the MEASURED export list into `EXPECTED_EXPORTS` in `tests/committed-wasm.test.ts`, sorted exactly as printed. It must contain `'prnParseFields:function'`. In externref mode a `Vec<String>` return can add an export such as `__externref_drop_slice:function`; write what the probe prints, do not guess. Write the MEASURED import list into `EXPECTED_IMPORTS`: a name with a 16-hex hash suffix gets a regex of the existing form (`/^\.\/paigasus_wasm_bg\.js\.__wbg_<Name>_[0-9a-f]{16}:function$/`); every other name is a literal regex. Update the comment at line 32: replace "Twelve" with the measured count of kernel exports (expected: Thirteen), and the import sentence with the measured import count and a short note for each new import. Record both measured lists in the PR-notes file.

- [ ] **Step 6: Update `ts/CLAUDE.md`**

At line 127, change "the committed pair replays all five parity corpora" to "the committed pair replays all six parity corpora". Do NOT change "commit all five" (line 125) or "the five files" (line 133): those count the wasm ARTIFACTS, which stay five.

- [ ] **Step 7: Run the kernel package tests and the typecheck**

Run: `moon run paigasus-kernel-ts:test --force && moon run paigasus-kernel-ts:typecheck --force`
(If the project has no separate `typecheck` task, `pnpm -C ts/packages/paigasus-kernel exec tsc -p tsconfig.json --noEmit`.)
Expected: PASS, including `committed-wasm.test.ts` checks 1, 2 and 3 (check 3 now lists `prn_parse`). Also run `moon run paigasus-console-core-ts:test --force` and confirm that the `setupFiles` check 4 passes (the reinstall in Step 4 makes it pass).

- [ ] **Step 8: Run the Rust gates for the binding crates**

Run: `cd rs && cargo clippy --locked -p paigasus-wasm -p paigasus-node-bindings -- -D warnings && cargo fmt --check -p paigasus-wasm -p paigasus-node-bindings` and `moon run repo:wasm-getrandom-free --force`
Expected: PASS. No new dependency was added.

- [ ] **Step 9: Commit**

```bash
git -C <worktree> branch --show-current   # must print feature/sma-673-prn-parse-binding
git add rs/crates/bindings/paigasus-wasm/src/lib.rs rs/crates/bindings/paigasus-node-bindings/src/lib.rs \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts \
  rs/crates/bindings/paigasus-node-bindings/index.js rs/crates/bindings/paigasus-node-bindings/index.d.ts \
  ts/packages/paigasus-kernel/src/binding-parity.types.ts ts/packages/paigasus-kernel/tests/committed-wasm.test.ts ts/packages/paigasus-kernel/tests/wasm-probe.mjs ts/CLAUDE.md
git commit -m "feat(rs): bind prnParseFields to wasm and napi (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Note for the PR body (append to the PR-notes file): `images.yml`'s `pull_request` path filter lists `paigasus-node-bindings/index.js` and `index.d.ts` (lines 55-56 on `e5f31a56`), so this PR starts the image workflow. The spec's section 11 says otherwise; that rejection is wrong on current `main`. The workflow is not a required check.

---

### Task 4: The `prnParse` adapter in `@paigasus/kernel` and the napi/wasm replays

**Files:**
- Create: `ts/packages/paigasus-kernel/src/prn-parse.ts`
- Modify: `ts/packages/paigasus-kernel/src/index.ts`, `ts/packages/paigasus-kernel/src/wasm.ts`
- Modify: `ts/packages/paigasus-kernel/tests/corpus.ts`
- Create: `ts/packages/paigasus-kernel/tests/prn-parse-adapter.test.ts`, `tests/prn-parse.test.ts`, `tests/prn-parse.wasm.test.ts`
- Modify: `ts/packages/paigasus-kernel/vitest.config.ts:31,58`

**Interfaces:**
- Consumes: `prnParseFields` from `@paigasus/node-bindings` and `@paigasus/wasm` (Task 3); `vectors/prn_parse.json` (Task 2).
- Produces (for Task 7): from BOTH `@paigasus/kernel` (`src/wasm.ts`) and `@paigasus/kernel/napi` (`src/index.ts`):

```ts
export type PrnParseResult =
  | { ok: true; service: string; region: string; org: string; resourceType: string; resourceId: string }
  | { ok: false; errorKind: string };
export function prnParse(prn: string): PrnParseResult;
```

- [ ] **Step 1: Write the adapter unit test**

Create `tests/prn-parse-adapter.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The typed adapter over the six-string wire form (SMA-673 D2). It checks the wire CONVENTION only
// (six strings; an error row has five empty fields), never PRN grammar: the corpus replays prove
// the grammar.
import { describe, expect, it } from 'vitest';
import { toPrnParseResult } from '../src/prn-parse';

describe('toPrnParseResult', () => {
  it('maps each position of a valid row to its own name', () => {
    // Every position holds a distinct value, so a swap of any two names is visible.
    expect(toPrnParseResult(['', 'svc', 'reg', 'org', 'type', 'id'])).toEqual({ ok: true, service: 'svc', region: 'reg', org: 'org', resourceType: 'type', resourceId: 'id' });
  });

  it('maps an error row to its error kind only', () => {
    expect(toPrnParseResult(['bad-org', '', '', '', '', ''])).toEqual({ ok: false, errorKind: 'bad-org' });
  });

  it.each([[[]], [['', 'a', 'b', 'c', 'd']], [['', 'a', 'b', 'c', 'd', 'e', 'f']]])('throws a TypeError for a wire array of the wrong length (%j)', (wire) => {
    expect(() => toPrnParseResult(wire)).toThrow(TypeError);
  });

  it('throws a TypeError for a non-string element', () => {
    expect(() => toPrnParseResult(['', 'svc', 'reg', 5, 'type', 'id'])).toThrow(TypeError);
  });

  it.each([1, 2, 3, 4, 5])('throws a TypeError for an error row with a non-empty field at position %i', (position) => {
    const wire = ['bad-org', '', '', '', '', ''];
    wire[position] = 'x';
    expect(() => toPrnParseResult(wire)).toThrow(TypeError);
  });

  it('does not put a field value into the error message', () => {
    expect(() => toPrnParseResult(['bad-org', 'secret-value', '', '', '', ''])).toThrow(/^(?!.*secret-value)/s);
  });
});
```

In `vitest.config.ts` line 31, add `'tests/prn-parse-adapter.test.ts', 'tests/prn-parse.test.ts'` to the `node` `include` list. At line 58, add `'tests/prn-parse.wasm.test.ts'` to the `browser` `include` list.

- [ ] **Step 2: Run it and confirm that it fails**

Run: `cd ts/packages/paigasus-kernel && pnpm exec vitest run --project node tests/prn-parse-adapter.test.ts`
Expected: FAIL — cannot resolve `../src/prn-parse`.

- [ ] **Step 3: Write the adapter**

Create `src/prn-parse.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ONE typed view of the kernel's one-call PRN parse (SMA-673 D1, D2). Both bindings return the
// positional wire form `[errorKind, service, region, org, resourceType, resourceId]`, so that the
// wasm and napi signatures are identical (`string[]`). The position map lives HERE and nowhere
// else: src/index.ts and src/wasm.ts export `prnParse` and do NOT re-export the raw
// `prnParseFields`, so no consumer can index a position.
//
// The two TypeErrors below are glue or binding defects, not bad input. They check the wire
// convention only. PRN grammar (for example a non-empty service on success) is the kernel's, and
// the corpus replays prove it.

export type PrnParseResult = { ok: true; service: string; region: string; org: string; resourceType: string; resourceId: string } | { ok: false; errorKind: string };

type Wire = readonly [string, string, string, string, string, string];

function isWire(wire: readonly unknown[]): wire is Wire {
  return wire.length === 6 && wire.every((element) => typeof element === 'string');
}

/** Map the six-string wire form to a `PrnParseResult`. Throws a `TypeError` on a broken wire form. */
export function toPrnParseResult(wire: readonly unknown[]): PrnParseResult {
  if (!isWire(wire)) {
    // The message names the shape only, never a value: a value can hold part of an attacker's PRN.
    throw new TypeError(`prnParseFields returned ${wire.length} elements or a non-string element; the wire form is six strings`);
  }
  const [errorKind, service, region, org, resourceType, resourceId] = wire;
  if (errorKind !== '') {
    if (service !== '' || region !== '' || org !== '' || resourceType !== '' || resourceId !== '') {
      throw new TypeError('prnParseFields returned an error kind and a non-empty field');
    }
    return { ok: false, errorKind };
  }
  return { ok: true, service, region, org, resourceType, resourceId };
}
```

(The parameter is `readonly unknown[]`, not the spec's `readonly string[]`: a `string[]` argument still type-checks, and the runtime `typeof` check then is not an "unnecessary condition" to the typed ESLint rules.)

- [ ] **Step 4: Run the adapter test and confirm that it passes**

Run: `cd ts/packages/paigasus-kernel && pnpm exec vitest run --project node tests/prn-parse-adapter.test.ts`
Expected: PASS (11 tests).

- [ ] **Step 5: Write the failing corpus replays**

In `tests/corpus.ts`, add after `PrnFieldsCase`:

```ts
export interface PrnParseCase {
  input: string;
  error_kind: string;
  service: string;
  region: string;
  org: string;
  resource_type: string;
  resource_id: string;
}
```

and after line 44: `export const prnParseCases = load<PrnParseCase>('prn_parse');`

Create `tests/prn-parse.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { prnParseFields } from '@paigasus/node-bindings';
import { prnParse } from '@paigasus/kernel/napi';
import type { PrnParseResult } from '../src/prn-parse';
import { prnParseCases, type PrnParseCase } from './corpus';

function expected(c: PrnParseCase): PrnParseResult {
  return c.error_kind === '' ? { ok: true, service: c.service, region: c.region, org: c.org, resourceType: c.resource_type, resourceId: c.resource_id } : { ok: false, errorKind: c.error_kind };
}

describe('kernel one-call PRN parse parity (napi)', () => {
  it('corpus is present and holds valid, invalid and region-ful rows', () => {
    expect(prnParseCases.length).toBeGreaterThan(0);
    expect(prnParseCases.some((c) => c.error_kind === '')).toBe(true);
    expect(prnParseCases.some((c) => c.error_kind !== '')).toBe(true);
    expect(prnParseCases.some((c) => c.error_kind === '' && c.region !== '' && c.org !== '')).toBe(true);
  });

  it.each(prnParseCases)('prn-parse($input)', (c) => {
    expect(prnParseFields(c.input)).toEqual([c.error_kind, c.service, c.region, c.org, c.resource_type, c.resource_id]);
    expect(prnParse(c.input)).toEqual(expected(c));
  });
});
```

Create `tests/prn-parse.wasm.test.ts` with the same body, except: the two imports are `import { prnParseFields } from '@paigasus/wasm';` and `import { prnParse } from '@paigasus/kernel';`, and the `describe` name ends in `(wasm)`.

Run: `cd ts/packages/paigasus-kernel && pnpm exec vitest run tests/prn-parse.test.ts tests/prn-parse.wasm.test.ts`
Expected: FAIL — `prnParse` is not exported by `@paigasus/kernel/napi` / `@paigasus/kernel`.

- [ ] **Step 6: Export `prnParse` from both entries**

`src/index.ts` becomes:

```ts
// SPDX-License-Identifier: Apache-2.0
import { sum, prnCanonicalize, prnErrorKind, prnBuild, prnService, prnRegion, prnOrg, prnResourceType, prnResourceId, prnParseFields, mintUuid7, prnCedarEntityType, prnCedarEntityId } from '@paigasus/node-bindings';
import { randHex10 } from './mint-util';
import { toPrnParseResult, type PrnParseResult } from './prn-parse';

// `prnParseFields` is imported, NOT re-exported: `prnParse` below is its only public form (SMA-673 D2).
export { sum, prnCanonicalize, prnErrorKind, prnBuild, prnService, prnRegion, prnOrg, prnResourceType, prnResourceId, mintUuid7, prnCedarEntityType, prnCedarEntityId };
export type { PrnParseResult };

/** Parse a PRN with ONE kernel call. Never throws for any input; a `TypeError` means a glue defect. */
export function prnParse(prn: string): PrnParseResult {
  return toPrnParseResult(prnParseFields(prn));
}

/** Mint a UUIDv7 from the ambient clock + CSPRNG (the injected FFI mint is pure). */
export function mint(): string {
  return mintUuid7(Date.now(), randHex10());
}
```

`src/wasm.ts` is the same, with `from '@paigasus/wasm'` in the first import.

- [ ] **Step 7: Run the package tests and confirm that they pass**

Run: `moon run paigasus-kernel-ts:test --force` and `pnpm -C ts/packages/paigasus-kernel exec tsc -p tsconfig.json --noEmit`
Expected: PASS. The vitest output lists `prn-parse.test.ts`, `prn-parse.wasm.test.ts` and `prn-parse-adapter.test.ts` with their row counts.

- [ ] **Step 8: Mutations M5 and M6 (the new files run)**

M5: in `src/index.ts` ONLY, change `prnParseFields(prn)` to `prnParseFields(prn.toUpperCase())`. Run `moon run paigasus-kernel-ts:test --force`. Expected: `prn-parse.test.ts` FAILS (napi), `prn-parse.wasm.test.ts` stays green. Restore with the Edit tool; `git diff src/index.ts` shows no mutation.

M6: in `src/prn-parse.ts`, swap `resourceType` and `resourceId` in the success `return` (`resourceType: resourceId, resourceId: resourceType`). Run `pnpm -C ts/packages/paigasus-kernel exec vitest run --project node tests/prn-parse-adapter.test.ts`. Expected: FAIL. Restore with the Edit tool and confirm with `git diff`.

Append both results to the PR-notes file under "Mutation proof".

- [ ] **Step 9: Lint, format and commit**

Run: `moon run paigasus-kernel-ts:lint --force` and `pnpm -C ts exec prettier --check packages/paigasus-kernel/src packages/paigasus-kernel/tests packages/paigasus-kernel/vitest.config.ts` (fix with `--write` if needed).

```bash
git -C <worktree> branch --show-current   # must print feature/sma-673-prn-parse-binding
git add ts/packages/paigasus-kernel/src/prn-parse.ts ts/packages/paigasus-kernel/src/index.ts ts/packages/paigasus-kernel/src/wasm.ts ts/packages/paigasus-kernel/tests/corpus.ts ts/packages/paigasus-kernel/tests/prn-parse-adapter.test.ts ts/packages/paigasus-kernel/tests/prn-parse.test.ts ts/packages/paigasus-kernel/tests/prn-parse.wasm.test.ts ts/packages/paigasus-kernel/vitest.config.ts
git commit -m "feat(ts): add the typed prnParse adapter to @paigasus/kernel (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The Python replay of `prn_parse.json`

**Files:**
- Modify: `py/packages/paigasus-kernel/tests/test_parity.py`

**Interfaces:**
- Consumes: `vectors/prn_parse.json` (Task 2); the existing exports `prn_error_kind`, `prn_service`, `prn_region`, `prn_org`, `prn_resource_type`, `prn_resource_id` of `paigasus_kernel`.
- Produces: nothing new. No binding, no stub change (D3).

- [ ] **Step 1: Add the replay**

After `class PrnFieldsCase` add:

```python
class PrnParseCase(TypedDict):
    input: str
    error_kind: str
    service: str
    region: str
    org: str
    resource_type: str
    resource_id: str
```

After line 73 add: `PRN_PARSE_CASES = cast("list[PrnParseCase]", _read("prn_parse"))`

In `test_corpora_present_and_non_empty`, add `("prn_parse", len(PRN_PARSE_CASES)),` to the list.

Append:

```python
@pytest.mark.parametrize("case", PRN_PARSE_CASES, ids=[c["input"][:80] or "<empty>" for c in PRN_PARSE_CASES])
def test_prn_parse_matches_corpus(case: PrnParseCase) -> None:
    # Python has NO one-call binding (SMA-673 D3, Q1). It replays the one-call corpus through its six
    # single-field accessors, so the Python view of every row agrees with the wire form.
    assert prn_error_kind(case["input"]) == case["error_kind"]
    if case["error_kind"] == "":
        assert prn_service(case["input"]) == case["service"]
        assert prn_region(case["input"]) == case["region"]
        assert prn_org(case["input"]) == case["org"]
        assert prn_resource_type(case["input"]) == case["resource_type"]
        assert prn_resource_id(case["input"]) == case["resource_id"]
    else:
        assert (case["service"], case["region"], case["org"], case["resource_type"], case["resource_id"]) == ("", "", "", "", "")
```

- [ ] **Step 2: Run it**

Run: `moon run paigasus-kernel-py:test --force` and `cd py && uv run ruff format --check packages/paigasus-kernel/tests/test_parity.py && uv run ruff check packages/paigasus-kernel/tests/test_parity.py`
Expected: PASS; the new parametrized test runs one case per corpus row. `moon run repo:pyo3-stub-drift --force` stays green with no stub change (confirms D3).

- [ ] **Step 3: Mutations M2 and M4**

M2: in `rs/crates/libs/paigasus-kernel/src/wire.rs`, swap the `region` and `org` lines of the `Ok` arm. Run `cd rs && cargo nextest run --locked -p paigasus-kernel-parity --no-fail-fast`, then `moon run paigasus-kernel-ts:test --force` (this rebuilds napi and the test wasm from source). Expected: the Rust replay, `prn-parse.test.ts`, `prn-parse.wasm.test.ts` FAIL. (`committed-wasm` check 3 replays the COMMITTED binary, which is not rebuilt, so it may stay green; that is expected.) Python stays green, because it does not call the wire function. Restore with the Edit tool, `git diff` clean, then `moon run paigasus-kernel-ts:test --force` green again.

M4: move `vectors/prn_parse.json` to the scratchpad (`mv`, not `git rm`). Run `moon run paigasus-kernel-ts:test --force` and `moon run paigasus-kernel-py:test --force`. Expected: the vitest corpus load, `wasm-probe.mjs` (`committed-wasm` check 3) and pytest collection FAIL. Move the file back, and confirm `git status` shows no change to it.

Append both results to the PR-notes file.

- [ ] **Step 4: Commit**

```bash
git -C <worktree> branch --show-current   # must print feature/sma-673-prn-parse-binding
git add py/packages/paigasus-kernel/tests/test_parity.py
git commit -m "test(py): replay the prn_parse corpus through the six accessors (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Baseline measurement on the OLD `prn-tenancy.ts` (no commit)

This task MUST run before Task 7. It measures the six-accessor code that is still on the branch.

**Files:**
- Create, run, then DELETE (never commit): `ts/packages/paigasus-console-core/tests/unit/sma673-bench.local.test.ts`
- Create in the scratchpad (never in the repo): `<scratchpad>/sma673-binding-bench.mjs`

**Interfaces:**
- Consumes: `parseTenancyPrn`, `ROOT_PRN` from `src/prn-tenancy.ts`; the committed wasm glue (Task 3).
- Produces: the "before" numbers in the PR-notes file (Task 8 reads them).

- [ ] **Step 1: Write the console-core bench and call-count file**

Create `ts/packages/paigasus-console-core/tests/unit/sma673-bench.local.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
// SMA-673 § 5.5 one-off measurement. NEVER COMMIT THIS FILE.
import { describe, it, vi } from 'vitest';
import * as kernel from '@paigasus/kernel';
import { ROOT_PRN, parseTenancyPrn } from '../../src/prn-tenancy';

vi.mock('@paigasus/kernel', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@paigasus/kernel')>();
  return Object.fromEntries(Object.entries(actual).map(([name, value]) => [name, typeof value === 'function' ? vi.fn(value as (...args: never[]) => unknown) : value]));
});

const INPUTS: Record<string, string> = {
  team: 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001',
  root: ROOT_PRN,
  gateway: 'prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:api-key/0190a1f6-0000-7000-8000-000000000005',
};
const PARSERS = ['prnParse', 'prnErrorKind', 'prnService', 'prnRegion', 'prnOrg', 'prnResourceType', 'prnResourceId'];

describe('SMA-673 measurement', () => {
  it('counts kernel calls per parse', () => {
    for (const [name, prn] of Object.entries(INPUTS)) {
      vi.clearAllMocks();
      parseTenancyPrn(prn);
      const calls = PARSERS.filter((fn) => fn in kernel).reduce((total, fn) => total + vi.mocked((kernel as Record<string, unknown>)[fn] as () => unknown).mock.calls.length, 0);
      console.log(`SMA673-CALLS ${name} ${calls}`);
    }
  });

  it('times parseTenancyPrn', () => {
    const N = 100_000;
    for (const [name, prn] of Object.entries(INPUTS)) {
      for (let i = 0; i < 1_000; i++) parseTenancyPrn(prn);
      const runs: number[] = [];
      for (let run = 0; run < 5; run++) {
        const start = performance.now();
        for (let i = 0; i < N; i++) parseTenancyPrn(prn);
        runs.push(performance.now() - start);
      }
      runs.sort((a, b) => a - b);
      console.log(`SMA673-TIME ${name} median_ms=${runs[2]?.toFixed(1)} runs_ms=${runs.map((t) => t.toFixed(1)).join(',')}`);
    }
  }, 900_000);
});
```

Note: the `vi.fn(value)` wrappers stay in place during timing, so both the before and the after run pay the same wrapper cost per kernel call. Say so in the report. If the wrapper cost hides the difference, ALSO time a copy of the file without the `vi.mock` block (the second `it` only) and report both.

- [ ] **Step 2: Run it five times and record the medians**

Run (five times): `cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/sma673-bench.local.test.ts 2>&1 | grep SMA673`
Expected call counts: team 6, root 6, gateway 2. Record every `SMA673-CALLS` and `SMA673-TIME` line, the host (the development Mac, `uname -m`, `node --version`) and the median of the five per-run medians for each input in the PR-notes file under "Before".

- [ ] **Step 3: Write and run the raw binding bench**

Create `<scratchpad>/sma673-binding-bench.mjs` (absolute worktree path in `DIR`):

```js
// SMA-673 § 5.5: six accessors against one prnParseFields call, on the committed wasm glue.
import { pathToFileURL } from 'node:url';
const DIR = pathToFileURL('/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-673-prn-parse-binding/rs/crates/bindings/paigasus-wasm/');
const api = await import(new URL('paigasus_wasm.js', DIR).href);
const INPUTS = {
  team: 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001',
  root: 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000',
  gateway: 'prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:api-key/0190a1f6-0000-7000-8000-000000000005',
};
const six = (p) => { api.prnErrorKind(p); api.prnService(p); api.prnRegion(p); api.prnOrg(p); api.prnResourceType(p); api.prnResourceId(p); };
const one = (p) => api.prnParseFields(p);
const N = 100_000;
function median(fn, p) {
  for (let i = 0; i < 1_000; i++) fn(p);
  const runs = [];
  for (let r = 0; r < 5; r++) { const t = performance.now(); for (let i = 0; i < N; i++) fn(p); runs.push(performance.now() - t); }
  return runs.sort((a, b) => a - b)[2];
}
for (const [name, p] of Object.entries(INPUTS)) {
  const a = median(six, p), b = median(one, p);
  console.log(`SMA673-RAW ${name} six_ms=${a.toFixed(1)} one_ms=${b.toFixed(1)} fall=${(100 * (1 - b / a)).toFixed(1)}%`);
}
```

Run: `node <scratchpad>/sma673-binding-bench.mjs`. Record the three lines in the PR-notes file under "Raw binding".

- [ ] **Step 4: Delete the bench file**

Run: `rm ts/packages/paigasus-console-core/tests/unit/sma673-bench.local.test.ts && git status --short ts/packages/paigasus-console-core`
Expected: no output. Keep a copy in the scratchpad for Task 8.

---

### Task 7: `prn-tenancy.ts` reads a PRN with one kernel call

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/prn-tenancy.ts:3-14,47-72,108-112`
- Modify: `ts/packages/paigasus-console-core/src/logger.ts:14-24`
- Modify: `ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts` (full rewrite below)
- Modify: `ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts:199-211` (source test)
- Modify: `ts/packages/paigasus-console-core/tests/unit/logger.test.ts` (one type-level case)

**Interfaces:**
- Consumes: `prnParse`, `PrnParseResult` from `@paigasus/kernel` (Task 4); `logger` from `./logger`.
- Produces: no export change. `AppEventName` gains `'prn.kernel_call_failed'`.

- [ ] **Step 1: Rewrite the delegation test**

Replace `tests/unit/prn-tenancy-delegation.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// src/prn-tenancy.ts must DELEGATE the PRN grammar to the kernel (SMA-634 spec § 6.3), and it must
// read a PRN with ONE kernel call (SMA-673 A4). The corpus replay in prn-tenancy.test.ts cannot
// prove either: a hand-written reader, or a six-call reader, passes the same rows. This file mocks
// the kernel and asserts that its one answer decides the result.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { prnBuild, prnErrorKind, prnOrg, prnParse, prnRegion, prnResourceId, prnResourceType, prnService, type PrnParseResult } from '@paigasus/kernel';
import { ROOT_PRN, organizationPrn, parsePrincipalPrn, parseTenancyPrn, principalPrn, projectPrn, teamPrn } from '../../src/prn-tenancy';

const { appEvent } = vi.hoisted(() => ({ appEvent: vi.fn() }));

vi.mock('@paigasus/kernel', () => ({
  prnParse: vi.fn(),
  // The six single-field accessors stay in the mock ONLY so the A4 test can assert that they are
  // never called. If prn-tenancy.ts calls one, it gets `undefined`, and the field tests red too.
  prnErrorKind: vi.fn(),
  prnService: vi.fn(),
  prnRegion: vi.fn(),
  prnResourceType: vi.fn(),
  prnResourceId: vi.fn(),
  prnOrg: vi.fn(),
  prnBuild: vi.fn(),
}));
vi.mock('../../src/logger', () => ({ logger: { appEvent } }));

const ORG = '0190a100-0000-7000-8000-0000000000aa';
const TEAM = '0190a1b2-0000-7000-8000-000000000001';
// A PRN that is valid and of a tenancy shape, so only the kernel's verdict can reject it.
const TEAM_PRN = `prn:pgs:iam::${ORG}:team/${TEAM}`;
const PRINCIPAL_PRN = `prn:pgs:iam:::principal/${TEAM}`;
const OLD_ACCESSORS = [prnErrorKind, prnService, prnRegion, prnResourceType, prnResourceId, prnOrg];

type Ok = Extract<PrnParseResult, { ok: true }>;
function ok(fields: Partial<Omit<Ok, 'ok'>> = {}): PrnParseResult {
  return { ok: true, service: 'iam', region: '', org: ORG, resourceType: 'team', resourceId: TEAM, ...fields };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(prnParse).mockReturnValue(ok());
  vi.mocked(prnBuild).mockReturnValue('built-by-the-kernel');
});

describe('parseTenancyPrn delegates the grammar to the kernel', () => {
  it('returns null when the kernel rejects a PRN that LOOKS valid — a local grammar would accept it', () => {
    vi.mocked(prnParse).mockReturnValue({ ok: false, errorKind: 'wrong-field-count' });
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(TEAM_PRN);
  });

  it('takes the fields from the kernel, not from the string', () => {
    const otherOrg = '0190a100-0000-7000-8000-0000000000bb';
    const otherId = '0190a1b2-0000-7000-8000-000000000002';
    vi.mocked(prnParse).mockReturnValue(ok({ org: otherOrg, resourceId: otherId }));
    // The argument still spells ORG and TEAM. A reader that split the string would return those.
    expect(parseTenancyPrn(TEAM_PRN)).toEqual({ kind: 'team', orgId: otherOrg, id: otherId });
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(TEAM_PRN);
  });

  it('reads an organization from the kernel, taking its orgId from the resource id', () => {
    const orgId = '0190a100-0000-7000-8000-0000000000cc';
    // An organization carries NO org field, so the kernel reports an empty one.
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'organization', resourceId: orgId, org: '' }));
    expect(parseTenancyPrn(`prn:pgs:iam:::organization/${orgId}`)).toEqual({ kind: 'organization', orgId, id: orgId });
  });

  it('returns null for an organization that the kernel reports WITH an org field', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'organization' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports another service', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ service: 'gateway' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a region', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ region: 'us-east-1' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a non-tenancy resource type', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'user' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });
});

describe('one kernel call per parse (SMA-673 A4)', () => {
  function expectOneCall(prn: string): void {
    expect(vi.mocked(prnParse)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(prn);
    for (const accessor of OLD_ACCESSORS) expect(vi.mocked(accessor)).not.toHaveBeenCalled();
  }

  it('reads a tenancy PRN with exactly one prnParse call', () => {
    expect(parseTenancyPrn(TEAM_PRN)).toEqual({ kind: 'team', orgId: ORG, id: TEAM });
    expectOneCall(TEAM_PRN);
  });

  it('reads a principal PRN with exactly one prnParse call', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'principal', org: '' }));
    expect(parsePrincipalPrn(PRINCIPAL_PRN)).toEqual({ id: TEAM });
    expectOneCall(PRINCIPAL_PRN);
  });

  it('reads ROOT_PRN with exactly one prnParse call', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'root', org: '', resourceId: '00000000-0000-0000-0000-000000000000' }));
    expect(parseTenancyPrn(ROOT_PRN)).toBeNull();
    expectOneCall(ROOT_PRN);
  });
});

/**
 * The `MAX_LEN` guard in src/prn-tenancy.ts is a RESOURCE limit, not grammar. The kernel enforces
 * its own 512-byte rule, but only AFTER the string is copied into wasm linear memory, which never
 * shrinks. The kernel is mocked here and accepts everything, so the ONLY thing that can stop an
 * over-long input is the guard. The assertion is on `prnParse`, the one call that remains
 * (SMA-673): an assertion on the old accessors would stay green with the guard deleted.
 */
describe('the parsers bound the input before the kernel call', () => {
  // MAX_LEN is 512 in src/prn-tenancy.ts. Spelled here so the boundary case below is exact.
  const MAX_LEN = 512;
  const overLong = `prn:pgs:iam::${ORG}:team/${'a'.repeat(MAX_LEN)}`;

  it('does not call the kernel for an over-long PRN', () => {
    expect(overLong.length).toBeGreaterThan(MAX_LEN);
    expect(parseTenancyPrn(overLong)).toBeNull();
    expect(parsePrincipalPrn(overLong)).toBeNull();
    expect(vi.mocked(prnParse)).not.toHaveBeenCalled();
  });

  it('does not call the kernel for an empty PRN', () => {
    expect(parseTenancyPrn('')).toBeNull();
    expect(parsePrincipalPrn('')).toBeNull();
    expect(vi.mocked(prnParse)).not.toHaveBeenCalled();
  });

  // The boundary, so the guard cannot be tightened into a `>=` that rejects a legal PRN, and so the
  // over-long case above is not satisfied by a guard that refuses everything.
  it('passes a PRN of exactly MAX_LEN characters to the kernel, once', () => {
    const head = `prn:pgs:iam::${ORG}:team/`;
    const exact = head + 'a'.repeat(MAX_LEN - head.length);
    expect(exact.length).toBe(MAX_LEN);
    expect(parseTenancyPrn(exact)).toEqual({ kind: 'team', orgId: ORG, id: TEAM });
    expect(vi.mocked(prnParse)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(exact);
  });
});

/**
 * Both parsers return `… | null`, so they must be TOTAL: a server component calls them on a URL
 * segment, and a throw there is a 500 where a 404 belongs, on input an attacker controls. The one
 * kernel call can still throw: a wasm runtime failure, or the adapter's TypeError on a glue defect.
 * The catch turns it into null AND logs it (SMA-673 D5), because the log line is the only runtime
 * signal of a glue defect. It logs the error's NAME only: never the PRN, never the message.
 */
describe('the parsers are total and log a redacted event when the kernel call fails', () => {
  const MESSAGE_WITH_PRN = `trap while reading ${TEAM_PRN}`;
  const THROWN: ReadonlyArray<readonly [string, unknown, string]> = [
    ['a wasm runtime failure', new Error(MESSAGE_WITH_PRN), 'Error'],
    ["the adapter's TypeError", new TypeError(MESSAGE_WITH_PRN), 'TypeError'],
    ['a non-Error value', MESSAGE_WITH_PRN, 'unknown'],
  ];
  const PARSERS: ReadonlyArray<readonly [string, (prn: string) => unknown]> = [
    ['parseTenancyPrn', parseTenancyPrn],
    ['parsePrincipalPrn', parsePrincipalPrn],
  ];

  for (const [parserName, parse] of PARSERS) {
    it.each(THROWN)(`${parserName} returns null for %s and logs only the error name`, (_label, thrown, name) => {
      vi.mocked(prnParse).mockImplementation(() => {
        throw thrown;
      });
      expect(parse(TEAM_PRN)).toBeNull();
      expect(appEvent).toHaveBeenCalledTimes(1);
      expect(appEvent).toHaveBeenCalledWith('prn.kernel_call_failed', { error: name });
      const logged = JSON.stringify(appEvent.mock.calls);
      expect(logged).not.toContain(TEAM_PRN);
      expect(logged).not.toContain('trap while reading');
    });
  }

  it('does not log when the kernel call succeeds or rejects the PRN', () => {
    parseTenancyPrn(TEAM_PRN);
    vi.mocked(prnParse).mockReturnValue({ ok: false, errorKind: 'bad-org' });
    parseTenancyPrn(TEAM_PRN);
    expect(appEvent).not.toHaveBeenCalled();
  });

  it('does not swallow a failure of the builders, which have no null contract', () => {
    vi.mocked(prnBuild).mockImplementation(() => {
      throw new Error('wasm trap');
    });
    expect(() => teamPrn(ORG, TEAM)).toThrow('wasm trap');
  });
});

describe('the builders call prnBuild with the IAM tenancy arguments', () => {
  it('organizationPrn sends an EMPTY org field', () => {
    expect(organizationPrn(ORG)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', '', 'organization', ORG);
  });

  it('teamPrn sends the org field', () => {
    expect(teamPrn(ORG, TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', ORG, 'team', TEAM);
  });

  it('projectPrn sends the org field', () => {
    expect(projectPrn(ORG, TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', ORG, 'project', TEAM);
  });

  it('principalPrn sends an EMPTY org field', () => {
    expect(principalPrn(TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', '', 'principal', TEAM);
  });

  it('rejects an id that is not a UUID before it reaches the kernel', () => {
    expect(() => organizationPrn('x')).toThrow(TypeError);
    expect(vi.mocked(prnBuild)).not.toHaveBeenCalled();
  });

  it('rejects a principal id that is not a UUID before it reaches the kernel', () => {
    expect(() => principalPrn('x')).toThrow(TypeError);
    expect(vi.mocked(prnBuild)).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Add the source test (5.3)**

In `tests/unit/prn-tenancy.test.ts`, inside `describe('the module holds no PRN grammar of its own', …)` (after the `it.each` at line 208-210), add:

```ts
  // SMA-673 5.3: IAM's tenancy rule stays OUTSIDE the try in readKernelFields, so a programming
  // error in the rule is never swallowed as "not a tenancy PRN". Same source-text method as above.
  it("keeps IAM's tenancy rule outside the try in readKernelFields", () => {
    const fn = source.split('function readKernelFields(')[1] ?? '';
    const tryBody = fn.split('try {')[1]?.split('} catch')[0] ?? '';
    // The anchor: without it, a renamed function or a missing try would make the two checks below
    // pass on an empty string.
    expect(tryBody).toContain('prnParse(');
    expect(tryBody).not.toContain('TENANCY_KINDS');
    expect(tryBody).not.toContain("'principal'");
  });
```

- [ ] **Step 3: Add the type-level logger case**

In `tests/unit/logger.test.ts`, after the `gateway.sa.grant_failed` type-level case (line 66-72), add:

```ts
  it('holds prn.kernel_call_failed in AppEventName at the TYPE level (SMA-673)', () => {
    // Enforced by `tsc --noEmit` in the typecheck task, not by `vitest run` (see the case above).
    expectTypeOf<'prn.kernel_call_failed'>().toMatchTypeOf<AppEventName>();
  });
```

- [ ] **Step 4: Run the tests and confirm that they fail**

Run: `cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/prn-tenancy-delegation.test.ts tests/unit/prn-tenancy.test.ts`
Expected: FAIL. `prnParse` is never called by the old code, the old accessors return `undefined`, and the source test finds no `prnParse(` in the `try`.

- [ ] **Step 5: Change `logger.ts`**

In `src/logger.ts`, change the end of `AppEventName` (lines 23-24) to:

```ts
  // SMA-636 § 5.2: CreateServiceAccount succeeded, and the gateway_user grant after it failed.
  | 'gateway.sa.grant_failed'
  // SMA-673 D5: the one kernel call in prn-tenancy.ts threw (a wasm runtime failure or a glue
  // defect). Fields: `error`, the thrown value's name only. Never the PRN, never the message.
  | 'prn.kernel_call_failed';
```

- [ ] **Step 6: Change `prn-tenancy.ts`**

1. Replace line 14 with:

```ts
import { prnBuild, prnParse } from '@paigasus/kernel';
import { logger } from './logger';
```

2. In the header comment (lines 11-12), change the delegation sentence to: `tests/unit/prn-tenancy-delegation.test.ts proves that the kernel, not this file, reads the PRN, and that it reads it with ONE kernel call (SMA-673).`

3. Replace lines 50-72 (the doc comment and `readKernelFields`) with:

```ts
/**
 * The kernel's view of `prn`, or null when the kernel rejects it, when it names another service,
 * when it carries a region, OR WHEN THE KERNEL CALL FAILS.
 *
 * ONE kernel call reads every field (SMA-673), so there is no cross-call invariant to trust. The try
 * is still here because `parseTenancyPrn` returns `TenancyRef | null` and must be TOTAL: a Next
 * server component calls it on a URL segment, and a throw there is a 500 where a 404 belongs, on
 * input an attacker controls. The call can still throw: a wasm runtime failure, or the TypeError
 * that `prnParse` raises on a glue defect. The catch logs that failure, because the log line is the
 * only runtime signal of such a defect. It logs the error's NAME only, never the PRN or the message:
 * redaction is the caller's contract (logger.ts). The catch covers the kernel call ONLY. IAM's
 * tenancy rule stays outside it, in the callers below, so a programming error of ours is never
 * swallowed as "not a tenancy PRN".
 */
function readKernelFields(prn: string): KernelFields | null {
  try {
    // The kernel is the grammar: `ok: false` is any malformed PRN.
    const parsed = prnParse(prn);
    if (!parsed.ok) return null;
    if (parsed.service !== 'iam' || parsed.region !== '') return null;
    // `org` is '' for an ABSENT org field; a malformed one is already `ok: false` above.
    return { resourceType: parsed.resourceType, resourceId: parsed.resourceId, org: parsed.org };
  } catch (error) {
    logger.appEvent('prn.kernel_call_failed', { error: error instanceof Error ? error.name : 'unknown' });
    return null;
  }
}
```

4. In the `PrincipalRef` doc comment (line 112), change "Kept here because it reads the same kernel calls as the tenancy readers above." to "Kept here because it reads the same kernel call as the tenancy readers above."

Do not use `.split(`, `.indexOf(` or `.slice(` in this file (the existing source test bans them).

- [ ] **Step 7: Run the console-core tests and confirm that they pass**

Run: `moon run paigasus-console-core-ts:test --force` and `pnpm -C ts/packages/paigasus-console-core exec tsc -p tsconfig.json --noEmit`
Expected: PASS. `tests/unit/prn-tenancy.test.ts` passes with NO change to its rows or expectations (A5). `git diff ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts` shows only the added source test.

- [ ] **Step 8: Mutations M1, M3, M7, M8, M9**

Run each, one at a time. After each: restore with the Edit tool, confirm with `git diff`, re-run green.

- M1: replace the body of the `try` with the old six-accessor code (and the old import line). Run the console-core tests. Expected: the A4 tests FAIL (the old accessors are called, `prnParse` is not), and the mocked-field tests FAIL (the old accessors return `undefined`).
- M3: remove the `try { … } catch (…) { … }` wrapper and keep only the body. Expected: the "returns null for … and logs only the error name" cases FAIL.
- M7: delete `prn.length === 0 || prn.length > MAX_LEN` in BOTH `parseTenancyPrn` and `parsePrincipalPrn` (replace the condition with `false` so it still compiles and `MAX_LEN` stays used — or delete the line and the `MAX_LEN` constant together). Expected: the bound block FAILS (`prnParse` is called).
- M8: add `if (!TENANCY_KINDS.has(parsed.resourceType)) return null;` inside the `try`, after the service check. Expected: the source test "keeps IAM's tenancy rule outside the try" FAILS.
- M9: delete the `logger.appEvent(…)` line in the `catch` (keep `catch (error)` compiling by changing it to `catch`). Expected: the "logs only the error name" cases FAIL.

Append each result (the failing test names) to the PR-notes file under "Mutation proof".

- [ ] **Step 9: Lint, format and commit**

Run: `moon run paigasus-console-core-ts:lint --force` and `pnpm -C ts exec prettier --check packages/paigasus-console-core/src packages/paigasus-console-core/tests/unit`.

```bash
git -C <worktree> branch --show-current   # must print feature/sma-673-prn-parse-binding
git add ts/packages/paigasus-console-core/src/prn-tenancy.ts ts/packages/paigasus-console-core/src/logger.ts ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts ts/packages/paigasus-console-core/tests/unit/logger.test.ts
git commit -m "feat(ts): read a PRN with one kernel call in prn-tenancy (SMA-673)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: After measurement, the stop rule and the Linear comment

**Files:**
- Create, run, then DELETE (never commit): `ts/packages/paigasus-console-core/tests/unit/sma673-bench.local.test.ts` (the scratchpad copy from Task 6)

**Interfaces:**
- Consumes: the "Before" and "Raw binding" numbers in the PR-notes file (Task 6).
- Produces: the A6 table in the PR-notes file and on Linear; a go or stop decision.

- [ ] **Step 1: Run the same bench on the new code**

Copy the scratchpad bench back to `tests/unit/sma673-bench.local.test.ts`. Run it five times exactly as in Task 6 Step 2, on the same host, with the same node version.
Expected call counts: team 1, root 1, gateway 1. Record the lines under "After". Then `rm` the file and confirm `git status --short ts/packages/paigasus-console-core` is empty.

- [ ] **Step 2: Apply the stop rule (Q5)**

Compute `fall = 1 - after_median / before_median` for the team PRN.
- `fall >= 30%`: go on to Step 3.
- `fall < 30%`: STOP before the PR. Post the table (Step 4) on Linear with the sentence "The saving is below the 30% threshold (Q5). Work stopped before merge; Sven decides." Do not start Task 9 or Task 10. Report a blocker with the numbers.

- [ ] **Step 3: Estimate one page render**

Read `ts/apps/iam-console/app/(console)/orgs/[org]/load.ts`, `ts/apps/iam-console/app/(console)/orgs/node-ref.ts` and `ts/packages/paigasus-console-core/testing/dev-world.ts`. Count the `parseTenancyPrn` and `parsePrincipalPrn` calls that one render of the org page makes for the dev-world fixture (one per team row in `[org]/load.ts`, plus each `node-ref.ts` call on the render path). Estimate: `calls × (before_median_ms − after_median_ms) / 100 000` ms saved per render. State that it is an estimate from a micro-benchmark on one host, not a measured render time (Q2, L3).

- [ ] **Step 4: Write the A6 table and post it**

Write this table into the PR-notes file:

| Measure | Input | Before | After | Change |
|---|---|---|---|---|
| Kernel calls per parse | team / root / gateway | 6 / 6 / 2 | 1 / 1 / 1 | … |
| Raw binding, 100 000 calls (median of 5) | team / root / gateway | six accessors ms | one `prnParseFields` ms | % |
| `parseTenancyPrn`, 100 000 calls (median of 5) | team / root / gateway | ms | ms | % |
| One org page render (estimate) | dev-world | calls × ms | | ms saved |

Name the host (development Mac, `uname -m`, `node --version`) under the table. Post the table as a comment on SMA-673 with the Linear MCP `save_comment` tool. Record the comment URL in the PR-notes file.

---

### Task 9: The Notion Development Guidelines entry (A8) and the follow-up issue check

**Files:** none in the repo.

**Interfaces:**
- Consumes: the final D1 convention as implemented (Tasks 1-4).
- Produces: a Notion page URL for the PR body.

- [ ] **Step 1: Find the Development Guidelines page**

Use the Notion MCP: `notion-search` for "Development Guidelines" (CONTRIBUTING.md links it). `notion-fetch` the page and read its existing entry format.

- [ ] **Step 2: Add one entry in the page's own format**

Title: "A multi-value return across the kernel bindings". Content (STE):
- The first positional multi-value return across the bindings is `prnParseFields` (SMA-673).
- The wire form is a fixed-length `Vec<String>`. It lives once, in `paigasus_kernel::wire` (`#[doc(hidden)] pub`, binding-only). Each binding exports it as a one-line wrapper.
- The reason: wasm-bindgen maps `Vec<String>` to `string[]` and napi-rs 3 to `Array<string>`. These are identical, so `binding-parity.types.ts` holds. A struct gives a wasm class with a `free()` handle and a napi interface, which are not identical.
- Each language has ONE typed adapter (TypeScript: `toPrnParseResult` in `@paigasus/kernel`). The package does not re-export the raw positional function.
- The parity corpus computes the expected values from the typed kernel accessors, NOT from the wire function.
- A change to the shape is a breaking change of the published kernel crate (semver).
- Python has no wire function (Q1); a new PyO3 `Vec<String>` return needs a decision on `ci/pyo3-stub/check.py` first.
- This is a guideline entry, not an ADR-0005 amendment (Q4).

Add it with `notion-update-page`. Record the page URL (with the block anchor if the tool gives one) in the PR-notes file under "Notion entry".

- [ ] **Step 3: Check the follow-up issue**

Use the Linear MCP `get_issue` on SMA-725. Confirm: team Sven Maschek, project Paigasus Polyglot, milestone Frontend, priority Low, labels `area:frontend`, `area:ffi`, Improvement, and a "related" link to SMA-673. If SMA-725 does not exist, create it with exactly these fields (title: "Deprecate the six single-field PRN accessors in @paigasus/kernel"). If a field is missing, set it. Record the result in the PR-notes file.

---

### Task 10: The whole mutation battery and the gate graph

**Files:** none changed, unless a gate finds a defect.

- [ ] **Step 1: Re-run the whole mutation battery**

After all fixes, re-run M1-M9 in one pass, in this order: M2, M4 (Task 5), M5, M6 (Task 4), M1, M3, M7, M8, M9 (Task 7). Same expected results. Restore each with the Edit tool and `git diff`. If any fix was made in between, the whole battery runs again. Update the PR-notes file with the final table: mutation, edit, expected red, observed red.

- [ ] **Step 2: Run the targeted gates**

Run:

```bash
moon ci :build :test :lint :fmt :typecheck :machete :parity-corpus-drift :wasm-getrandom-free :version-lockstep :pyo3-stub-drift --base origin/main --include-relations
```

Expected: green. `:pyo3-stub-drift` green with no stub change confirms D3. If a task fails, follow the root `CLAUDE.md` moon-diagnosis procedure (capture state first, then read the failed task's logs).

- [ ] **Step 3: Run the full gate graph**

Run the full command between the `ci-targets` markers in the root `CLAUDE.md`. Then re-run the gates that need another bash directly, per "This development Mac only": `/opt/homebrew/bin/bash ci/publish-metadata/run.sh`, `/opt/homebrew/bin/bash ci/release-parity/run.sh` (and its `-py`/`-ts` variants per its README), and `repo:affected-smoke` under system bash 3.2. If the `repo:actionlint` preflight reports a small pipe (SMA-612), run that gate in a Linux container (`docker run ubuntu:24.04`). Read those verdicts instead of the full-graph verdict for them.

Known: `repo:deny` fails on the yanked `yoke-derive` 0.8.3. Do not fix it here. Record it in the PR-notes file.

- [ ] **Step 4: Final branch check**

Run: `git -C <worktree> status --short` (expected: empty, no bench file, no mutation) and `git -C <worktree> log --oneline origin/main..HEAD` (expected: the plan commit plus the seven commits of Tasks 1-7).
