# SMA-599 — assert the cargo-invocation invariants across the Moon graph

Status: design approved 2026-08-29. Absorbs SMA-552.

## 1. Problem

Two repo-wide rules govern every Moon task that runs cargo, and until SMA-601 neither
was enforced by anything:

* **Half A** — a task that runs cargo with cwd inside `rs/` must key on
  `rs/.cargo/config.toml` (SMA-594). Cargo discovers that file by walking up from the
  working directory, so it influences every such task's output.
* **Half B** — a task that resolves the dependency graph must pass `--locked`
  (SMA-534, SMA-552). An unlocked invocation rewrites an inconsistent `rs/Cargo.lock`
  in place, so every later gate reads a resolution the PR never shipped.

Both rules need the same derived set: every Moon task whose resolved invocation
reaches cargo. The issue asks for that derivation to be built once and consumed twice.

### 1.1 What SMA-601 already delivered

SMA-601 merged on 2026-08-29 as `003a4c5`, one day after SMA-599 was filed, and it
changes this ticket materially. It built the derivation (`check_cargo_locked`'s
matched set) and it implemented Half B: `--locked` is on the shared
`build`/`build-release`/`test`/`lint` tasks in `.moon/tasks/rust.yml`, asserted
generically by A8 with an anti-vacuity floor (`REQUIRED_LOCKED_TASKS`) and a reasoned
allowlist (`ALLOW_UNLOCKED_CARGO`).

So Half B's *behaviour* is done. What Half B still owes is the **record** — AC 5 and
AC 6 ask for the decision and its proof to be written down where the next person will
find them. Section 6 does that.

Half A is untouched and is the substance of this change.

### 1.2 Half A's current state — measured, not counted from the issue

The issue states 61 tasks selected, 16 asserted. Both numbers are about a different
question than the one A9 answers, so this design re-measured the set it actually
governs. Against the graph at `64c9624`:

| Quantity | Count |
| -- | -- |
| Moon tasks whose resolved blob reaches cargo (A8's derived set) | 60 |
| …of which already declare `rs/.cargo/config.toml` | 58 |
| …of which do not | 2 |

The two that do not are `repo:deny` (runs cargo from the repo root, so the file is not
on cargo's upward walk) and `repo:wasm-getrandom-free` (the documented `cargo tree`
exclusion). Neither is a defect.

This table counts the **blob-matched** set only. Script-following (§3.2) adds two more
tasks that no blob match reaches: `repo:publish-metadata`, which already declares the
file, and `repo:version-lockstep`, which does not and correctly should not (§2.3).

The issue's "39 unasserted `build`/`build-release`/`test` declarations" are really
**three lines** in `.moon/tasks/rust.yml`, inherited by thirteen crates. Deleting one
line removes the input from thirteen tasks at once. That is what makes A9 cheap: it
reads moon's RESOLVED inputs, so inheritance is transparent to it.

### 1.3 A hole the issue does not mention

`repo:publish-metadata` runs `cargo package --list --locked` and
`cargo publish --dry-run --locked` with cwd inside `rs/`
(`ci/publish-metadata/run.sh:89,1654`). Its Moon blob is
`bash ci/publish-metadata/run.sh`, so the cargo calls live inside the script and
**A8 never sees it**. Any gate reaching cargo through a `ci/**/run.sh` wrapper is
silently outside both rules.

That is a live gap in the gate SMA-601 shipped, not only in the one being added.
Closing it is in scope for this change (Section 6, D1).

## 2. Measurements

Every number below was measured on this branch, from the worktree at `64c9624`.

### 2.1 The staleness experiment (AC 6)

`rs/Cargo.lock` was made genuinely stale by adding `itoa = "1"` to
`rs/crates/bindings/paigasus-wasm/Cargo.toml`. The authoritative check for a rewrite is
`git status --short rs/Cargo.lock`, never a grep of the lockfile — `itoa` is already
present as a transitive dependency, so a grep would mislead.

| # | Invocation | exit | `rs/Cargo.lock` after |
| -- | -- | -- | -- |
| A | `cargo metadata --format-version 1 --no-deps --locked` | 0 | clean |
| B | `cargo metadata --format-version 1 --no-deps` | 0 | **clean — not rewritten** |
| C | `cargo metadata --format-version 1` (deps, unlocked) | 0 | **REWRITTEN** |

C proves the lock was genuinely stale and that the repair mechanism fires. B proves
`--no-deps` never resolves, so it cannot repair a lock. A proves that adding `--locked`
to a `--no-deps` call is **inert**: it passes on a stale lock and asserts nothing.

This is why A8 must treat `cargo metadata --no-deps` as non-resolving rather than
demand a flag on it. Demanding the flag would be cargo-cult compliance that a future
reader would reasonably mistake for a real guarantee.

`ci/cargo-lock-integrity/run.sh:47` runs `cargo metadata --locked` **with** deps, so
SMA-601's own gate does resolve and does bite. It was checked and needs no change.

### 2.2 The line classifier

Three Moon tasks invoke a `ci/**/*.sh`: `repo:publish-metadata`,
`repo:version-lockstep`, `repo:workflow-credentials`. The A8 source comment records a
measured 45 matches / ~14 real on a repo-wide text scan, which is why a naive scan was
rejected there. Scoping the scan to the script a task actually invokes, and layering
the filters, removes the noise entirely:

| Script | raw | + comments stripped | + heredocs skipped | + strings stripped | rows reported |
| -- | -- | -- | -- | -- | -- |
| `ci/publish-metadata/run.sh` | 18 | 8 | 4 | 3 | **0** |
| `ci/version-lockstep/run.sh` | 4 | 3 | 3 | 2 | **2** |
| `ci/workflow-credentials/run.sh` | 0 | 0 | 0 | 0 | **0** |

Zero false positives. The filters, and what each one earns:

1. **Heredoc bodies skipped.** `ci/publish-metadata/run.sh:179` is
   `print(f"FATAL: cannot read cargo metadata JSON: {exc}", ...)` — Python inside a
   heredoc. This is the measured false positive the A8 comment warns about.
2. **`#` comments stripped.** The rule `check_dockerfile_locked` already uses.
3. **Quoted string literals stripped.** A cargo *invocation* is never inside a quoted
   string, but a diagnostic is: `|| die_infra "FATAL: \`cargo metadata\` failed in
   $RS_DIR"` at `:1664`. Stripping `"..."` and `'...'` leaves `|| die_infra`, which
   matches nothing. It does not damage real invocations — stripping `"$pkg"` from
   `cargo package --list --locked -p "$pkg"` leaves the verb and the flag intact.
4. **`cargo metadata --no-deps` marked non-resolving**, per §2.1.

The two rows that remain are `ci/version-lockstep/run.sh:583-584`, both genuine
`cargo update -w` invocations.

### 2.3 `cargo update -w` is unreachable from the Moon task

`repo:version-lockstep`'s script runs `run.sh --self-test`, `run.sh
--negative-control` and `run.sh` bare. The `cargo update -w` pair lives inside
`run_write()`, reached only by `run.sh --write`, which no Moon task passes. Check mode
invokes no cargo at all.

So a whole-file scan flags a line the task never executes, and whose purpose is
*deliberately* to rewrite the lock. `--locked` there would defeat the function. This is
the path-insensitivity limitation recorded in §7.

`repo:version-lockstep` also does not declare `rs/.cargo/config.toml`, and correctly so
for the same reason: as the Moon task runs it, no cargo executes, so the file cannot
influence its output.

## 3. Design

All of it lands in `ci/affected-graph/cargo_moon_parity.py`, inside the existing
`repo:affected-smoke` gate. No new `repo:*` task, therefore none of the five registry
obligations a new gate carries.

### 3.1 `script_cargo_lines(path)` — the shared classifier

Returns `(lineno, code, raw_line, resolves)` for each cargo invocation in a shell
script, applying the four filters of §2.2 in order. `resolves` is `False` for
`cargo metadata --no-deps`.

Shared by A8 and A9 deliberately. A classifier that A8 uses and A9 does not — or the
reverse — is a hole neither check can see, the same argument `derive_ffi_tasks` already
makes for A5 and A7.

### 3.2 `derive_cargo_tasks(projects, root)` — the shared derivation (AC 1)

Every `<pid>:<task>` reaching cargo, by either route:

* the resolved `command` + `args` + `script` blob matches `CARGO_INVOCATION_RE` or an
  FFI marker (today's A8 rule, unchanged); or
* the blob invokes a `ci/**/*.sh` whose `script_cargo_lines` is non-empty.

Anti-vacuity floor: `REQUIRED_LOCKED_TASKS` (existing four) plus
`repo:publish-metadata`. That last member is load-bearing — it is reachable *only*
through script-following, so if the following silently stops working, the floor reds
instead of the derivation degrading to a vacuous PASS.

### 3.3 A9 — `rs/.cargo/config.toml` inputs (Half A, AC 2–4)

**Scope.** A task is in scope when it reaches cargo **and** its cwd resolves inside
`rs/`:

* the project's `source_dir` is under `rs/` (the thirteen crates), or
* a cd-into-`rs` appears in the blob or the followed script, after one round of literal
  variable substitution. The real shapes are `cd rs` (`repo:wasm-getrandom-free`),
  `( cd rs && … )` (`repo:parity-corpus-drift`), `--cwd ../../../rs/…`
  (`paigasus-kernel-ts:build`), and `RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"`
  (`ci/publish-metadata/run.sh:89,1654`).

Deriving cwd rather than default-denying every cargo task means `repo:deny` falls out
**structurally**: its cwd is the repo root and `--manifest-path` does not move cargo's
config walk. If it ever gained a `cd rs`, it returns to scope on its own. A waiver would
have kept it exempt — the wrong direction for a default-deny gate.

**Verdict.** An in-scope task must declare `rs/.cargo/config.toml` among its resolved
inputs, or carry an `ALLOW_MISSING_CARGO_CONFIG` entry with a non-empty reason (the
idiom `ALLOW_NO_CARGO_BACKING` and `ALLOW_UNLOCKED_CARGO` already use — an empty reason
is itself a violation).

**Floor.** `REQUIRED_CARGO_CONFIG_TASKS` names tasks that must be *in scope*, so a
degraded cwd derivation reds rather than silently emptying the set:
`paigasus-kernel-rs:build`, `paigasus-iam-rs:test`, `paigasus-kernel-ts:build`,
`repo:parity-corpus-drift`, `repo:publish-metadata`.

A default-deny gate has a second vacuity mode the FFI floors do not: an allowlist that
grows to swallow the derived set. The floor therefore asserts its members are in scope
**and not allowlisted**.

**Waivers.**

| Task | Reason |
| -- | -- |
| `repo:wasm-getrandom-free` | `cargo tree` resolves the dependency graph and never compiles or links, so the two `*-apple-darwin` rustflags cannot change what it prints (AC 4, encoded rather than removed) |
| `repo:version-lockstep` | its only cargo call is `cargo update -w` inside `run_write()`, behind `--write`, which no Moon task passes — measured §2.3 |

### 3.4 A8 widening

`check_cargo_locked` additionally reports script-derived rows through
`script_cargo_lines`. A row fires when a line `resolves` and lacks `--locked`.

`ALLOW_UNLOCKED_CARGO_SCRIPT` is keyed by `(script_path, lineno, stripped line text)`,
following `COE_SKIP`'s precedent: keying on both the number and the text means a
shifted entry stops matching rather than silently absorbing a different occurrence that
lands on the vacated line. One entry per line, so two for `version-lockstep`.

A task-level waiver was rejected: it would exempt `repo:publish-metadata`'s genuine
`--locked` lines too, and would silently cover a future unlocked line added anywhere in
the same script.

### 3.5 Reachability

`repo:affected-smoke` gains `ci/**/*` in its `inputs`. Without it the script pins are
real but unreachable — the gate stays green on exactly the PR that breaks them, the
SMA-553 failure class.

The three existing narrow globs (`ci/affected-graph/**/*`, `ci/actionlint/**/*`,
`ci/release-parity/**/*`) are **kept**, not replaced. Check 8e in `ci/actionlint/run.sh`
asserts containment over a twenty-entry list and floors it at `-ge 20`; replacing three
entries with one would take it to eighteen and force loosening that floor. Adding is
free, and `T_AFFECTED_SMOKE_REQUIRED_INPUTS` grows by the one new entry.

## 4. Registry obligations

| Obligation | Needed? |
| -- | -- |
| `ci.yml` `T=(…)` array | No — no new `repo:*` task |
| CLAUDE.md marker-delimited command | No — same reason |
| `SELF_SCHEDULED_GATES` | No — `repo:affected-smoke`'s invocation lines are unchanged |
| `SELF_TASK_EXPECTED_GLOBS` | No — affected-smoke's globs are pinned by check 8e, deliberately not in `ci_targets.py` |
| `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (check 8e) | **Yes** — add `ci/**/*` |
| `EXPECTED_FINDING_KEYS` | **Yes** — add `a9` |
| `SELF_TEST_COUNT` in `ci/actionlint/run.sh` | No — that counts `ci/actionlint/run.sh`'s own `*_self_test` tables, a different file |

## 5. Testing

`cargo_moon_parity.py --self-test` gains fixture tables for:

1. `script_cargo_lines` — one fixture per filter: a heredoc body, a `#` comment, a
   cargo verb inside a double-quoted diagnostic, a `--no-deps` metadata call, and a
   real invocation that must survive all four.
2. A9 — an in-scope task missing the input reports; one declaring it does not; a waiver
   with a reason clears it; a waiver with an empty reason is itself a row; a floor
   member that drops out of scope reports; a floor member that becomes allowlisted
   reports.
3. The widened A8 — a script-derived unlocked resolving line reports; a `--locked` one
   does not; a `--no-deps` one does not; a shifted waiver stops matching.
4. `derive_cargo_tasks` — emptying it fails via the floor (AC 1).

Mutation proofs on the real tree, each restored afterwards (AC 3):

* remove `rs/.cargo/config.toml` from `repo:observability-drift`'s `inputs` → A9 names
  that task → restore;
* remove the same line from `.moon/tasks/rust.yml`'s `test` task → A9 names thirteen
  crates → restore;
* drop `--locked` from `ci/publish-metadata/run.sh:742` → widened A8 names the script
  and line → restore.

Restore by reverting the marked edit, never by `git checkout --` on the file: that
would also discard the uncommitted fix under test, and the next mutation would then run
against original code and print a meaningless result.

`ci/affected-graph/run.sh` must report no expected-set movement (AC 8); any movement is
explained in the PR.

## 6. Decisions and rejected alternatives

**D1 — widen A8 to follow gate scripts, here.** Chosen over deferring it to a follow-up
issue. The measured yield today is zero defects, so the case is forward-looking: it is
the argument every derived set in this gate already makes, that a new invocation is
covered on the day it is added. The cost is honest and bounded — one waiver (two lines)
and the §7 path-insensitivity limitation. Recorded because the evidence was gathered
*after* the choice was first made and does not support it as strongly as expected; a
future reader should weigh it knowing that.

**D2 — derive cwd rather than default-deny every cargo task.** Rejected alternative:
reuse A8's set verbatim and waive `repo:deny`. That records a *fact* about cargo's
config discovery as if it were a *decision*, and the waiver would keep the task exempt
even if it later gained a `cd rs`. See §3.3.

**D3 — keep the `cargo tree` exclusion, encoded.** AC 4 permits either encoding it or
widening the rule to drop it. Encoding was chosen: the over-trigger is small but the
exclusion is a real property worth stating. The AC's own worry — that the exclusion
rests on the file's current *content*, so a future `[source]` or `[build]` key would
change what `cargo tree` resolves — is recorded in §7 as a residual, not silently
dropped.

**D4 — `--no-deps` handled structurally, not by waiver.** Measurement A shows a
`--locked` there asserts nothing. A waiver would have implied there was something to
waive.

**D5 — waive A8's script rows by line, not by task.** See §3.4.

**D6 — Half B's decision, recorded (AC 5).** SMA-601 took **option 1** (`--locked` on
the shared Rust tasks, closing root cause 2 for all thirteen crates) **and option 2**
(a `cargo metadata --locked` preflight, as an unconditional `ci.yml` step placed before
`moon ci` rather than a `repo:*` task, because a gate reading the lock from the working
tree inside `moon ci` races the repair). Option 3 was not taken. The local-ergonomics
cost option 1 carries is real and accepted: a working tree whose manifest and lock
disagree now reds every Rust task rather than being repaired silently. AC 7 does not
apply, because the behavioural change was not declined.

**D7 — function-scoped waivers rejected.** Keying A8's script waiver to the enclosing
shell function (`run_write`) would be more stable than line numbers and would express
the real reason. It needs brace-matched shell function parsing inside a gate that must
stay cheap and `toolchain: 'system'`. Not worth it for one waiver; revisit if a second
appears.

**D8 — path-aware scanning rejected.** Modelling which code path a task's arguments
reach would exclude `cargo update -w` structurally instead of waiving it. That is shell
static analysis — `case`/`if` dispatch on `"$1"` — and is out of proportion here.

## 7. Limitations and residuals

**L1 — the script scan is path-insensitive.** It reports a cargo line the task's
arguments never reach (§2.3). Today that costs one waiver. A future gate whose cargo
call sits behind an unused flag will need the same treatment, and a reviewer must check
reachability by hand rather than trusting the row.

**L2 — script-following is one level deep.** A task invoking a script that invokes
*another* script is not followed. No such case exists today. The floor does not catch
it, because the outer script would still match if it had any cargo line of its own.

**L3 — the `cargo tree` exclusion still rests on file content.** If
`rs/.cargo/config.toml` ever gains a `[source]` replacement or a `[build]` key, `cargo
tree`'s output becomes sensitive to it and the D3 waiver silently becomes wrong.
Nothing detects that. Widening the rule (dropping the waiver) is the fix if that day
comes.

**L4 — A9 proves declared inputs are *needed*, never that needed ones are *declared*,
outside the cargo rule.** It closes the `rs/.cargo/config.toml` case specifically. The
general CLAUDE.md warning stands: a future `repo:*` task can omit some other input it
reads and nothing reds.

**L5 — the cwd derivation resolves one round of literal variable substitution.** A
script computing its cargo cwd dynamically would be missed. The floor
(`REQUIRED_CARGO_CONFIG_TASKS`) is the only control, and it names today's shapes, not
tomorrow's.
