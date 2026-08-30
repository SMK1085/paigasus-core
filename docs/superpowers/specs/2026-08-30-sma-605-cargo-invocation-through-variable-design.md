# SMA-605 — a cargo invocation through a variable, seen by A8 and A10

Status: design, approved 2026-08-30.
Supersedes limitation **L10** of
`docs/superpowers/specs/2026-08-29-sma-599-cargo-invocation-invariants-design.md`.

## 1. Problem

`ci/affected-graph/cargo_moon_parity.py` finds cargo through `CARGO_INVOCATION_RE`, which
needs the literal word `cargo` followed by a lock-resolving verb. Two assertions read that
regex:

* **A8** (`check_cargo_locked`, `check_cargo_locked_scripts`) asserts `--locked`. An unlocked
  cargo call re-resolves the graph and rewrites an inconsistent `rs/Cargo.lock` in place.
* **A10** (`check_cargo_config_inputs`) asserts the `rs/.cargo/config.toml` task input. Its
  derivation filters on the same regex.

An invocation that reaches cargo through a variable carries no literal `cargo` token. Neither
assertion sees it.

## 2. Measurements

Every number below comes from the real corpus (`ci/**/*.sh`, `moon.yml`,
`.moon/tasks/*.yml`) on 2026-08-30. Reasoning about this classifier has produced two silent
false negatives before (SMA-599 §7), so nothing here is argued from first principles.

### M1 — the shape the issue names does not exist

SMA-605 names `ci/release-parity/ecosystems/release-plz.sh:63` as the one call site. That line
assigns `CARGO_BIN`, but no line runs `"$CARGO_BIN" <verb>`. Line 152 uses the value as an
environment variable:

```sh
if ! out="$(cd "$1" && CARGO="$CARGO_BIN" CARGO_NET_OFFLINE=true "$RELEASE_PLZ_BIN" update …)"
```

The repo holds **zero** instances of `"$VAR" <cargo-verb>`.

### M2 — a fourth indirection shape, not named in the issue

`CARGO=<path> <tool>` reaches cargo through a tool that shells out to it. This is what the repo
actually does. None of the three options in the issue covers it.

### M3 — naive widening measures at three false positives and no true positives

Accepting any variable in command position before a lock-resolving verb gives three rows:

| Site | Text | Why it is wrong |
|---|---|---|
| `ci/publish-metadata/run.sh:1647` | `echo "negative control: $failures check(s) failed to bite"` | prose in an `echo`; the conservative rule does not strip strings (SMA-599 L8) |
| `ci/release-parity/ecosystems/release-plz.sh:152` | `"$RELEASE_PLZ_BIN" update` | release-plz, not cargo |
| `ci/release-plan/run.sh:149` | `git -C "$dir" add -A` | `git add`; `$dir` is an argument to `git -C` |

The third is the expensive one. `ci/release-plan/run.sh` holds **zero** literal cargo
invocations, so `repo:release-plan` is not derived at all today. That false positive makes it
kind `script` and pulls a cargo-free task into **both** A8 and A10 scope.

### M4 — a name-constrained command-position arm measures at zero rows

If the variable's own name must contain `cargo` (case-insensitive), the corpus reports **0
rows**: no false positives, and no true positives either. This arm is forward cover.

### M5 — the environment arm must key on the exact name `CARGO`

Line 152 carries two cargo-named prefixes: `CARGO="$CARGO_BIN"` and `CARGO_NET_OFFLINE=true`.
Only the first names the cargo binary. `CARGO_NET_OFFLINE`, `CARGO_HOME` and `CARGO_TERM_COLOR`
configure cargo; they do not redirect it. A "name mentions cargo" predicate reports them.

Both predicates measure identically on today's corpus (1 candidate, in an unfollowed file), so
this is decided on principle, not on the measurement.

A crude probe consuming the trailing word missed the second of two consecutive prefixes:
`CARGO_NET_OFFLINE=true CARGO=/p tool` reports nothing, because the first prefix eats the
separator the second needs. The arm therefore reads its trailing word through a lookahead.

### M6 — no substitution-body scan is needed in A8

`_classify_shell_line` splits a logical line on `[;&|]+` **before** matching, and that split
already reaches inside a `$( … )` body. Measured on line 152 above:

```
seg[1] = ' CARGO="$CARGO_BIN" CARGO_NET_OFFLINE=true "$RELEASE_PLZ_BIN" update 2>'
arm-2 match: ' CARGO="$CARGO_BIN"'
```

An earlier draft of this design added a substitution-body scan to A8. That draft was wrong: it
generalised from a probe that stripped substitutions, which the production scanner does not do.
`_cwd_inside_rs` already scans both the stripped text and each substitution body, so A10 needs
nothing either. The mechanism is dropped.

### M7 — no Moon task sets `CARGO`

No `moon.yml` or `.moon/tasks/*.yml` task declares `CARGO` in an `env:` block.

### M8 — the one real site is unfollowed

`ci/release-parity/ecosystems/release-plz.sh` is **sourced** by `ci/release-parity/run.sh`.
`task_script_refs` reads a task's own blob, which names `ci/release-parity/run.sh` and nothing
else, so the ecosystem module is never scanned (SMA-599 L2). The environment arm gives forward
cover there; it does not give present coverage.

## 3. Decision

Take option 1 from the issue, **narrowed**, and add an arm for the shape M2 found.

Rejected, with reasons:

* **Naive widening (issue option 1, unmodified).** M3: three false positives, no true positives,
  at least three new waivers, and one of them bootstraps a cargo-free task into two assertions.
* **Ban the indirection (issue option 2).** A name-based ban on `CARGO=`-style assignment hits
  three sites, two of which are unrelated array constants in `ci/actionlint/run.sh`
  (`T_CARGO_LOCK_STEP_REQUIRED`, `T_CARGO_LOCK_SH_CALL_SITES`). It constrains how gate scripts
  may name a constant, and it still misses M2.
* **Accept and document (issue option 3).** It satisfies AC 2 vacuously and leaves the gap open.

## 4. Design

`CARGO_INVOCATION_RE` is unchanged. Two new regexes sit beside it, merged into one
start-sorted match list, so `_classify_shell_line`'s per-invocation tail arithmetic keeps
working without modification.

### 4.1 Arm 1 — a cargo-named variable in command position

```python
CARGO_VAR_CMD_RE = re.compile(
    r"""(?:^|[\s;&|(])["']?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?["']?\s+"""
    r"(?:\+\S+\s+)?(?:" + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
)
```

The captured name is post-filtered on `"cargo" in name.lower()`. The predicate is a function,
not a regex, so the filter reads plainly and the failing name appears in the row.

This is a real invocation with a tail, so it behaves exactly like a literal match: a `--locked`
in its own tail satisfies A8.

### 4.2 Arm 2 — the `CARGO=` environment prefix

```python
CARGO_ENV_PREFIX_RE = re.compile(
    r"""(?:^|[\s;&|(])CARGO=(?:"[^"]*"|'[^']*'|[^\s;&|]*)(?=\s+\S)"""
)
```

The name is exactly `CARGO` (M5). The trailing word is read through a lookahead, never consumed
(M5). There is no verb requirement: the tool's verbs belong to the tool.

Arm 2 carries **wrapper** semantics, the rule the three FFI tasks already carry. You cannot pass
`--locked` through `CARGO=<path> <tool>`, so a task or line matching it always needs an
`ALLOW_UNLOCKED_CARGO` or `ALLOW_UNLOCKED_CARGO_SCRIPT` entry. A flag never satisfies it.

### 4.3 Integration

* `ScriptCargoLine` gains a `kind` field: `literal`, `var`, or `env`. Without it, arm 2's match
  ends at the end of `CARGO=<value>` and a `--locked` belonging to the **tool** sits in its tail
  and falsely satisfies the row.
* `check_cargo_locked_scripts` reports every `env` row regardless of `line.locked`, with its own
  message naming the wrapper rule.
* `derive_cargo_tasks` treats an arm-1 or arm-2 blob match the way it treats a literal one, with
  arm 2 mapped to kind `wrapper` so the existing precedence rule (wrapper > literal > script)
  governs it.
* `check_cargo_locked`'s blob arm applies the same split: arm 1 is satisfied by `--locked` in
  the blob, arm 2 needs a waiver.
* `check_dockerfile_locked` reads the merged match list too. `rs/Dockerfile` carries no such
  line today, and its floor (`seen == 0`) still counts only what the merged list finds, so the
  floor cannot be satisfied by an indirect match that a literal scan would have missed. Leaving
  the Dockerfile on the literal-only regex would make it the one place the two arms do not
  reach, for no stated reason.

### 4.4 A10 symmetry

A10 gets the same two arms:

* arm 1, restricted to `CONFIG_SENSITIVE_VERBS` rather than `LOCK_RESOLVING_VERBS`, matching
  the split SMA-599 established;
* arm 2, sensitive unconditionally — the tool's inner cargo may compile, and A10 cannot know.

The cwd rule is unchanged and still applies to both.

## 5. Verification

### 5.1 Self-test fixtures

`self_test()` gains fixtures in both directions. The negative ones pin the narrowing itself, so
a later "simplification" of either predicate reds the self-test rather than the corpus.

| Fixture | Expected |
|---|---|
| `"$CARGO_BIN" build` in a followed script | A8 row |
| `"$CARGO_BIN" build --locked` | no row |
| the same task, cwd inside `rs/`, no `rs/.cargo/config.toml` | A10 row |
| `CARGO=/p release-plz update` | A8 row, wrapper rule, needs a waiver |
| `CARGO=/p release-plz update --locked` | A8 row (a flag never satisfies arm 2) |
| the same inside `$( … )` | A8 row (M6 regression pin) |
| `CARGO_NET_OFFLINE=true tool update` | no row |
| `CARGO_NET_OFFLINE=true CARGO=/p tool update` | A8 row |
| `git -C "$dir" add -A` | no row |
| `echo "negative control: $failures check(s) failed"` | no row |
| `"$RELEASE_PLZ_BIN" update` | no row |

The count in `self_test`'s closing line stays "all ten assertions", and
`EXPECTED_FINDING_KEYS` is unchanged: this widens A8 and A10, it does not add an assertion.

### 5.2 Mutation proofs (AC 2)

Five mutations. Each must red a named fixture. A mutation that survives means the fixture
asserts nothing, and the fixture is wrong.

| Mutation | Must red |
|---|---|
| delete arm 1 | the `"$CARGO_BIN" build` A8 row and the A10 row |
| delete arm 2 | the `CARGO=/p release-plz update` row |
| relax arm 1's name predicate to any name | the three false-positive fixtures |
| relax arm 2's name from exact `CARGO` to "mentions cargo" | the `CARGO_NET_OFFLINE=true tool update` fixture |
| consume arm 2's trailing word instead of looking ahead | the consecutive-prefix fixture |

The measured result of each goes into this section before the branch merges.

### 5.3 Corpus differential (AC 3, AC 4)

Run `cargo_moon_parity.py` against the real graph before and after, and diff three things:

1. the full row set;
2. A8's `matched` size;
3. A10's `in_scope` size (58 today).

Then run `ci/affected-graph/run.sh` for expected-set movement. The prediction is no movement in
any of the four. Movement gets explained here, never re-baselined.

## 6. Documentation

* SMA-599's spec L10 is rewritten to record this decision and point here.
* SMA-599's spec L11 gains a pointer: L10 is closed for the variable shape, while L11's
  subcommand shape (`cargo llvm-cov`, `insta`, `udeps`, `bloat`) stays open.
* `ci/affected-graph/README.md`: the A8 and A10 bullets gain the two arms.
* `CLAUDE.md`: one sentence in the A8/A10 paragraph.

## 7. Limitations and residuals

**R1 — the variable's NAME is the whole test.** `BIN="$(command -v cargo)"; "$BIN" build`
stays invisible. Resolving the assignment instead was measured and rejected: `VAR_ASSIGN_RE`
captures `$(` as the value of `CARGO_BIN="$( command -v cargo … )"`, so value resolution does
not reach the real shape, and a value predicate would fire on the three variables in
`ci/actionlint/run.sh` whose literal values mention cargo — the file SMA-599 L4 already names
as one edit from a spurious row.

**R2 — `export CARGO=…` separated from its tool is invisible.** Arm 2 needs the prefix and the
command on one segment. An `export` on one line and the tool on another needs environment
tracking across a script, which is out of scope.

**R3 — the environment arm covers `CARGO` only.** A tool reading its own variable for a cargo
path (`FOO_CARGO_BIN`) is not covered.

**R4 — SMA-599 L2 still holds.** `release-plz.sh` is sourced, not referenced, so the one real
site stays unfollowed (M8). This is forward cover.

**R5 — arm 1 reports prose the way the literal arm does.** The conservative rule does not strip
strings (SMA-599 L8), so an `echo` naming a cargo-named variable before a verb reports. No such
line exists today. The failure direction is a loud row and a waiver, never a silent pass.

## 8. Acceptance criteria

| AC | Where |
|---|---|
| 1 — the decision is explicit and recorded | §3 |
| 2 — mutation-proven A8 and A10 rows | §5.1, §5.2 |
| 3 — corpus re-measured, movement explained | §2, §5.3 |
| 4 — `ci/affected-graph/run.sh` reports no expected-set movement | §5.3 |
| 5 — SMA-599's L10 updated | §6 |
