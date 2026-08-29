# SMA-599 — assert the cargo-invocation invariants across the Moon graph

Status: revised 2026-08-29 after an adversarial spec review. Absorbs SMA-552.
All measurements on cargo 1.95.0, moon 2.5.3, at `64c9624`.

## 1. Problem

Two repo-wide rules govern Moon tasks that run cargo, and until SMA-601 neither was
enforced:

* **Half A** — a task whose cargo invocation is *influenced by* `rs/.cargo/config.toml`
  must key on that file (SMA-594). Cargo discovers it by walking up from the working
  directory.
* **Half B** — a task that resolves the dependency graph must pass `--locked`
  (SMA-534, SMA-552). An unlocked invocation rewrites an inconsistent `rs/Cargo.lock`
  in place, so every later gate reads a resolution the PR never shipped.

The two rules ask different questions and therefore need **different verb predicates**
over a **shared derivation**. Conflating them was the first draft's central error; see
§3.3 and D9.

### 1.1 The rule statement, corrected

SMA-594 and the first draft of this spec both stated Half A as *"a task that runs cargo
with cwd inside `rs/` keys on `rs/.cargo/config.toml`."* **That statement is false as
written**, and thirteen tasks already violate it: `cargo fmt --check` runs with cwd
inside `rs/` and deliberately does not declare the file, because it neither compiles
nor links, so rustflags cannot change its result (`.moon/tasks/rust.yml:125-149`).

The operative rule is the one CLAUDE.md actually argues — **influence**, not location:

> A task is in scope when its cargo subcommand's OUTPUT can be changed by
> `rs/.cargo/config.toml`, and its cwd is inside `rs/`.

`fmt` is excluded because it formats; `tree` and `metadata` because they resolve
without compiling; `deny` and `machete` because they are third-party static scans.
§3.3 encodes this as a named verb list rather than leaving it to coincide with a list
built for the lock question.

### 1.2 What SMA-601 already delivered

SMA-601 merged as `003a4c5`, one day after SMA-599 was filed. It built the derivation
(`check_cargo_locked`'s matched set) and implemented Half B: `--locked` on the shared
`build`/`build-release`/`test`/`lint` tasks, asserted generically by A8 with a floor
(`REQUIRED_LOCKED_TASKS`) and a reasoned allowlist (`ALLOW_UNLOCKED_CARGO`).

Half B's *behaviour* is therefore done. What it still owes is the **record** — AC 5 and
AC 6. D6 supplies it.

### 1.3 Half A's current state — measured

| Quantity | Count |
| -- | -- |
| Moon tasks whose resolved blob reaches cargo (A8's derived set) | 60 |
| …already declaring `rs/.cargo/config.toml` | 58 |
| …not declaring it | 2 |

The two are `repo:deny` (cwd is the repo root — §2.3) and `repo:wasm-getrandom-free`
(`cargo tree`). Neither is a defect, and under §3.3's verb predicate both fall out
structurally rather than by waiver.

This table counts the **blob-matched** set only. Script-following (§3.2) adds
`repo:publish-metadata`, which already declares the file, and `repo:version-lockstep`,
which does not and correctly should not (§2.4).

The issue's "39 unasserted `build`/`build-release`/`test` declarations" are really
**three lines** in `.moon/tasks/rust.yml`, inherited by thirteen crates. A9 reads
moon's RESOLVED inputs, so inheritance is transparent to it and deleting one line reds
thirteen tasks at once.

### 1.4 A hole the issue does not mention

`repo:publish-metadata` runs `cargo package --list --locked` and `cargo publish
--dry-run --locked` with cwd inside `rs/` (`ci/publish-metadata/run.sh:89,1654`). Its
Moon blob is `bash ci/publish-metadata/run.sh`, so **A8 never sees it**. Any gate
reaching cargo through a script is outside both rules. Closing that is in scope (D1).

## 2. Measurements

### 2.1 The staleness experiment (AC 6)

`rs/Cargo.lock` was made genuinely stale by adding `itoa = "1"` to
`rs/crates/bindings/paigasus-wasm/Cargo.toml`. The authoritative check for a rewrite is
`git status --short rs/Cargo.lock`, never a grep — `itoa` is already present
transitively, so a grep would mislead.

| # | Invocation | exit | `rs/Cargo.lock` after |
| -- | -- | -- | -- |
| A | `cargo metadata --format-version 1 --no-deps --locked` | 0 | clean |
| B | `cargo metadata --format-version 1 --no-deps` | 0 | **clean — not rewritten** |
| C | `cargo metadata --format-version 1` (deps, unlocked) | 0 | **REWRITTEN** |

C proves the lock was genuinely stale and the repair mechanism fires. B proves
`--no-deps` does not resolve. A proves `--locked` on a `--no-deps` call is **inert** —
it passes on a stale lock and asserts nothing, so demanding it would be cargo-cult
compliance a later reader would mistake for a guarantee.

**Scope of this measurement, stated because D4 generalises it:** one staleness shape
(a member manifest gaining a dependency), one cargo version (1.95.0), with a lockfile
present. It was *not* measured with no `Cargo.lock` at all, nor with a new
`[workspace] members` entry. §7 L6 records the residual.

`ci/cargo-lock-integrity/run.sh:47` runs `cargo metadata --locked` **with** deps, so
SMA-601's own gate does resolve and does bite. Checked; no change needed.

### 2.2 The line classifier — re-measured over the full scope

**The first draft measured 3 of 8 scripts.** Its extractor required a `bash `/`sh `
prefix, but the live invocation shape is often bare (`ci/release-parity/run.sh
--ecosystem semantic-release`). Corrected, **ten Moon tasks invoke eight distinct
scripts**:

| Script | Invoked by |
| -- | -- |
| `ci/actionlint/run.sh` | `repo:actionlint` |
| `ci/affected-graph/run.sh` | `repo:affected-smoke` |
| `ci/next-env/run.sh` | `repo:next-env-drift` |
| `ci/osv/run.sh` | `repo:osv` |
| `ci/publish-metadata/run.sh` | `repo:publish-metadata` |
| `ci/release-parity/run.sh` | `repo:release-parity`, `-py`, `-ts` |
| `ci/version-lockstep/run.sh` | `repo:version-lockstep` |
| `ci/workflow-credentials/run.sh` | `repo:workflow-credentials` |

Re-measured across all eight, with the corrected filter order of §3.1:

| Script | cargo lines surviving filters | rows reported |
| -- | -- | -- |
| `ci/publish-metadata/run.sh` | 3 | **0** — two already `--locked`, one `--no-deps` |
| `ci/version-lockstep/run.sh` | 2 | **2** — both `cargo update -w` |
| the other six | 0 | 0 |

**`ci/actionlint/run.sh` carries eight raw cargo-verb matches** (`:2136, :2153, :2178,
:3692, :3714, :3716, :5075, :5094`) and `ci/affected-graph/run.sh` one (`:318`). Every
one is a comment or a quoted string — several are check 8f's *pins of another gate's
cargo line*, and `:3714` deliberately constructs an **unlocked** cargo string as a
mutation fixture. All nine are correctly filtered today, but they survive on the
filters, not on absence. D9 addresses the fragility that creates.

### 2.3 `--manifest-path` does not move cargo's config walk (D2's premise)

The first draft asserted this. Measured: `rs/.cargo/config.toml` was temporarily
replaced with malformed TOML.

| cwd | command | rc |
| -- | -- | -- |
| `rs/` | `cargo metadata --no-deps` (control) | **101** — the broken file IS noticed |
| repo root | `cargo metadata --no-deps --manifest-path rs/Cargo.toml` | **0** |

The control is what makes the test meaningful: without it, rc 0 could equally mean the
malformed file was never bad enough to matter. So `repo:deny`
(`cargo deny --locked --manifest-path rs/Cargo.toml check`) and `repo:machete`
(`cargo machete rs`) genuinely do not read the file, and §3.3 excludes them
structurally.

### 2.4 `cargo update -w` is unreachable from the Moon task

`repo:version-lockstep`'s Moon script runs `run.sh --self-test`, `--negative-control`
and bare. The `cargo update -w` pair lives in `run_write()`, reached only by
`run.sh --write`, which no Moon task passes; check mode invokes no cargo at all.

So a whole-file scan flags a line the task never executes, whose purpose is
*deliberately* to rewrite the lock. This is the path-insensitivity limitation (L1), and
it is why that case is an A8 waiver rather than a structural exclusion.

## 3. Design

All of it lands in `ci/affected-graph/cargo_moon_parity.py`, inside the existing
`repo:affected-smoke` gate. No new `repo:*` task.

### 3.1 `script_cargo_lines(path)` — the shared classifier

Returns `(first_lineno, code, raw, resolves)` per cargo invocation. Order is
load-bearing and pinned by fixtures:

1. **Join backslash continuations** into one logical line, reported against the FIRST
   physical line number. Required: `ci/version-lockstep/run.sh:583-584` and
   `ci/publish-metadata/run.sh:1663` all end in `\`. Without joining, reflowing
   `cargo build \` / `  --locked` yields a false row, and `cargo metadata \` /
   `  --no-deps` is misread as resolving.
2. **Skip heredoc bodies.** Accepted forms: `<<DELIM`, `<<'DELIM'`, `<<"DELIM"`,
   `<<-DELIM` (terminator may be tab-indented). A heredoc still open at EOF raises
   `MoonOutputError` (rc 2) — otherwise the scanner silently skips the rest of the file
   and reports zero rows, the same "infrastructure, never a silent pass" contract
   `check_dockerfile_locked` uses (`cargo_moon_parity.py:484-488`).
3. **Strip quoted string literals, THEN `#` comments — in that order.** The first draft
   had these reversed, which deletes a real invocation:
   `echo "a # b" && cargo build` truncates at the `#` inside the string and the
   `cargo build` vanishes. That is a **false negative**, the fatal direction for a
   default-deny gate. `check_dockerfile_locked` uses the naive order legitimately, on
   the stated premise that the Dockerfile "holds one cargo line and no prose"; a general
   shell scanner has no such premise. On today's corpus both orders agree — the fixture,
   not the corpus, is what holds this.
4. **Report an unclassifiable line rather than passing it over.** The premise "a cargo
   invocation is never inside a quoted string" is **false**: `bash -c "cargo build"`,
   `eval "cargo build"`, `sh -c '…'` are all invocations inside quotes. When a stripped
   string contained a cargo verb and the surviving code holds `bash -c`, `sh -c` or
   `eval`, emit an infra-shaped row ("cannot classify") instead of silence. No live
   instance exists today (the three `bash -c`/`eval` hits in `ci/actionlint/run.sh` are
   a grep pattern, a label assignment and prose), so this is forward cover.
5. **Command-scope both flag tests.** `--locked` and `--no-deps` are each evaluated
   within the `[^;&|]` segment holding the cargo verb. The first draft tested `--locked`
   per LINE and `--no-deps` per COMMAND, so `cargo build && cargo metadata --locked`
   would have passed with `cargo build` unlocked — the per-blob form of which
   `ci/affected-graph/README.md:174-177` already records as an open residual.

### 3.2 `derive_cargo_tasks(projects, root)` — the shared derivation (AC 1)

Returns `{target: kind}` where kind is `literal`, `wrapper` or `script`. **It must not
return a flat set.** A8 records, as measured, that a wrapper match and a literal match
cannot be treated alike — `paigasus-kernel-ts:build` carries a `--locked` belonging to a
different command (`cargo_moon_parity.py:407-415`, self-test A8-f at `:1294-1314`) — so
a flat set would silently reintroduce the measured-vacuous form.

Script paths are extracted allowing an optional `bash`/`sh` prefix, covering the three
live shapes (`bash ci/x/run.sh --flag`, bare `ci/x/run.sh`, `ci/x/run.sh --ecosystem y`).
A blob naming a `ci/**/*.sh` that does not resolve to a readable file raises
`MoonOutputError`, so a rename cannot silently empty the set.

Floor: `REQUIRED_LOCKED_TASKS` plus `repo:publish-metadata` and
`repo:version-lockstep` — the two reachable *only* through script-following, so a broken
follower reds instead of degrading to a vacuous PASS. A self-test asserts the three
kinds remain distinguishable at the derivation boundary, not only inside A8.

### 3.3 A9 — `rs/.cargo/config.toml` inputs (Half A, AC 2–4)

**Its own verb predicate.** `CONFIG_SENSITIVE_VERBS` names subcommands that compile or
link, and is deliberately NOT `LOCK_RESOLVING_VERBS`: `bench`, `build`, `check`,
`clippy`, `doc`, `fix`, `nextest`, `package`, `publish`, `run`, `test`. Excluded, each
with its reason recorded beside the constant: `fmt` (formats only), `tree` and
`metadata` (resolve without compiling), `deny` and `machete` (third-party static scans),
`add`/`remove`/`update`/`generate-lockfile`/`vendor`/`fetch` (lock manipulation).

Two consequences, both improvements over the first draft:

* the thirteen `fmt` tasks are excluded **by a stated rule** instead of by coincidence
  with a list written for the lock question; and
* `repo:wasm-getrandom-free` (`cargo tree`) and `repo:version-lockstep`
  (`cargo update`) fall out **structurally**, so A9 ships with **zero waivers**.

AC 4 asks that the `cargo tree` exclusion be "encoded with a stated reason or removed".
Encoding it in the verb predicate satisfies that more durably than a per-task waiver:
it covers a future `cargo tree` gate on day one.

**cwd derivation.** Reads the **raw** line, never the quote-stripped code — otherwise
`RS_DIR="$REPO_ROOT/rs"` becomes `RS_DIR=` and the shape dies. Recognised tokens are
exactly `cd`, `pushd` and `--cwd`, each followed by a path that resolves under `rs/`
after one round of literal `VAR=<literal>` substitution (`$VAR` and `${VAR}` both).
Live shapes covered: `cd rs`, `( cd rs && … )`, `cd "$REPO_ROOT/rs"`,
`RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"`, `--cwd ../../../rs/…`.

A bare `rs`-containing *argument* must NOT confer scope — `--manifest-path
rs/Cargo.toml` and `cargo machete rs` are both fixtures asserting exactly that, because
a loose "blob mentions `rs/`" test would destroy D2's structural exclusion and
`repo:deny` is in the examined set.

**Verdict.** An in-scope task must declare `rs/.cargo/config.toml` in `inputFiles` or
`inputGlobs` (both are read; an absent key is a violation, never a skip — the contract
A4/A5/A6/A7 share). Measured: `moon.yml:239`'s `rs/.cargo/config.toml` and
`.moon/tasks/rust.yml:46`'s `/rs/.cargo/config.toml` both resolve to the same
slash-free `rs/.cargo/config.toml`, which is why all 58 declaring tasks match verbatim.

`ALLOW_MISSING_CARGO_CONFIG` exists but is **empty**, like `ALLOW_OVER_APPROXIMATION`.
An entry requires a non-empty reason, and an entry matching no task is itself a row —
the stale-skip idiom `ci/actionlint/run.sh:2376-2383` uses.

**Floors.** `REQUIRED_CARGO_CONFIG_TASKS` (`paigasus-kernel-rs:build`,
`paigasus-iam-rs:test`, `paigasus-kernel-ts:build`, `repo:parity-corpus-drift`,
`repo:publish-metadata`) must be in scope **and not allowlisted** — a default-deny gate's
second vacuity mode is an allowlist that grows to swallow the set. A separate
`REQUIRED_CWD_SHAPES` floor asserts each of the four cd forms still resolves for at
least one task, because the bare `cd rs` shape's only exemplar
(`repo:wasm-getrandom-free`, `moon.yml:326`) is now out of scope on its verb and so
cannot be a floor member.

### 3.4 A8 widening

`check_cargo_locked` gains script-derived rows via the shared classifier, keeping its
literal/wrapper distinction and adding `script` as a third kind. A row fires when a
command segment resolves and lacks `--locked`.

`ALLOW_UNLOCKED_CARGO_SCRIPT` is keyed by `(script_path, stripped text)` **plus an
assertion that the text occurs exactly once in the file** — not by line number. Both
`version-lockstep` lines are textually distinct and unique, and a line-number key would
red `repo:affected-smoke` on any unrelated insertion above line 583 in a 620-line file
that SMA-576 and SMA-579 both edited. A stale entry matching nothing is a row.

The waiver's reason — "behind `--write`, which no Moon task passes" — is itself
asserted: a check confirms `repo:version-lockstep`'s resolved blob contains no
`--write`. Without it, adding `--write` to `moon.yml:588-592` would make the waiver
silently wrong.

### 3.5 Reachability

`repo:affected-smoke` gains `ci/**/*` in its `inputs`. Without it the script pins are
real but unreachable — green on exactly the PR that breaks them (the SMA-553 class).

The **four** existing narrow globs (`ci/affected-graph/**/*`, `ci/actionlint/**/*`,
`ci/release-parity/**/*`, `ci/workflow-credentials/**/*`) are **kept**, not replaced.
Check 8e asserts containment and floors the array at `-ge 20`;
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` holds **21** entries today
(`ci/actionlint/run.sh:2100-2122`) and the task declares 22 resolved inputs. Replacing
four with one gives 18 and would force loosening that floor. Adding gives 22 and 23.

## 4. Registry and documentation obligations

| Obligation | Needed? |
| -- | -- |
| `ci.yml` `T=(…)` array | No — no new `repo:*` task |
| CLAUDE.md marker-delimited command | No — same reason |
| `SELF_SCHEDULED_GATES` | No — affected-smoke's invocation lines unchanged |
| `SELF_TASK_EXPECTED_GLOBS` | No — affected-smoke's globs are pinned by check 8e |
| `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (check 8e) | **Yes** — add `ci/**/*` |
| `EXPECTED_FINDING_KEYS` | **Yes** — add `a9` |
| `SELF_TEST_COUNT` in `ci/actionlint/run.sh` | No — counts that file's own tables |
| **CLAUDE.md prose** | **Yes** — three sentences become false (below) |
| **`ci/affected-graph/README.md`** | **Yes** — add the A9 bullet; `:173-177` becomes false |
| `cargo_moon_parity.py:1673` PASS string | **Yes** — "all eight assertions" → nine |

CLAUDE.md's `rs/.cargo/config.toml` bullet currently states *"Only 16 of those 61
declarations are asserted"*, *"delete one and CI stays green"*, and the follow-on bullet
*"Nothing enforces that one rule."* A9 makes all three false. **No gate asserts this
prose**, so it must be corrected by hand in the same PR. `README.md:173-177` states that
a cargo call inside a `.sh` is outside A8's derived set — also now false.

## 5. Testing

`--self-test` fixture tables for:

1. **`script_cargo_lines`**, one per filter and one per ordering hazard:
   `echo "a # b" && cargo build` must report (pins filter order);
   `cargo build \` + `--locked` must NOT report (pins continuation joining);
   `cargo build && cargo metadata --locked` must report `cargo build` (pins command
   scoping); an unterminated heredoc must raise; `bash -c "cargo build"` must emit the
   unclassifiable row; a `--no-deps` call must not report.
2. **A9** — missing input reports; declared does not; empty-reason waiver is a row;
   stale waiver is a row; floor member out of scope reports; floor member allowlisted
   reports; `--manifest-path rs/Cargo.toml` and `cargo machete rs` do NOT confer scope;
   each of the four cwd shapes resolves.
3. **Widened A8** — script-derived unlocked resolving line reports; `--locked` does not;
   `--no-deps` does not; a waiver whose text no longer occurs is a row; a `--write` in
   version-lockstep's blob is a row.
4. **`derive_cargo_tasks`** — emptying it fails via the floor (AC 1); the three kinds
   stay distinguishable; an unresolvable script path raises.
5. **Non-emptiness controls for every new constant.** `cargo_moon_parity.py:912-931`
   records, as measured, that passing an explicit `floor=` in each self-test call left
   the real constant unasserted — `REQUIRED_LOCKED_TASKS = ()` kept `--self-test` at
   rc 0. Every new table needs its own `if not X: fail` guard.
6. **The `collect_findings` arity fixture's tmp-root contract** (`:1032-1044`) must be
   restated: it builds a tmp root holding only `rs/Dockerfile`, and once
   `collect_findings` follows `ci/**/*.sh` under that root, the fixture must either
   provide them or the derivation must treat an absent `ci/` tree under a tmp root
   explicitly. State which; do not let it pass vacuously.

Mutation proofs on the real tree, each restored afterwards (AC 3):

* remove `rs/.cargo/config.toml` from `repo:observability-drift`'s `inputs` → A9 names
  it → restore;
* remove the same line from `.moon/tasks/rust.yml`'s `test` task → A9 names thirteen
  crates → restore;
* drop `--locked` from `ci/publish-metadata/run.sh:742` → widened A8 names script and
  line → restore.

Restore by reverting the marked edit, never `git checkout --` on the file: that would
also discard the uncommitted fix under test, and the next mutation would run against
original code and print a meaningless result.

**AC 8** — `ci/affected-graph/run.sh` must be RUN and report no expected-set movement.
Inspection suggests none (every case at `:258-353` touches `contracts/` or `rs/`, none
`ci/`), but the AC asks for a run, not an inspection.

**Rollback.** `repo:affected-smoke` is in `ci.yml`'s `T=(…)` array, so an A9 reporting
unexpected rows blocks every PR. If the first real run produces rows this design did not
predict, the fix is to correct the derivation, not to widen the allowlist under time
pressure; if that cannot be done same-day, revert the A9 entry from
`EXPECTED_FINDING_KEYS` and land the rest.

## 6. Decisions and rejected alternatives

**D1 — widen A8 to follow gate scripts, here.** Chosen over a follow-up issue. Measured
yield today is zero defects, so the case is forward-looking: a new invocation is covered
the day it is added. Cost: one waiver and L1. Recorded because the evidence was gathered
*after* the choice was made and does not support it as strongly as expected; a future
reader should weigh it knowing that.

**D2 — derive cwd rather than default-deny every cargo task**, so `repo:deny` falls out
structurally. A waiver would have kept it exempt after it gained a `cd rs`. Premise now
measured (§2.3), not asserted.

**D3 — encode the `cargo tree` exclusion in the verb predicate**, not as a per-task
waiver (§3.3). Covers a future `cargo tree` gate on day one. The AC's worry — that the
exclusion rests on the file's current *content* — stands as L3.

**D4 — `--no-deps` handled structurally, not by waiver.** Measurement A shows `--locked`
there asserts nothing; a waiver would imply there was something to waive. Generalisation
bounded by §2.1's stated scope and L6.

**D5 — waive A8's script rows by unique TEXT, not line number** (§3.4). The first draft's
line-number key would red on unrelated edits above the line.

**D6 — Half B's decision, recorded (AC 5).** SMA-601 took **option 1** (`--locked` on the
shared Rust tasks, closing root cause 2 for all thirteen crates) **and option 2** (a
`cargo metadata --locked` preflight as an unconditional `ci.yml` step before `moon ci`,
rather than a `repo:*` task, because a gate reading the lock from the working tree inside
`moon ci` races the repair). Option 3 was not taken. The local-ergonomics cost of option
1 is real and accepted: a tree whose manifest and lock disagree now reds every Rust task
instead of being repaired silently. AC 7 does not apply — the change was not declined.

**D7 — function-scoped waiver keys rejected.** Keying to the enclosing shell function
would express the real reason but needs brace-matched function parsing in a gate that
must stay cheap and `toolchain: 'system'`. D5's text-plus-uniqueness key gets most of the
stability for none of the parsing.

**D8 — path-aware scanning rejected.** Excluding `cargo update -w` structurally would
require modelling `case`/`if` dispatch on `"$1"` — shell static analysis, out of
proportion here. Hence L1.

**D9 — A9 gets its own verb predicate, separate from `LOCK_RESOLVING_VERBS`.** The first
draft reused A8's list, which made A9 fail to implement the rule §1 states: `fmt` fell out
by coincidence, and any future compiling-but-not-resolving subcommand (`cargo llvm-cov`,
`insta`, `udeps`, `bloat`) would be silently out of scope with no floor able to see it.
Two named constants sharing a classifier but not a membership rule.

**D10 — `ci/actionlint/run.sh` and `ci/affected-graph/run.sh` stay in the followed set.**
They contain cargo only as data — prose, pins of another gate's cargo line, and at
`:3714` a deliberately unlocked mutation fixture — and today all nine matches are
filtered correctly. Excluding them by allowlist was considered and rejected: an
exclusion would also hide a real cargo call added to either script later, and these two
gates are exactly where such a call would be least expected and most damaging. L4
records the residual.

## 7. Limitations and residuals

**L1 — the script scan is path-insensitive.** It reports a cargo line the task's
arguments never reach (§2.4). Today that costs one waiver. A reviewer must check
reachability by hand rather than trusting the row.

**L2 — script-following is one level deep, and shell-only.** A script invoking another
script is not followed. Nor are non-`ci/**/*.sh` entrypoints: `repo:nats-permissions`
invokes `bash ops/nats/check-subjects.sh` (`moon.yml:271`), and three gates invoke `.py`
files directly (`moon.yml:503-504, :709-710, :739-740`). A cargo call in any of those
shapes is unfollowed.

**L3 — the `cargo tree` exclusion rests on file content.** If `rs/.cargo/config.toml`
gains a `[source]` replacement or a `[build]` key, `cargo tree`'s output becomes
sensitive to it and D3 becomes wrong, silently. A cheap future fix: assert the file
contains only `[target.*-apple-darwin]` `rustflags` keys, with the file among
`repo:affected-smoke`'s inputs. Not taken here — it is a separate assertion with its own
mutation proofs.

**L4 — `ci/actionlint/run.sh`'s cargo prose is one edit from a row.** Its eight matches
survive on the string and comment filters; `:5094` (`the 'cargo metadata --locked' line
IS the assertion`) stays clean only because of its inner single quotes. Rewriting that
sentence without them fires a row against a required check, with a message no reviewer
will immediately understand.

**L5 — the cwd derivation resolves one round of literal substitution.** A script
computing its cargo cwd dynamically is missed. `REQUIRED_CWD_SHAPES` covers today's four
shapes, not tomorrow's.

**L6 — `--no-deps` was measured in one shape only** (§2.1): one cargo version, a lockfile
present, staleness induced by a member manifest gaining a dependency. Not measured with
no lockfile, nor with a new workspace member. D4 generalises beyond what was measured;
re-measure on a cargo bump.

**L7 — A9 proves the `rs/.cargo/config.toml` rule specifically.** CLAUDE.md's general
warning stands: a future `repo:*` task can omit some *other* input it reads and nothing
reds.
