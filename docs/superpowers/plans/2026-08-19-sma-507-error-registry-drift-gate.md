# Error-code registry drift gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Guard the canonical error-code vocabulary that `buf breaking` cannot see, by closing the two uncovered emission sites in Rust and adding one repo-level Moon gate that forces any new site to register.

**Architecture:** A single `repo:error-code-single-site` Moon task runs `ci/error-registry/check.py`, which derives the 46 codes from `error.proto`, cross-checks that derivation against the Rust mirror in `paigasus-proto`, then asserts every file under `rs/crates/*/*/src/**` that spells one of those codes is on a reviewed manifest. Two in-crate membership tests are added or widened to close sites that emit codes today with no assertion. No scheduling task is built: measurement showed a contracts change already schedules both services' `:test`.

**Tech Stack:** Python 3 (stdlib only), Moon 2.3.2, Rust (edition 2024), `strum::EnumIter`, `cargo nextest`.

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Conventional commits with a workspace scope: `feat(rs): …`, `chore(repo): …`. Subject **must start lowercase** and be **≤ 100 chars**.
- No body line may start `word:` — commitlint parses it as a trailer and fails `footer-leading-blank`. Write "owner/repo PR NNN", never `#NNN`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/nextest resolve to the repo-pinned versions.
- Run all commands from the worktree root `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-507`. Never `cd` to the main checkout.
- Moon does **not** enable errexit for `script:` blocks — any multi-command script needs `set -euo pipefail` explicitly.
- `cargo nextest` needs `--no-tests=pass` on whole-workspace runs. The commands in this plan are crate-scoped and their tests exist, so they do not need it.
- Rust crates use edition 2024 + rust-version 1.95.
- `strum` is already a `[dev-dependencies]` entry in both `paigasus-iam` and `paigasus-gateway`. Do not add it.
- After editing a file and re-running a Rust test, do not restore via a `.bak` move — `mv` rolls mtime backwards and cargo reuses the stale binary. Revert with an edit, then re-run.

## File Structure

| File | Responsibility |
|---|---|
| `ci/error-registry/check.py` (create) | Derives codes from `error.proto`, cross-checks against the Rust mirror, scans `rs/crates/*/*/src/**` and enforces the manifest. Holds the manifest as module-level data. |
| `ci/error-registry/README.md` (create) | Why the gate exists, what it does and does not catch, how to add a manifest row. Mirrors `ci/actionlint/README.md`. |
| `moon.yml` (modify) | Adds the `error-code-single-site` task to the root `repo` project. |
| `.github/workflows/ci.yml` (modify) | Adds `:error-code-single-site` to the `T=(…)` target array. |
| `CLAUDE.md` (modify) | Adds the target to the documented full-graph command and a Gotchas entry. |
| `rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs` (modify) | Gains the membership test it never had. |
| `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` (modify) | `envelope_rejection`'s two codes move into a `RejectionKind` enum so its membership test enumerates rather than restates them. |

---

### Task 1: Membership test for `system_retirement.rs`

Closes spec E2 — the one emission site in the crate with no registry assertion at all.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs` (inside the existing `#[cfg(test)] mod tests`, after `response_for_needs_acknowledgement_is_409_with_the_content_that_would_be_destroyed`)

**Interfaces:**
- Consumes: `response_for(RetireOutcome) -> Response` and the test helper `body_json(Response) -> serde_json::Value`, both already in this module. `RetireOutcome` and `PolicyKind` are already imported by the test module.
- Produces: a test named `every_system_retirement_code_is_declared_in_the_canonical_registry`. Task 4's manifest names it verbatim — do not rename it without updating `ci/error-registry/check.py`.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
    /// AC 1 for this module: every code `response_for` can put on the wire is declared in the
    /// canonical registry (`contracts/proto/paigasus/common/v1/error.proto`, SMA-498).
    ///
    /// SMA-504 renamed nothing here — both codes were already canonical — so this module got no
    /// membership test either, leaving it the one emission site in the crate with no registry
    /// assertion at all (SMA-507 E2).
    ///
    /// Exhaustiveness comes from the `match` below, NOT from `strum::EnumIter`: `RetireOutcome`'s
    /// variants are struct variants carrying `PolicyKind` and `Vec<GrantRef>`, and `EnumIter`
    /// requires `Default` for every field type. `RetireOutcome` is not `#[non_exhaustive]`, so a
    /// new variant fails to COMPILE this match rather than silently escaping the assertion.
    ///
    /// The code is read back out of the rendered body rather than compared against the literal
    /// `response_for` is built from — a comparison against that same literal would pass even if
    /// the code were never registered.
    #[tokio::test]
    async fn every_system_retirement_code_is_declared_in_the_canonical_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let outcomes = [
            RetireOutcome::Retired { policy_id: "p".to_string(), kind: PolicyKind::Template, role_deleted: true },
            RetireOutcome::Blocked { role_key: "r".to_string(), grants: Vec::new(), total: 1, truncated: false },
            RetireOutcome::NeedsAcknowledgement {
                policy_id: "p".to_string(),
                kind: PolicyKind::Static,
                source: "s".to_string(),
                description: "d".to_string(),
            },
        ];

        for outcome in outcomes {
            // Exhaustive: a new RetireOutcome variant fails to compile here, which is what forces
            // it into `outcomes` above and therefore into this assertion.
            let expects_code = match outcome {
                RetireOutcome::Retired { .. } => false,
                RetireOutcome::Blocked { .. } | RetireOutcome::NeedsAcknowledgement { .. } => true,
            };
            let body = body_json(response_for(outcome)).await;
            let code = body["error"]["code"].as_str();
            if expects_code {
                let code = code.expect("a refusal must carry an error.code");
                assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
            } else {
                assert!(code.is_none(), "Retired carries no error.code today; if it grows one, register it and assert it here");
            }
        }
    }
```

- [ ] **Step 2: Run it and confirm it PASSES**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib \
  -E 'test(=adapters::http::system_retirement::tests::every_system_retirement_code_is_declared_in_the_canonical_registry)'
```

Expected: `1 test run: 1 passed`. It passes immediately because both codes *are* registered — this test documents an invariant that currently holds. Step 3 is what proves it is not vacuous.

- [ ] **Step 3: Prove the assertion bites**

Edit `system_retirement.rs:110`, changing `"grants-survive"` to `"grants-survive-typo"`. Re-run the exact command from Step 2.

Expected: FAIL with `grants-survive-typo is not declared in common/v1/error.proto`.

If it PASSES, the test is vacuous — stop and fix it before continuing.

- [ ] **Step 4: Revert and re-run**

Revert `"grants-survive-typo"` back to `"grants-survive"` **with an edit, not by restoring a backup file** (a `mv` rolls mtime backwards and cargo will reuse the binary built from the typo). Re-run Step 2's command.

Expected: `1 test run: 1 passed`.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs
git commit -m "test(rs): assert system retirement codes are in the registry (SMA-507)"
```

---

### Task 2: Enumerate `envelope_rejection`'s codes instead of restating them

Closes spec E3 — a covered file whose membership test hand-copied the two literals it was meant to check, so a third branch would escape both the test and the gate.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` — `envelope_rejection` (around `:86-93`) and its membership test `every_authn_http_code_is_in_the_registry` (around `:274-285`)

**Interfaces:**
- Consumes: `JsonRejection`, `StatusCode`, and the existing `rendered()` / `all_authn_errors()` test helpers — all already in scope.
- Produces: a private `enum RejectionKind { TooLarge, Invalid }` with `fn parts(self) -> (&'static str, &'static str)`. Nothing outside this file consumes it. The test name `every_authn_http_code_is_in_the_registry` is unchanged and is named by Task 4's manifest.

- [ ] **Step 1: Write the failing test**

Replace the body of `every_authn_http_code_is_in_the_registry` with the version that enumerates:

```rust
    /// AC 1: every code this funnel and its extractor can emit is in the canonical registry.
    ///
    /// The `AuthnError` half is driven off `all_authn_errors()`, so a new variant is covered
    /// automatically. The extractor half used to hand-restate `envelope_rejection`'s two literals
    /// here, which meant a third branch there would have escaped this test AND
    /// `repo:error-code-single-site` (this file is on the manifest) — SMA-507 E3. It now
    /// enumerates `RejectionKind`, so a new kind must state its parts or fail to compile.
    #[tokio::test]
    async fn every_authn_http_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        let mut codes: Vec<String> = RejectionKind::iter().map(|kind| kind.parts().0.to_owned()).collect();
        assert!(!codes.is_empty(), "RejectionKind must yield at least one code, or this half asserts nothing");
        for err in crate::adapters::retryable::tests_support::all_authn_errors() {
            let (_, _, body) = rendered(err).await;
            codes.push(body["error"]["code"].as_str().expect("a code").to_owned());
        }
        for code in codes {
            assert!(ErrorReason::from_wire_reason(&code).is_some(), "{code} is not declared in common/v1/error.proto");
        }
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib \
  -E 'test(=adapters::http::authn::tests::every_authn_http_code_is_in_the_registry)'
```

Expected: FAIL to compile — `cannot find type RejectionKind in this scope`.

- [ ] **Step 3: Write the minimal implementation**

Immediately above `fn envelope_rejection`, add:

```rust
/// Every `(code, message)` pair [`envelope_rejection`] can put on the wire.
///
/// Extracted from the `if` that used to inline both literals so the membership test can enumerate
/// them rather than restate them. The test previously hand-copied `"request-too-large"` and
/// `"invalid-request-body"`, so a third branch here would have escaped both it and
/// `repo:error-code-single-site` (SMA-507 E3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
enum RejectionKind {
    /// The body exceeded the configured byte limit.
    TooLarge,
    /// The body could not be deserialized.
    Invalid,
}

impl RejectionKind {
    /// This kind's canonical registry code and its static, caller-safe message.
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            // `invalid-request-body` is merged with the gateway's identical case: one code for one
            // condition across both services (ADR-0019 A1.3).
            RejectionKind::Invalid => ("invalid-request-body", "invalid request body"),
            RejectionKind::TooLarge => ("request-too-large", "request body too large"),
        }
    }
}
```

Then replace `envelope_rejection`'s opening `let (code, message) = if …` block with:

```rust
    let kind = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE { RejectionKind::TooLarge } else { RejectionKind::Invalid };
    let (code, message) = kind.parts();
```

Leave the rest of the function — the `json!` envelope, the status, the `RETRYABLE_HEADER` insert — exactly as it is.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(/adapters::http::authn::/)'
```

Expected: all of the module's tests pass, including the two that assert `invalid-request-body` end-to-end through the real extractor.

- [ ] **Step 5: Prove the enumeration bites**

Temporarily change `RejectionKind::TooLarge`'s code in `parts()` from `"request-too-large"` to `"request-too-large-typo"`, re-run Step 4's command.

Expected: FAIL with `request-too-large-typo is not declared in common/v1/error.proto`.

Revert with an edit (not a backup restore) and re-run Step 4. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/authn.rs
git commit -m "refactor(rs): enumerate the envelope rejection codes (SMA-507)"
```

---

### Task 3: `check.py` — code derivation and the mirror cross-check

The gate's foundation: derive the vocabulary from the proto, and prove the derivation is right by comparing it against the independent transcription that already exists in Rust.

**Files:**
- Create: `ci/error-registry/check.py`

**Interfaces:**
- Produces, for Task 4:
  - `REPO: Path` — the repo root, `Path(__file__).resolve().parents[2]`
  - `derive_codes(text: str) -> set[str]` — codes from `error.proto` source text
  - `mirror_codes(text: str) -> set[str]` — codes from `paigasus-proto/src/error.rs`'s `EXPECTED_REASONS`
  - `InfraError(RuntimeError)` — raised for "the inputs are broken", mapped to rc 2
  - `self_test() -> int`
  - `main()` dispatching on `--self-test` / `--single-site`

- [ ] **Step 1: Write the failing test**

Create `ci/error-registry/check.py` containing **only** the self-test and the pieces it exercises:

```python
# SPDX-License-Identifier: Apache-2.0
# SMA-507 — single-site gate for the canonical error-code vocabulary.
#
# Error codes are strings on the wire (google.rpc.ErrorInfo.reason), deliberately not a proto enum,
# so `buf breaking` cannot see them: it does not read kebab strings and never runs against Rust.
#
# WHAT THIS GATES: every file under rs/crates/*/*/src that spells one of the registry's codes must
# be on MANIFEST below. That forces a NEW emission site to be registered and given a membership
# test, which is the failure this repo already had twice (system_retirement.rs had no assertion at
# all; authn.rs's test hand-restated the codes it was meant to check).
#
# WHAT THIS DOES NOT GATE: an UNDECLARED code. The scan greps for the strings the registry
# declares, so a site inventing "widget-jammed" produces no hit. Catching that would mean flagging
# kebab literals by SHAPE, which collides with "content-type"/"application/json" and would drown
# the gate in false positives. See ci/error-registry/README.md.
#
# It never shells out to cargo (the Moon task is toolchain: 'system') and reads no YAML.
#
# usage: check.py [--self-test | --single-site]
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
REGISTRY_PROTO = REPO / "contracts/proto/paigasus/common/v1/error.proto"
REGISTRY_MIRROR = REPO / "rs/crates/libs/paigasus-proto/src/error.rs"


class InfraError(RuntimeError):
    """The inputs or environment are broken, NOT 'the tree regressed'.

    main() maps this to rc 2 so a broken checker aborts loudly instead of folding into a green.
    """


# Matches a declaration line inside `enum ErrorReason`, e.g. `  ERROR_REASON_SLUG_CONFLICT = 1;`.
# Anchored on the `= N;` tail on purpose: a bare ERROR_REASON_[A-Z0-9_]+ scan also matches the
# prefix and the value names error.proto mentions in its own prose comments.
_DECL = re.compile(r"^\s*ERROR_REASON_([A-Z0-9_]+)\s*=\s*\d+\s*;\s*$", re.MULTILINE)

# The `const EXPECTED_REASONS: &[&str] = &[ … ];` block in paigasus-proto's test module.
_MIRROR_BLOCK = re.compile(r"const EXPECTED_REASONS:\s*&\[&str\]\s*=\s*&\[(.*?)\];", re.DOTALL)
_MIRROR_ITEM = re.compile(r'"([a-z0-9-]+)"')


def derive_codes(text):
    """The wire codes error.proto declares, by the mapping rule that file states normatively:
    strip ERROR_REASON_, lowercase, '_' -> '-'. The UNSPECIFIED sentinel is not a code."""
    names = [m.group(1) for m in _DECL.finditer(text)]
    return {name.lower().replace("_", "-") for name in names if name != "UNSPECIFIED"}


def mirror_codes(text):
    """The same vocabulary as transcribed by hand in paigasus-proto/src/error.rs's test module.

    Parsed rather than trusted: this and derive_codes are INDEPENDENT transcriptions of the same
    proto, so they can only agree if both are right. That mutual check is what makes this file's
    re-implementation of the mapping rule safe — without it, a parser returning 3 of 46 codes would
    silently scan for almost nothing and the gate would pass while guarding nothing.
    """
    block = _MIRROR_BLOCK.search(text)
    if block is None:
        raise InfraError(f"EXPECTED_REASONS not found in {REGISTRY_MIRROR} — moved or renamed?")
    return set(_MIRROR_ITEM.findall(block.group(1)))


def self_test():
    """Exercise both parsers against in-process fixtures, so a rotted parser reds even if the real
    tree happens to be clean. Runs FIRST in the Moon task, per moon.yml's affected-smoke and
    publish-metadata precedent."""
    rc = 0

    proto_fixture = """
    enum ErrorReason {
      ERROR_REASON_UNSPECIFIED = 0;
      // "slug-conflict" — mentioned in prose, must not be double-counted
      ERROR_REASON_SLUG_CONFLICT = 1;
        ERROR_REASON_NOT_FOUND = 12;
      ERROR_REASON_INTERNAL = 900;
    }
    """
    got = derive_codes(proto_fixture)
    want = {"slug-conflict", "not-found", "internal"}
    if got != want:
        print(f"  FAIL [derive_codes] {sorted(got)} != {sorted(want)}", file=sys.stderr)
        rc = 1

    mirror_fixture = 'const EXPECTED_REASONS: &[&str] = &[\n // IAM\n "slug-conflict",\n "not-found",\n "internal",\n];'
    got = mirror_codes(mirror_fixture)
    if got != want:
        print(f"  FAIL [mirror_codes] {sorted(got)} != {sorted(want)}", file=sys.stderr)
        rc = 1

    # A truncated mirror must DIFFER from the proto — this is the control that proves the
    # cross-check in check_single_site() can actually fail.
    truncated = mirror_codes('const EXPECTED_REASONS: &[&str] = &["slug-conflict"];')
    if truncated == want:
        print("  FAIL [cross-check control] a truncated mirror compared equal", file=sys.stderr)
        rc = 1

    try:
        mirror_codes("no such const here")
    except InfraError:
        pass
    else:
        print("  FAIL [mirror_codes] a missing EXPECTED_REASONS block did not raise", file=sys.stderr)
        rc = 1

    print("self-test: OK" if rc == 0 else "self-test: FAILED", file=sys.stderr)
    return rc


def main():
    args = sys.argv[1:]
    if args == ["--self-test"]:
        return self_test()
    print(f"usage: {Path(__file__).name} [--self-test | --single-site]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 2: Run the self-test to verify it passes**

```bash
python3 ci/error-registry/check.py --self-test
```

Expected: `self-test: OK`, exit 0.

- [ ] **Step 3: Verify both parsers agree on the REAL tree**

```bash
python3 - <<'PY'
import sys; sys.path.insert(0, "ci/error-registry")
from check import REGISTRY_PROTO, REGISTRY_MIRROR, derive_codes, mirror_codes
p = derive_codes(REGISTRY_PROTO.read_text())
m = mirror_codes(REGISTRY_MIRROR.read_text())
print("proto:", len(p), "mirror:", len(m), "equal:", p == m)
print("only in proto:", sorted(p - m))
print("only in mirror:", sorted(m - p))
PY
```

Expected: `proto: 46 mirror: 46 equal: True`, both difference lists empty.

If the counts are not 46, the anchored regex is wrong — fix it before continuing. Do **not** hardcode 46 in `check.py`; the count anchor already lives in `paigasus-proto/src/error.rs:217` and a third copy would be one more thing to update.

- [ ] **Step 4: Prove the self-test bites**

Temporarily change `_DECL`'s regex to `r"^\s*ERROR_REASON_(NOPE)\s*=\s*\d+\s*;\s*$"`. Run Step 2's command.

Expected: `FAIL [derive_codes]`, exit 1. Revert with an edit and re-run — expected `self-test: OK`.

- [ ] **Step 5: Commit**

```bash
git add ci/error-registry/check.py
git commit -m "feat(repo): derive the error registry and cross-check its rust mirror (SMA-507)"
```

---

### Task 4: `check.py --single-site` — the manifest and the scan

**Files:**
- Modify: `ci/error-registry/check.py`
- Create: `ci/error-registry/README.md`

**Interfaces:**
- Consumes: everything Task 3 produced.
- Produces: `--single-site` mode, exit 0 clean / 1 violation / 2 infrastructure.

- [ ] **Step 1: Add the manifest, the scan and the checks**

Insert after `mirror_codes`, before `self_test`:

```python
SCAN_GLOB = "*/*/src/**/*.rs"
SCAN_ROOT = REPO / "rs/crates"

# Files permitted to spell a registry code.
#   emits    — puts a code on the wire. `guard` names the membership test proving its codes are
#              all declared; check.py asserts that test still exists.
#   asserts  — test code only; it checks codes rather than emitting them.
#   excluded — the string is not a registry code here at all. `why` must say why.
# A path may be a literal repo-relative path or a glob.
MANIFEST = (
    ("rs/crates/services/paigasus-iam/src/application/error.rs", "emits",
     "every_tenancy_code_is_declared_in_the_canonical_registry", "TenancyError::code()"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/error.rs", "emits",
     "every_gateway_code_is_declared_in_the_canonical_registry", "GatewayError::parts()"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/authn.rs", "emits",
     "every_authn_http_code_is_in_the_registry", "the authn funnel and envelope_rejection"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs", "emits",
     "every_system_retirement_code_is_declared_in_the_canonical_registry", "the two 409 refusals"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs", "emits",
     "the_terminal_sse_frame_carries_a_registered_code", "the terminal SSE error frame"),
    ("rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs", "asserts", None,
     "hosts three membership tests; emits its codes via ErrorReason LazyLock statics, not literals"),
    ("rs/crates/libs/paigasus-proto/src/error.rs", "asserts", None,
     "EXPECTED_REASONS — the registry's own mirror, which this gate cross-checks against"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/error.rs", "asserts", None, "test assertions only"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs", "asserts", None, "test assertion only"),
    ("rs/crates/services/paigasus-iam/src/application/create_user.rs", "asserts", None, "test assertion only"),
    ("rs/crates/libs/paigasus-observability/src/grpc.rs", "excluded", None,
     "grpc_code_name maps tonic::Code to a METRIC LABEL; its \"internal\" collides with the "
     "registry's reason by spelling only. Single-word codes are ordinary English."),
    ("rs/crates/libs/paigasus-proto/src/generated/**/*.rs", "excluded", None,
     "prost output, not authored here. Its hits are all /// doc comments carried over from "
     "error.proto. Excluded by PATH rather than by a comment filter so a prost change to "
     "#[doc = \"…\"] cannot turn 47 generated lines into offenders overnight."),
)


def code_pattern(codes):
    """Match a code wrapped in either `"…"` or `\\"…\\"`.

    The escaped form is load-bearing: chat.rs builds its terminal SSE frame as one big string
    literal, so its `upstream-error` is surrounded by `\\"`, and a plain-quote anchor misses it
    entirely (SMA-507 E5).
    """
    return re.compile("|".join(r'\\?"' + re.escape(c) + r'\\?"' for c in sorted(codes)))


def scan(codes):
    """{repo-relative path: hit count} for every scanned .rs file that spells a code.

    The WHOLE file is scanned — there is no production/test split. Cutting each file at its first
    column-0 `#[cfg(test)]` was tried and rejected: seven files in this scope open one that is not
    a test module (paigasus-iam/src/config.rs at line 1316 of 3318), which would have silently
    exempted ~4560 production lines (SMA-507 E6).
    """
    pattern = code_pattern(codes)
    hits = {}
    for path in sorted(SCAN_ROOT.glob(SCAN_GLOB)):
        found = len(pattern.findall(path.read_text(encoding="utf-8")))
        if found:
            hits[path.relative_to(REPO).as_posix()] = found
    return hits


def guard_exists(guard):
    """Is `guard` still a test function somewhere under rs/crates?

    Cheap, and it recovers the only load-bearing part of the scheduling task this design dropped:
    deleting a membership test now reds THIS gate, even though nothing here runs it.
    """
    needle = f"fn {guard}("
    return any(needle in p.read_text(encoding="utf-8") for p in SCAN_ROOT.glob(SCAN_GLOB))


def check_single_site():
    proto = derive_codes(REGISTRY_PROTO.read_text(encoding="utf-8"))
    mirror = mirror_codes(REGISTRY_MIRROR.read_text(encoding="utf-8"))
    if not proto:
        raise InfraError(f"no codes derived from {REGISTRY_PROTO} — the gate would scan for nothing")
    if proto != mirror:
        print("the proto registry and its rust mirror disagree — one of them is wrong:", file=sys.stderr)
        for c in sorted(proto - mirror):
            print(f"    only in error.proto:      {c}", file=sys.stderr)
        for c in sorted(mirror - proto):
            print(f"    only in EXPECTED_REASONS: {c}", file=sys.stderr)
        return 1

    hits = scan(proto)
    if not hits:
        raise InfraError("no code literal found anywhere under rs/crates — pattern or scan root is wrong")

    listed = {path for path, *_ in MANIFEST}
    rc = 0

    offenders = sorted(p for p in hits if not any(_matches(p, entry) for entry in listed))
    if offenders:
        print("file spells a canonical error code but is not on ci/error-registry/check.py's MANIFEST:", file=sys.stderr)
        for p in offenders:
            print(f"    {p}  ({hits[p]} hit(s))", file=sys.stderr)
        print("  add a row (emits/asserts/excluded). An `emits` row needs a membership test.", file=sys.stderr)
        rc = 1

    for path, role, guard, why in MANIFEST:
        if not any(_matches(hit, path) for hit in hits):
            print(f"stale MANIFEST row: {path} no longer spells any code ({why})", file=sys.stderr)
            rc = 1
        if role == "emits" and not guard_exists(guard):
            print(f"MANIFEST names guard `{guard}` for {path}, but no `fn {guard}(` exists", file=sys.stderr)
            rc = 1
    return rc
```

Add the glob helper next to them, and `from fnmatch import fnmatch` to the imports at the top of the file:

```python
def _matches(path, entry):
    """Does repo-relative `path` match a MANIFEST entry (a literal path or a glob)?

    fnmatch, not PurePosixPath.full_match: the latter is 3.13+, and this runs on the CI runner's
    system python3 (the Moon task is toolchain: 'system'). fnmatch's `*` crosses `/`, which is
    what the generated-output glob wants anyway.
    """
    return path == entry or fnmatch(path, entry)
```

Extend `main()`:

```python
def main():
    args = sys.argv[1:]
    try:
        if args == ["--self-test"]:
            return self_test()
        if args == ["--single-site"]:
            return check_single_site()
    except InfraError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    print(f"usage: {Path(__file__).name} [--self-test | --single-site]", file=sys.stderr)
    return 2
```

Add two manifest invariants to `self_test()`, before its final print:

```python
    seen = [path for path, *_ in MANIFEST]
    if len(seen) != len(set(seen)):
        print("  FAIL [manifest] duplicate path rows", file=sys.stderr)
        rc = 1
    for path, role, guard, why in MANIFEST:
        if role not in ("emits", "asserts", "excluded"):
            print(f"  FAIL [manifest] {path} has unknown role {role!r}", file=sys.stderr)
            rc = 1
        if (role == "emits") != (guard is not None):
            print(f"  FAIL [manifest] {path}: exactly the `emits` rows must name a guard", file=sys.stderr)
            rc = 1
        if not why:
            print(f"  FAIL [manifest] {path} has no stated reason", file=sys.stderr)
            rc = 1

    if not code_pattern({"upstream-error"}).search(r'\"code\":\"upstream-error\"'):
        print("  FAIL [code_pattern] the escaped-quote form is not matched (E5 regression)", file=sys.stderr)
        rc = 1
```

- [ ] **Step 2: Run it and verify it passes on the clean tree**

```bash
python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
echo "rc=$?"
```

Expected: `self-test: OK`, no offender output, `rc=0`.

If any file is reported as an offender, that is a census miss — add it to `MANIFEST` with the correct role and a stated reason, do not widen the exclusions.

- [ ] **Step 3: Prove the offender check bites**

```bash
printf '// SPDX-License-Identifier: Apache-2.0\npub const X: &str = "slug-conflict";\n' \
  > rs/crates/services/paigasus-iam/src/adapters/http/probe_offender.rs
python3 ci/error-registry/check.py --single-site; echo "rc=$?"
rm rs/crates/services/paigasus-iam/src/adapters/http/probe_offender.rs
```

Expected: the new file is listed as an offender, `rc=1`. After the `rm`, re-run — expected `rc=0`.

- [ ] **Step 4: Prove the stale-row and missing-guard checks bite**

Comment out the `paigasus-observability/src/grpc.rs` row and re-run `--single-site`.
Expected: `rc=1`, reported as an offender (it still spells `"internal"`). Restore it.

Change that same row's path to `.../grpc_gone.rs` and re-run.
Expected: `rc=1`, reported as a **stale MANIFEST row**. Restore it.

Change the `system_retirement.rs` row's guard to `no_such_test` and re-run.
Expected: `rc=1`, `no `fn no_such_test(` exists`. Restore it, re-run, expect `rc=0`.

- [ ] **Step 5: Write the README**

Create `ci/error-registry/README.md`:

```markdown
# error-registry — the canonical error-code single-site gate

`repo:error-code-single-site` runs `check.py --self-test && check.py --single-site`.

## What it gates

Every file under `rs/crates/*/*/src/**/*.rs` that spells one of the 46 codes declared in
`contracts/proto/paigasus/common/v1/error.proto` must be on `check.py`'s `MANIFEST`.

That forces a **new emission site** to be registered and given a membership test. It is the
failure this repo already had twice: `system_retirement.rs` emitted two codes with no assertion at
all, and `authn.rs`'s membership test hand-restated the two codes it was meant to check, so a third
`envelope_rejection` branch would have escaped both.

## What it does NOT gate

**An undeclared code.** The scan greps for the strings the registry declares, so a site inventing
`"widget-jammed"` produces no hit and passes. Catching that would mean flagging kebab-case literals
by *shape*, which collides with `"content-type"`, `"application/json"` and `"paigasus-retryable"`.
The residual risk is bounded: a reason absent from the registry resolves through
`ErrorReason::from_wire_reason` on no consumer, so the code is dead on the wire regardless.

Also uncovered: codes composed at runtime (`format!("{prefix}-conflict")`), and a code added to an
already-listed file but outside the enum its guard enumerates.

## What it is NOT for

It does not check that a *removed* code is still emitted. Nothing needs to: both service crates
declare `test: deps: ['^:build']` in their own `moon.yml`, so a `contracts/` change already
schedules `paigasus-iam-rs:test` and `paigasus-gateway-rs:test`, and the membership tests run.
Verify with:

    printf 'contracts/proto/paigasus/common/v1/error.proto\n' | moon query tasks --affected --downstream deep

## Adding a row

| Role | Use when | Needs |
|---|---|---|
| `emits` | the file puts a code on the wire | a membership test, named in the row; `check.py` asserts it still exists |
| `asserts` | test code that checks codes | a stated reason |
| `excluded` | the string is not a registry code here | a stated reason |

Every row must keep matching at least one hit — a stale row reds the gate rather than rotting.
```

- [ ] **Step 6: Commit**

```bash
git add ci/error-registry/check.py ci/error-registry/README.md
git commit -m "feat(repo): gate the error-code vocabulary to reviewed sites (SMA-507)"
```

---

### Task 5: Wire the gate into Moon, CI and the docs

Delivers AC 4.

**Files:**
- Modify: `moon.yml` (append a task after `actionlint`)
- Modify: `.github/workflows/ci.yml` (the `T=(…)` array, around `:215`)
- Modify: `CLAUDE.md` (the full-graph command at `:63-68`, plus a Gotchas entry)

**Interfaces:**
- Consumes: `ci/error-registry/check.py` from Tasks 3-4.
- Produces: the Moon target `repo:error-code-single-site`.

- [ ] **Step 1: Add the Moon task**

Append to `moon.yml`:

```yaml
  error-code-single-site:
    description: 'Assert every file spelling a canonical error code is on a reviewed manifest, so a new emission site cannot ship without a registry membership test (SMA-507).'
    # WHY `inputs: rs/crates/**/src/**/*.rs` AND NOT A NARROW LIST — this gate's whole job is to
    # notice a NEW emission site in a NEW file. Narrow inputs would schedule it only when an
    # already-listed file changed, so the one case it exists for would be the one case it never
    # runs on. Cheap for the same reason repo:actionlint is: .moon/workspace.yml's
    # hasher.ignorePatterns keeps gitignored trees out of the hash walk, and the check is a
    # pure-python regex scan of ~200 files with no cargo invocation.
    #
    # `--self-test` runs FIRST and in the SAME script block, per repo:affected-smoke and
    # repo:publish-metadata: a rotted checker must red rather than ship green. `set -euo pipefail`
    # is REQUIRED — Moon does not enable errexit for `script:` blocks, so without it a failing
    # --self-test would be masked by a passing --single-site.
    #
    # This gate does NOT guard code REMOVAL. It does not need to: both service crates declare
    # `test: deps: ['^:build']` in their own moon.yml, so a contracts change already schedules
    # paigasus-{iam,gateway}-rs:test and the membership tests run. See ci/error-registry/README.md.
    script: |
      set -euo pipefail
      python3 ci/error-registry/check.py --self-test
      python3 ci/error-registry/check.py --single-site
    toolchain: 'system'
    inputs:
      - 'rs/crates/**/src/**/*.rs'
      - 'contracts/proto/paigasus/common/v1/error.proto'
      - 'ci/error-registry/**/*'
```

- [ ] **Step 2: Verify the task runs through Moon**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:error-code-single-site
```

Expected: the task succeeds. Confirm `self-test: OK` appears in the output (`.moon/tasks.yml` sets `buffer-only-failure`, so on success you may need `moon run repo:error-code-single-site --force -- --log debug`, or simply trust the exit code and rely on Step 3's failure run to show the output).

- [ ] **Step 3: Verify Moon reds when the tree is dirty**

```bash
printf '// SPDX-License-Identifier: Apache-2.0\npub const X: &str = "slug-conflict";\n' \
  > rs/crates/services/paigasus-iam/src/adapters/http/probe_offender.rs
moon run repo:error-code-single-site; echo "rc=$?"
rm rs/crates/services/paigasus-iam/src/adapters/http/probe_offender.rs
moon run repo:error-code-single-site; echo "rc=$?"
```

Expected: first run non-zero with the offender named; second run 0. The second run must actually re-execute rather than replay a cached PASS — confirm by the absence of a `(cached)` marker; if it is cached, the inputs glob is wrong.

- [ ] **Step 4: Add the target to ci.yml**

In `.github/workflows/ci.yml`, append `:error-code-single-site` to the `T=(…)` array, immediately after `:iam-docker-policy-single-site` so the two single-site gates sit together.

- [ ] **Step 5: Add the target to CLAUDE.md**

In the full-graph command at `CLAUDE.md:63-68`, add `:error-code-single-site` in the same position, keeping the two lists byte-identical in ordering.

Then add a Gotchas bullet:

```markdown
- Adding a **new error-code emission site** in Rust reds `repo:error-code-single-site` until the file
  is added to `ci/error-registry/check.py`'s `MANIFEST` — as `emits` (which also requires a
  membership test asserting every code it emits resolves via `ErrorReason::from_wire_reason`),
  `asserts`, or `excluded` with a stated reason. The gate matches the registry's **declared**
  vocabulary, so it cannot see a code you invented and never added to
  `contracts/proto/paigasus/common/v1/error.proto`; adding the code there is what makes it
  resolvable on any consumer. Code **removal** needs no gate — both service crates carry
  `test: deps: ['^:build']`, so a contracts change already runs their membership tests.
```

- [ ] **Step 6: Verify the two target lists agree**

```bash
grep -o ':error-code-single-site' .github/workflows/ci.yml CLAUDE.md
```

Expected: one hit in each file.

- [ ] **Step 7: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md
git commit -m "ci(repo): run the error-code single-site gate in the affected graph (SMA-507)"
```

---

### Task 6: Verification — negative controls and the full graph

The spec's Verification table, executed. A control that cannot attribute its red is not a control, so each step records **which task** went red.

**Files:** none modified permanently. Every injected defect is reverted in the same step.

- [ ] **Step 1: Control — a declared code in an unlisted file**

Already run in Task 5 Step 3. Record: `repo:error-code-single-site` reds, no other task does.

- [ ] **Step 2: Control — an undeclared code at the `system_retirement` site**

Edit `system_retirement.rs:110`, `"grants-survive"` → `"grants-survive-typo"`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(/system_retirement/)'
```

Expected: FAIL, `grants-survive-typo is not declared in common/v1/error.proto`. Revert via edit, re-run, expect PASS.

- [ ] **Step 3: Control — an undeclared code in a new `envelope_rejection` branch**

Add a third variant `Weird` to `RejectionKind` returning `("weird-thing", "weird")`, and a matching arm in `parts()`.

```bash
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(=adapters::http::authn::tests::every_authn_http_code_is_in_the_registry)'
```

Expected: FAIL, `weird-thing is not declared in common/v1/error.proto`. Remove the variant, re-run, expect PASS. This is the E3 regression test.

- [ ] **Step 4: Control — AC 3, a removed code that is still emitted**

This one needs its full form. The naive version (delete the value and stop) reds `paigasus-proto-rs:test` and `contracts:lint` first and proves nothing about the membership tests.

1. In `error.proto`, replace `ERROR_REASON_SLUG_CONFLICT = 1;` with `reserved 1; reserved "ERROR_REASON_SLUG_CONFLICT";`
2. `buf format -w contracts/proto/paigasus/common/v1/error.proto`
3. Regenerate the bindings: `moon run contracts:generate`
4. Remove `"slug-conflict"` from `EXPECTED_REASONS` in `paigasus-proto/src/error.rs` and change its `assert_eq!(actual.len(), 46, …)` to `45`.

```bash
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(=adapters::grpc::convert::tests::every_tenancy_code_is_declared_in_the_canonical_registry)'
```

Expected: FAIL — `TenancyError::SlugConflict emits "slug-conflict", which is not declared in common/v1/error.proto`.

Then confirm the scheduling claim that makes this control meaningful:

```bash
printf 'contracts/proto/paigasus/common/v1/error.proto\n' | moon query tasks --affected --downstream deep
```

Expected: `paigasus-iam-rs` and `paigasus-gateway-rs` both list `test`.

Revert everything: `git checkout -- contracts/ rs/crates/libs/paigasus-proto/` and re-run `moon run contracts:generate` to confirm no drift remains.

- [ ] **Step 5: Control — the mirror cross-check**

Remove one entry from `EXPECTED_REASONS` in `paigasus-proto/src/error.rs` (leave `error.proto` alone), then:

```bash
python3 ci/error-registry/check.py --single-site; echo "rc=$?"
```

Expected: `rc=1`, `only in error.proto: <that code>`. Revert with an edit.

- [ ] **Step 6: Run the full graph as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Expected: all green. If Moon reports a bare "N failed" with no attribution, read `.moon/cache/ciReport.json`:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

Docker must be running for the IAM container suites; if it is not, `tests/docker_preflight.rs` fails deliberately — start Docker rather than setting `PAIGASUS_SKIP_DOCKER`.

- [ ] **Step 7: Record the deferral on Linear**

Amend SMA-507's AC 2 to state that the consumed-side check ships with `@paigasus/sdk`, and add a comment on SMA-508 noting its AC 3 now carries SMA-507's consumed-side requirement. Do not attach PR links by hand — the integration auto-links by branch name.

- [ ] **Step 8: Commit any residue**

```bash
git status --short
```

Expected: clean. If `contracts:generate` or a revert left anything, restore it before opening the PR.

---

## Self-Review

**Spec coverage.** §1 → Task 5. §2 (no split) → Task 4 Step 1's `scan` docstring. §3 → Task 3. §4 → Task 4. §5 → Task 1. §6 → Task 2. §7 controls → Tasks 3-4 steps and Task 6. §8 → Task 5. Verification table rows 1-7 → Task 6 Steps 1-5 plus Task 4 Steps 3-4. Limitations → the README in Task 4 Step 5 and the CLAUDE.md bullet in Task 5 Step 5. Out of scope / Departures → Task 6 Step 7.

**Type consistency.** `derive_codes`/`mirror_codes`/`code_pattern`/`scan`/`guard_exists`/`_matches`/`check_single_site`/`self_test`/`main`/`InfraError`/`MANIFEST`/`REPO`/`SCAN_ROOT`/`SCAN_GLOB` are spelled identically in Tasks 3 and 4. The four test names in `MANIFEST` match the ones `nextest list` reported, and the fifth is the one Task 1 creates.

**One deliberate deviation from the spec.** Spec §7 says the derivation control should assert set equality against `EXPECTED_REASONS` "and number 46". The plan asserts set equality but does **not** hardcode 46 in Python: that count already lives in `paigasus-proto/src/error.rs:217` and runs in `paigasus-proto-rs:test`, so a third copy would be one more thing to update on every registry addition while adding no coverage — set equality against the mirror already pins the count transitively. Flagged rather than silently dropped.
