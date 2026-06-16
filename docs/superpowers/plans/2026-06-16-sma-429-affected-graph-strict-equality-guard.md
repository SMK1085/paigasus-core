# SMA-429 — Affected-graph strict-equality (default-deny) guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `kernel->bindings` case's hand-maintained forbid-regex enumeration in `ci/affected-graph/run.sh` with a strict-equality (default-deny) meta-check — each case asserts the affected set *equals* its expected set exactly — and pin moon to 2.3.2 (the version the guard is grounded on).

**Architecture:** `assert_case` drops its `FORBID_REGEX` parameter and compares the affected project set (minus `repo`) against an exact expected set using `comm`; on mismatch it reports `missing` and `unexpected` buckets with maintenance guidance. A new negative control proves the default-deny direction (an incomplete expected set must fail red). The `.prototools` moon pin is bumped 2.2.5 → 2.3.2 in the same change because strict equality couples the guard to a specific moon version's affected-set output.

**Tech Stack:** Bash (`set -euo pipefail`, process substitution, `comm`), Moon 2.3.2 (`moon query projects --affected --downstream deep`), proto-pinned toolchain.

**Spec:** `docs/superpowers/specs/2026-06-16-sma-429-affected-graph-completeness-guard-design.md`

---

## Prerequisites — environment

Moon is proto-managed and **off the default Bash-tool PATH**. Every command below assumes this
prefix, with **`shims` BEFORE `bin`** so the *repo-context* (pinned) moon resolves, not the global
binary:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

`~/.proto/shims/moon` = repo-pinned (reads `.prototools`); `~/.proto/bin/moon` = global pin. They
differ until Task 1 lands. There is no macOS `timeout`.

## File Structure

- **Modify** `.prototools` — bump `moon = "2.2.5"` → `"2.3.2"` (Task 1).
- **Modify** `ci/affected-graph/run.sh` — rewrite `assert_case` to strict equality, drop forbid
  args from the four `run_case` calls, restructure the negative-control block to two controls,
  update all comments (Tasks 2–3).
- **Modify** `ci/affected-graph/README.md` — rewrite the maintenance section, trim the per-case
  "no unrelated" parentheticals (Task 4).

No new files. The guard script *is* its own test harness (`--negative-control`).

---

## Task 1: Pin moon 2.3.2

**Files:**
- Modify: `.prototools` (the `moon = "2.2.5"` line)

- [ ] **Step 1: Confirm the current pin and the drift**

Run:
```bash
grep moon .prototools
"$HOME/.proto/shims/moon" --version   # repo-context (pinned)
grep -n "2\.[0-9]\.[0-9]" CLAUDE.md   # CLAUDE.md already says 2.3.2
```
Expected: `.prototools` shows `moon = "2.2.5"`; the shim prints `moon 2.2.5`; CLAUDE.md mentions `2.3.2` (docs already assume it).

- [ ] **Step 2: Bump the pin**

Edit `.prototools`, changing exactly:
```toml
moon = "2.2.5"
```
to:
```toml
moon = "2.3.2"
```
(Leave every other line untouched.)

- [ ] **Step 3: Provision and verify the pinned version**

Run:
```bash
proto install
"$HOME/.proto/shims/moon" --version
```
Expected: `proto install` succeeds; the **shim** now prints `moon 2.3.2` (proves the pin, not just the global binary).

- [ ] **Step 4: Verify moon 2.3.2 parses the workspace and the guard still passes**

Run:
```bash
moon query projects >/dev/null && echo "config parses on 2.3.2"
moon run repo:affected-smoke
```
Expected: config parses with no error; `repo:affected-smoke` (the existing, still-old guard) is **green** on 2.3.2 — the affected sets are identical to 2.2.5, so the old forbid-regex guard still passes. This confirms the bump is affected-graph-neutral.

- [ ] **Step 5: Commit**

```bash
git add .prototools
git commit -m "build(repo): pin moon 2.3.2 (reconcile .prototools with CLAUDE.md/.moon config)

The repo already behaves as a 2.3.2 workspace (CLAUDE.md + .moon config keys);
.prototools lagged at 2.2.5. Required by SMA-429: strict equality couples the
affected-graph guard to moon's affected-set output, so the guard ships grounded
on the exact version CI runs.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Rewrite the guard to strict equality (TDD via negative controls)

**Files:**
- Modify: `ci/affected-graph/run.sh` — `assert_case` (lines ~29–47), the negative-control block (lines ~109–121)

The new incomplete-expected negative control is the failing test: under the old positive-superset
model an *under-specified* expected set wrongly PASSES; strict equality makes it fail red.

- [ ] **Step 1: Write the failing test — restructure the negative-control block to two controls**

Replace the entire `if [ "$NEGATIVE" = 1 ]; then ... fi` block (currently lines ~109–121) with:

```bash
if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: assert deliberately-wrong expectations report red =="
  NEG_RC=0
  # expect_red LABEL FILE EXPECTED_CSV — assert the harness reports a red (rc=1) for a wrong
  # expectation; record a failed control if it green-lights one; abort on infra error (rc=2).
  expect_red() {
    local rc=0
    assert_case "$1" "$2" "$3" || rc=$?
    case "$rc" in
      1) echo "  OK   [$1] harness reported red as expected" ;;
      0) echo "  FAIL [$1] harness accepted a wrong expectation" >&2; NEG_RC=1 ;;
      *) echo "  INCONCLUSIVE [$1] infrastructure error (rc=$rc)" >&2; exit 2 ;;
    esac
  }
  # 1) wrong project: a kernel edit does NOT affect paigasus-proto-py, so requiring it must fail.
  expect_red "neg-wrong-expect"     "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-proto-py"
  # 2) default-deny direction (NEW in SMA-429): an INCOMPLETE expected set must fail on the extras.
  #    Under the old positive-superset model this PASSED (a subset satisfied the must-include check),
  #    silently unasserting every project left out — the exact gap strict equality closes.
  expect_red "neg-incomplete-expect" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-rs"
  if [ "$NEG_RC" = 0 ]; then
    echo "negative-control OK: harness reported red on all wrong expectations"; exit 0
  else
    echo "negative-control FAILED: harness green-lit a wrong expectation" >&2; exit 1
  fi
fi
```

Note: `expect_red` calls `assert_case` with 3 args; the still-old `assert_case` reads a 4th
`forbid` param that is simply unset here (empty → skipped), so this block runs against the old
code without error.

- [ ] **Step 2: Run the negative control against the OLD assert_case — verify it fails**

Run:
```bash
ci/affected-graph/run.sh --negative-control; echo "exit=$?"
```
Expected: **RED** — `exit=1`. Output shows `OK [neg-wrong-expect]` (a wrong project fails under the old model too) but `FAIL [neg-incomplete-expect] harness accepted a wrong expectation` (the old positive-superset model green-lit the subset). This demonstrates the gap.

- [ ] **Step 3: Implement strict equality — rewrite `assert_case`**

Replace the `assert_case` function and its doc comment (currently lines ~29–47) with:

```bash
# assert_case LABEL FILE EXPECTED_CSV
#   EXPECTED_CSV : comma-separated project ids. The affected set (minus `repo`) must EQUAL this
#                  set exactly — default-deny: any project present that is not listed fails the
#                  case (no separate forbid list; cross-stack isolation is implicit).
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_case() {
  local label="$1" file="$2" expected_csv="$3" got want missing unexpected
  got="$(affected_ids "$file")" || { echo "FATAL [$label]: moon query failed" >&2; return 2; }
  # Split the CSV on commas into lines and sort, to match affected_ids' sorted output. Use `tr`,
  # NOT an unquoted `${expected_csv//,/ }` word-split: the latter depends on IFS word-splitting
  # (absent in zsh) and is exposed to globbing — fragile. The expected CSV is hand-written in
  # arbitrary order, so the sort makes the comparison order-insensitive.
  want="$(tr ',' '\n' <<<"$expected_csv" | sort)"
  if [ "$got" = "$want" ]; then
    printf 'PASS  %-18s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
    return 0
  fi
  missing="$(comm -23 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  unexpected="$(comm -13 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  echo "FAIL  [$label] affected set != expected set" >&2
  if [ -n "$missing" ]; then
    echo "  missing  (expected but absent — likely a dropped dependsOn edge or a lost --include-relations):" >&2
    sed 's/^/    /' <<<"$missing" >&2
  fi
  if [ -n "$unexpected" ]; then
    echo "  unexpected (present but not expected — a cross-stack leak/regression, OR a legitimate new" >&2
    echo "  dependent: if the new edge is intended, add it to this case's expected set):" >&2
    sed 's/^/    /' <<<"$unexpected" >&2
  fi
  return 1
}
```

- [ ] **Step 4: Update the four `run_case` calls — drop the forbid argument**

Replace the body of `run_suite` (currently lines ~85–107) with (note: no 4th arg on any case, and the cross-stack/one-directionality intent now lives in comments):

```bash
run_suite() {
  SUITE_RC=0
  # contracts proto edit -> proto packages in all three languages + the gateway rebuild.
  run_case "contracts->proto" "contracts/proto/paigasus/gateway/v1/health.proto" \
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs"
  # kernel edit -> kernel + both bindings + gateway + both language wrappers (SMA-419/420).
  # Strict equality (default-deny): any OTHER project appearing (an unrelated *-py/*-ts package, a
  # contracts/py/ts root) fails the case automatically — no forbid enumeration needed.
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts"
  # py binding edit -> the binding + the py wrapper that depends on it (SMA-419). One-directional
  # w.r.t. the kernel: paigasus-kernel-rs is deliberately ABSENT (a binding edit must not rebuild
  # the kernel), now enforced implicitly by strict equality rather than a forbid-regex.
  run_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs,paigasus-kernel-py"
  # node binding edit -> the node binding + the ts wrapper that depends on it (SMA-420). Likewise
  # one-directional: paigasus-kernel-rs deliberately absent.
  run_case "binding-oneway-node" "rs/crates/bindings/paigasus-node-bindings/src/lib.rs" \
    "paigasus-node-bindings-rs,paigasus-kernel-ts"
  # assert_include_relations returns only 0/1 (no infra code), so collapsing is correct here.
  assert_include_relations || SUITE_RC=1
  return "$SUITE_RC"
}
```

- [ ] **Step 5: Run the negative control against the NEW code — verify it passes**

Run:
```bash
ci/affected-graph/run.sh --negative-control; echo "exit=$?"
```
Expected: **GREEN** — `exit=0`. Both controls now print `OK` (`neg-wrong-expect` *and* `neg-incomplete-expect` report red as expected), ending with `negative-control OK: harness reported red on all wrong expectations`.

- [ ] **Step 6: Run the positive suite — verify all four cases pass on 2.3.2**

Run:
```bash
ci/affected-graph/run.sh; echo "exit=$?"
```
Expected: **GREEN** — `exit=0`. Four `PASS` lines (`contracts->proto`, `kernel->bindings`, `binding-oneway`, `binding-oneway-node`), the `ci-include-relations` PASS, and `== affected-graph cascade intact ==`.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "refactor(ci): strict-equality affected-graph guard, drop forbid enumeration

assert_case now asserts affected==expected per case (default-deny) via comm,
reporting missing/unexpected buckets; drops the FORBID_REGEX param and the
kernel->bindings per-package regex. Adds a neg-incomplete-expect control proving
an under-specified expected set fails red (the gap the old superset model missed).
Closes SMA-429 / SMA-420 review F4.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Update `run.sh` header / doc comments referencing the old model

**Files:**
- Modify: `ci/affected-graph/run.sh` (any remaining comment referencing forbid / positive-superset / the `--` grep guard)

- [ ] **Step 1: Find stale comment references**

Run:
```bash
grep -nE "forbid|positive-superset|explicit-negative|must-exclude|cross-stack leak|-- is required" ci/affected-graph/run.sh
```
Expected: any hits are in *comments* (the code references were removed in Task 2). If a hit is in code, Task 2 was incomplete — fix before proceeding.

- [ ] **Step 2: Update the `affected_ids` doc comment (if it references forbid)**

The `affected_ids` comment (lines ~21–22) is about filtering `repo` and is still accurate — leave it unless it mentions forbid. No change expected.

- [ ] **Step 3: Confirm no `FORBID`/forbid remnants remain anywhere**

Run:
```bash
grep -niE "forbid|leaked|positive-superset" ci/affected-graph/run.sh || echo "(clean)"
```
Expected: `(clean)` — no occurrences in code or comments. If Task 2's `assert_case` and `run_suite` replacements were applied verbatim, this is already clean and Task 3 is a no-op confirmation.

- [ ] **Step 4: Re-run both guard modes to confirm no regression from comment edits**

Run:
```bash
ci/affected-graph/run.sh && ci/affected-graph/run.sh --negative-control; echo "exit=$?"
```
Expected: both green; `exit=0`.

- [ ] **Step 5: Commit (only if Step 1/2 changed anything)**

```bash
git add ci/affected-graph/run.sh
git commit -m "docs(ci): scrub stale forbid/positive-superset comments from affected guard

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
If Step 3 already reported `(clean)` and no comment was edited, skip this commit.

---

## Task 4: Rewrite the README maintenance section

**Files:**
- Modify: `ci/affected-graph/README.md` (the `## Maintenance …` section, lines ~28–36; the per-case bullets, lines ~11–20)

- [ ] **Step 1: Trim the "no unrelated" parenthetical from the kernel-edit bullet**

Replace the kernel-edit bullet (lines ~12–16) — currently:

```markdown
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-node-bindings-rs`
  + `paigasus-gateway-rs` + `paigasus-kernel-py` + `paigasus-kernel-ts` (both language wrappers now
  wrap their bindings, SMA-419/420); still **no `contracts` / unrelated `*-py`
  (`paigasus-proto/workflows/ml-py`) / unrelated `*-ts`** (`paigasus-proto/sdk/ui/console/docs-ts`,
  `commitlint-config-ts`).
```

with:

```markdown
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-node-bindings-rs`
  + `paigasus-gateway-rs` + `paigasus-kernel-py` + `paigasus-kernel-ts` (both language wrappers
  wrap their bindings, SMA-419/420). Strict equality rejects any other project implicitly.
```

(The other three bullets are unchanged.)

- [ ] **Step 2: Rewrite the Maintenance section**

Replace the entire `## Maintenance — the must-exclude assertions are topology-coupled (SMA-409 F5)`
section (lines ~28–36) with:

```markdown
## Maintenance — expected sets are exact (default-deny, SMA-429)

Each case asserts the affected set (minus `repo`) **equals** its expected set exactly — there is
no separate must-exclude list and no forbid enumeration. Cross-stack isolation is enforced
implicitly: any project that appears but isn't in the expected set fails the case.

- A project **unrelated** to a case never enters its downstream set, so it never appears → no
  maintenance (this is what the old hand-maintained forbid-regex existed to track).
- A project that **legitimately** becomes a new dependent (e.g. a future wasm kernel binding)
  makes the case fail with an `unexpected` entry → confirm the new edge is intended, then add the
  one project to that case's expected set.

The expected sets are a snapshot of `moon query --affected --downstream deep` output at the
**pinned moon version** (currently 2.3.2). A moon upgrade that changes the affected-set output —
even benignly — will fail the guard, so re-grounding the expected sets is a known step of any
moon bump.
```

- [ ] **Step 3: Verify the README has no stale forbid/must-exclude references**

Run:
```bash
grep -niE "forbid|must-exclude|positive-superset|strict equality" ci/affected-graph/README.md
```
Expected: matches only in the new Maintenance section (the phrase "strict equality" and the explanatory "forbid-regex existed to track"); no leftover description of a live forbid enumeration.

- [ ] **Step 4: Commit**

```bash
git add ci/affected-graph/README.md
git commit -m "docs(ci): rewrite affected-graph README for the default-deny model

Replaces the topology-coupled must-exclude maintenance note with the exact-set
(default-deny) model + the moon-version coupling note (SMA-429 review F2).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full guard run, both modes, on 2.3.2**

Run:
```bash
"$HOME/.proto/shims/moon" --version          # 2.3.2
moon run repo:affected-smoke                 # exercises run.sh via Moon, green
ci/affected-graph/run.sh --negative-control; echo "neg exit=$?"
```
Expected: moon 2.3.2; `repo:affected-smoke` green; negative control `exit=0`.

- [ ] **Step 2: Confirm the spec's acceptance criteria mechanically**

Run:
```bash
grep -c "FORBID\|forbid" ci/affected-graph/run.sh   # expect 0
grep "moon = " .prototools                          # expect moon = "2.3.2"
```
Expected: `0` forbid references; pin is `2.3.2`. (Maps to spec Verification §1 and §3.)

- [ ] **Step 3: F3 — query-depth ↔ build-depth equivalence (note + CI gate)**

The guard's `--downstream deep` query asserts 2-hop cascades (kernel → binding → wrapper). This is
inherited unchanged from SMA-409 and was already exercised when SMA-419/420 added the
`paigasus-kernel-py` / `-ts` wrappers to a green CI (a real `moon ci --include-relations` rebuilt
them). The PR's `moon ci` job is the standing gate — confirm it stays green and that the
`kernel->bindings` cascade builds the wrappers. No local scratch-branch integration test (SMA-409
deliberately rejected it).

- [ ] **Step 4: Confirm the full branch is green under `moon ci` on the PR**

The `.prototools` bump is repo-wide, so the real gate is a full green `moon ci` on 2.3.2 (spec
Verification §3). Push the branch and confirm the `CI / moon ci` required check passes. (Opening
the PR is an outward-facing step — confirm with the maintainer first; see the finishing-a-branch
handoff.)

---

## Notes for the executor

- **Each task leaves a green tree.** After Task 1 the *old* guard passes on 2.3.2 (sets identical);
  after Task 2 the new guard passes both modes.
- **Order matters for Task 2's TDD:** Step 1 (add the failing control) must run *before* Step 3
  (rewrite assert_case), and you must observe the RED in Step 2 — don't skip it.
- **`comm` needs sorted inputs.** Both `want` (sorted in `assert_case`) and `got` (sorted in
  `affected_ids`) already are; don't remove either sort.
- **Run the guard via its shebang (`ci/affected-graph/run.sh` / `moon run repo:affected-smoke`),
  which is bash.** Do not paste the script's body into an interactive shell to "test" it — this
  repo's interactive shell is zsh, where unquoted expansions don't word-split and the behavior
  diverges from the bash the script actually runs under. The `tr`-based `want` is shell-agnostic,
  but other lines still assume bash.
- **Commit signing** is SSH via 1Password. If a commit fails with `1Password: failed to fill whole
  buffer`, 1Password is locked — ask the maintainer to unlock, then re-run the commit.
