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
structurally rather than by waiver. Note the ORDER that leaves: it is the VERB that
excludes them, and `_cwd_inside_rs` is never called for either — `deny` and `tree` are
both absent from `CONFIG_SENSITIVE_VERBS`. §2.3's cwd measurement is what justifies
`repo:deny` needing no declaration at all; it is not the mechanism that removes it.

This table counts the **blob-matched** set only. Script-following (§3.2) adds three more:
`repo:publish-metadata`, which already declares the file; `repo:version-lockstep`, which
does not and correctly should not (§2.4); and `repo:actionlint`, which enters because its
script quotes another gate's cargo line as pinned text (§2.2) and which A10 excludes on
both the verb and the cwd test.

The issue's "39 unasserted `build`/`build-release`/`test` declarations" are really
**four lines** in `.moon/tasks/rust.yml` — `build:46`, `build-release:52`, `test:67` and
`lint:124` — each inherited by thirteen crates. (An earlier draft of this section said three;
counted on disk, it is four.) A10 reads
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

Re-measured across all eight under the **shipped** conservative rule of §3.1. Two
columns, and the distinction is load-bearing: a *row* is a cargo invocation the scanner
found, a *would-report* row is one that resolves and lacks `--locked`. Only the second
column produces a finding.

| Script | rows | would-report |
| -- | -- | -- |
| `ci/version-lockstep/run.sh` | 3 | **3** — two real `cargo update -w`, one `die_infra` prose |
| `ci/publish-metadata/run.sh` | 4 | **1** — a `die_infra` prose row; the two real calls carry `--locked`, one is `--no-deps` |
| `ci/actionlint/run.sh` | 5 | **0** — all five carry `--locked` |
| the other five | 0 | 0 |

**These numbers changed when the classifier changed, and the earlier draft of this
section was measured against a design that no longer ships.** The four-layer classifier
stripped quoted strings, so `ci/actionlint/run.sh`'s five matches — several of them
check 8f's *pins of another gate's cargo line*, and one that deliberately constructs an
**unlocked** cargo string as a mutation fixture — were filtered out entirely. The
conservative rule does not strip strings, so they surface as rows and are excluded by
carrying `--locked` instead. That is the intended trade: they now survive on their own
content rather than on a filter that could silently drop real code.

One consequence for §3.2: because `script_cargo_lines` returns a non-empty list for
`ci/actionlint/run.sh`, `repo:actionlint` is a **third** member of the derived set's
`script` kind. It is harmless for both consumers and needs no waiver — all five of its
rows carry `--locked` for A8, and for A10 the script holds zero config-sensitive verbs
and zero cd-into-`rs` tokens, so A10 excludes it on both tests independently.

`ci/cargo-lock-integrity/run.sh` carries 2 rows and 1 would-report row, but **no Moon
task invokes it** — it is an unconditional `ci.yml` step (SMA-601). A8's script arm
follows only scripts named in a Moon task's resolved blob, so that row is out of reach
and needs no waiver. Do not add one; it would be stale on arrival.

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

Returns one `ScriptCargoLine(lineno, raw, segment, resolves, locked)` per cargo invocation.

**THE CONSERVATIVE RULE.** Report every cargo invocation whose own command segment does not
carry `--locked` after the verb. Exclude exactly three regions, because in each the shell
provably never executes the text:

1. **Heredoc bodies.** Accepted openers: `<<DELIM`, `<<'DELIM'`, `<<"DELIM"`, `<<-DELIM`
   (terminator may be tab-indented). A heredoc still open at EOF raises `MoonOutputError`
   (rc 2) — otherwise the scanner silently skips the rest of the file and reports zero rows,
   the same "infrastructure, never a silent pass" contract `check_dockerfile_locked` uses.
2. **`#` comment tails.** The cut runs **per physical line, before continuations are joined**:
   a `#` comment ends at the newline even when the previous line ends in a backslash, so
   joining first would pull the next line's real invocation into the comment.
3. **Bracketed operator spans** — `$(( ... ))`, a bare `(( ... ))` arithmetic command, and
   anything in `[ ... ]` (array subscript, `[[ ]]` test, glob). Inside them a `<<` is a SHIFT
   and a `#` is a base marker, so `$((1 << BITS))`, `(( MASK = 1 << BITS ))` and
   `a[1 << N]=2` would each otherwise read as a heredoc opener and swallow every line to the
   next bare terminator.

Exclusions 1, 2 and 3 are **one decision, taken together** in `_line_regions`, from ONE
within-line quote mask (matched pairs blanked to equal-length spaces, so every surviving
offset still indexes the original, and the code region is sliced out of the ORIGINAL). Keeping
them apart is what shipped a phantom heredoc in round 4 — a `<<EOF` inside a string opened a
real heredoc and swallowed the lines after it, silently — and, in round 5, the same defect
survived unfixed for `<<`'s siblings. Six sub-decisions, each with a fixture and a mutation:

- **A `#` must start a WORD, and be UNESCAPED.** `${#arr[@]}` / `${#var}` is bash's length
  operator, never a comment; cutting there drops `n=${#arr[@]} && cargo build`. And an escaped
  space is not a word boundary, so `echo a\ #b && cargo build` keeps `#b` inside the word and
  runs cargo. The word-start guard shipped in an earlier round **with no fixture**, and its
  mutant passed the self-test at rc 0.
- **Operator spans are blanked IN THE MASK, never in the code, and only when they CLOSE.**
  The first form ran on the raw line before any quote mask and blanked from `$((` to end of
  line when the span never closed, so `echo '$(( x' && cargo build` deleted the invocation
  from the code itself and reported nothing. Blanking the mask cannot do that: it only ever
  REFUSES a cut or an open, which is the false-positive direction. An unclosed span is left
  alone — `ls a[bc <<EOF` is a glob word followed by a REAL heredoc, and that row is what
  makes the closed-span requirement testable.
- **`<<<` is a here-STRING, not an opener.** `HEREDOC_OPEN_RE` matches at the SECOND `<` of
  `cat <<<EOF`, where the mask check passes, so the third `<` is rejected explicitly.
- **A heredoc body starts after the whole LOGICAL line.** `cat <<EOF \` + newline +
  `| cargo build` is one command; ending the line at the opener made the continuation its
  body. The opener is HELD across the continuation instead.
- **A heredoc opener must be UNMASKED.** `HEREDOC_OPEN_RE` is matched against the code region
  and accepted only where the `<<` itself survives the mask, so `echo "a <<EOF b"` opens
  nothing while `cat <<'EOF' > "$out"` still does — there the `<<` sits outside every quote
  pair even though its delimiter is quoted. Five string shapes and the comment shape are
  fixtures; the real-heredoc positive control is what stops "never open one" passing them all.
- **Ambiguous quote parity refuses BOTH.** An unpaired quote means the mask paired the wrong
  characters. The heredoc decision counts `"` and `'`; the comment cut counts `"` only,
  because an apostrophe in prose is English, not shell quoting — counting singles there turns
  `ci/publish-metadata/run.sh:772`, a plain comment mentioning `cargo metadata`, into a
  would-report row (MEASURED, and its own fixture pins the asymmetry in both directions).
  Refusing to cut, and refusing to open, can only add a **false positive**: the text is then
  scanned as ordinary code. Opening wrongly is what SWALLOWS. A/B-measured over the whole
  `ci/**/*.sh` corpus: the `"` guard fires on 343 physical lines and changes not one row.

Three more rules complete it. Backslash continuations are joined into one logical line, reported
against the FIRST physical line number. **One row per INVOCATION, not per segment** — `finditer`,
not `search`: a segment holding two cargo calls emits two rows, because reading only the first
hid the nested unlocked call in `cargo build --locked --features "$(cargo test)"`, a silent
false negative in the one direction this design claims it cannot have (SMA-599 final review).
And `--locked` counts only **in the segment holding the verb, after that verb, and before the
next invocation in that segment** — the segment scope keeps `cargo build && cargo metadata
--locked` reporting `cargo build`; the after-the-verb scope keeps a `--locked` that is string
content sitting *before* the verb (`X="abc` / `--locked" cargo build`, one bash statement across
two physical lines) from covering a genuinely unlocked call; and the next-invocation bound keeps
a nested call's flag off the outer one (`cargo build "$(cargo test --locked)"` reports
`cargo build`). `cargo metadata --no-deps` is carved out (§2.1: `--no-deps` never resolves, so
`--locked` on it is inert), scoped the same way — a `--no-deps` on a neighbouring invocation does
not excuse a resolving one.

**Quoted string literals are NOT stripped.** A cargo verb inside a string reports like any
other.

#### Why this replaced a four-layer lexer

The first implementation stripped quoted strings and then tried to decide, per line, whether a
verb inside one still executed — `bash -c "…"`, `eval`, a `$( … )` body, a quote span crossing
physical lines. Four layers, 441 lines, of which the cross-line quote tracker alone was 196
with six states feeding three consumers. Three review rounds each found a different **silent
false negative**, and rounds 2-4 were each an interaction between one layer and the layer added
before it. The last one, measured against real bash:

```bash
bash -c \
  "cargo build"
```

reported zero rows and no error while bash runs cargo unlocked, because the exec-vs-plain
decision read the RAW physical line while continuation joining happened later, on the LOGICAL
line.

The conservative rule is ~25 lines of decision logic against 441, and it answers all four
historical defects (10/10 on the reviewers' scoring against the lexer's 7/10) because it has
**one** decision to get wrong. That is the real argument: a future defect in this design can
only be a **false positive** — a benign string that mentions a cargo verb reports, CI reds
loudly, and a reviewer adds a waiver. The lexer's defects were silent passes, and a gate whose
defects are silent passes cannot converge.

The cost, MEASURED on today's corpus, is five would-report rows instead of two: the two real
`cargo update -w` calls in `ci/version-lockstep/run.sh`; two cargo mentions inside error message
strings (`ci/version-lockstep/run.sh:583`, `ci/publish-metadata/run.sh:1663`); and — added by
the finditer rule above — `ci/actionlint/run.sh:3715`, a `${var/old/new}` substitution that
names both the locked and the unlocked form of a cargo line. All five are A8 waivers. See L8.
(`ci/cargo-lock-integrity/run.sh` is NOT among them: it is a `ci.yml` step, not a Moon task, so
no derivation follows it.)

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

### 3.3 A10 — `rs/.cargo/config.toml` inputs (Half A, AC 2–4)

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
  (`cargo update`) fall out **structurally**, so A10 ships with **zero waivers**.

AC 4 asks that the `cargo tree` exclusion be "encoded with a stated reason or removed".
Encoding it in the verb predicate satisfies that more durably than a per-task waiver:
it covers a future `cargo tree` gate on day one.

**cwd derivation.** Reads the **raw** line, never the quote-stripped code — otherwise
`RS_DIR="$REPO_ROOT/rs"` becomes `RS_DIR=` and the shape dies. Recognised tokens are
exactly `cd`, `pushd` and `--cwd`, each followed by a path that resolves under `rs/`
after one round of literal `VAR=<literal>` substitution (`$VAR` and `${VAR}` both).
Live shapes covered: `cd rs`, `( cd rs && … )`, `cd "$REPO_ROOT/rs"`,
`RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"`, `--cwd ../../../rs/…`.

Substitution runs **longest name first**, so a short variable name cannot eat a longer
one it prefixes (`R=zzz` ahead of `RS_DIR=…` — measured both ways, SMA-599 final review).

A bare `rs`-containing *argument* must NOT confer scope — `--manifest-path
rs/Cargo.toml` and `cargo machete rs` are both fixtures asserting exactly that, because
a loose "blob mentions `rs/`" test would destroy D2's structural exclusion. Both fixture
rows are KNOWN-VACUOUS forward guards today and say so in the self-test: `repo:deny`
reaches `derive_cargo_tasks` but never the cwd test (`deny` is not a sensitive verb), and
`repo:machete` is never derived at all (`machete` is in neither verb list). The live
cwd-only exclusion is the `repo:root-build` fixture — verb-sensitive, cwd at the repo root.

**Script text is followed for EVERY kind, not only `script`.** `derive_cargo_tasks`
assigns `literal` on any cargo verb anywhere in a blob, PROSE INCLUDED, so a gate whose
invocation reads `echo "running cargo check"; bash ci/foo/run.sh` is `literal` while the
identical gate without the echo is `script`. Reading the referenced file only for kind
`script` therefore let a benign `echo` in a `moon.yml` block switch A10 off for that gate
(SMA-599 final review; `task_script_refs` returns `[]` for a blob-only task, so following
unconditionally costs nothing — measured, in_scope stays 58 and no row appears).

**Verdict.** An in-scope task must declare `rs/.cargo/config.toml` in `inputFiles` or
`inputGlobs` (both are read; an absent key is a violation, never a skip — the contract
A4/A5/A6/A7 share). Measured: `moon.yml:239`'s `rs/.cargo/config.toml` and
`.moon/tasks/rust.yml:46`'s `/rs/.cargo/config.toml` both resolve to the same
slash-free `rs/.cargo/config.toml`, which is why every declaring task matches verbatim.
Final counts on the shipped corpus: **59** tasks declare the file, A10 examines **58** of
them, and the one it does not — `paigasus-kernel-py:test` — is asserted by **A5** instead
(its `FFI_TASK_INPUTS` splat already demands the file; A10's cwd rule does not reach it,
because the task's own cwd is `py/packages/kernel`).

`ALLOW_MISSING_CARGO_CONFIG` exists but is **empty**, like `ALLOW_OVER_APPROXIMATION`.
An entry requires a non-empty reason, and an entry matching no task is itself a row —
the stale-skip idiom `ci/actionlint/run.sh:2376-2383` uses.

**Floors.** `REQUIRED_CARGO_CONFIG_TASKS` (`paigasus-kernel-rs:build`,
`paigasus-iam-rs:test`, `paigasus-kernel-ts:build`, `repo:parity-corpus-drift`,
`repo:publish-metadata`) must be in scope **and not allowlisted** — a default-deny gate's
second vacuity mode is an allowlist that grows to swallow the set.

An earlier draft mandated a second `REQUIRED_CWD_SHAPES` floor, asserting that each cd
form still resolves for at least one task. **It is not built, deliberately** (SMA-599
final review). `REQUIRED_CARGO_CONFIG_TASKS` already covers the property against the REAL
corpus: mutating `CWD_TOKEN_RE` to match nothing was measured to leave three floor rows,
because every floor member reaches its cwd through one of those forms. A second floor
asserting the same property from the same derivation adds a table to maintain and no
coverage. The cd forms themselves are exercised directly instead, by the
`_cwd_inside_rs` fixtures in the self-test — including the prefix-collision case below.

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

## 3.6 Naming: this assertion is A10, not A9

SMA-604 (`05fa484`, PR 189) landed a DIFFERENT assertion named **A9**
(`check_member_globs`, Dependabot's `[workspace] members` expansion) in this same file
while SMA-599 was in flight. It took the `"a9"` key, the `collect_findings` arity
fixture and both PASS strings. Since it reached `main` first, this design's assertion is
**A10** (`check_cargo_config_inputs`, key `"a10"`), ordered after A9 in
`EXPECTED_FINDING_KEYS` and in `collect_findings`.

The two are independent — A9 asks whether Dependabot can see every workspace crate, A10
asks whether a compiling cargo task keys on `rs/.cargo/config.toml` — so the merge kept
both sides of every conflict verbatim. The only judgement was the rename and the
ordering. `ci/affected-graph/README.md`'s A9 bullets belong to SMA-604 and are not about
this change.

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
| **`ci/affected-graph/README.md`** | **Yes** — add the A10 bullet; `:173-177` becomes false |
| `cargo_moon_parity.py:1673` PASS string | **Yes** — "all eight assertions" → nine |

CLAUDE.md's `rs/.cargo/config.toml` bullet currently states *"Only 16 of those 61
declarations are asserted"*, *"delete one and CI stays green"*, and the follow-on bullet
*"Nothing enforces that one rule."* A10 makes all three false. **No gate asserts this
prose**, so it must be corrected by hand in the same PR. `README.md:173-177` states that
a cargo call inside a `.sh` is outside A8's derived set — also now false.

## 5. Testing

`--self-test` fixture tables for:

1. **`script_cargo_lines`**, one fixture per decision in §3.1. Must NOT report: a heredoc
   body; a full-line comment; `cargo build --locked`; `cargo metadata --no-deps`. Must
   report — each verified against real bash to actually run cargo:
   `echo "a # b" && cargo build` (quote-aware comment cut);
   `VERSION="$(cargo metadata … | jq …)"` and `if ! OUT="$(cargo build 2>&1)"; then` (the
   repo's house idiom); `X="abc` / `--locked" cargo build` with `locked=False` (the flag
   scope after the verb); `X="$(` / `cargo build` / `)"` and
   `X="$(cargo build) more` / `stuff"`; `bash -c \` / `"cargo build"` (the defect that
   retired the previous design); `MASK=$((1 << BITS))` / `cargo build` / `BITS`;
   `X="a` / `b # c" cargo build` (the odd-double-quote guard); `# note \` / `cargo build`
   (the per-physical-line comment cut); and `echo "start` / `cargo build` / `end"`, the
   accepted false positive of L8, pinned so nobody reintroduces string stripping.
   `n=${#arr[@]} && cargo build` (the `#` word-start guard) and `echo a\ #b && cargo build`
   (the escaped-space guard); a `<<EOF` that must NOT open a heredoc — inside a double-quoted
   string, as `<<-` with a tab-indented terminator, as `<<'EOF'` inside a string, inside a
   single-quoted string, in realistic prose (`die_infra "run <<EOF to reproduce"`), inside a
   comment, and on a line whose quote parity is ambiguous (`echo \" # <<EOF`) — plus
   `X='a` / `b <<EOF c'` for single-quote parity and `cat <<<EOF` for the here-string;
   `echo '$(( x' && cargo build` and `echo '$((' ; cargo build ; echo '))'` for the
   mask-not-code blanking; `(( MASK = 1 << BITS ))` and `a[1 << N]=2` for the two operator
   spans beyond `$(( ))`; and `cat <<EOF \` / `| cargo build` for the held opener. Five
   positive controls: `cat <<'EOF' > "$out"`, plain `cat <<EOF` and `cat <<-EOF` all still
   open a heredoc whose body is skipped, an apostrophe in a trailing comment does not stop
   one, and `ls a[bc <<EOF` — an UNCLOSED bracketed span before a real opener — still opens.
   An unterminated heredoc must raise `MoonOutputError`. Three more, added by the final
   review: `cargo build --locked --features "$(cargo test)"` emits TWO rows, the second
   unlocked (the finditer rule); `cargo build "$(cargo test --locked)"` reports the outer
   call (the next-invocation flag bound); and `cargo build "$(cargo metadata --no-deps)"`
   keeps `resolves=True` on the outer call. A **twenty-five**-mutation battery — one
   mutation per decision — must kill every mutant.
2. **A10** — missing input reports; declared does not; empty-reason waiver is a row;
   stale waiver is a row; floor member out of scope reports; floor member allowlisted
   reports; `--manifest-path rs/Cargo.toml` and `cargo machete rs` do NOT confer scope;
   a verb-sensitive task whose cwd is the repo root does NOT (`repo:root-build` — the only
   live exercise of the cwd exclusion); two gates running the SAME script, differing only by
   a benign `echo "running cargo check"` in the blob, are BOTH in scope; and `_cwd_inside_rs`
   is called directly on each cd shape, including the prefix-collision case
   (`R=zzz` before `RS_DIR="$REPO_ROOT/rs"`).
3. **Widened A8** — script-derived unlocked resolving line reports; `--locked` does not;
   `--no-deps` does not; a waiver whose text no longer occurs is a row; a `--write` in
   version-lockstep's blob is a row; a waiver text occurring twice is a row, BUT a segment
   holding one locked and one unlocked invocation is not ambiguous, because only reporting
   rows are counted.
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

* remove `rs/.cargo/config.toml` from `repo:observability-drift`'s `inputs` → A10 names
  it → restore;
* remove the same line from `.moon/tasks/rust.yml`'s `test` task → A10 names thirteen
  crates → restore;
* drop `--locked` from `ci/publish-metadata/run.sh:742` → widened A8 names script and
  line → restore.

Restore by reverting the marked edit, never `git checkout --` on the file: that would
also discard the uncommitted fix under test, and the next mutation would run against
original code and print a meaningless result.

**AC 8** — `ci/affected-graph/run.sh` must be RUN and report no expected-set movement.
Inspection suggests none (every case at `:258-353` touches `contracts/` or `rs/`, none
`ci/`), but the AC asks for a run, not an inspection.

**Rollback.** `repo:affected-smoke` is in `ci.yml`'s `T=(…)` array, so an A10 reporting
unexpected rows blocks every PR. If the first real run produces rows this design did not
predict, the fix is to correct the derivation, not to widen the allowlist under time
pressure; if that cannot be done same-day, revert the A10 entry from
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

**D9 — A10 gets its own verb predicate, separate from `LOCK_RESOLVING_VERBS`.** The first
draft reused A8's list, which made A10 fail to implement the rule §1 states: `fmt` fell out
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

**L4 — `ci/actionlint/run.sh`'s cargo prose is one edit from a row.** Of its eight matches,
three are full-line comments (excluded) and five are fixture strings that classify as
ordinary rows and stay clean **only because each carries `--locked` after the verb** — this
is measured, not assumed. `:5094` (`the 'cargo metadata --locked' line IS the assertion`) is
the fragile one: the conservative rule does not strip strings, so rewriting that sentence to
drop the `--locked` — or to put it before the verb — fires a row against a required check,
with a message no reviewer will immediately understand. The same holds for `:3714`, whose
`${script/…/…}` substitution names both the locked and the unlocked form.

**L5 — the cwd derivation resolves one round of literal substitution.** A script
computing its cargo cwd dynamically is missed, in the false-negative direction. Two shapes
are known and neither is fixed:

* **Two-level indirection.** `A=rs; B="$A"; cd "$B"` needs a second round. Substituting
  `B` yields the literal text `$A`, which `_cwd_inside_rs` never re-scans. No live script
  does this; a fixed-point loop is not worth adding for a shape that does not exist yet.
* **Anything computed at run time** — a cwd read from a command's output, a `case` arm, an
  argument.

Prefix collision USED to be a third shape and is now closed: substitution runs longest
name first, so an `R=zzz` assignment can no longer eat a later `$RS_DIR` (measured both
ways, SMA-599 final review).

What actually holds the cwd rule honest is **`REQUIRED_CARGO_CONFIG_TASKS` against the
real corpus**, not a shape table: with `CWD_TOKEN_RE` mutated to match nothing, A10's
examined set falls from 58 tasks to 52 and three floor members red
(`paigasus-kernel-ts:build`, `repo:parity-corpus-drift`, `repo:publish-metadata`).
The individual cd forms, including the prefix-collision case, are exercised by direct
`_cwd_inside_rs` fixtures in the self-test.

**L6 — `--no-deps` was measured in one shape only** (§2.1): one cargo version, a lockfile
present, staleness induced by a member manifest gaining a dependency. Not measured with
no lockfile, nor with a new workspace member. D4 generalises beyond what was measured;
re-measure on a cargo bump.

**L7 — A10 proves the `rs/.cargo/config.toml` rule specifically.** CLAUDE.md's general
warning stands: a future `repo:*` task can omit some *other* input it reads and nothing
reds.

**L8 — a benign multi-line string containing a cargo verb reports.** The conservative rule
(§3.1) does not strip quoted strings, so prose such as
`die_infra "cargo update -w failed (site 16)"` produces a row. Three such rows exist today and
all three are A8 waivers: two `die_infra` diagnostics (`ci/version-lockstep/run.sh:583`,
`ci/publish-metadata/run.sh:1663`) and — since the finditer rule — `ci/actionlint/run.sh:3715`,
a `${var/old/new}` substitution that builds a mutated script text naming the unlocked form.
That third one is the price of closing the nested-invocation false negative, and it is the
trade this design makes on purpose: one waiver bought back the claim that a future defect here
can only be a false positive. This is the design's accepted false-positive direction, pinned by a
self-test fixture so nobody "fixes" it by reintroducing string stripping; it reds CI loudly
rather than passing silently.


**L9 — the comment cut ignores single-quote parity, so one narrow silent drop remains.**
`X='a` / `b # c' cargo build` is one bash statement that runs cargo; the second line's `#` is
string content, but the cut counts double quotes only and truncates there, dropping the row
(MEASURED). Counting single quotes closes it and costs a false positive on every prose comment
carrying an apostrophe, which is far more common — so the trade was taken deliberately, in the
direction that keeps today's corpus honest. The heredoc decision, where a wrong answer swallows
whole blocks rather than one line, does count them. No instance of this shape exists in
`ci/**/*.sh`.

**L10 — cargo invoked through a variable is not seen, and this is DEFERRED, not covered.**
`$CARGO build`, `"$cargo_bin" build`, and any other indirection past a literal `cargo` token
produce no row at all. The gap is pre-existing and shared with A8's own literal-token match,
which merged under SMA-601, so closing it means widening `CARGO_INVOCATION_RE` repo-wide —
a spec decision with its own blast radius, parked for a follow-up issue rather than taken
here. Until then, neither A8's blob arm nor its script arm can see a variable-driven cargo
call, and the spec claims no coverage of one.

**L11 — A10's `CONFIG_SENSITIVE_VERBS` split narrows only what's already derived; it does not
widen A10's coverage to a new subcommand.** `CONFIG_SENSITIVE_VERBS` is a strict subset of
`LOCK_RESOLVING_VERBS` (A8's list), and both `derive_cargo_tasks` and `script_cargo_lines`
gate on `CARGO_INVOCATION_RE` — built from `LOCK_RESOLVING_VERBS`, not from
`CONFIG_SENSITIVE_VERBS` — before A10 ever runs. So `cargo llvm-cov`, `insta`, `udeps`,
`bloat`, or `tarpaulin` yield an EMPTY derivation today, and A10 examines nothing for them —
no row, and no `FLOOR:` row either, since the floor only sees what the derivation produced.
An earlier version of the in-code comment overclaimed that A10's own verb list "covered" this
case; it does not (SMA-599 review, corrected in-code alongside this entry). Closing this
properly means widening `CARGO_INVOCATION_RE` itself, the same class of change L10 defers.

**L12 — the stale-waiver check is unasserted, by construction.** `check_cargo_config_inputs`
reports a row when `ALLOW_MISSING_CARGO_CONFIG` names a task A10 does not examine (`set(allow) -
in_scope`), the same "delete the dead waiver" contract A8's waiver lists carry. No self-test
fixture exercises this path because the allowlist is EMPTY (by design — every A10 exclusion is
meant to be structural, via the verb or the cwd rule, never a waiver). The moment a first entry
is ever added to `ALLOW_MISSING_CARGO_CONFIG`, this arm becomes reachable and should gain a
fixture then; recorded here rather than fixed now, per SMA-599 review scope.

**L13 — `repo:version-lockstep`'s own `run.sh` runs a real `napi build --cwd
.../rs/crates/bindings/paigasus-node-bindings` (ci/version-lockstep/run.sh:592-593), and A10
does not see it.** The task is derived as kind `script` (its moon.yml blob carries no cargo
verb; the script's `cargo update -w` line is what makes `derive_cargo_tasks` follow it), never
as kind `wrapper` — the `kind == "wrapper"` auto-sensitive bypass in
`check_cargo_config_inputs` only fires when an FFI marker sits in the moon.yml BLOB itself, not
inside a referenced script's text. `CONFIG_SENSITIVE_RE` looks for a literal `cargo <verb>`
token and does not recognize `napi build` at all, so this call is invisible to A10's sensitivity
test regardless of its `--cwd`. This is harmless ONLY because both the `cargo update -w` line
and the `napi build` line above it live inside `run_write()`, which `ci/version-lockstep/run.sh`
invokes only under `--write` — and `check_version_lockstep_no_write` (in
`cargo_moon_parity.py`) separately asserts the moon.yml task never passes `--write`. If that
premise ever changes (the task starts passing `--write`, or `run_write()`'s napi call moves
out from behind the flag), this napi build becomes reachable and rustflags-sensitive with no
gate covering it — a coupling between two independent assertions that must not be broken
silently (SMA-599 review; recorded, not fixed, per review scope).
